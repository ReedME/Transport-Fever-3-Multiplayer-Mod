//! Plays a scenario in a room of its own and judges it.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tpf3mp_agent::{
    Client, ConnectOptions,
    bridge::{self, Bridge, BridgeOptions, Rejoin},
    connect,
};
use tpf3mp_ipc::{Config as LinkConfig, Link, Role};
use tpf3mp_net::{ServerIdentity, ServerTrust};
use tpf3mp_proto::{RoomSettings, RulesName, Speed};
use tpf3mp_server::{NATIVE, RulesMenu, Server, ServerConfig};

use super::{
    replica::{self, ReplicaConfig, ReplicaReport},
    script::{Item, Scenario},
};
use crate::scenario::{link_name, player_options, seat_and_start, toy_content};

/// What the harness plays a room at unless told otherwise: the fastest the
/// server allows, so the world runs thousands of steps a second, with the
/// shortest input delay, and a checkpoint comparison often.
pub const FAST: RoomSettings = RoomSettings {
    scan_plan: tpf3mp_proto::ScanPlan::DEFAULT,
    steps_per_second: 240,
    input_delay_ms: 20,
    checkpoint_interval: 25,
};

/// A server in this process for the harness's rooms: the game's own rules
/// only, turns sealed every few milliseconds, and room for many players
/// from one address. Stops when dropped.
pub struct LocalServer {
    pub address: SocketAddr,
    pub trust: ServerTrust,
    task: tokio::task::JoinHandle<()>,
}

impl LocalServer {
    /// Starts it on the current Tokio runtime.
    pub fn start() -> Result<Self> {
        let identity = ServerIdentity::self_signed(&["localhost"])?;
        let trust = ServerTrust::Pinned(identity.leaf().clone());
        let mut config = ServerConfig::new("127.0.0.1:0".parse()?, identity);
        config.rules = RulesMenu::native();
        config.tick = Duration::from_millis(5);
        config.max_sessions_per_address = 100_000;
        config.max_handshakes_per_address = 100_000;
        config.max_rooms_per_address = 100_000;
        let server = Server::bind(config)?;
        let address = server.local_addr()?;
        let task = tokio::spawn(server.run(std::future::pending()));
        Ok(Self {
            address,
            trust,
            task,
        })
    }

    /// A plan for `scenario` in a room of this server.
    pub fn plan(&self, scenario: Arc<Scenario>) -> HarnessPlan {
        HarnessPlan::new(self.address, "localhost", self.trust.clone(), scenario)
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// How long a game waits for its agent to beat. Generous: a busy machine
/// playing several rooms at full speed can starve an agent for seconds.
const PATIENCE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct HarnessPlan {
    pub server: SocketAddr,
    pub server_name: String,
    pub trust: ServerTrust,
    pub settings: RoomSettings,
    pub speed: Speed,
    pub scenario: Arc<Scenario>,
    /// Games in the room: at least the scenario's actors. The first plays
    /// actor 0, and so on; games beyond the actors only watch.
    pub replicas: usize,
    pub world_seed: u64,
    /// This replica's world deviates once at this step, to show the harness
    /// catches it.
    pub drift: Option<(usize, u64)>,
    pub deadline: Duration,
    /// An act fails when the room has not ordered it this many steps after
    /// its turn came.
    pub stall_steps: u64,
    /// The least time between two acts of one game. A room takes 20
    /// intents a second from a player, after a burst of 40 (`room.rs`);
    /// a script can act faster than any player, so it is held to less.
    pub min_gap: Duration,
}

impl HarnessPlan {
    /// A plan for `scenario` on a server, with every other setting at the
    /// harness's defaults: two replicas, or one per actor.
    pub fn new(
        server: SocketAddr,
        server_name: &str,
        trust: ServerTrust,
        scenario: Arc<Scenario>,
    ) -> Self {
        Self {
            server,
            server_name: server_name.to_owned(),
            trust,
            settings: FAST,
            speed: Speed::MAX,
            replicas: scenario.players(2),
            scenario,
            world_seed: 1,
            drift: None,
            deadline: Duration::from_secs(120),
            stall_steps: 40_000,
            min_gap: Duration::from_millis(60),
        }
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub scenario: String,
    pub elapsed: Duration,
    pub reports: Vec<ReplicaReport>,
    /// Everything that went wrong; none when the scenario passed.
    pub failures: Vec<String>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }

    /// Steps the world ran.
    pub fn steps(&self) -> u64 {
        self.reports.iter().map(|r| r.ran).max().unwrap_or(0)
    }
}

/// Plays `plan.scenario` in a room of its own, played by the game's own
/// rules as rooms are by default, and judges every replica. Fails only when
/// the room could not be played at all; a failing scenario is an
/// [`Outcome`] with failures.
pub async fn run_scenario(plan: HarnessPlan) -> Result<Outcome> {
    let scenario = plan.scenario.clone();
    if plan.replicas == 0 || plan.replicas != scenario.players(plan.replicas) {
        bail!(
            "{} is played by {} games, not {}",
            scenario.name,
            scenario.players(plan.replicas).max(1),
            plan.replicas
        );
    }
    let started = Instant::now();
    let mut players = Vec::with_capacity(plan.replicas);
    for index in 0..plan.replicas {
        let options = player_options(
            plan.server,
            &plan.server_name,
            &plan.trust,
            &format!("r{index}"),
        )?;
        let (client, events) = connect(options.clone())
            .await
            .context("connecting a player")?;
        players.push((options, client, events));
    }
    // Players join in seat order, so replica i plays actor i.
    let seated: Vec<&Client> = players.iter().map(|(_, client, _)| client).collect();
    let rules = RulesName::new(NATIVE).expect("short name");
    let invite = seat_and_start(&seated, plan.replicas, plan.settings, Some(rules)).await?;
    if plan.speed != Speed::NORMAL {
        players[0].1.set_speed(plan.speed).await?;
    }

    let mut games = Vec::with_capacity(players.len());
    for (index, (options, client, events)) in players.into_iter().enumerate() {
        let drift_at = plan
            .drift
            .and_then(|(replica, step)| (replica == index).then_some(step));
        games.push(play(&plan, options, client, events, invite, drift_at)?);
    }
    let mut reports = Vec::with_capacity(games.len());
    let mut failures = Vec::new();
    let mut bridges = Vec::with_capacity(games.len());
    let deadline = tokio::time::Instant::now() + plan.deadline;
    for (index, (game, bridge)) in games.into_iter().enumerate() {
        let joined = tokio::task::spawn_blocking(move || game.join());
        match tokio::time::timeout_at(deadline, joined).await {
            Ok(Ok(Ok(Ok(report)))) => reports.push(report),
            Ok(Ok(Ok(Err(error)))) => failures.push(format!("r{index}: {error}")),
            Ok(Ok(Err(_))) | Ok(Err(_)) => failures.push(format!("r{index}: the game panicked")),
            Err(_) => failures.push(format!("r{index}: not done in {:?}", plan.deadline)),
        }
        bridges.push(bridge);
    }
    for bridge in bridges {
        bridge.abort();
    }
    if failures.is_empty() {
        failures = judge(&scenario, &reports);
    }
    Ok(Outcome {
        scenario: scenario.name.clone(),
        elapsed: started.elapsed(),
        reports,
        failures,
    })
}

type Game = std::thread::JoinHandle<Result<ReplicaReport, replica::ReplicaError>>;
type BridgeTask = tokio::task::JoinHandle<Result<bridge::BridgeEnd, bridge::BridgeFault>>;

fn play(
    plan: &HarnessPlan,
    options: ConnectOptions,
    client: Client,
    events: tpf3mp_agent::Events,
    invite: tpf3mp_proto::Invite,
    drift_at: Option<u64>,
) -> Result<(Game, BridgeTask)> {
    let rejoin = Rejoin {
        options,
        invite,
        password: None,
        content: Some(tpf3mp_agent::picker::Declaration::Content(toy_content())),
        give_up_after: plan.deadline,
    };
    let name = link_name("regress");
    let link = Link::create(&LinkConfig::new(&name), Role::Agent)?;
    let game = replica::spawn(ReplicaConfig {
        link_name: name,
        player: client.player(),
        world_seed: plan.world_seed,
        scenario: plan.scenario.clone(),
        drift_at,
        patience: PATIENCE,
        stall_steps: plan.stall_steps,
        min_gap: plan.min_gap,
    });
    let bridge = tokio::spawn(async move {
        let mut bridge = Bridge::new(link, BridgeOptions::default());
        bridge::play(&mut bridge, client, events, &rejoin).await
    });
    Ok((game, bridge))
}

/// Everything wrong with a room's reports: a replica that did not finish
/// the script, a check that failed, an action the room refused, and any
/// difference between replicas, which is a desync even where every check
/// passed.
pub fn judge(scenario: &Scenario, reports: &[ReplicaReport]) -> Vec<String> {
    let mut failures = Vec::new();
    let item = |index: usize| {
        scenario
            .items
            .get(index)
            .map_or_else(|| "the end".to_owned(), Item::to_string)
    };
    for (index, report) in reports.iter().enumerate() {
        let r = format!("r{index}");
        for (item_index, why) in &report.refused {
            let what = item_index.map_or_else(|| "an action".to_owned(), item);
            failures.push(format!("{r}: the room refused {what}: {why}"));
        }
        if report.stalled {
            failures.push(format!(
                "{r}: stalled at item {} ({}), step {}",
                report.reached,
                item(report.reached),
                report.ran
            ));
        } else if report.ended {
            failures.push(format!("{r}: the session ended at step {}", report.ran));
        } else if report.finished_at.is_none() && report.refused.is_empty() {
            failures.push(format!(
                "{r}: stopped at item {} ({})",
                report.reached,
                item(report.reached)
            ));
        }
        for check in &report.checks {
            if let Some(why) = &check.failure {
                failures.push(format!(
                    "{r}: item {} at step {}: expected {}: {why}",
                    check.item, check.step, check.check
                ));
            }
        }
        for (step, lanes) in &report.diverged {
            failures.push(format!("{r}: diverged at step {step} in lanes {lanes:?}"));
        }
    }
    if let Some((first, rest)) = reports.split_first() {
        for (offset, other) in rest.iter().enumerate() {
            let r = format!("r{}", offset + 1);
            if other.ran != first.ran {
                failures.push(format!(
                    "{r} stopped at step {}, r0 at {}",
                    other.ran, first.ran
                ));
            } else if other.lanes != first.lanes {
                let lanes: Vec<u16> = other
                    .lanes
                    .iter()
                    .zip(&first.lanes)
                    .filter(|(a, b)| a != b)
                    .map(|(a, _)| a.lane)
                    .collect();
                failures.push(format!(
                    "{r} differs from r0 at step {} in lanes {lanes:?}",
                    other.ran
                ));
            }
            if other.checks != first.checks {
                failures.push(format!("{r} judged the checks differently from r0"));
            }
        }
    }
    failures
}
