//! Finding a game Steam installed: its folder and its build.
//!
//! Steam keeps, in each library, `steamapps/appmanifest_<app>.acf`, which
//! names the app's folder under `steamapps/common` and the build installed,
//! and in its own folder `steamapps/libraryfolders.vdf`, which lists every
//! library. Both are in Valve's text KeyValues format, read here.
//!
//! Everything here only reads, and gives up quietly: a player without
//! Steam, or with the game elsewhere, simply has no game found.

use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

/// Transport Fever 3's Steam app ID.
pub const TRANSPORT_FEVER_3: u32 = 3_493_540;
/// Largest Steam file read.
const MAX_FILE: u64 = 1 << 20;
/// Deepest nesting read.
const MAX_DEPTH: usize = 32;

/// A game Steam installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub app: u32,
    /// The game's name, as Steam shows it.
    pub name: String,
    /// The game's folder.
    pub dir: PathBuf,
    /// Steam's build ID of the installed game: the same for every platform
    /// once a player has the latest update.
    pub build: String,
}

/// `app`, if Steam installed it on this machine.
pub fn find(app: u32) -> Option<Installed> {
    find_in(&steam_roots(), app)
}

/// `app`, in the libraries of the Steam installations at `roots`.
pub fn find_in(roots: &[PathBuf], app: u32) -> Option<Installed> {
    find_in_where(roots, app, |_| true)
}

/// The first install of `app` in the libraries of the Steam installations
/// at `roots` that `fits`.
fn find_in_where(
    roots: &[PathBuf],
    app: u32,
    mut fits: impl FnMut(&Installed) -> bool,
) -> Option<Installed> {
    for root in roots {
        for library in libraries(root) {
            if let Some(installed) = installed_in(&library, app).filter(&mut fits) {
                return Some(installed);
            }
        }
    }
    None
}

/// Transport Fever 3 as the launcher runs it. With `game_exe` (the
/// launcher's `--game-exe`), that program's own folder, with Steam's
/// record of it when the folder is a Steam library's. Without, the install
/// Steam lists whose program runs on this system: under Proton, Steam's
/// libraries can hold the native Linux game, which the Windows launcher
/// cannot start, and whose build is not the Windows game's.
pub fn find_game(game_exe: Option<&Path>) -> Option<Installed> {
    match game_exe {
        Some(exe) => {
            let exe = std::path::absolute(exe).unwrap_or_else(|_| exe.to_owned());
            exe.parent().map(installed_at)
        }
        None => find_in_where(&steam_roots(), TRANSPORT_FEVER_3, |found| {
            let runs = tpf3mp_launch::find_executable(&found.dir).is_some();
            if !runs {
                tracing::info!(
                    "passing over {}: it has no game program this system runs \
                     (the native Linux game, seen from Proton?)",
                    found.dir.display()
                );
            }
            runs
        }),
    }
}

/// Transport Fever 3 in the folder `dir`: as Steam's manifest records it
/// when `dir` is `<library>/steamapps/common/<folder>`, else with no build
/// known.
pub fn installed_at(dir: &Path) -> Installed {
    let recorded = (|| {
        let common = dir.parent()?;
        let steamapps = common.parent()?;
        if !common.file_name()?.eq_ignore_ascii_case("common")
            || !steamapps.file_name()?.eq_ignore_ascii_case("steamapps")
        {
            return None;
        }
        installed_in(steamapps.parent()?, TRANSPORT_FEVER_3)
            .filter(|installed| installed.dir.file_name() == dir.file_name())
    })();
    recorded.unwrap_or_else(|| Installed {
        app: TRANSPORT_FEVER_3,
        name: "Transport Fever 3".to_owned(),
        dir: dir.to_owned(),
        build: "unknown".to_owned(),
    })
}

fn installed_in(library: &Path, app: u32) -> Option<Installed> {
    let steamapps = library.join("steamapps");
    let text = read_small(&steamapps.join(format!("appmanifest_{app}.acf")))?;
    let manifest = parse(&text).ok()?;
    let state = manifest.map("AppState")?;
    let folder = state.text("installdir")?;
    // One plain folder name: nothing that leaves `common`.
    let mut parts = Path::new(folder).components();
    let (Some(Component::Normal(_)), None) = (parts.next(), parts.next()) else {
        return None;
    };
    let dir = steamapps.join("common").join(folder);
    dir.is_dir().then(|| Installed {
        app,
        name: state.text("name").unwrap_or(folder).to_owned(),
        dir,
        build: state.text("buildid").unwrap_or("unknown").to_owned(),
    })
}

/// The libraries of the Steam installation at `root`: its own folder, and
/// every one `libraryfolders.vdf` lists.
fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.to_owned()];
    let listed = read_small(&root.join("steamapps").join("libraryfolders.vdf"))
        .and_then(|text| parse(&text).ok());
    if let Some(folders) = listed
        .as_ref()
        .and_then(|listed| listed.map("libraryfolders"))
    {
        for path in listed_library_paths(folders, cfg!(windows)) {
            if path.is_absolute() && !found.contains(&path) {
                found.push(path);
            }
        }
    }
    found
}

/// Linux Steam's VDF still contains Unix paths when the reader runs under
/// Proton. Convert each library through Wine's Z: mapping before applying
/// Windows's absolute-path check, as we already do for the client root.
fn listed_library_paths(folders: &Block, windows: bool) -> Vec<PathBuf> {
    folders
        .0
        .values()
        .filter_map(|value| {
            // Current Steam: a block with a "path"; older Steam: the path.
            let path = match value {
                Value::Map(folder) => folder.text("path"),
                Value::Text(path) => Some(path.as_str()),
            }?;
            Some(if windows {
                wine_path(path).unwrap_or_else(|| PathBuf::from(path))
            } else {
                PathBuf::from(path)
            })
        })
        .collect()
}

/// Where Steam may be installed on this system, the likeliest first.
pub fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(windows)]
    {
        if let Some(path) = windows_steam_path() {
            roots.push(path);
        }
        for base in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(base) = std::env::var_os(base) {
                roots.push(PathBuf::from(base).join("Steam"));
            }
        }
        // Under Proton, the registry names Proton's stand-in for Steam,
        // which has no accounts' folders: the Steam client Proton runs for
        // has them.
        if let Some(path) = std::env::var("STEAM_COMPAT_CLIENT_INSTALL_PATH")
            .ok()
            .and_then(|path| wine_path(&path))
        {
            roots.push(path);
        }
    }
    if let Some(home) = dirs::home_dir() {
        if cfg!(target_os = "macos") {
            roots.push(home.join("Library/Application Support/Steam"));
        } else if cfg!(target_os = "linux") {
            for path in [
                ".steam/steam",
                ".local/share/Steam",
                ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                "snap/steam/common/.local/share/Steam",
            ] {
                roots.push(home.join(path));
            }
        }
    }
    let mut unique = Vec::new();
    for root in roots {
        let root = fs::canonicalize(&root).unwrap_or(root);
        if root.is_dir() && !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

/// Wine's name for the Unix path `unix`, through the drive Z: that Wine
/// and Proton map to `/`; none for a path that is not a Unix one.
pub fn wine_path(unix: &str) -> Option<PathBuf> {
    unix.starts_with('/')
        .then(|| PathBuf::from(format!("Z:{}", unix.replace('/', "\\"))))
}

/// Steam's folder as the registry names it, for installations outside
/// Program Files.
#[cfg(windows)]
fn windows_steam_path() -> Option<PathBuf> {
    let path = registry_value(r"HKCU\Software\Valve\Steam", "SteamPath", "REG_SZ")?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The Steam account playing, as the registry names it; none on other
/// systems, or when Steam is not running.
fn active_account() -> Option<u32> {
    #[cfg(windows)]
    {
        let value = registry_value(
            r"HKCU\Software\Valve\Steam\ActiveProcess",
            "ActiveUser",
            "REG_DWORD",
        )?;
        let hex = value.strip_prefix("0x")?;
        u32::from_str_radix(hex, 16)
            .ok()
            .filter(|account| *account != 0)
    }
    #[cfg(not(windows))]
    None
}

/// One value of the current user's registry, as `reg query` prints it.
#[cfg(windows)]
fn registry_value(key: &str, value: &str, kind: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    // Without a console window flashing up from the launcher's window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("reg")
        .args(["query", key, "/v", value])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| line.contains(value))?;
    let (_, found) = line.split_once(kind)?;
    Some(found.trim().to_owned())
}

/// The file of Transport Fever 3's save `name`: `name` itself when it is
/// the path of a file, otherwise `<name>.sav` in the game's save folder,
/// `<Steam>/userdata/<account>/3493540/local/save`, of the account Steam
/// names as playing, or of the one account on this machine that has it.
pub fn find_save(name: &str) -> Result<PathBuf, String> {
    find_save_in(&steam_roots(), active_account(), name)
}

/// [`find_save`] in the Steam installations at `roots`, `active` being the
/// account playing, if known.
pub fn find_save_in(roots: &[PathBuf], active: Option<u32>, name: &str) -> Result<PathBuf, String> {
    let named = Path::new(name);
    if named.is_file() {
        return Ok(named.to_owned());
    }
    let file = if name.to_ascii_lowercase().ends_with(".sav") {
        name.to_owned()
    } else {
        format!("{name}.sav")
    };
    // A save's name, not a way out of the save folder.
    let mut parts = Path::new(&file).components();
    let (Some(Component::Normal(_)), None) = (parts.next(), parts.next()) else {
        return Err(format!("{name} is neither a file nor the name of a save"));
    };
    let mut found: Vec<(Option<u32>, PathBuf)> = Vec::new();
    for root in roots {
        let Ok(accounts) = fs::read_dir(root.join("userdata")) else {
            continue;
        };
        for account in accounts.flatten() {
            let save = account
                .path()
                .join(TRANSPORT_FEVER_3.to_string())
                .join("local")
                .join("save")
                .join(&file);
            if save.is_file() {
                let id = account.file_name().to_str().and_then(|id| id.parse().ok());
                found.push((id, save));
            }
        }
    }
    if let Some((_, save)) = found.iter().find(|(id, _)| id.is_some() && *id == active) {
        return Ok(save.clone());
    }
    match found.len() {
        1 => Ok(found.remove(0).1),
        0 => Err(format!(
            "no save {file} in Transport Fever 3's save folder of any Steam account on this machine"
        )),
        _ => Err(format!(
            "several Steam accounts have a save {file}, and Steam names none as playing; give the save's path"
        )),
    }
}

/// The saves in Transport Fever 3's save folder that [`find_save`] finds by
/// name, newest named saves first, then autosaves: of the account Steam names as playing, or, when it
/// names none, those only one account on this machine has.
pub fn list_saves() -> Vec<String> {
    list_saves_in(&steam_roots(), active_account())
}

/// [`list_saves`] in the Steam installations at `roots`, `active` being the
/// account playing, if known. Each name, with `.sav` added, is what
/// [`find_save_in`] takes to find the same file.
pub fn list_saves_in(roots: &[PathBuf], active: Option<u32>) -> Vec<String> {
    // (account, name, modified)
    let mut found: Vec<(Option<u32>, String, std::time::SystemTime)> = Vec::new();
    for root in roots {
        let Ok(accounts) = fs::read_dir(root.join("userdata")) else {
            continue;
        };
        for account in accounts.flatten() {
            let id = account.file_name().to_str().and_then(|id| id.parse().ok());
            let folder = account
                .path()
                .join(TRANSPORT_FEVER_3.to_string())
                .join("local")
                .join("save");
            let Ok(files) = fs::read_dir(&folder) else {
                continue;
            };
            for file in files.flatten() {
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                let name = file.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(stem) = name
                    .len()
                    .checked_sub(4)
                    .filter(|&cut| name.is_char_boundary(cut))
                    .filter(|&cut| name[cut..].eq_ignore_ascii_case(".sav"))
                    .map(|cut| &name[..cut])
                else {
                    continue;
                };
                if !meta.is_file() || stem.is_empty() || !offered_save(stem) {
                    continue;
                }
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                found.push((id, stem.to_owned(), modified));
            }
        }
    }
    let playing = active.filter(|active| found.iter().any(|(id, ..)| *id == Some(*active)));
    let mut saves: Vec<(String, std::time::SystemTime)> = match playing {
        Some(active) => found
            .into_iter()
            .filter(|(id, ..)| *id == Some(active))
            .map(|(_, name, modified)| (name, modified))
            .collect(),
        // Without an account playing, a name several accounts have is not
        // found by name: it is not offered.
        None => found
            .iter()
            .filter(|(id, name, _)| {
                !found
                    .iter()
                    .any(|(other, again, _)| other != id && again.eq_ignore_ascii_case(name))
            })
            .map(|(_, name, modified)| (name.clone(), *modified))
            .collect(),
    };
    // Keep automatic recovery saves available, after the player's named saves.
    let automatic = |name: &str| name.to_ascii_lowercase().starts_with("autosave_");
    saves.sort_by(|a, b| {
        automatic(&a.0)
            .cmp(&automatic(&b.0))
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    saves.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    saves.into_iter().map(|(name, _)| name).collect()
}

/// Exclude only exact internal copy names. User saves such as
/// `tpf3mp_fixture` and automatic recovery saves remain selectable.
fn offered_save(stem: &str) -> bool {
    let plain = |text: &str| {
        !text.is_empty()
            && text.bytes().all(|b| b.is_ascii_digit())
            && (text == "0" || !text.starts_with('0'))
    };
    let pid = |text: &str| plain(text) && text.parse::<u32>().is_ok_and(|n| n > 0);
    if let Some(rest) = stem.strip_prefix("tpf3mp_room_") {
        return !pid(rest);
    }
    if let Some((process, event)) = stem.strip_prefix("tpf3mp_").and_then(|s| s.split_once('_')) {
        return !(pid(process) && plain(event) && event.parse::<u64>().is_ok());
    }
    true
}

fn read_small(path: &Path) -> Option<String> {
    (fs::metadata(path).ok()?.len() <= MAX_FILE)
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

/// A KeyValues block: its keys, compared without regard to case as Steam
/// does, and their values.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Block(BTreeMap<String, Value>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Map(Block),
}

impl Block {
    fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Text(text) => Some(text),
            Value::Map(_) => None,
        }
    }

    pub fn map(&self, key: &str) -> Option<&Block> {
        match self.get(key)? {
            Value::Map(block) => Some(block),
            Value::Text(_) => None,
        }
    }

    /// Every entry that is a block, with its key.
    pub fn blocks(&self) -> impl Iterator<Item = (&str, &Block)> {
        self.0.iter().filter_map(|(key, value)| match value {
            Value::Map(block) => Some((key.as_str(), block)),
            Value::Text(_) => None,
        })
    }
}

/// The Steam account's display name (its persona name), for the player's
/// name when they have not chosen one: from `config/loginusers.vdf` in the
/// first Steam folder that has it.
pub fn persona_name() -> Option<String> {
    let active = active_account();
    steam_roots().iter().find_map(|root| {
        let text = read_small(&root.join("config").join("loginusers.vdf"))?;
        persona_name_in(&text, active)
    })
}

/// The persona name in a `loginusers.vdf`: the active account's when Steam
/// names one (its 64-bit id is 76561197960265728 plus the account), else the
/// account marked most recent, else the one used last. At most 32
/// characters, the longest name the server takes.
pub fn persona_name_in(text: &str, active: Option<u32>) -> Option<String> {
    const ACCOUNT_BASE: u64 = 76_561_197_960_265_728;
    let file = parse(text).ok()?;
    let users = file.map("users")?;
    let accounts: Vec<(&str, &Block)> = users.blocks().collect();
    let chosen = active
        .and_then(|account| {
            let id = (ACCOUNT_BASE + u64::from(account)).to_string();
            accounts.iter().find(|(key, _)| *key == id)
        })
        .or_else(|| {
            accounts
                .iter()
                .find(|(_, user)| user.text("MostRecent") == Some("1"))
        })
        .or_else(|| {
            accounts.iter().max_by_key(|(_, user)| {
                user.text("Timestamp")
                    .and_then(|t| t.parse::<u64>().ok())
                    .unwrap_or(0)
            })
        })?;
    let name: String = chosen
        .1
        .text("PersonaName")?
        .trim()
        .chars()
        .take(32)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Reads Valve's text KeyValues: `"key" "value"` pairs and `"key" { … }`
/// blocks, with `//` comments and `[$PLATFORM]` conditions, which are
/// skipped.
pub fn parse(text: &str) -> Result<Block, String> {
    let mut tokens = Tokens::new(text);
    let block = parse_block(&mut tokens, 0)?;
    match tokens.next()? {
        None => Ok(block),
        Some(_) => Err("a '}' closes nothing".into()),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    Text(String),
    Open,
    Close,
}

fn parse_block(tokens: &mut Tokens<'_>, depth: usize) -> Result<Block, String> {
    if depth > MAX_DEPTH {
        return Err("nested too deeply".into());
    }
    let mut block = Block::default();
    loop {
        let key = match tokens.peek()? {
            None | Some(Token::Close) => return Ok(block),
            Some(Token::Open) => return Err("a block without a key".into()),
            Some(Token::Text(_)) => match tokens.next()? {
                Some(Token::Text(key)) => key,
                _ => unreachable!("peeked a key"),
            },
        };
        let value = match tokens.next()? {
            Some(Token::Text(value)) => Value::Text(value),
            Some(Token::Open) => {
                let inner = parse_block(tokens, depth + 1)?;
                match tokens.next()? {
                    Some(Token::Close) => Value::Map(inner),
                    _ => return Err(format!("the block \"{key}\" is not closed")),
                }
            }
            Some(Token::Close) | None => return Err(format!("\"{key}\" has no value")),
        };
        block.0.insert(key, value);
    }
}

struct Tokens<'a> {
    rest: &'a str,
    peeked: Option<Option<Token>>,
}

impl<'a> Tokens<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            rest: text.trim_start_matches('\u{feff}'),
            peeked: None,
        }
    }

    fn peek(&mut self) -> Result<Option<&Token>, String> {
        if self.peeked.is_none() {
            self.peeked = Some(self.read()?);
        }
        Ok(self.peeked.as_ref().and_then(Option::as_ref))
    }

    fn next(&mut self) -> Result<Option<Token>, String> {
        match self.peeked.take() {
            Some(token) => Ok(token),
            None => self.read(),
        }
    }

    fn read(&mut self) -> Result<Option<Token>, String> {
        loop {
            self.rest = self.rest.trim_start();
            if let Some(comment) = self.rest.strip_prefix("//") {
                self.rest = comment.split_once('\n').map_or("", |(_, rest)| rest);
                continue;
            }
            if self.rest.starts_with('[') {
                // A platform condition such as [$WIN32]: not a token.
                let end = self.rest.find(']').ok_or("an unclosed '['")?;
                self.rest = &self.rest[end + 1..];
                continue;
            }
            break;
        }
        let mut chars = self.rest.chars();
        let Some(first) = chars.next() else {
            return Ok(None);
        };
        match first {
            '{' => {
                self.rest = &self.rest[1..];
                Ok(Some(Token::Open))
            }
            '}' => {
                self.rest = &self.rest[1..];
                Ok(Some(Token::Close))
            }
            '"' => {
                let mut text = String::new();
                let mut escaped = false;
                for (at, c) in self.rest.char_indices().skip(1) {
                    if escaped {
                        text.push(match c {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        self.rest = &self.rest[at + 1..];
                        return Ok(Some(Token::Text(text)));
                    } else {
                        text.push(c);
                    }
                }
                Err("an unclosed '\"'".into())
            }
            _ => {
                let end = self
                    .rest
                    .find(|c: char| c.is_whitespace() || matches!(c, '{' | '}' | '"'))
                    .unwrap_or(self.rest.len());
                let token = self.rest[..end].to_owned();
                self.rest = &self.rest[end..];
                Ok(Some(Token::Text(token)))
            }
        }
    }
}

#[cfg(test)]
mod tests {

    const LOGINUSERS: &str = r#""users"
{
	"76561198000000001"
	{
		"AccountName"		"silver2127"
		"PersonaName"		"ComradeSilver"
		"Timestamp"		"1790697642"
	}
	"76561198000000002"
	{
		"AccountName"		"other"
		"PersonaName"		"Shiro"
		"MostRecent"		"1"
		"Timestamp"		"1700000000"
	}
}"#;

    #[test]
    fn the_players_name_is_the_steam_accounts_persona_name() {
        // The active account wins: 76561197960265728 + 39734273.
        assert_eq!(
            persona_name_in(LOGINUSERS, Some(39_734_273)).as_deref(),
            Some("ComradeSilver")
        );
        // Without one, the account marked most recent.
        assert_eq!(persona_name_in(LOGINUSERS, None).as_deref(), Some("Shiro"));
        // Without that, the one used last.
        let no_recent = LOGINUSERS.replace("\"MostRecent\"		\"1\"", "");
        assert_eq!(
            persona_name_in(&no_recent, None).as_deref(),
            Some("ComradeSilver")
        );
        // Long names are cut to what the server takes; nothing is no name.
        let long = LOGINUSERS.replace("Shiro", &"x".repeat(40));
        assert_eq!(
            persona_name_in(&long, None).map(|n| n.chars().count()),
            Some(32)
        );
        assert_eq!(persona_name_in("\"users\" { }", None), None);
        assert_eq!(persona_name_in("not vdf {", None), None);
    }
    use super::*;

    const LIBRARIES: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"LIBRARY_ONE"
		"label"		""
		"apps"
		{
			"228980"		"584971454"
		}
	}
	"1"
	{
		"path"		"LIBRARY_TWO"
		"apps"
		{
			"3493540"		"40740960308"
		}
	}
}
"#;

    fn manifest(installdir: &str) -> String {
        format!(
            "\"AppState\"\n{{\n\t\"appid\"\t\t\"3493540\"\n\t\"name\"\t\t\"Transport Fever 3\"\n\t\"installdir\"\t\t\"{installdir}\"\n\t\"buildid\"\t\t\"20412345\"\n\t\"UserConfig\"\n\t{{\n\t\t\"language\"\t\t\"english\"\n\t}}\n}}\n"
        )
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tpf3mp-steam-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A Steam installation at `root` whose second library holds the game.
    fn steam(root: &Path, installdir: &str) -> PathBuf {
        let one = root.join("steam");
        let two = root.join("games");
        fs::create_dir_all(one.join("steamapps")).unwrap();
        fs::create_dir_all(
            two.join("steamapps")
                .join("common")
                .join("Transport Fever 3"),
        )
        .unwrap();
        let listing = LIBRARIES
            .replace(
                "LIBRARY_ONE",
                &one.display().to_string().replace('\\', "\\\\"),
            )
            .replace(
                "LIBRARY_TWO",
                &two.display().to_string().replace('\\', "\\\\"),
            );
        fs::write(one.join("steamapps").join("libraryfolders.vdf"), listing).unwrap();
        fs::write(
            two.join("steamapps").join("appmanifest_3493540.acf"),
            manifest(installdir),
        )
        .unwrap();
        one
    }

    #[test]
    fn keyvalues_read_as_steam_writes_them() {
        let block = parse(&manifest("Transport Fever 3")).unwrap();
        let state = block.map("appstate").unwrap();
        assert_eq!(state.text("installdir"), Some("Transport Fever 3"));
        assert_eq!(state.text("BuildID"), Some("20412345"));
        assert_eq!(
            state.map("UserConfig").unwrap().text("language"),
            Some("english")
        );
        let odd = parse("// a comment\nkey value [$WIN32]\n\"quoted \\\"one\\\"\" { inner \"x\" }")
            .unwrap();
        assert_eq!(odd.text("key"), Some("value"));
        assert_eq!(odd.map("quoted \"one\"").unwrap().text("inner"), Some("x"));
        assert!(parse("\"a\" { \"b\" \"c\"").is_err());
        assert!(parse("\"a\"").is_err());
        assert!(parse("}").is_err());
        let deep = "\"a\" {".repeat(64);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn the_game_is_found_in_any_library() {
        let root = temp("found");
        let steam_root = steam(&root, "Transport Fever 3");
        let installed = find_in(&[steam_root], TRANSPORT_FEVER_3).unwrap();
        assert_eq!(installed.name, "Transport Fever 3");
        assert_eq!(installed.build, "20412345");
        assert_eq!(
            installed.dir,
            root.join("games")
                .join("steamapps")
                .join("common")
                .join("Transport Fever 3")
        );
        assert_eq!(find_in(&[root.join("steam")], 1_066_780), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_game_programs_own_folder_is_the_game() {
        let root = temp("game-exe");
        steam(&root, "Transport Fever 3");
        // In a Steam library: Steam's record of it.
        let dir = root.join("games/steamapps/common/Transport Fever 3");
        let installed = installed_at(&dir);
        assert_eq!(installed.build, "20412345");
        assert_eq!(installed.dir, dir);
        let found = find_game(Some(&dir.join("TransportFever3.exe"))).unwrap();
        assert_eq!(found.build, "20412345");
        // Anywhere else, such as a copy run under Proton: that folder, with
        // no build known, never another install Steam lists.
        let elsewhere = root.join("tf3-win/Transport Fever 3");
        fs::create_dir_all(&elsewhere).unwrap();
        let found = find_game(Some(&elsewhere.join("TransportFever3.exe"))).unwrap();
        assert_eq!(found.dir, elsewhere);
        assert_eq!(found.build, "unknown");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_install_that_does_not_fit_is_passed_over() {
        let root = temp("passed-over");
        let steam_root = steam(&root, "Transport Fever 3");
        assert_eq!(
            find_in_where(&[steam_root], TRANSPORT_FEVER_3, |_| false),
            None
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unix_paths_are_taken_through_wines_drive_z() {
        assert_eq!(
            wine_path("/home/me/.local/share/Steam"),
            Some(PathBuf::from(r"Z:\home\me\.local\share\Steam"))
        );
        assert_eq!(wine_path(r"C:\Steam"), None);
    }

    #[test]
    fn linux_vdf_secondary_libraries_are_translated_before_windows_path_checks() {
        // Parse the actual Linux spelling, including the legacy text entry,
        // rather than constructing VDF paths in the test runner's native OS.
        let parsed = parse(
            r#""libraryfolders" {
            "0" { "path" "/home/player/.local/share/Steam" }
            "1" { "path" "/mnt/Steam games" }
            "2" "/media/another-library"
            "3" { "path" "C:\\Steam" }
            "4" { "path" "relative/library" }
        }"#,
        )
        .unwrap();
        let folders = parsed.map("libraryfolders").unwrap();
        let windows = listed_library_paths(folders, true);
        assert_eq!(
            windows,
            vec![
                PathBuf::from(r"Z:\home\player\.local\share\Steam"),
                PathBuf::from(r"Z:\mnt\Steam games"),
                PathBuf::from(r"Z:\media\another-library"),
                PathBuf::from(r"C:\Steam"),
                PathBuf::from("relative/library"),
            ]
        );
        assert_eq!(
            listed_library_paths(folders, false)[1],
            PathBuf::from("/mnt/Steam games")
        );
        #[cfg(windows)]
        {
            assert!(windows[..4].iter().all(|path| path.is_absolute()));
            assert!(!windows[4].is_absolute());
        }
    }

    #[test]
    fn a_folder_name_that_leaves_the_library_is_ignored() {
        let root = temp("escape");
        let steam_root = steam(&root, "../../elsewhere");
        assert_eq!(find_in(&[steam_root], TRANSPORT_FEVER_3), None);
        fs::remove_dir_all(&root).unwrap();
    }

    fn put_save(root: &Path, account: &str, file: &str) -> PathBuf {
        let folder = root
            .join("userdata")
            .join(account)
            .join("3493540")
            .join("local")
            .join("save");
        fs::create_dir_all(&folder).unwrap();
        let save = folder.join(file);
        fs::write(&save, b"a save").unwrap();
        save
    }

    #[test]
    fn a_save_is_found_by_name_in_the_accounts_save_folder() {
        let root = temp("saves");
        let mine = put_save(&root, "111", "mptest.sav");
        let roots = [root.clone()];
        assert_eq!(find_save_in(&roots, None, "mptest").unwrap(), mine);
        assert_eq!(find_save_in(&roots, None, "mptest.sav").unwrap(), mine);
        // A path is taken as it is.
        let path = mine.display().to_string();
        assert_eq!(find_save_in(&[], None, &path).unwrap(), mine);
        assert!(find_save_in(&roots, None, "other").is_err());
        assert!(
            find_save_in(&roots, None, "../111/3493540/local/save/mptest").is_err(),
            "a name, not a way out of the folder"
        );
        // Two accounts with the save: the one playing, else neither.
        let theirs = put_save(&root, "222", "mptest.sav");
        assert!(find_save_in(&roots, None, "mptest").is_err());
        assert_eq!(find_save_in(&roots, Some(222), "mptest").unwrap(), theirs);
        assert_eq!(find_save_in(&roots, Some(111), "mptest").unwrap(), mine);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_internal_room_copies_are_hidden_and_named_saves_come_first() {
        let root = temp("save-list-filter");
        put_save(&root, "111", "twomptest.sav");
        put_save(&root, "111", "autosave_two_1913-02-01.sav");
        put_save(&root, "111", "Autosave_New Game_1912-1-1.sav");
        put_save(&root, "111", "tpf3mp_room_56032.sav");
        put_save(&root, "111", "tpf3mp_84808_26.sav");
        put_save(&root, "111", "tpf3mp_fixture.sav");
        put_save(&root, "111", "tpf3mp_01_2.sav");
        let roots = [root.clone()];
        let listed = list_saves_in(&roots, None);
        assert_eq!(listed.len(), 5);
        assert!(listed[..3].iter().any(|s| s == "twomptest"));
        assert!(listed[..3].iter().any(|s| s == "tpf3mp_fixture"));
        assert!(listed[..3].iter().any(|s| s == "tpf3mp_01_2"));
        assert!(
            listed[3..]
                .iter()
                .all(|s| s.to_ascii_lowercase().starts_with("autosave_"))
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_saves_found_by_name_are_listed_newest_first() {
        let root = temp("save-list");
        let older = put_save(&root, "111", "older.sav");
        let newer = put_save(&root, "111", "newer.sav");
        put_save(&root, "111", "notes.txt");
        let set = |file: &Path, secs: u64| {
            fs::File::options()
                .write(true)
                .open(file)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        set(&older, 1_000);
        set(&newer, 2_000);
        let roots = [root.clone()];
        assert_eq!(list_saves_in(&roots, None), ["newer", "older"]);
        // Every name listed is found by that name.
        for name in list_saves_in(&roots, None) {
            assert!(find_save_in(&roots, None, &name).is_ok(), "{name}");
        }
        // Another account's save: without an account playing, a name both
        // have is not offered; with one, only its saves are.
        let theirs = put_save(&root, "222", "older.sav");
        set(&theirs, 3_000);
        put_save(&root, "222", "only-theirs.sav");
        let listed = list_saves_in(&roots, None);
        assert!(!listed.contains(&"older".to_owned()), "{listed:?}");
        assert!(listed.contains(&"only-theirs".to_owned()));
        assert_eq!(list_saves_in(&roots, Some(111)), ["newer", "older"]);
        assert_eq!(list_saves_in(&[], None), Vec::<String>::new());
        fs::remove_dir_all(&root).unwrap();
    }

    /// This machine's Steam, if it has Transport Fever 2: a check by hand.
    #[test]
    #[ignore = "reads this machine's Steam libraries"]
    fn this_machines_transport_fever_2() {
        println!("{:?}", find(1_066_780));
    }
}
