//! The server's rooms: creation, recovery, lookup and removal.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use ring::hmac;
use thiserror::Error;
use tokio::{sync::mpsc, task::JoinHandle};
use tpf3mp_proto::{
    BoundedVec, Code, CreateRoom, FixedBytes, Invite, ListedRoom, MAX_ROOM_MEMBERS, PlayerId,
    ROOMS_PER_PAGE, RequestError, RoomId, RoomPage, RoomPhase, RoomView, RulesOffer, Text,
};
use tracing::{info, warn};

use crate::{
    admission::RoomShare,
    metrics,
    persist::{self, LogError, RoomLog},
    room::{
        NewMember, ROOM_QUEUE, Room, RoomEnv, RoomHandle, RoomSecrets, RoomSpec, SharedSummary,
    },
    ruleset::RulesMenu,
};

/// How long a shutdown waits for each room to finish.
const ROOM_SHUTDOWN: Duration = Duration::from_secs(5);

/// A startup failure found before room recovery is allowed to change data.
#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("cannot preflight room data at {}: {source}", .path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("cannot identify the format of room log {}: {source}; preserve the data for repair or use a fresh data directory", .path.display())]
    UnknownFormat { path: PathBuf, source: LogError },
    #[error("room log {} uses format {found}, but this server requires {supported}; preserve the data and use matching binaries or a fresh data directory", .path.display())]
    UnsupportedFormat {
        path: PathBuf,
        found: u16,
        supported: u16,
    },
}

struct RecoveryFiles {
    logs: Vec<PathBuf>,
    compacting: Vec<PathBuf>,
}

pub(crate) struct Directory {
    rooms: Mutex<Rooms>,
    max_rooms: usize,
    key: hmac::Key,
    rules: RulesMenu,
    env: RoomEnv,
}

/// The rooms, by their IDs and by their invites' tags.
#[derive(Default)]
struct Rooms {
    by_id: HashMap<RoomId, Registered>,
    by_invite: HashMap<Vec<u8>, RoomId>,
}

impl Rooms {
    fn insert(&mut self, id: RoomId, registered: Registered) {
        self.by_invite.insert(registered.invite_tag.clone(), id);
        self.by_id.insert(id, registered);
    }
}

/// A room the directory knows: the way to reach it, its task, the tag of
/// its invite, and what the room list shows of it.
struct Registered {
    handle: RoomHandle,
    task: JoinHandle<()>,
    invite_tag: Vec<u8>,
    summary: SharedSummary,
    /// A public room's invite, which the list gives anyone; `None` for a
    /// private room, whose invite the server keeps only as its tag. Kept in
    /// memory only: a room restored after a restart is private.
    invite: Option<Invite>,
}

pub(crate) struct DirectoryConfig {
    pub(crate) secret: [u8; 32],
    pub(crate) max_rooms: usize,
    pub(crate) rules: RulesMenu,
    pub(crate) env: RoomEnv,
}

impl Directory {
    pub(crate) fn new(config: DirectoryConfig) -> Self {
        Self {
            rooms: Mutex::default(),
            max_rooms: config.max_rooms,
            key: hmac::Key::new(hmac::HMAC_SHA256, &config.secret),
            rules: config.rules,
            env: config.env,
        }
    }

    /// Checks every known log before opening stores, removing partial files,
    /// rewriting a room or collecting snapshots. A version prefix remains
    /// readable even when the rest of that older layout cannot be decoded.
    pub(crate) fn preflight(dir: Option<&Path>) -> Result<(), RecoveryError> {
        Self::recovery_files(dir).map(|_| ())
    }

    fn recovery_files(dir: Option<&Path>) -> Result<RecoveryFiles, RecoveryError> {
        let empty = || RecoveryFiles {
            logs: Vec::new(),
            compacting: Vec::new(),
        };
        let Some(dir) = dir else {
            return Ok(empty());
        };
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty()),
            Err(source) => {
                return Err(RecoveryError::Read {
                    path: dir.to_owned(),
                    source,
                });
            }
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| RecoveryError::Read {
                path: dir.to_owned(),
                source,
            })?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|source| RecoveryError::Read {
                path: path.clone(),
                source,
            })?;
            // Never follow planted links into another player's files.
            if kind.is_file() {
                files.push(path);
            }
        }
        files.sort();
        let has_extension =
            |path: &Path, wanted: &str| path.extension().is_some_and(|ext| ext == wanted);
        // A prior buggy upgrade may already have quarantined an unsupported
        // log. Its snapshots must remain protected on the next startup too.
        for path in files.iter().filter(|path| {
            has_extension(path, "log")
                || has_extension(path, "compacting")
                || has_extension(path, "broken")
        }) {
            match RoomLog::format_version(path) {
                Ok(found) if found != persist::FORMAT_VERSION => {
                    return Err(RecoveryError::UnsupportedFormat {
                        path: path.clone(),
                        found,
                        supported: persist::FORMAT_VERSION,
                    });
                }
                Err(LogError::Io(source)) => {
                    return Err(RecoveryError::Read {
                        path: path.clone(),
                        source,
                    });
                }
                Err(source) => {
                    return Err(RecoveryError::UnknownFormat {
                        path: path.clone(),
                        source,
                    });
                }
                Ok(_) => {}
            }
        }
        Ok(RecoveryFiles {
            logs: files
                .iter()
                .filter(|path| has_extension(path, "log"))
                .cloned()
                .collect(),
            compacting: files
                .into_iter()
                .filter(|path| has_extension(path, "compacting"))
                .collect(),
        })
    }

    /// Restores every running room logged in the data directory. A log that
    /// cannot be recovered is renamed to `*.broken`, unmodified, and kept
    /// for diagnosis, never deleted. Returns how many rooms were restored.
    /// Unsupported formats refuse all recovery before any room is changed.
    pub(crate) fn recover(self: &Arc<Self>) -> Result<usize, RecoveryError> {
        let files = Self::recovery_files(self.env.data_dir.as_deref())?;
        let mut held = Vec::new();
        let restored = self.recover_rooms(files, &mut held);
        // Snapshots of rooms that are gone would otherwise stay forever.
        if let Some(snapshots) = &self.env.snapshots {
            snapshots.release_all_but(&held);
        }
        Ok(restored)
    }

    /// Restores the logged rooms, noting the snapshots they hold.
    fn recover_rooms(
        self: &Arc<Self>,
        files: RecoveryFiles,
        held: &mut Vec<tpf3mp_snapshot::ManifestId>,
    ) -> usize {
        for partial in files.compacting {
            // A compaction the last process did not finish; the log it
            // would have replaced is whole.
            let _ = fs::remove_file(partial);
        }
        let mut restored = 0;
        for path in files.logs {
            let recovered = Room::recover(&path, self.key.clone(), &self.rules, self.env.clone());
            match recovered {
                Ok(Some(room)) => {
                    info!(room = %room.id(), "restored a running room from its log");
                    held.extend(room.held_snapshots());
                    self.register(room);
                    restored += 1;
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(path = %path.display(), %error, "cannot restore a room; keeping its log aside");
                    set_aside(&path);
                }
            }
        }
        restored
    }

    /// The rules hosts pick from.
    pub(crate) fn offers(&self) -> Vec<RulesOffer> {
        self.rules.offers()
    }

    /// Creates a room with `owner` as its first member and starts its task.
    /// The room holds `share` until it closes.
    pub(crate) fn create(
        self: &Arc<Self>,
        owner: NewMember,
        request: CreateRoom,
        share: RoomShare,
    ) -> Result<(RoomHandle, Invite, RoomView), RequestError> {
        if !request.settings.is_valid() || !(1..=MAX_ROOM_MEMBERS).contains(&request.max_players) {
            return Err(RequestError::InvalidSettings);
        }
        let rules = self
            .rules
            .find(request.rules.as_ref().map(|name| name.as_str()))
            .ok_or(RequestError::UnknownRules)?;
        let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
        if rooms.by_id.len() >= self.max_rooms {
            return Err(RequestError::TooManyRooms);
        }
        let id = loop {
            let id = RoomId(FixedBytes(random()));
            if !rooms.by_id.contains_key(&id) {
                break id;
            }
        };
        // No two open rooms share an invite.
        let (invite, invite_tag) = loop {
            let invite = Invite(Code::random());
            let tag = self.invite_tag(&invite);
            if !rooms.by_invite.contains_key(&tag) {
                break (invite, tag);
            }
        };
        let secrets = RoomSecrets {
            key: self.key.clone(),
            invite_tag: invite_tag.clone(),
            password_tag: request.password.as_ref().map(|password| {
                hmac::sign(&self.key, &RoomSecrets::password_input(&id, password))
                    .as_ref()
                    .to_vec()
            }),
        };
        let room = Room::new(
            RoomSpec {
                id,
                name: request.name,
                max_players: request.max_players,
                settings: request.settings,
                secrets,
                rules: rules.name.clone(),
                ruleset: (rules.factory)(),
                env: self.env.clone(),
                share,
                listing: request.listing.clone(),
                competitive: request.competitive,
            },
            owner,
        );
        let view = room.view();
        let summary = room.summary();
        let (commands, receiver) = mpsc::channel(ROOM_QUEUE);
        let handle = RoomHandle::new(commands);
        let task = tokio::spawn(room.run(receiver, Arc::clone(self)));
        rooms.insert(
            id,
            Registered {
                handle: handle.clone(),
                task,
                invite_tag,
                summary,
                invite: request.listing.is_some().then_some(invite),
            },
        );
        drop(rooms);
        metrics::increment(&self.env.metrics.rooms_created);
        Ok((handle, invite, view))
    }

    fn register(self: &Arc<Self>, room: Room) {
        let (commands, receiver) = mpsc::channel(ROOM_QUEUE);
        let id = room.id();
        let invite_tag = room.invite_tag();
        let summary = room.summary();
        let task = tokio::spawn(room.run(receiver, Arc::clone(self)));
        self.rooms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                Registered {
                    handle: RoomHandle::new(commands),
                    task,
                    invite_tag,
                    summary,
                    invite: None,
                },
            );
    }

    /// The tag an invite's room is found by: an HMAC under the server's
    /// key, so a room's log does not give its invite away.
    fn invite_tag(&self, invite: &Invite) -> Vec<u8> {
        hmac::sign(&self.key, &RoomSecrets::invite_input(invite))
            .as_ref()
            .to_vec()
    }

    /// The room `invite` is for, if it is open.
    pub(crate) fn find(&self, invite: &Invite) -> Option<RoomHandle> {
        let tag = self.invite_tag(invite);
        let rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
        let id = rooms.by_invite.get(&tag)?;
        rooms
            .by_id
            .get(id)
            .map(|registered| registered.handle.clone())
    }

    /// Checks an invite and optional room password at the matching room,
    /// without joining it. A room that closes between lookup and check is
    /// indistinguishable from an unknown invite.
    pub(crate) async fn resolve_invite(
        &self,
        player: PlayerId,
        invite: Invite,
        password: Option<Text<64>>,
    ) -> Result<(), RequestError> {
        let handle = self.find(&invite).ok_or(RequestError::BadInvite)?;
        handle
            .request(|reply| crate::room::RoomCommand::ResolveInvite {
                player,
                invite,
                password,
                reply,
            })
            .await
            .map_err(|_| RequestError::BadInvite)
    }

    /// Stops every room once its connections are gone: without the
    /// directory's handles, a room's queue closes and its task ends, closing
    /// its log and letting go of the snapshot store. Waits for each a while.
    pub(crate) async fn shut_down(&self) {
        let registered: Vec<Registered> = {
            let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
            rooms.by_invite.clear();
            rooms
                .by_id
                .drain()
                .map(|(_, registered)| registered)
                .collect()
        };
        for Registered { handle, task, .. } in registered {
            drop(handle);
            if tokio::time::timeout(ROOM_SHUTDOWN, task).await.is_err() {
                warn!("a room did not stop in time");
            }
        }
    }

    /// Page `page` of the public rooms, [`ROOMS_PER_PAGE`] a page: those in
    /// their lobby first, then the fuller, then by name. A private room is
    /// never in it, nor one nobody is connected to: its game waits out its
    /// grace period for its own players, and closes unless they return.
    pub(crate) fn list(&self, page: u16) -> RoomPage {
        let mut listed: Vec<ListedRoom> = {
            let rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
            rooms
                .by_id
                .values()
                .filter_map(|registered| {
                    let invite = registered.invite?;
                    let summary = registered
                        .summary
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    if summary.connected == 0 {
                        return None;
                    }
                    let listing = summary.listing.clone()?;
                    Some(ListedRoom {
                        invite,
                        name: summary.name.clone(),
                        rules: summary.rules.clone(),
                        players: summary.players,
                        max_players: summary.max_players,
                        has_password: summary.has_password,
                        phase: summary.phase,
                        listing,
                        competitive: summary.competitive,
                    })
                })
                .collect()
        };
        listed.sort_by(|a, b| {
            (a.phase != RoomPhase::Lobby)
                .cmp(&(b.phase != RoomPhase::Lobby))
                .then(b.players.cmp(&a.players))
                .then_with(|| a.name.as_str().cmp(b.name.as_str()))
                .then_with(|| a.invite.0.cmp(&b.invite.0))
        });
        let start = usize::from(page).saturating_mul(ROOMS_PER_PAGE);
        let more = listed.len() > start.saturating_add(ROOMS_PER_PAGE);
        let rooms: Vec<ListedRoom> = listed
            .into_iter()
            .skip(start)
            .take(ROOMS_PER_PAGE)
            .collect();
        RoomPage {
            page,
            rooms: BoundedVec::new(rooms).unwrap_or_default(),
            more,
        }
    }

    pub(crate) fn remove(&self, id: &RoomId) {
        let mut rooms = self.rooms.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(registered) = rooms.by_id.remove(id) {
            rooms.by_invite.remove(&registered.invite_tag);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.rooms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .by_id
            .len()
    }
}

/// Renames a log that cannot be restored to `<room>.broken`, or
/// `<room>.<n>.broken` if that exists, so an earlier one is never replaced.
fn set_aside(path: &Path) {
    for attempt in 0..1000 {
        let aside = if attempt == 0 {
            path.with_extension("broken")
        } else {
            path.with_extension(format!("{attempt}.broken"))
        };
        if aside.exists() {
            continue;
        }
        if let Err(error) = fs::rename(path, &aside) {
            warn!(path = %path.display(), %error, "cannot set a broken log aside");
        }
        return;
    }
    warn!(path = %path.display(), "too many broken logs of one room; leaving this one in place");
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the operating system's random source is available");
    bytes
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tpf3mp_net::{ServerIdentity, bulk};
    use tpf3mp_proto::{Platform, RoomSettings};
    use tpf3mp_snapshot::{ChunkParams, ChunkStore, StoreConfig};

    use super::*;
    use crate::{
        DiagnosticsConfig, Server, ServerConfig, ServerError, SnapshotConfig,
        metrics::Metrics,
        persist::{StartMember, StartRecord},
        room::Timeouts,
        snapshots::{Pointer, SavePoint, Snapshots},
    };

    fn start(id: u8, version: u16) -> StartRecord {
        let owner = PlayerId(FixedBytes([2; 32]));
        StartRecord {
            version,
            history: 1,
            id: RoomId(FixedBytes([id; 16])),
            name: Text::new("room").unwrap(),
            rules: Text::new("native").unwrap(),
            manifest: None,
            owner,
            max_players: 4,
            settings: RoomSettings::DEFAULT,
            invite_tag: vec![id; 32],
            password_tag: None,
            members: vec![StartMember {
                player: owner,
                name: Text::new("owner").unwrap(),
                platform: Platform::current(),
                content: None,
            }],
            base: None,
        }
    }

    /// A valid room sorts before the incompatible room. Without preflight,
    /// recovery rewrites and registers it before quarantining the old log.
    fn logged_rooms(data: &Path, store: &ChunkStore) -> PathBuf {
        let valid = start(0, persist::FORMAT_VERSION);
        drop(RoomLog::create(data, &valid).unwrap());
        let old = start(1, 8);
        drop(RoomLog::create(data, &old).unwrap());
        // Cleanup of an interrupted, current-format compaction must not run.
        let partial = RoomLog::path_for(data, &valid.id).with_extension("log.compacting");
        fs::copy(RoomLog::path_for(data, &valid.id), partial).unwrap();
        let manifest = store
            .ingest(&[7_u8; 1000][..], ChunkParams::DEFAULT)
            .unwrap();
        Pointer::new(
            bulk::snapshot_id(&manifest.id()),
            SavePoint {
                event: 0,
                after_turn: 0,
                history: 1,
                sealed_through: 0,
            },
        )
        .write(data, &old.id)
        .unwrap();
        RoomLog::path_for(data, &old.id)
    }

    fn file_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn read(dir: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    read(&path, root, files);
                } else {
                    let relative = path.strip_prefix(root).unwrap();
                    let bytes = if relative == Path::new("lock") {
                        // The store's root lock is an empty sentinel. Windows
                        // prohibits reading it while this process holds its
                        // byte lock, so prove its empty contents by length.
                        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
                        Vec::new()
                    } else {
                        fs::read(&path).unwrap()
                    };
                    files.insert(relative.to_owned(), bytes);
                }
            }
        }
        let mut files = BTreeMap::new();
        read(root, root, &mut files);
        files
    }

    fn config(data: &Path, snapshots: &Path) -> ServerConfig {
        let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
        let mut config = ServerConfig::new("127.0.0.1:0".parse().unwrap(), identity);
        config.data_dir = Some(data.to_owned());
        config.snapshots = Some(SnapshotConfig::new(snapshots));
        config
    }

    #[tokio::test]
    async fn incompatible_startup_preserves_every_log_pointer_and_snapshot_byte() {
        for extension in ["log", "broken", "log.compacting"] {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path().join("data");
            let snapshots = temp.path().join("snapshots");
            let store = ChunkStore::open(&snapshots, StoreConfig::new(1 << 20)).unwrap();
            let original = logged_rooms(&data, &store);
            let legacy = original.with_extension(extension);
            if legacy != original {
                fs::rename(&original, &legacy).unwrap();
            }
            // ChunkStore::open clears tmp: preservation proves bind refuses
            // before reopening the store, as well as before recovery/GC.
            fs::write(snapshots.join("tmp/unfinished"), b"keep pending disk work").unwrap();
            drop(store);
            let before_data = file_bytes(&data);
            let before_snapshots = file_bytes(&snapshots);
            let diagnostics = temp.path().join("diagnostics");
            let mut config = config(&data, &snapshots);
            config.diagnostics = Some(DiagnosticsConfig::new(
                diagnostics.clone(),
                Duration::ZERO,
                0,
            ));
            let error = match Server::bind(config) {
                Err(error) => error,
                Ok(_) => panic!("incompatible room data must refuse startup"),
            };
            assert!(
                matches!(error, ServerError::Recovery(RecoveryError::UnsupportedFormat { path, found: 8, supported: persist::FORMAT_VERSION }) if path == legacy)
            );
            assert_eq!(
                file_bytes(&data),
                before_data,
                "all rooms and partials are unchanged"
            );
            assert_eq!(
                file_bytes(&snapshots),
                before_snapshots,
                "roots, chunks and unfinished work are unchanged"
            );
            assert!(
                !diagnostics.exists(),
                "preflight precedes diagnostics startup"
            );
        }
    }

    #[tokio::test]
    async fn direct_recovery_refuses_before_rewriting_registering_or_collecting() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let snapshot_dir = temp.path().join("snapshots");
        let snapshots = Arc::new(Snapshots::open(&SnapshotConfig::new(&snapshot_dir)).unwrap());
        let legacy = logged_rooms(&data, &snapshots.store);
        let before_data = file_bytes(&data);
        let before_snapshots = file_bytes(&snapshot_dir);
        let directory = Arc::new(Directory::new(DirectoryConfig {
            secret: [1; 32],
            max_rooms: 100,
            rules: RulesMenu::native(),
            env: RoomEnv {
                tick: Duration::from_millis(100),
                metrics: Arc::new(Metrics::default()),
                data_dir: Some(data.clone()),
                timeouts: Timeouts {
                    stall: Duration::from_secs(10),
                    load: Duration::from_secs(10),
                    abandoned: Duration::from_secs(600),
                },
                snapshots: Some(snapshots),
                compact_log_at: 1 << 20,
            },
        }));
        let error = directory.recover().unwrap_err();
        assert!(
            matches!(error, RecoveryError::UnsupportedFormat { path, found: 8, supported: persist::FORMAT_VERSION } if path == legacy)
        );
        assert_eq!(directory.len(), 0, "no earlier valid room was registered");
        assert_eq!(file_bytes(&data), before_data);
        assert_eq!(file_bytes(&snapshot_dir), before_snapshots);
    }

    #[tokio::test]
    async fn unidentified_first_records_preserve_the_same_data_as_unsupported_formats() {
        for extension in ["log", "broken", "log.compacting"] {
            for damage in ["crc", "truncated", "unreadable-version"] {
                let temp = tempfile::tempdir().unwrap();
                let data = temp.path().join("data");
                let snapshots = temp.path().join("snapshots");
                let store = ChunkStore::open(&snapshots, StoreConfig::new(1 << 20)).unwrap();
                let original = logged_rooms(&data, &store);
                let legacy = original.with_extension(extension);
                let mut bytes = fs::read(&original).unwrap();
                match damage {
                    "crc" => bytes[8] ^= 1,
                    "truncated" => bytes.truncate(4),
                    "unreadable-version" => {
                        let payload = [0xff_u8; 10];
                        bytes.clear();
                        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                        bytes.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
                        bytes.extend_from_slice(&payload);
                    }
                    _ => unreachable!(),
                }
                fs::write(&original, bytes).unwrap();
                if legacy != original {
                    fs::rename(&original, &legacy).unwrap();
                }
                fs::write(snapshots.join("tmp/unfinished"), b"keep pending disk work").unwrap();
                drop(store);
                let before_data = file_bytes(&data);
                let before_snapshots = file_bytes(&snapshots);
                let error = match Server::bind(config(&data, &snapshots)) {
                    Err(error) => error,
                    Ok(_) => panic!("an unidentified header cannot be declared current format"),
                };
                assert!(
                    matches!(error, ServerError::Recovery(RecoveryError::UnknownFormat { path, source: LogError::Damaged(0) }) if path == legacy)
                );
                assert_eq!(file_bytes(&data), before_data, "{extension} with {damage}");
                assert_eq!(
                    file_bytes(&snapshots),
                    before_snapshots,
                    "{extension} with {damage}"
                );
            }
        }
    }

    #[tokio::test]
    async fn unreadable_data_directory_refuses_before_creating_any_store() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::write(&data, b"a file is not a data directory").unwrap();
        let snapshots = temp.path().join("snapshots");
        let error = match Server::bind(config(&data, &snapshots)) {
            Err(error) => error,
            Ok(_) => panic!("unreadable recovery input must refuse startup"),
        };
        assert!(
            matches!(error, ServerError::Recovery(RecoveryError::Read { path, .. }) if path == data)
        );
        assert!(!snapshots.exists());
        assert_eq!(fs::read(&data).unwrap(), b"a file is not a data directory");
    }
}
