//! The regression harness: plays every scenario (or the quick subset) in a
//! room of its own, several rooms at once, and says which passed. Each
//! scenario's actors build, buy, make lines and assign vehicles through the
//! whole stack a game uses, and every replica checks its world as it goes;
//! the replicas must also end in the same world. See `docs/REGRESSION.md`.
//!
//! Against a server in this process by default; `--server` plays against a
//! deployed one, `--offline` without any, which checks only the scenarios'
//! own expectations, in milliseconds.

use std::{
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::Semaphore;
use tpf3mp_net::{CertificateDer, ServerTrust};
use tpf3mp_proto::{RoomSettings, Speed};
use tpf3mp_testkit::regress::{
    library,
    offline::{OfflinePlan, play_offline},
    run::{FAST, HarnessPlan, LocalServer, Outcome, judge, run_scenario},
    script::Scenario,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// List the scenarios and exit.
    #[arg(long)]
    list: bool,
    /// Only the quick subset.
    #[arg(long)]
    smoke: bool,
    /// Only these scenarios; repeat for more.
    #[arg(long = "only")]
    only: Vec<String>,
    /// Rooms played at once. By default half the processors.
    #[arg(long)]
    parallel: Option<usize>,
    /// Games per room: at least each scenario's actors. By default two, or
    /// one per actor.
    #[arg(long)]
    replicas: Option<usize>,
    /// Play each scenario this many times, each with another world seed.
    #[arg(long, default_value_t = 1)]
    repeat: u64,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// The room's speed in percent; 1600 is the fastest.
    #[arg(long, default_value_t = Speed::MAX.0)]
    speed: u16,
    #[arg(long, default_value_t = FAST.steps_per_second)]
    sps: u16,
    #[arg(long, default_value_t = FAST.input_delay_ms)]
    input_delay_ms: u16,
    #[arg(long, default_value_t = FAST.checkpoint_interval)]
    checkpoint_interval: u32,
    /// Play against this server (host:port) instead of one in this process.
    #[arg(long, conflicts_with = "offline")]
    server: Option<String>,
    /// Trust exactly this certificate (DER) of `--server`.
    #[arg(long, requires = "server")]
    pin_cert: Option<PathBuf>,
    /// Play without a server: only the scenarios' own expectations.
    #[arg(long)]
    offline: bool,
    /// Seconds a scenario may take.
    #[arg(long, default_value_t = 300)]
    deadline: u64,
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let args = Args::parse();
    let scenarios: Vec<Arc<Scenario>> = library::scenarios()
        .into_iter()
        .filter(|s| !args.smoke || s.smoke)
        .filter(|s| args.only.is_empty() || args.only.iter().any(|name| *name == s.name))
        .collect();
    if args.list {
        for s in library::scenarios() {
            let quick = if s.smoke { "smoke" } else { "     " };
            println!("{quick} {:<14} {} actor(s), {}", s.name, s.actors, s.about);
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(unknown) = args
        .only
        .iter()
        .find(|name| !library::scenarios().iter().any(|s| s.name == **name))
    {
        anyhow::bail!("no scenario {unknown}; --list shows them");
    }
    let settings = RoomSettings {
        scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
        steps_per_second: args.sps,
        input_delay_ms: args.input_delay_ms,
        checkpoint_interval: args.checkpoint_interval,
    };
    anyhow::ensure!(settings.is_valid(), "room settings out of range");

    let started = Instant::now();
    let runs: Vec<(Arc<Scenario>, u64)> = scenarios
        .iter()
        .flat_map(|s| (0..args.repeat).map(move |r| (s.clone(), args.seed + r)))
        .collect();
    let outcomes = if args.offline {
        offline(&args, &runs)
    } else {
        online(&args, settings, runs).await?
    };

    let mut failed = 0;
    for outcome in &outcomes {
        let verdict = if outcome.passed() { "PASS" } else { "FAIL" };
        let mut lag: Vec<Duration> = outcome
            .reports
            .iter()
            .flat_map(|r| r.lag.iter().map(|(_, time)| *time))
            .collect();
        lag.sort_unstable();
        let lag = match (lag.get(lag.len() / 2), lag.last()) {
            (Some(p50), Some(max)) => format!("  act lag p50 {p50:.0?} max {max:.0?}"),
            _ => String::new(),
        };
        println!(
            "{verdict} {:<14} {:>7} steps {:>7.2} s  {} replicas{lag}",
            outcome.scenario,
            outcome.steps(),
            outcome.elapsed.as_secs_f64(),
            outcome.reports.len(),
        );
        for failure in &outcome.failures {
            println!("       {failure}");
        }
        failed += usize::from(!outcome.passed());
    }
    println!(
        "{} runs: {} passed, {failed} failed, {:.1} s",
        outcomes.len(),
        outcomes.len() - failed,
        started.elapsed().as_secs_f64()
    );
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn offline(args: &Args, runs: &[(Arc<Scenario>, u64)]) -> Vec<Outcome> {
    runs.iter()
        .map(|(scenario, seed)| {
            let started = Instant::now();
            let mut plan = OfflinePlan::new(scenario.clone());
            plan.world_seed = *seed;
            plan.checkpoint_interval = u64::from(args.checkpoint_interval);
            if let Some(replicas) = args.replicas {
                plan.replicas = scenario.players(replicas);
            }
            let (reports, failures) = match play_offline(&plan) {
                Ok(reports) => {
                    let failures = judge(scenario, &reports);
                    (reports, failures)
                }
                Err(error) => (Vec::new(), vec![error.to_string()]),
            };
            Outcome {
                scenario: scenario.name.clone(),
                elapsed: started.elapsed(),
                reports,
                failures,
            }
        })
        .collect()
}

async fn online(
    args: &Args,
    settings: RoomSettings,
    runs: Vec<(Arc<Scenario>, u64)>,
) -> Result<Vec<Outcome>> {
    let (address, server_name, trust, _local) = match &args.server {
        Some(server) => {
            let (host, _) = server
                .rsplit_once(':')
                .context("the server address must be host:port")?;
            let address: SocketAddr = tpf3mp_agent::resolve(server)
                .await
                .with_context(|| format!("resolving {server}"))?;
            let trust = match &args.pin_cert {
                Some(path) => ServerTrust::Pinned(CertificateDer::from(std::fs::read(path)?)),
                None => ServerTrust::WebPki,
            };
            (address, host.to_owned(), trust, None)
        }
        None => {
            let local = LocalServer::start()?;
            (
                local.address,
                "localhost".to_owned(),
                local.trust.clone(),
                Some(local),
            )
        }
    };
    let parallel = args.parallel.unwrap_or_else(|| {
        std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1))
    });
    let permits = Arc::new(Semaphore::new(parallel.max(1)));
    let tasks: Vec<_> = runs
        .into_iter()
        .map(|(scenario, seed)| {
            let mut plan = HarnessPlan::new(address, &server_name, trust.clone(), scenario);
            plan.settings = settings;
            plan.speed = Speed(args.speed);
            plan.world_seed = seed;
            plan.deadline = Duration::from_secs(args.deadline);
            if let Some(replicas) = args.replicas {
                plan.replicas = plan.scenario.players(replicas);
            }
            let permits = permits.clone();
            tokio::spawn(async move {
                let _permit = permits.acquire_owned().await;
                let name = plan.scenario.name.clone();
                run_scenario(plan).await.unwrap_or_else(|error| Outcome {
                    scenario: name,
                    elapsed: Duration::ZERO,
                    reports: Vec::new(),
                    failures: vec![format!("could not play the room: {error:#}")],
                })
            })
        })
        .collect();
    let mut outcomes = Vec::with_capacity(tasks.len());
    for task in tasks {
        outcomes.push(task.await.context("a scenario task panicked")?);
    }
    Ok(outcomes)
}
