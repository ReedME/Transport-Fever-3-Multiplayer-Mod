//! A multiplayer test rig on one PC: starts several games, each with its
//! own agent (a headless launcher in this process), its own data folder
//! and its own link name, and puts them all in one room. The first player
//! hosts and the others join its invite. Until Transport Fever 3 is out,
//! the games are fake ones (`tpf3mp-fakegame`); a real game is started as
//! the launcher starts it, with the hook loaded into it (`tpf3mp-launch`),
//! and told its link, data folder and starter through the environment its
//! hook reads (`TPF3MP_GAME_LINK`, `TPF3MP_DATA_DIR`,
//! `TPF3MP_LAUNCHER_PID`).
//!
//! It runs until every game has exited, or Ctrl-C or the end of its
//! --time-limit, which stop everything it started. When the games print their lane digests, as the fake game
//! does, it checks that all of them ended in the same world.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::oneshot,
    task::JoinHandle,
    time::Instant,
};
use tpf3mp_agent::{
    Worlds, content,
    launcher::{
        Action, Launcher, LauncherConfig, LauncherHandle, State,
        setup::{self, TunnelArgs},
    },
};
use tpf3mp_net::{Identity, ServerIdentity, ServerTrust};
use tpf3mp_proto::RoomSettings;
use tpf3mp_server::{Server, ServerConfig, SnapshotConfig};

/// How long setting up the room may take: connecting, joining, getting
/// ready.
const SETUP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a game may take from its start to its hook attaching, with
/// --wait-for-games.
const GAME_START_TIMEOUT: Duration = Duration::from_secs(300);
/// How often the rig looks at its launchers' states.
const POLL: Duration = Duration::from_millis(100);
/// The environment the game's hook reads its link name, starter and data
/// folder from. `DATA_DIR_ENV` is `tpf3mp_hook::DATA_DIR_ENV`, not taken
/// from that crate, whose load-time entry points would run in this process.
use tpf3mp_ipc::{LAUNCHER_PID_ENV, LINK_ENV};
const DATA_DIR_ENV: &str = "TPF3MP_DATA_DIR";
/// Space each player's worlds may take.
const WORLDS_BYTES: u64 = 8 << 30;

/// Starts N games on this PC, each with its own agent, in one room.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Games to start; the first hosts the room.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=64))]
    players: u8,

    /// The server as host:port, or `local` for a throwaway server in this
    /// process.
    #[arg(long)]
    server: String,

    /// Trust exactly this DER certificate (from the server's
    /// --dev-self-signed) instead of the public certificate authorities.
    #[arg(long)]
    pin_cert: Option<PathBuf>,

    /// The game to start for each player: `fake` for tpf3mp-fakegame next
    /// to this program, or the path of a game executable.
    #[arg(long, default_value = "fake")]
    game: String,

    /// An argument for each game; repeat for several.
    #[arg(long = "game-arg", allow_hyphen_values = true)]
    game_args: Vec<OsString>,

    /// With a real game: the hook library to load into it; without, the
    /// one next to this program.
    #[arg(long)]
    hook: Option<PathBuf>,

    /// With a real game: seconds it stays suspended for its hook to say it
    /// is ready. TPF3-MP's hook says so at once; a stand-in library never
    /// does, so tests that load one give 0.
    #[arg(long, default_value_t = tpf3mp_launch::HOOK_READY_WAIT.as_secs())]
    hook_ready_wait: u64,

    /// Seconds the whole run may take: past them the rig stops everything
    /// it started and fails. A real game still starting then ends once its
    /// start returns: after its hook loaded (at most 30 s) and the
    /// --hook-ready-wait.
    #[arg(long)]
    time_limit: Option<u64>,

    /// Where each player's folder goes (`p1`, `p2`, ...: identity, worlds,
    /// and the game hook's log and profiles). Kept between runs, so the
    /// players stay the same.
    #[arg(long)]
    data_root: Option<PathBuf>,

    /// With the fake game: stop each game after this step.
    #[arg(long)]
    steps: Option<u64>,

    /// Seconds to wait between starting one game and the next. Two
    /// Transport Fever 3 games started at the same moment on one PC can
    /// fail while setting up their graphics.
    #[arg(long, default_value_t = 0)]
    stagger: u64,

    /// Start the room's game only once every player's game has attached,
    /// so that none joins it late. A late joiner is handed the room's
    /// world as a save.
    #[arg(long)]
    wait_for_games: bool,

    /// Run the local server without a snapshot store. Every player then
    /// loads the world it starts from itself, as on a server that keeps no
    /// worlds, so every game must start from the same one: the fake games
    /// all take seed 0, and real games must load the same save.
    #[arg(long)]
    no_snapshots: bool,

    /// Simulation steps per second of the room.
    #[arg(long, default_value_t = RoomSettings::DEFAULT.steps_per_second)]
    step_rate: u16,

    /// The rules the room is played by; the server's default without.
    #[arg(long)]
    rules: Option<String>,

    /// The game's build every player declares.
    #[arg(long, default_value = "tpf3")]
    game_build: String,

    /// The game's mods in load order, one per line, for every player.
    #[arg(long)]
    mods: Option<PathBuf>,

    #[command(flatten)]
    tunnel: TunnelArgs,
}

/// What starting a player's game needs of the player.
#[derive(Clone)]
struct Seat {
    name: String,
    link: String,
    dir: PathBuf,
}

/// A game's task, aborted (which stops the game) when this is dropped.
struct AbortOnDrop(JoinHandle<Result<GameEnd>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One player: an agent and a game.
struct Player {
    name: String,
    link: String,
    dir: PathBuf,
    /// The agent, which stops when dropped.
    _launcher: Launcher,
    handle: LauncherHandle,
}

impl Player {
    fn seat(&self) -> Seat {
        Seat {
            name: self.name.clone(),
            link: self.link.clone(),
            dir: self.dir.clone(),
        }
    }
}

/// What a game left behind when it exited.
struct GameEnd {
    name: String,
    success: bool,
    status: String,
    /// The lane digest lines it printed, in order.
    lanes: Vec<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    match Box::pin(run(Args::parse())).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("rig: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<ExitCode> {
    if args.steps.is_some() && args.game != "fake" {
        bail!("--steps drives the fake game; give a real game its own options with --game-arg");
    }
    let root = match &args.data_root {
        Some(root) => root.clone(),
        None => std::env::temp_dir().join("tpf3mp-rig"),
    };
    let deadline = args
        .time_limit
        .map(|limit| Instant::now() + Duration::from_secs(limit));
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let game = GameCommand::new(&args)?;

    let (server, trust, local) = if args.server.eq_ignore_ascii_case("local") {
        let local = LocalServer::start(&root.join("server"), args.players, !args.no_snapshots)?;
        println!(
            "rig: local server on {}, certificate in {}",
            local.address,
            local.cert.display()
        );
        (local.address.clone(), local.trust.clone(), Some(local))
    } else {
        (
            args.server.clone(),
            setup::trust(args.pin_cert.as_deref())?,
            None,
        )
    };

    let content = content::manifest(&args.game_build, args.mods.as_deref())?;
    let tunnel = args.tunnel.choice()?;
    let room_settings = RoomSettings {
        steps_per_second: args.step_rate,
        ..RoomSettings::DEFAULT
    };
    let mut players = Vec::new();
    for number in 1..=args.players {
        let name = format!("p{number}");
        let dir = root.join(&name);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let identity = Arc::new(Identity::load_or_create(&dir.join("identity.key"))?);
        let worlds = Worlds::open(&dir.join("worlds"), WORLDS_BYTES)
            .with_context(|| format!("opening the worlds in {}", dir.display()))?;
        // The process ID keeps two rigs on one PC apart.
        let link = format!("tpf3mp.rig.{}.{name}", std::process::id());
        let launcher = Launcher::start_local(LauncherConfig {
            listen: ([127, 0, 0, 1], 0).into(),
            tunnel: tunnel.clone(),
            remember: None,
            server: Some(server.clone()),
            // Every player on the rig's server, as a package's on its own.
            server_fixed: true,
            default_server: Some(server.clone()),
            server_name: None,
            servers: Vec::new(),
            trust: trust.clone(),
            identity,
            name: name.clone(),
            content: content.clone(),
            mods: None,
            picker: None,
            native: None,
            // The rig's players run the fake game, not one Steam installed.
            installed: None,
            diagnostics: None,
            game_logs: None,
            hook: None,
            game_exe: None,
            game_env: Vec::new(),
            start_save: None,
            link: link.clone(),
            worlds,
            room_settings,
        });
        let handle = launcher.handle();
        players.push(Player {
            name,
            link,
            dir,
            _launcher: launcher,
            handle,
        });
    }

    let game = Arc::new(game);
    let mut games = Vec::new();
    let time_up = async {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending().await,
        }
    };
    let outcome = tokio::select! {
        outcome = async {
            start_games(&args, &game, &players, &mut games).await?;
            play(&args, &server, &players, &mut games).await
        } => outcome,
        _ = tokio::signal::ctrl_c() => {
            println!("rig: stopped");
            Ok(ExitCode::from(130))
        }
        () = time_up => Err(anyhow!(
            "stopped: the run took longer than its {} s",
            args.time_limit.unwrap_or_default()
        )),
    };
    // Aborting the games' tasks kills the games that still run. A game
    // still starting is ended as soon as its start returns: its guard is
    // dropped with the start's result.
    drop(games);
    for player in &players {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            player.handle.act(Action::Disconnect),
        )
        .await;
    }
    drop(players);
    if let Some(local) = local {
        local.stop().await;
    }
    outcome
}

/// Starts every player's game: the first ones now, one after another, and
/// each later one after its --stagger delay, on its own while the room is
/// set up (the players' links must be there before any game's hook gives
/// up on them).
async fn start_games(
    args: &Args,
    game: &Arc<GameCommand>,
    players: &[Player],
    games: &mut Vec<AbortOnDrop>,
) -> Result<()> {
    for (index, player) in players.iter().enumerate() {
        let delay = Duration::from_secs(args.stagger.saturating_mul(index as u64));
        let (game, seat) = (Arc::clone(game), player.seat());
        // Starting a real game blocks while its hook loads. The guard comes
        // back with the start, so a start nobody waits for any more ends
        // its game.
        let start = move || game.spawn(&seat, index).map(AbortOnDrop);
        if delay.is_zero() {
            games.push(
                tokio::task::spawn_blocking(start)
                    .await
                    .context("starting a game failed")??,
            );
            continue;
        }
        println!(
            "rig: starting {}'s game in {} s",
            player.name,
            delay.as_secs()
        );
        games.push(AbortOnDrop(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let mut started = tokio::task::spawn_blocking(start)
                .await
                .context("starting a game failed")??;
            (&mut started.0).await.context("a game's task failed")?
        })));
    }
    Ok(())
}

/// Sets up the room, then follows it until every game has exited.
async fn play(
    args: &Args,
    server: &str,
    players: &[Player],
    games: &mut [AbortOnDrop],
) -> Result<ExitCode> {
    let watch = tokio::spawn(watch(
        players
            .iter()
            .map(|player| (player.name.clone(), player.handle.clone()))
            .collect(),
    ));
    // Waiting for the games takes as long as they take to start.
    let setup_timeout = if args.wait_for_games {
        SETUP_TIMEOUT
            + GAME_START_TIMEOUT
            + Duration::from_secs(args.stagger.saturating_mul(u64::from(args.players)))
    } else {
        SETUP_TIMEOUT
    };
    let setup = tokio::time::timeout(setup_timeout, set_up_room(args, server, players)).await;
    match setup {
        Ok(result) => result?,
        Err(_) => bail!(
            "the room was not set up within {} seconds",
            setup_timeout.as_secs()
        ),
    }

    let mut ends = Vec::new();
    for game in games {
        ends.push((&mut game.0).await.context("a game's task failed")??);
    }
    watch.abort();
    Ok(report(&ends))
}

/// The host creates the room; everyone else joins its invite; all get
/// ready, and the host starts the game.
async fn set_up_room(args: &Args, server: &str, players: &[Player]) -> Result<()> {
    let (host, guests) = players.split_first().context("no players")?;
    act(
        host,
        Action::Connect {
            server: server.to_owned(),
            name: host.name.clone(),
        },
    )
    .await?;
    act(
        host,
        Action::Create {
            room: "rig".into(),
            max_players: args.players,
            password: None,
            rules: args.rules.clone(),
            start_save: None,
            listing: None,
            competitive: false,
        },
    )
    .await?;
    let invite = host
        .handle
        .state()
        .room
        .and_then(|room| room.invite)
        .context("the room has no invite")?;
    println!("rig: invite: {invite}");
    for guest in guests {
        // The invite's code where the server goes connects and joins.
        act(
            guest,
            Action::Connect {
                server: invite.clone(),
                name: guest.name.clone(),
            },
        )
        .await?;
    }
    if args.wait_for_games {
        println!("rig: waiting for every game to attach");
        while !players
            .iter()
            .all(|player| player.handle.state().game.attached.is_some())
        {
            tokio::time::sleep(POLL).await;
        }
    }
    for player in players {
        act(player, Action::Ready { ready: true }).await?;
    }
    let wanted = players.len();
    loop {
        let everyone_ready = host.handle.state().room.is_some_and(|room| {
            room.members.len() >= wanted && room.members.iter().all(|member| member.ready)
        });
        if everyone_ready {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    act(host, Action::Start).await?;
    println!("rig: game started with {wanted} players");
    Ok(())
}

async fn act(player: &Player, action: Action) -> Result<()> {
    let what = format!("{action:?}");
    player
        .handle
        .act(action)
        .await
        .map_err(|error| anyhow::anyhow!("{}: {what} was refused: {error}", player.name))
}

/// Prints what changes in each player's launcher: its game attaching,
/// loading and playing, its notices and errors.
async fn watch(players: Vec<(String, LauncherHandle)>) {
    let mut seen: Vec<(String, usize, Option<String>)> =
        vec![(String::new(), 0, None); players.len()];
    loop {
        for ((name, handle), seen) in players.iter().zip(&mut seen) {
            let state = handle.state();
            let game = describe_game(&state);
            if game != seen.0 {
                println!("[{name}] {game}");
                seen.0 = game;
            }
            for notice in state.notices.iter().skip(seen.1) {
                println!("[{name}] notice: {notice}");
            }
            seen.1 = state.notices.len();
            if state.error != seen.2 {
                if let Some(error) = &state.error {
                    println!("[{name}] error: {error}");
                }
                seen.2 = state.error;
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

fn describe_game(state: &State) -> String {
    match &state.game.attached {
        Some(build) => format!("game {build} attached, world {:?}", state.game.world),
        None => "waiting for the game".into(),
    }
}

/// Says how the games ended, and whether they agree.
fn report(ends: &[GameEnd]) -> ExitCode {
    let mut success = true;
    for end in ends {
        if !end.success {
            println!("rig: {}'s game failed: {}", end.name, end.status);
            success = false;
        }
    }
    match agreement(ends) {
        Agreement::Same => println!(
            "rig: all {} games ended on the same lane digests",
            ends.len()
        ),
        Agreement::Differ => {
            println!("rig: the games ended on different lane digests");
            for end in ends {
                println!("  {}: {}", end.name, end.lanes.join(", "));
            }
            success = false;
        }
        Agreement::Unknown => println!("rig: the games did not report their lanes"),
    }
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Agreement {
    Same,
    Differ,
    /// Not every game printed its lanes.
    Unknown,
}

fn agreement(ends: &[GameEnd]) -> Agreement {
    let Some(first) = ends.first() else {
        return Agreement::Unknown;
    };
    if ends.iter().any(|end| end.lanes.is_empty()) {
        Agreement::Unknown
    } else if ends.iter().all(|end| end.lanes == first.lanes) {
        Agreement::Same
    } else {
        Agreement::Differ
    }
}

/// A lane digest line of the fake game's report, such as `  lane 0:
/// 0123456789abcdef`, trimmed.
fn lane_line(line: &str) -> Option<&str> {
    let line = line.trim();
    let (lane, digest) = line.strip_prefix("lane ")?.split_once(": ")?;
    let valid = lane.bytes().all(|byte| byte.is_ascii_digit())
        && !digest.is_empty()
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit());
    valid.then_some(line)
}

/// How each player's game is started.
struct GameCommand {
    program: PathBuf,
    steps: Option<u64>,
    /// Every fake game takes seed 0, for a server that keeps no worlds.
    same_world: bool,
    args: Vec<OsString>,
    /// Profiles copied into each player's folder for a real game's hook.
    profiles: Option<PathBuf>,
    /// The hook loaded into a real game; `None` for the fake game.
    hook: Option<PathBuf>,
    /// How long a real game waits, suspended, for its hook to be ready.
    ready_wait: Duration,
}

/// A real game started with the hook, ended when dropped: the rig stops
/// what it started.
struct RealGame(tpf3mp_launch::Started);

impl Drop for RealGame {
    fn drop(&mut self) {
        if self.0.is_running() {
            self.0.kill();
        }
    }
}

impl GameCommand {
    fn new(args: &Args) -> Result<Self> {
        let fake = args.game == "fake";
        let program = if fake {
            let me = std::env::current_exe().context("finding this program")?;
            let fakegame =
                me.with_file_name(format!("tpf3mp-fakegame{}", std::env::consts::EXE_SUFFIX));
            if !fakegame.is_file() {
                bail!(
                    "{} is missing: build it with `cargo build -p tpf3mp-testkit --bin tpf3mp-fakegame`",
                    fakegame.display()
                );
            }
            fakegame
        } else {
            let game = PathBuf::from(&args.game);
            if !game.is_file() {
                bail!("{} is not a game executable", game.display());
            }
            game
        };
        // A real game's hook finds its build profiles in its data folder;
        // each player's starts with the ones the user has.
        let profiles = (!fake)
            .then(|| setup::data_dir().ok().map(|dir| dir.join("profiles")))
            .flatten()
            .filter(|dir| dir.is_dir());
        // A real game runs TPF3-MP only with the hook loaded into it (D11).
        let hook = if fake {
            None
        } else if !tpf3mp_launch::SUPPORTED {
            bail!("{}", tpf3mp_launch::LaunchError::Unsupported);
        } else {
            let hook = args
                .hook
                .clone()
                .or_else(setup::package_hook)
                .with_context(|| {
                    format!(
                        "no {} next to this program: build it with `cargo build -p tpf3mp-hook`, or give --hook",
                        tpf3mp_launch::HOOK_FILE
                    )
                })?;
            Some(hook)
        };
        Ok(Self {
            program,
            steps: args.steps,
            same_world: args.no_snapshots,
            args: args.game_args.clone(),
            profiles,
            hook,
            ready_wait: Duration::from_secs(args.hook_ready_wait),
        })
    }

    /// Starts `player`'s game; the task ends with it, and kills it when
    /// dropped.
    fn spawn(&self, player: &Seat, index: usize) -> Result<JoinHandle<Result<GameEnd>>> {
        if let Some(profiles) = &self.profiles {
            copy_profiles(profiles, &player.dir.join("profiles"))?;
        }
        if let Some(hook) = &self.hook {
            return self.launch(player, hook);
        }
        // The fake game, told its link, seed and steps on its command line.
        let mut command = Command::new(&self.program);
        let seed = if self.same_world { 0 } else { index };
        command
            .arg(&player.link)
            .arg("--seed")
            .arg(seed.to_string());
        if let Some(steps) = self.steps {
            command.arg("--steps").arg(steps.to_string());
        }
        command
            .args(&self.args)
            .env(LINK_ENV, &player.link)
            .env(DATA_DIR_ENV, &player.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", self.program.display()))?;
        println!(
            "rig: {} plays on link {} with its files in {}",
            player.name,
            player.link,
            player.dir.display()
        );
        let name = player.name.clone();
        let (lanes_tx, lanes_rx) = oneshot::channel();
        let stdout = child.stdout.take().context("the game's output")?;
        let stderr = child.stderr.take().context("the game's errors")?;
        let out = tokio::spawn(relay(name.clone(), stdout, Some(lanes_tx)));
        let err = tokio::spawn(relay(name.clone(), stderr, None));
        Ok(tokio::spawn(async move {
            let status = child.wait().await.context("waiting for a game")?;
            let _ = tokio::join!(out, err);
            Ok(GameEnd {
                name,
                success: status.success(),
                status: status.to_string(),
                lanes: lanes_rx.await.unwrap_or_default(),
            })
        }))
    }

    /// Starts a real game as the launcher does, with the hook in it; the
    /// task ends with the game, and ends the game when dropped.
    fn launch(&self, player: &Seat, hook: &Path) -> Result<JoinHandle<Result<GameEnd>>> {
        let dir = player
            .dir
            .to_str()
            .context("the player's folder is not valid Unicode")?;
        let started = tpf3mp_launch::start(&tpf3mp_launch::Launch {
            exe: self.program.clone(),
            args: self
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            hook: hook.to_owned(),
            env: vec![
                (LINK_ENV.to_owned(), player.link.clone()),
                (DATA_DIR_ENV.to_owned(), dir.to_owned()),
                (LAUNCHER_PID_ENV.to_owned(), std::process::id().to_string()),
            ],
            ready_wait: self.ready_wait,
        })
        .with_context(|| format!("starting {}", self.program.display()))?;
        println!(
            "rig: {} plays on link {} with its files in {} (game pid {})",
            player.name,
            player.link,
            player.dir.display(),
            started.pid
        );
        let name = player.name.clone();
        let mut game = RealGame(started);
        Ok(tokio::spawn(async move {
            // Its output is its own (the launcher does not read a game's), so
            // a real game reports no lanes here.
            let code = loop {
                if let Some(code) = game.0.exit_code() {
                    break code;
                }
                tokio::time::sleep(POLL).await;
            };
            Ok(GameEnd {
                name,
                success: code == 0,
                status: format!("exit code {code}"),
                lanes: Vec::new(),
            })
        }))
    }
}

/// Prints a game's output under its player's name, and collects its lane
/// digest lines.
async fn relay(
    name: String,
    stream: impl AsyncRead + Unpin,
    lanes: Option<oneshot::Sender<Vec<String>>>,
) {
    let mut lines = BufReader::new(stream).lines();
    let mut found = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        println!("[{name}] {line}");
        if let Some(lane) = lane_line(&line) {
            found.push(lane.to_owned());
        }
    }
    if let Some(lanes) = lanes {
        let _ = lanes.send(found);
    }
}

/// Copies the `*.toml` profiles in `from` to `to`, replacing older copies.
fn copy_profiles(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "toml")
            && let Some(file) = path.file_name()
        {
            fs::copy(&path, to.join(file))
                .with_context(|| format!("copying {}", path.display()))?;
        }
    }
    Ok(())
}

/// A throwaway development server in this process.
struct LocalServer {
    address: String,
    trust: ServerTrust,
    cert: PathBuf,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl LocalServer {
    /// Listens on a free loopback port, keeping world snapshots in `dir` so
    /// players can join running games (unless `snapshots` is false), and
    /// writes its certificate there for agents started by hand.
    fn start(dir: &Path, players: u8, snapshots: bool) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let identity = ServerIdentity::self_signed(&["localhost", "127.0.0.1", "::1"])?;
        let cert = dir.join("dev-cert.der");
        fs::write(&cert, identity.leaf()).with_context(|| format!("writing {}", cert.display()))?;
        let trust = ServerTrust::Pinned(identity.leaf().clone());
        let mut config = ServerConfig::new(([127, 0, 0, 1], 0).into(), identity);
        // Every player connects from this PC, one address; leave room for
        // a few more started by hand.
        let players = usize::from(players);
        config.max_sessions_per_address = players + 8;
        config.max_handshakes_per_address = players + 8;
        if snapshots {
            let mut snapshots = SnapshotConfig::new(dir.join("snapshots"));
            snapshots.max_bytes = 4 << 30;
            snapshots.min_gap = Duration::from_secs(5);
            config.snapshots = Some(snapshots);
        }
        let server = Server::bind(config)?;
        let address = server.local_addr()?.to_string();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        Ok(Self {
            address,
            trust,
            cert,
            stop,
            task,
        })
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        let _ = tokio::time::timeout(Duration::from_secs(10), self.task).await;
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn lane_lines_are_recognised() {
        assert_eq!(lane_line("  lane 0: 0123abcd"), Some("lane 0: 0123abcd"));
        assert_eq!(lane_line("lane 12: ff"), Some("lane 12: ff"));
        assert_eq!(lane_line("ran 100 steps"), None);
        assert_eq!(lane_line("  lane x: 00"), None);
        assert_eq!(lane_line("  lane 1: not hex"), None);
    }

    fn ended(lanes: &[&str]) -> GameEnd {
        GameEnd {
            name: "p".into(),
            success: true,
            status: String::new(),
            lanes: lanes.iter().map(|lane| (*lane).to_owned()).collect(),
        }
    }

    #[test]
    fn games_agree_only_on_the_same_lanes() {
        let same = [ended(&["lane 0: aa"]), ended(&["lane 0: aa"])];
        assert_eq!(agreement(&same), Agreement::Same);
        let differ = [ended(&["lane 0: aa"]), ended(&["lane 0: bb"])];
        assert_eq!(agreement(&differ), Agreement::Differ);
        let silent = [ended(&["lane 0: aa"]), ended(&[])];
        assert_eq!(agreement(&silent), Agreement::Unknown);
        assert_eq!(agreement(&[]), Agreement::Unknown);
    }
}
