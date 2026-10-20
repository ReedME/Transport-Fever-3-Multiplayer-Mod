//! A fake game for trying the whole stack by hand, as separate processes:
//! the toy game behind the step gate, on the link a launcher or an agent
//! created (`--game-link`). Until Transport Fever 3 is out, it stands in
//! for the game in playtests: start the launcher, then this.

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tpf3mp_proto::{FixedBytes, PlayerId};
use tpf3mp_testkit::fake_hook::{self, FakeHookConfig};

/// Plays the toy game through an agent's game link.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// The link name given to the launcher's or agent's `--game-link`;
    /// their default without one. Read from `TPF3MP_GAME_LINK` when not
    /// given, as the game's hook does.
    #[arg(env = "TPF3MP_GAME_LINK", default_value = tpf3mp_bridge::DEFAULT_LINK)]
    link: String,
    /// Seed of this player's choices.
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// Seed of the world; the same for every game in a room.
    #[arg(long, default_value_t = 42)]
    world_seed: u64,
    /// Steps between this player's commands; 0 never sends any.
    #[arg(long, default_value_t = 10)]
    act_every: u64,
    /// Stop after running this step.
    #[arg(long, default_value_t = u64::MAX)]
    steps: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    println!("waiting for the agent on link {}", args.link);
    let report = fake_hook::spawn(FakeHookConfig {
        link_name: args.link,
        // Without the player's identity the game only builds track, which
        // any player may.
        player: PlayerId(FixedBytes([0; 32])),
        player_name: "fakegame".into(),
        seed: args.seed,
        world_seed: args.world_seed,
        act_every: args.act_every,
        target_step: args.steps,
        drift_at: None,
        patience: Duration::from_secs(3600),
        at_menu: false,
        notices: None,
    })
    .join()
    .map_err(|_| anyhow::anyhow!("the game panicked"))?
    .context("the game stopped")?;
    println!(
        "ran {} steps, applied {} events, sent {} commands ({} refused), saved {} times, loaded {} worlds from the room, {} divergences{}",
        report.ran,
        report.applied,
        report.commands,
        report.refused,
        report.saves,
        report.received,
        report.diverged.len(),
        if report.ended {
            "; the session ended"
        } else {
            ""
        },
    );
    for lane in &report.lanes {
        println!("  lane {}: {}", lane.lane, hex(&lane.digest.0[..8]));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
