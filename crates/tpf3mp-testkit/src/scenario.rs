//! Runs a room of bots against a server: seat everyone, start the game,
//! play to the target step, and collect every bot's report.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tpf3mp_agent::{
    Client, ClientError, ConnectOptions, Events, Requests, Route, Worlds,
    bridge::{self, Bridge, BridgeEnd, BridgeFault, BridgeOptions, Rejoin},
    connect,
};
use tpf3mp_ipc::{Config as LinkConfig, Link, Role};
use tpf3mp_net::{Identity, ServerTrust, tunnel::TunnelUrl};
use tpf3mp_proto::{
    ContentManifest, CreateRoom, Invite, JoinRoom, Request, RequestError, RoomSettings, RulesName,
    Speed, Text,
};

use crate::{
    bot::{Bot, BotConfig, BotReport},
    fake_hook::{self, FakeHookConfig, HookReport},
};

/// What every bot plays: the toy game, without mods.
pub fn toy_content() -> ContentManifest {
    ContentManifest::new(Text::new("toy").expect("short build name"), Vec::new())
}

pub struct RoomPlan {
    pub server: SocketAddr,
    pub server_name: String,
    pub trust: ServerTrust,
    /// Connect the bots through this tunnel instead of over UDP.
    pub tunnel: Option<TunnelUrl>,
    pub settings: RoomSettings,
    pub speed: Speed,
    pub bots: Vec<BotConfig>,
    /// How long the bots may take to reach their target.
    pub deadline: Duration,
}

/// Plays one room and returns the bots' reports in seat order. The first
/// bot creates and owns the room.
pub async fn play_room(plan: RoomPlan) -> Result<Vec<BotReport>> {
    if plan.bots.is_empty() {
        bail!("a room needs at least one bot");
    }
    let names: Vec<&str> = plan.bots.iter().map(|bot| bot.name.as_str()).collect();
    let clients = connect_all(&plan, &names).await?;
    let seated: Vec<&Client> = clients.iter().map(|(client, _)| client).collect();
    seat_and_start(&seated, seated.len(), plan.settings, None).await?;
    if plan.speed != Speed::NORMAL {
        clients[0].0.set_speed(plan.speed).await?;
    }

    let tasks: Vec<_> = clients
        .into_iter()
        .zip(plan.bots)
        .map(|((client, events), config)| {
            tokio::spawn(Bot::new(client, events, config).play(plan.deadline))
        })
        .collect();
    let mut finished = Vec::with_capacity(tasks.len());
    for task in tasks {
        finished.push(task.await.context("a bot task panicked")?);
    }
    let mut reports = Vec::with_capacity(finished.len());
    let mut connected = Vec::with_capacity(finished.len());
    for result in finished {
        let (client, report) = result?;
        connected.push(client);
        reports.push(report);
    }
    // Everyone stays connected until every bot is done, so nobody leaves the
    // pacing set early.
    for client in connected {
        client.close().await;
    }
    Ok(reports)
}

/// Connects one client per name, each with a fresh identity.
async fn connect_all(plan: &RoomPlan, names: &[&str]) -> Result<Vec<(Client, Events)>> {
    let mut clients = Vec::with_capacity(names.len());
    for name in names {
        let mut options = player_options(plan.server, &plan.server_name, &plan.trust, name)?;
        if let Some(url) = &plan.tunnel {
            options.route = Route::Tunnel(url.clone());
        }
        clients.push(connect(options).await.context("connecting a player")?);
    }
    Ok(clients)
}

/// How a player with a fresh identity connects.
pub(crate) fn player_options(
    server: SocketAddr,
    server_name: &str,
    trust: &ServerTrust,
    name: &str,
) -> Result<ConnectOptions> {
    Ok(ConnectOptions::new(
        server,
        server_name,
        trust.clone(),
        Arc::new(Identity::generate()?.0),
        Text::new(name).context("player name")?,
    ))
}

/// Seats every client in one room of `max_players` seats, the first as its
/// owner, played by `rules` or the server's default, and starts the game.
/// Returns the room's invite.
pub(crate) async fn seat_and_start(
    clients: &[&Client],
    max_players: usize,
    settings: RoomSettings,
    rules: Option<RulesName>,
) -> Result<Invite> {
    let invite = seat(clients, max_players, settings, rules).await?;
    for client in clients {
        client.set_ready(true).await?;
    }
    clients[0].start_game().await?;
    Ok(invite)
}

/// Seats every client in one room, as [`seat_and_start`] does, without
/// marking anyone ready or starting the game.
async fn seat(
    clients: &[&Client],
    max_players: usize,
    settings: RoomSettings,
    rules: Option<RulesName>,
) -> Result<Invite> {
    let Some((owner, others)) = clients.split_first() else {
        bail!("a room needs at least one player");
    };
    for client in clients {
        client.declare_content(toy_content()).await?;
    }
    let (invite, _) = owner
        .create_room(CreateRoom {
            name: Text::new("testkit").context("room name")?,
            max_players: u8::try_from(max_players).context("too many players")?,
            password: None,
            settings,
            rules,
            listing: None,
            competitive: false,
        })
        .await?;
    for client in others {
        client
            .join_room(JoinRoom {
                invite,
                password: None,
                resume: None,
            })
            .await?;
    }
    Ok(invite)
}

pub struct BridgedPlan {
    pub server: SocketAddr,
    pub server_name: String,
    pub trust: ServerTrust,
    pub settings: RoomSettings,
    pub players: Vec<BridgedPlayer>,
    /// How long the games may take to reach their target.
    pub deadline: Duration,
    /// Where each player keeps its worlds, in a directory of its own name.
    /// Without it, players can neither save for the room nor join late.
    pub worlds: Option<PathBuf>,
    /// A save the room starts from, which the owner's agent hands over in
    /// the lobby (`BridgeOptions::start_world`). The games then wait at
    /// their main menus, each player is marked ready there by its agent,
    /// the owner once the room has the save, and the room starts once all
    /// are. Without it, everyone is marked ready and the room starts at
    /// once, from the owner's world.
    pub start_world: Option<PathBuf>,
}

pub struct BridgedPlayer {
    pub name: String,
    pub seed: u64,
    pub world_seed: u64,
    pub act_every: u64,
    pub target_step: u64,
    /// This player's world deviates once at this step.
    pub drift_at: Option<u64>,
    /// Join the game this long after it started, instead of from the lobby.
    pub join_after: Option<Duration>,
    /// Reach the server only through this tunnel, as behind a network that
    /// blocks UDP.
    pub tunnel: Option<TunnelUrl>,
}

impl BridgedPlayer {
    /// How this player connects.
    fn options(&self, plan: &BridgedPlan) -> Result<ConnectOptions> {
        let mut options = player_options(plan.server, &plan.server_name, &plan.trust, &self.name)?;
        if let Some(url) = &self.tunnel {
            options.route = Route::Tunnel(url.clone());
        }
        Ok(options)
    }
}

static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

/// A shared-memory link name no other game of this process uses.
pub(crate) fn link_name(kind: &str) -> String {
    format!(
        "tpf3mp-{kind}-{}-{}",
        std::process::id(),
        NEXT_LINK.fetch_add(1, Ordering::Relaxed)
    )
}

/// Plays one room through the whole stack a game uses: each player is a
/// fake hook (the toy game behind the step gate) on a shared-memory link to
/// its agent's bridge. An agent that loses the server rejoins the room and
/// resumes, as the real one does; a player who joins late receives the
/// world from the room. Returns the hooks' reports in plan order.
pub async fn play_bridged_room(plan: BridgedPlan) -> Result<Vec<HookReport>> {
    play_bridged_room_controlled(plan, BridgedControl::default()).await
}

/// Fixture milestones and gates, indexed by the plan's player seat. They
/// synchronize scenarios with completed game work; production timing is unchanged.
#[derive(Default)]
pub struct BridgedControl {
    /// Receives the next step only after that game loaded a snapshot and
    /// acknowledged it to its bridge.
    pub snapshot_loaded: BTreeMap<usize, tokio::sync::mpsc::UnboundedSender<u64>>,
    /// A late player also waits for this signal before connecting. Its
    /// scheduled `join_after` remains the earliest time it can arrive.
    pub before_join: BTreeMap<usize, tokio::sync::oneshot::Receiver<()>>,
}

/// As [`play_bridged_room`], with explicit milestones for restart fixtures.
pub async fn play_bridged_room_controlled(
    plan: BridgedPlan,
    mut control: BridgedControl,
) -> Result<Vec<HookReport>> {
    if control.before_join.keys().any(|seat| {
        plan.players
            .get(*seat)
            .is_none_or(|player| player.join_after.is_none())
    }) {
        bail!("a join gate must name a late player's seat");
    }
    let mut starting = Vec::new();
    for player in plan.players.iter().filter(|p| p.join_after.is_none()) {
        let options = player.options(&plan)?;
        let (client, events) = connect(options.clone())
            .await
            .context("connecting a player")?;
        starting.push((options, client, events));
    }
    let seated: Vec<&Client> = starting.iter().map(|(_, client, _)| client).collect();
    let (invite, owner) = if plan.start_world.is_some() {
        let invite = seat(&seated, plan.players.len(), plan.settings, None).await?;
        (invite, seated.first().map(|owner| owner.requests()))
    } else {
        let invite = seat_and_start(&seated, plan.players.len(), plan.settings, None).await?;
        (invite, None)
    };

    let mut games: Vec<Option<(Hook, BridgeTask)>> = plan.players.iter().map(|_| None).collect();
    let starters = plan
        .players
        .iter()
        .enumerate()
        .filter(|(_, player)| player.join_after.is_none());
    for (seat, ((index, player), connection)) in starters.zip(starting).enumerate() {
        let start_world = plan.start_world.clone().filter(|_| seat == 0);
        games[index] = Some(play_through_hook(
            &plan,
            player,
            &invite,
            connection,
            start_world,
            control.snapshot_loaded.remove(&index),
        )?);
    }
    if let Some(owner) = owner {
        start_when_ready(&owner, plan.deadline).await?;
    }
    let started = tokio::time::Instant::now();
    let mut late: Vec<(usize, &BridgedPlayer, Duration)> = plan
        .players
        .iter()
        .enumerate()
        .filter_map(|(index, player)| player.join_after.map(|after| (index, player, after)))
        .collect();
    late.sort_by_key(|(_, _, after)| *after);
    for (index, player, after) in late {
        tokio::time::sleep_until(started + after).await;
        if let Some(gate) = control.before_join.remove(&index) {
            tokio::time::timeout(plan.deadline, gate)
                .await
                .context("waiting for a late player's join gate")?
                .context("a late player's join gate closed before it was opened")?;
        }
        let options = player.options(&plan)?;
        let (client, events) = join_late(&options, &invite, plan.deadline).await?;
        games[index] = Some(play_through_hook(
            &plan,
            player,
            &invite,
            (options, client, events),
            None,
            control.snapshot_loaded.remove(&index),
        )?);
    }

    let mut reports = Vec::with_capacity(games.len());
    let mut bridges = Vec::with_capacity(games.len());
    for (hook, bridge) in games.into_iter().flatten() {
        let report = tokio::task::spawn_blocking(move || hook.join())
            .await
            .context("waiting for a game")?
            .map_err(|_| anyhow::anyhow!("a game panicked"))?;
        reports.push(report.context("a game failed")?);
        bridges.push(bridge);
    }
    for bridge in bridges {
        if bridge.is_finished() {
            let ended = bridge.await.context("a bridge panicked")?;
            bail!("a bridge ended before its game: {ended:?}");
        }
        bridge.abort();
    }
    Ok(reports)
}

/// Connects a late player and joins the running game, trying again after a
/// failure until `patience` has passed, backing off as the agent's rejoin
/// does: a server that restarts refuses connections until it is back.
async fn join_late(
    options: &ConnectOptions,
    invite: &Invite,
    patience: Duration,
) -> Result<(Client, Events)> {
    let deadline = tokio::time::Instant::now() + patience;
    let mut backoff = Duration::from_millis(250);
    loop {
        let attempt = async {
            let (client, events) = connect(options.clone())
                .await
                .context("connecting a late player")?;
            client.declare_content(toy_content()).await?;
            client
                .join_room(JoinRoom {
                    invite: *invite,
                    password: None,
                    resume: None,
                })
                .await
                .context("joining the running game")?;
            anyhow::Ok((client, events))
        };
        match attempt.await {
            Ok(joined) => return Ok(joined),
            Err(error) if tokio::time::Instant::now() + backoff >= deadline => return Err(error),
            Err(_) => {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(2));
            }
        }
    }
}

type Hook = std::thread::JoinHandle<Result<HookReport, fake_hook::HookError>>;
type BridgeTask = tokio::task::JoinHandle<Result<BridgeEnd, BridgeFault>>;

/// Starts once every player is marked ready, their agents deciding: the
/// room refuses until then, and while the world it starts from is still on
/// its way. Asked within the server's request rate (10 a second for one
/// connection), and asked again after a pause when the server says it was
/// asked too often: on a slow machine the room took long enough to be ready
/// that a faster loop spent its burst and was refused.
async fn start_when_ready(owner: &Requests, deadline: Duration) -> Result<()> {
    let give_up = tokio::time::Instant::now() + deadline;
    loop {
        match owner.done(Request::StartGame).await {
            Ok(()) => return Ok(()),
            Err(ClientError::Refused(
                RequestError::NotAllReady | RequestError::StartWorldPending,
            )) if tokio::time::Instant::now() < give_up => {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            Err(ClientError::Refused(RequestError::RateLimited))
                if tokio::time::Instant::now() < give_up =>
            {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => return Err(error).context("starting the room's game"),
        }
    }
}

/// Starts a player's fake game and the bridge between it and its client.
/// With `start_world`, this player owns the room and hands it that save.
fn play_through_hook(
    plan: &BridgedPlan,
    player: &BridgedPlayer,
    invite: &Invite,
    (options, client, events): (ConnectOptions, Client, Events),
    start_world: Option<PathBuf>,
    snapshot_loaded: Option<tokio::sync::mpsc::UnboundedSender<u64>>,
) -> Result<(Hook, BridgeTask)> {
    let rejoin = Rejoin {
        options,
        invite: *invite,
        password: None,
        content: Some(tpf3mp_agent::picker::Declaration::Content(toy_content())),
        give_up_after: plan.deadline,
    };
    let worlds = match &plan.worlds {
        Some(dir) => Some(
            Worlds::open(&dir.join(&player.name), 1 << 30).context("opening a player's worlds")?,
        ),
        None => None,
    };
    let link_name = link_name("bridged");
    let link = Link::create(&LinkConfig::new(&link_name), Role::Agent)?;
    let hook = fake_hook::spawn_observed(
        FakeHookConfig {
            link_name,
            player: client.player(),
            seed: player.seed,
            world_seed: player.world_seed,
            act_every: player.act_every,
            target_step: player.target_step,
            drift_at: player.drift_at,
            patience: plan.deadline,
            // A room that starts from a save starts with the games at their
            // menus; late joiners arrive at a running game.
            at_menu: plan.start_world.is_some() && player.join_after.is_none(),
        },
        snapshot_loaded,
    );
    let bridge = tokio::spawn(async move {
        let options = BridgeOptions {
            worlds,
            start_world,
            ..BridgeOptions::default()
        };
        let mut bridge = Bridge::new(link, options);
        bridge::play(&mut bridge, client, events, &rejoin).await
    });
    Ok((hook, bridge))
}

/// Latency percentiles over every report, in milliseconds: p50, p95, p99, max.
pub fn latency_summary(reports: &[BotReport]) -> Option<[u128; 4]> {
    let mut all: Vec<Duration> = reports
        .iter()
        .flat_map(|report| report.latencies.iter().copied())
        .collect();
    if all.is_empty() {
        return None;
    }
    all.sort_unstable();
    let at = |per_mille: usize| all[(all.len() - 1) * per_mille / 1000].as_millis();
    Some([at(500), at(950), at(990), at(1000)])
}
