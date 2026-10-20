//! A launcher's setup from its command line, shared by `tpf3mp-agent
//! launcher` (the page in a browser) and `tpf3mp-launcher` (the native
//! window): the same options give the same player, server and game.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use clap::Args;
use tpf3mp_net::{CertificateDer, Identity, ServerTrust, tunnel::TunnelUrl};
use tpf3mp_proto::RoomSettings;

use super::{LauncherConfig, Remembered};
use crate::{TunnelChoice, Worlds, content, steam};

/// The game build a player declares: the one given, else Steam's build ID
/// of the installed game as `steam-<ID>`, else `tpf3`, which players
/// without the game (a playtest with the fake game) share.
pub fn game_build(given: Option<&str>, installed: Option<&steam::Installed>) -> String {
    match (given, installed) {
        (Some(given), _) => given.to_owned(),
        (None, Some(installed)) => format!("steam-{}", installed.build),
        (None, None) => "tpf3".to_owned(),
    }
}

/// This fork's development default. Public packages must explicitly name
/// their operated server with `TPF3MP_DEFAULT_SERVER`; local sessions use a
/// pinned certificate. Never fall back to the upstream production relay.
pub const RELAY: &str = "127.0.0.1:29470";
/// What players see of [`RELAY`] in place of its address.
pub const RELAY_NAME: &str = "Local";

/// Where the per-user files live: `TPF3-MP` in the user's local data
/// directory.
pub fn data_dir() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .context("this system has no per-user data directory")?
        .join("TPF3-MP"))
}

/// The identity key file: the one given, or the per-user default.
pub fn identity_path(given: Option<&Path>) -> Result<PathBuf> {
    match given {
        Some(path) => Ok(path.to_owned()),
        None => Ok(data_dir().context("pass --identity")?.join("identity.key")),
    }
}

/// The worlds kept for the game on `link`: in `dir`, or in a directory per
/// link in the per-user data directory.
pub fn open_worlds(dir: Option<&Path>, gib: u64, link: &str) -> Result<Worlds> {
    let dir = match dir {
        Some(dir) => dir.to_owned(),
        None => data_dir()
            .context("pass --worlds")?
            .join("worlds")
            .join(link),
    };
    Worlds::open(&dir, gib << 30)
        .with_context(|| format!("opening the worlds in {}", dir.display()))
}

/// How servers are trusted: exactly the certificate in `pin_cert`, or the
/// public certificate authorities.
pub fn trust(pin_cert: Option<&Path>) -> Result<ServerTrust> {
    match pin_cert {
        Some(path) => Ok(ServerTrust::Pinned(CertificateDer::from(
            std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
        ))),
        None => Ok(ServerTrust::WebPki),
    }
}

/// Opens `url` in the default browser, as far as the system allows.
/// Returns whether a browser could be started.
pub fn open_in_browser(url: &str) -> bool {
    open_with_system(url)
}

fn open_with_system(target: &str) -> bool {
    // Explorer opens folders and addresses alike, without the console
    // window `cmd /C start` would flash from a windowed program.
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(target)
        .spawn()
        .is_ok()
}

/// For networks that block UDP: QUIC through a WebSocket tunnel.
#[derive(Debug, Clone, Default, Args)]
pub struct TunnelArgs {
    /// The tunnel to take when UDP gets no answer, as a wss:// URL.
    /// Defaults to wss://<server host>/tpf3mp.
    #[arg(long)]
    pub tunnel: Option<String>,

    /// Connect through the tunnel only, never over UDP.
    #[arg(long, conflicts_with = "no_tunnel")]
    pub tunnel_only: bool,

    /// Never take a tunnel.
    #[arg(long, conflicts_with = "tunnel")]
    pub no_tunnel: bool,
}

impl TunnelArgs {
    pub fn choice(&self) -> Result<TunnelChoice> {
        if self.no_tunnel {
            return Ok(TunnelChoice::Off);
        }
        let url = self
            .tunnel
            .as_deref()
            .map(|url| {
                url.parse::<TunnelUrl>()
                    .with_context(|| format!("the tunnel URL {url}"))
            })
            .transpose()?;
        Ok(match (url, self.tunnel_only) {
            (url, true) => TunnelChoice::Only(url),
            (Some(url), false) => TunnelChoice::Url(url),
            (None, false) => TunnelChoice::Default,
        })
    }
}

/// What the player, the server and the game are.
#[derive(Debug, Clone, Args)]
pub struct LauncherArgs {
    /// Where the page is served, when there is one. Loopback only.
    #[arg(long, default_value = "127.0.0.1:47470")]
    pub listen: SocketAddr,

    /// The server to play on this run, as host:port, over the player's
    /// server setting: for development and playtests. Without it, the
    /// player's setting, else the default server.
    #[arg(long)]
    pub server: Option<String>,

    /// The launcher's default server, which its server setting's "Reset to
    /// default" goes back to: the one the package is built for. Without it,
    /// --server or a server setting, the player types the server, and the
    /// last one is offered.
    #[arg(long)]
    pub default_server: Option<String>,

    /// What players see of the default server, such as EU, in place of its
    /// address.
    #[arg(long)]
    pub server_name: Option<String>,

    /// The release's other servers, besides the default, as
    /// NAME=host:port separated by commas (`TPF3MP_SERVERS` in a
    /// package). With them, the launcher lists the rooms of all, and the
    /// rooms its player creates go to the closest; invites may name only
    /// these. Not with --server, which plays on that one alone.
    #[arg(long, value_name = "LIST")]
    pub more_servers: Option<String>,

    /// Trust exactly this DER certificate instead of public certificate
    /// authorities (for development servers).
    #[arg(long)]
    pub pin_cert: Option<PathBuf>,

    /// The name offered first. Without it, the name last used.
    #[arg(long)]
    pub name: Option<String>,

    /// Identity key file. Created on first use.
    #[arg(long)]
    pub identity: Option<PathBuf>,

    #[command(flatten)]
    pub tunnel: TunnelArgs,

    /// The shared-memory link the game's hook opens.
    #[arg(long, default_value = tpf3mp_bridge::DEFAULT_LINK)]
    pub game_link: String,

    /// The game's build. Every player in a room must run the same.
    /// Without it, Steam's build ID of the installed game (see
    /// [`game_build`]).
    #[arg(long)]
    pub game_build: Option<String>,

    /// The game's program, when it is not found in the folder Steam
    /// installed the game in. Its folder is then the game's, with the
    /// build Steam records for it when it is in a Steam library (under
    /// Proton, not the native Linux game Steam may list).
    #[arg(long)]
    pub game_exe: Option<PathBuf>,

    /// A file listing the game's active mods in load order, one per line:
    /// the mod's name, then its version. Each is scanned: personal ones
    /// (GUI mods) may differ between the room's players (docs/MODS.md).
    #[arg(long)]
    pub mods: Option<PathBuf>,

    /// Count game-script mods whose commands the room carries (a timetable
    /// mod, a line namer) as personal too. For playtests until the room
    /// lets them differ (docs/MODS.md, proposed D25).
    #[arg(long)]
    pub personal_game_scripts: bool,

    /// Where worlds are kept. Defaults to the per-user data directory.
    #[arg(long)]
    pub worlds: Option<PathBuf>,

    /// Space worlds may take, in GiB.
    #[arg(long, default_value_t = 8)]
    pub worlds_gib: u64,
}

/// The hook library in the package: next to this program.
pub fn package_hook() -> Option<PathBuf> {
    let hook = std::env::current_exe()
        .ok()?
        .parent()?
        .join(tpf3mp_launch::HOOK_FILE);
    hook.is_file().then_some(hook)
}

/// This player's mods, sorted for the room (`content::split`): each listed
/// mod looked for among those installed (Mod Hub's, the Steam accounts'
/// local ones, the game's own) and scanned. What each mod was taken for
/// goes to the log. `carried_personal`: game-script mods whose commands the
/// room carries count as personal (`--personal-game-scripts`).
pub fn split_mods(
    game_build: &str,
    mods: Option<&std::path::Path>,
    installed: Option<&steam::Installed>,
    carried_personal: bool,
) -> Result<content::Split> {
    let found = if mods.is_some() {
        tpf3mp_modscan::roots::installed(&tpf3mp_modscan::roots::default_roots(
            installed.map(|game| game.dir.as_path()),
            &steam::steam_roots(),
        ))
    } else {
        Vec::new()
    };
    let split = content::split(game_build, mods, &found, carried_personal)?;
    for verdict in &split.verdicts {
        tracing::info!(
            "mod {} {} is {}: {}",
            verdict.listed.id,
            verdict.listed.version,
            verdict.class,
            verdict.why
        );
    }
    if mods.is_some() && split.lists.is_none() {
        tracing::warn!(
            "more mods listed than the room's worlds can be loaded with; they load with their saves' mods"
        );
    }
    Ok(split)
}

/// The server a launcher plays on (D12, as amended): the one `given` on its
/// command line, else the player's `chosen` setting, else the `default`.
/// A setting that is no `host:port` (an old or edited file) is passed over.
pub fn plays_on(
    given: Option<&str>,
    chosen: Option<&str>,
    default: Option<&str>,
) -> Option<String> {
    let named = |server: Option<&str>| {
        server
            .map(str::trim)
            .filter(|server| !server.is_empty())
            .map(str::to_owned)
    };
    named(given)
        .or_else(|| chosen.and_then(|chosen| super::server_address(chosen).ok()))
        .or_else(|| named(default))
}

/// An old setting may name an arbitrary server, from before a release had a
/// compiled regional list. Keep a remembered pin only when that release
/// vouches for it; `--server` builds have an empty list and keep their old
/// development behavior.
fn remembered_server<'a>(
    chosen: Option<&'a str>,
    listed: &[super::ListedServer],
) -> Option<&'a str> {
    if listed.len() >= 2 {
        chosen.filter(|server| super::servers::find(listed, server).is_some())
    } else {
        chosen
    }
}

/// The servers a launcher plays on (D12's approved regional amendment):
/// none with a server `given` on its command line, which it
/// plays on alone; else its `default`, named `name`, then the `more` a
/// release lists. A list that does not read stops the launcher: it never
/// guesses where players meet.
pub fn listed_servers(
    given: Option<&str>,
    default: Option<&str>,
    name: Option<&str>,
    more: Option<&str>,
) -> Result<Vec<super::ListedServer>> {
    if given.is_some_and(|given| !given.trim().is_empty()) {
        return Ok(Vec::new());
    }
    let more = super::servers::parse_list(more.unwrap_or_default())
        .map_err(|why| anyhow::anyhow!("the release's servers: {why}"))?;
    super::servers::release_list(default, name, &more)
        .map_err(|why| anyhow::anyhow!("the release's servers: {why}"))
}

impl LauncherArgs {
    /// The launcher's default server, if it has one.
    fn default_server(&self) -> Option<String> {
        self.default_server
            .as_deref()
            .map(str::trim)
            .filter(|server| !server.is_empty())
            .map(str::to_owned)
    }

    /// The launcher these options describe: the player's identity (created
    /// on first use), with the server and name remembered from last time
    /// where none are given, and the game's content and worlds.
    pub fn config(&self) -> Result<LauncherConfig> {
        let installed = steam::find_game(self.game_exe.as_deref());
        let identity_file = identity_path(self.identity.as_deref())?;
        let identity = Arc::new(Identity::load_or_create(&identity_file)?);
        // Next to the identity: the same player's last server and name.
        let remember = identity_file.with_file_name("launcher.json");
        let remembered = Remembered::load(&remember);
        let build = game_build(self.game_build.as_deref(), installed.as_ref());
        // The campaign's portraits, from this player's own install into the
        // installed mod, once per game build (docs/LOBBY.md, "Portraits").
        match installed.as_ref() {
            Some(game) => crate::portraits::prepare_installed(&game.dir, &build),
            None => tracing::info!("portraits: the game is not installed; no portraits"),
        }
        let split = split_mods(
            &build,
            self.mods.as_deref(),
            installed.as_ref(),
            self.personal_game_scripts,
        )?;
        // Without --mods, the launcher finds the player's mods itself and
        // lets them choose their personal ones (docs/MODS.md).
        let picker = self.mods.is_none().then(|| {
            let found = crate::picker::discover(
                installed.as_ref().map(|game| game.dir.as_path()),
                &steam::steam_roots(),
            );
            for m in &found {
                tracing::info!("mod {} {} is {}: {}", m.id, m.version, m.class, m.reason);
            }
            crate::picker::Mods::new(
                tpf3mp_proto::Text::lossy(build.trim()),
                found,
                remembered.mods.clone().unwrap_or_default(),
                self.personal_game_scripts,
            )
        });
        let default_server = self.default_server();
        let server_name = self
            .server_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let servers = listed_servers(
            self.server.as_deref(),
            default_server.as_deref(),
            server_name.as_deref(),
            self.more_servers.as_deref(),
        )?;
        let server = plays_on(
            self.server.as_deref(),
            remembered_server(remembered.chosen_server.as_deref(), &servers),
            default_server.as_deref(),
        );
        Ok(LauncherConfig {
            // The launcher window sets it: it records the player's log.
            diagnostics: None,
            // The hook's log in the per-user folder, unless a playtest's
            // game keeps its own (`TPF3MP_DATA_DIR` in `game_env`).
            game_logs: data_dir()
                .ok()
                .map(|dir| crate::game_logs::Places::of_this_computer(&dir)),
            hook: package_hook(),
            game_exe: self.game_exe.clone(),
            game_env: Vec::new(),
            listen: self.listen,
            server_fixed: server.is_some(),
            server: server.or(remembered.server),
            default_server,
            server_name,
            servers,
            tunnel: self.tunnel.choice()?,
            remember: Some(remember),
            trust: trust(self.pin_cert.as_deref())?,
            identity,
            name: self
                .name
                .clone()
                .or(remembered.name)
                // Unchosen, the player is called what Steam calls them.
                .or_else(crate::steam::persona_name)
                .unwrap_or_else(|| "player".to_owned()),
            content: split.manifest,
            mods: split.lists,
            picker,
            native: None,
            installed,
            link: self.game_link.clone(),
            worlds: open_worlds(self.worlds.as_deref(), self.worlds_gib, &self.game_link)?,
            room_settings: RoomSettings::DEFAULT,
            start_save: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_game_build_comes_from_the_player_then_steam() {
        let installed = steam::Installed {
            app: steam::TRANSPORT_FEVER_3,
            name: "Transport Fever 3".into(),
            dir: PathBuf::from("games/Transport Fever 3"),
            build: "20412345".into(),
        };
        assert_eq!(game_build(Some("beta"), Some(&installed)), "beta");
        assert_eq!(game_build(None, Some(&installed)), "steam-20412345");
        assert_eq!(game_build(None, None), "tpf3");
    }

    #[test]
    fn the_command_line_then_the_players_setting_then_the_default() {
        let relay = Some(RELAY);
        let chosen = Some("play.example.net:29470");
        assert_eq!(
            plays_on(Some(" 127.0.0.1:29470 "), chosen, relay).as_deref(),
            Some("127.0.0.1:29470"),
            "a playtest's --server"
        );
        assert_eq!(plays_on(None, chosen, relay).as_deref(), chosen);
        assert_eq!(plays_on(None, None, relay).as_deref(), relay);
        assert_eq!(
            plays_on(Some(" "), Some("not a server"), relay).as_deref(),
            relay,
            "nothing given, and a setting that is no host:port"
        );
        assert_eq!(plays_on(None, None, None), None);
    }

    #[test]
    fn a_regional_release_ignores_a_remembered_server_outside_its_list() {
        let listed = listed_servers(
            None,
            Some(RELAY),
            Some(RELAY_NAME),
            Some("US=us.example.org:29470"),
        )
        .unwrap();
        assert_eq!(
            remembered_server(Some("old.example.org:29470"), &listed),
            None,
            "a pre-region arbitrary server is ignored at startup"
        );
        assert_eq!(
            plays_on(
                None,
                remembered_server(Some("old.example.org:29470"), &listed),
                Some(RELAY),
            )
            .as_deref(),
            Some(RELAY),
            "the regional release starts on its trusted default"
        );
        assert_eq!(
            remembered_server(Some("US.EXAMPLE.ORG:29470"), &listed),
            Some("US.EXAMPLE.ORG:29470"),
            "a trusted explicit region pin remains available"
        );

        let dev_servers = listed_servers(
            Some("127.0.0.1:29470"),
            Some(RELAY),
            Some(RELAY_NAME),
            Some("US=us.example.org:29470"),
        )
        .unwrap();
        assert!(
            dev_servers.is_empty(),
            "--server remains a single-server override"
        );
        assert_eq!(
            plays_on(
                Some("127.0.0.1:29470"),
                remembered_server(Some("old.example.org:29470"), &dev_servers),
                Some(RELAY),
            )
            .as_deref(),
            Some("127.0.0.1:29470")
        );
    }

    #[test]
    fn the_release_lists_its_default_first_and_server_pins_one() {
        let more = Some("US=us.example.org:29470, ASIA=asia.example.org:29470");
        let listed = listed_servers(None, Some(RELAY), Some(RELAY_NAME), more).unwrap();
        let names: Vec<&str> = listed.iter().map(|server| server.name.as_str()).collect();
        assert_eq!(
            names,
            [RELAY_NAME, "US", "ASIA"],
            "the default first, then the other servers in release order"
        );
        assert_eq!(listed[0].address, RELAY);
        assert!(
            listed_servers(Some("127.0.0.1:29470"), Some(RELAY), None, more)
                .unwrap()
                .is_empty(),
            "--server plays on one server alone"
        );
        assert_eq!(
            listed_servers(None, Some(RELAY), Some("EU"), None)
                .unwrap()
                .len(),
            1,
            "a release with its default alone plays as before"
        );
        assert!(
            listed_servers(None, Some(RELAY), None, Some("US=not a server")).is_err(),
            "a list that does not read stops the launcher"
        );
        assert!(
            listed_servers(
                None,
                Some(RELAY),
                Some(RELAY_NAME),
                Some("US=us.example.org:99999")
            )
            .is_err(),
            "the release validator and runtime use the same port parser"
        );
        let duplicate_name = format!("{RELAY_NAME}=another.example.org:29470");
        assert!(
            listed_servers(None, Some(RELAY), Some(RELAY_NAME), Some(&duplicate_name)).is_err(),
            "the default name cannot be reused"
        );
        assert_eq!(super::super::server_address(RELAY).as_deref(), Ok(RELAY));
    }
}
