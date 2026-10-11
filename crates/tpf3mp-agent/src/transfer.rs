//! Moving worlds between this client and the server: fetching the snapshot
//! a turn stream starts from, and uploading a save the room asked for. See
//! "Snapshots" in `docs/PROTOCOL.md`.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, BufReader},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use quinn::{RecvStream, SendStream};
use tpf3mp_net::{
    bulk::{self, BulkError, Completion, Served},
    read_preamble, write_message, write_preamble,
};
use tpf3mp_proto::{
    BULK_REQUEST_MAX_FRAME, BulkOpen, PROTOCOL_VERSION, SavedWorld, SnapshotId, WorldOffer,
};
use tpf3mp_snapshot::{ChunkParams, ChunkStore, Manifest, ManifestId, Progress, StoreConfig};

/// Silence after which a bulk stream is given up.
pub const BULK_IDLE: Duration = Duration::from_secs(60);

/// Where a player keeps worlds: every snapshot saved or received, and the
/// files the game writes and loads.
#[derive(Debug, Clone)]
pub struct Worlds {
    store: ChunkStore,
    /// Where the game writes its saves.
    saves: PathBuf,
    /// Where received worlds are put together for the game to load.
    received: PathBuf,
    retention: Arc<Mutex<Retention>>,
    /// Serialize collectors without making metadata publication wait for GC.
    cleanup: Arc<Mutex<()>>,
}

#[derive(Debug, Default)]
struct Retention {
    keep: HashSet<ManifestId>,
    /// Completed disk work not yet published to the bridge's live set.
    pending: HashMap<ManifestId, usize>,
    pins: HashMap<ManifestId, usize>,
    /// Ingest has no manifest ID until its reader is exhausted. Defer root
    /// releases while such work runs; its pending hold is published on exit.
    active_ingests: usize,
}

struct WorldIngest {
    retention: Arc<Mutex<Retention>>,
    completed: Option<ManifestId>,
}

impl Drop for WorldIngest {
    fn drop(&mut self) {
        let mut state = self
            .retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(id) = self.completed {
            *state.pending.entry(id).or_default() += 1;
        }
        state.active_ingests -= 1;
    }
}

struct WorldPin {
    retention: Arc<Mutex<Retention>>,
    id: ManifestId,
}

/// Owns one already-counted completion hold until its result is published
/// or dropped. A closed bridge channel cannot leak unpublished holds.
#[derive(Debug)]
pub(crate) struct PendingWorld {
    worlds: Worlds,
    id: ManifestId,
}

impl Drop for PendingWorld {
    fn drop(&mut self) {
        self.worlds.discard_pending(self.id);
    }
}

impl Drop for WorldPin {
    fn drop(&mut self) {
        let mut state = self
            .retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = state.pins.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                state.pins.remove(&self.id);
            }
        }
    }
}

impl Worlds {
    /// Opens (or creates) the worlds kept under `dir`, taking at most
    /// `max_bytes` for snapshots.
    pub fn open(dir: &Path, max_bytes: u64) -> io::Result<Self> {
        let mut config = StoreConfig::new(max_bytes);
        // A damaged chunk after a power cut only costs fetching it again, and
        // flushing every chunk almost halves transfer speed on Windows.
        config.sync_chunks = false;
        let store = ChunkStore::open(dir.join("store"), config).map_err(io::Error::other)?;
        let saves = dir.join("saves");
        let received = dir.join("received");
        fs::create_dir_all(&saves)?;
        fs::create_dir_all(&received)?;
        Ok(Self {
            store,
            saves,
            received,
            retention: Arc::default(),
            cleanup: Arc::default(),
        })
    }

    pub fn store(&self) -> &ChunkStore {
        &self.store
    }

    /// Where the game writes its saves.
    pub fn saves(&self) -> &Path {
        &self.saves
    }

    /// Where the world of `snapshot` is put together for the game to load.
    pub fn received_file(&self, snapshot: &SnapshotId) -> PathBuf {
        self.received
            .join(format!("{}.sav", bulk::manifest_id(snapshot)))
    }

    /// Cuts a save the game wrote into the store, and deletes the file: the
    /// store holds it now. Blocking.
    pub fn ingest(&self, file: &Path) -> io::Result<(Manifest, SavedWorld)> {
        let taken = self.ingest_copy(file)?;
        // The game may still hold the file open; the store has what it needs.
        let _ = fs::remove_file(file);
        Ok(taken)
    }

    /// Cuts a copy of a save into the store and leaves the file alone: a
    /// player's own save, such as the one a room starts from. Blocking.
    pub fn ingest_copy(&self, file: &Path) -> io::Result<(Manifest, SavedWorld)> {
        // Disk work never holds the bridge's publication lock. Collection
        // defers release until this guard atomically publishes a pending hold.
        let mut ingest = self.begin_ingest();
        let source = BufReader::new(fs::File::open(file)?);
        let manifest = self
            .store
            .ingest(source, ChunkParams::DEFAULT)
            .map_err(io::Error::other)?;
        let world = SavedWorld {
            snapshot: bulk::snapshot_id(&manifest.id()),
            size: manifest.total_size(),
        };
        ingest.completed = Some(manifest.id());
        Ok((manifest, world))
    }

    fn begin_ingest(&self) -> WorldIngest {
        self.retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active_ingests += 1;
        WorldIngest {
            retention: Arc::clone(&self.retention),
            completed: None,
        }
    }

    fn pin(&self, id: ManifestId) -> WorldPin {
        let mut state = self
            .retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *state.pins.entry(id).or_default() += 1;
        WorldPin {
            retention: Arc::clone(&self.retention),
            id,
        }
    }

    fn retain_pending(&self, id: ManifestId) {
        *self
            .retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .entry(id)
            .or_default() += 1;
    }

    pub(crate) fn discard_pending(&self, id: ManifestId) {
        Self::consume_pending(
            &mut self
                .retention
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            id,
        );
    }

    pub(crate) fn pending_result(&self, id: ManifestId) -> PendingWorld {
        PendingWorld {
            worlds: self.clone(),
            id,
        }
    }

    fn consume_pending(state: &mut Retention, id: ManifestId) {
        if let Some(count) = state.pending.get_mut(&id) {
            *count -= 1;
            if *count == 0 {
                state.pending.remove(&id);
            }
        }
    }

    /// Publish the latest live set synchronously, before scheduling disk work.
    pub(crate) fn set_keep(&self, keep: &[ManifestId]) {
        self.publish(keep, None);
    }

    pub(crate) fn publish(&self, keep: &[ManifestId], claimed: Option<ManifestId>) {
        let mut state = self
            .retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.keep = keep.iter().copied().collect();
        if let Some(id) = claimed {
            Self::consume_pending(&mut state, id);
        }
    }

    /// Stops keeping snapshots other than `keep`, drops transfers that no
    /// fetch is running any more, deletes received world files other than
    /// those of `keep`, and collects unused chunks. Blocking.
    pub fn keep_only(&self, keep: &[ManifestId]) -> io::Result<()> {
        self.set_keep(keep);
        self.collect()
    }

    /// Every queued cleanup reads the current live set under the same lock
    /// as publication. Pending results and active transfers retain their roots.
    pub(crate) fn collect(&self) -> io::Result<()> {
        let _cleanup = self.cleanup.lock().unwrap_or_else(PoisonError::into_inner);
        let state = self
            .retention
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.active_ingests != 0 {
            return Ok(());
        }
        let keep: HashSet<ManifestId> = state
            .keep
            .iter()
            .chain(state.pending.keys())
            .chain(state.pins.keys())
            .copied()
            .collect();
        self.store
            .abandon_idle_transfers()
            .map_err(io::Error::other)?;
        for id in self.store.retained().map_err(io::Error::other)? {
            if !keep.contains(&id) {
                self.store.release(&id).map_err(io::Error::other)?;
            }
        }
        let wanted: Vec<String> = keep.iter().map(|id| format!("{id}.sav")).collect();
        for entry in fs::read_dir(&self.received)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !wanted.contains(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
        // ChunkStore serializes GC against ingest/transfer/assembly itself.
        // The expensive chunk scan must not block live-set publication.
        drop(state);
        self.store.gc([]).map_err(io::Error::other)?;
        Ok(())
    }
}

/// Opens bulk streams on a client's connection.
#[derive(Debug, Clone)]
pub struct BulkOpener {
    pub(crate) connection: quinn::Connection,
}

impl BulkOpener {
    /// Opens a bulk stream for `open`, exchanging version preambles first.
    pub async fn open(&self, open: BulkOpen) -> Result<(SendStream, RecvStream), BulkError> {
        let (mut send, mut recv) = self.connection.open_bi().await?;
        write_preamble(&mut send, PROTOCOL_VERSION).await?;
        write_message(&mut send, &open, BULK_REQUEST_MAX_FRAME).await?;
        if read_preamble(&mut recv).await? != PROTOCOL_VERSION {
            return Err(BulkError::Violation("a bulk stream of another version"));
        }
        Ok((send, recv))
    }
}

/// Fetches the world a turn stream offered into `worlds`, and puts its file
/// together. Returns the file and the snapshot's manifest.
pub async fn fetch_world(
    opener: &BulkOpener,
    worlds: &Worlds,
    offer: WorldOffer,
    progress: impl FnMut(Progress),
) -> Result<(PathBuf, Manifest), BulkError> {
    let id = bulk::manifest_id(&offer.snapshot);
    let _pin = worlds.pin(id);
    let file = worlds.received_file(&offer.snapshot);
    let (mut send, mut recv) = opener
        .open(BulkOpen::Fetch {
            snapshot: offer.snapshot,
        })
        .await?;
    let manifest = bulk::fetch(
        &mut send,
        &mut recv,
        &worlds.store,
        &bulk::manifest_id(&offer.snapshot),
        Completion::File(file.clone()),
        BULK_IDLE,
        progress,
    )
    .await?;
    worlds.retain_pending(id);
    Ok((file, manifest))
}

/// Uploads a save this client made, which the room asked for.
pub async fn upload_world(
    opener: &BulkOpener,
    worlds: &Worlds,
    snapshot: SnapshotId,
) -> Result<Served, BulkError> {
    upload_world_with_progress(opener, worlds, snapshot, |_| {}).await
}

/// [`upload_world`], telling `progress` how much of the save went up, in
/// percent of its chunks. The room asks only for the chunks it lacks, so
/// it may end below 100.
pub async fn upload_world_with_progress(
    opener: &BulkOpener,
    worlds: &Worlds,
    snapshot: SnapshotId,
    mut progress: impl FnMut(u8),
) -> Result<Served, BulkError> {
    let _pin = worlds.pin(bulk::manifest_id(&snapshot));
    let store = worlds.store.clone();
    let id = bulk::manifest_id(&snapshot);
    let manifest = tokio::task::spawn_blocking(move || store.manifest(&id))
        .await
        .map_err(|error| BulkError::Task(error.to_string()))??;
    let chunks = manifest.chunks().len() as u64;
    let (mut send, mut recv) = opener.open(BulkOpen::Serve { snapshot }).await?;
    let served = bulk::serve_with_progress(
        &mut send,
        &mut recv,
        &worlds.store,
        &manifest,
        BULK_IDLE,
        |served| {
            let percent = (served.chunks.min(chunks) * 100)
                .checked_div(chunks)
                .unwrap_or(100);
            progress(u8::try_from(percent).unwrap_or(100));
        },
    )
    .await;
    let _ = send.finish();
    served
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_active_ingest_defers_collection_without_locking_publication() {
        let dir = tempfile::tempdir().unwrap();
        let worlds = Worlds::open(dir.path(), 1 << 20).unwrap();
        let file = worlds.saves().join("world.sav");
        fs::write(&file, [4u8; 1000]).unwrap();
        let mut ingest = worlds.begin_ingest();
        let manifest = worlds
            .store
            .ingest(
                BufReader::new(fs::File::open(&file).unwrap()),
                ChunkParams::DEFAULT,
            )
            .unwrap();
        worlds.publish(&[], None);
        // The manifest exists but its result has not yet been published.
        worlds.collect().unwrap();
        assert!(worlds.store.manifest(&manifest.id()).is_ok());
        ingest.completed = Some(manifest.id());
        drop(ingest);
        worlds.collect().unwrap();
        assert!(worlds.store.manifest(&manifest.id()).is_ok());
        worlds.discard_pending(manifest.id());
        worlds.collect().unwrap();
        assert!(worlds.store.manifest(&manifest.id()).is_err());
    }

    #[test]
    fn delayed_cleanup_reads_the_latest_retention_and_keeps_unpublished_work() {
        let dir = tempfile::tempdir().unwrap();
        let worlds = Worlds::open(dir.path(), 1 << 20).unwrap();
        let file = worlds.saves().join("a.sav");
        fs::write(&file, [1u8; 1000]).unwrap();
        let (a, _) = worlds.ingest_copy(&file).unwrap();
        worlds.publish(&[a.id()], Some(a.id()));
        let (run, wait) = std::sync::mpsc::channel();
        let old_worlds = worlds.clone();
        let old_cleanup = std::thread::spawn(move || {
            wait.recv().unwrap();
            old_worlds.collect().unwrap();
        });
        // Disk work completes after the old cleanup was queued. It has not
        // yet reached the bridge; its pending hold protects both root and file.
        fs::write(&file, [2u8; 1000]).unwrap();
        let (b, saved) = worlds.ingest_copy(&file).unwrap();
        let received = worlds.received_file(&saved.snapshot);
        fs::write(&received, [2u8; 1000]).unwrap();
        worlds.collect().unwrap();
        assert!(worlds.store().manifest(&b.id()).is_ok());
        assert!(received.exists());
        // Publication then changes the desired set before the old job runs.
        worlds.publish(&[b.id()], Some(b.id()));
        worlds.collect().unwrap();
        run.send(()).unwrap();
        old_cleanup.join().unwrap();
        assert_eq!(worlds.store().retained().unwrap(), [b.id()]);
        assert!(worlds.store().missing(&b).unwrap().is_empty());
        assert!(received.exists());
    }

    #[test]
    fn active_world_transfer_is_pinned_until_its_guard_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let worlds = Worlds::open(dir.path(), 1 << 20).unwrap();
        let file = worlds.saves().join("world.sav");
        fs::write(&file, [3u8; 1000]).unwrap();
        let (manifest, _) = worlds.ingest_copy(&file).unwrap();
        worlds.publish(&[manifest.id()], Some(manifest.id()));
        let pin = worlds.pin(manifest.id());
        worlds.set_keep(&[]);
        worlds.collect().unwrap();
        assert!(worlds.store().missing(&manifest).unwrap().is_empty());
        drop(pin);
        worlds.collect().unwrap();
        assert!(worlds.store().manifest(&manifest.id()).is_err());
        assert!(!worlds.store().missing(&manifest).unwrap().is_empty());
    }

    #[test]
    fn discarding_one_same_snapshot_result_does_not_release_another_pending_result() {
        let dir = tempfile::tempdir().unwrap();
        let worlds = Worlds::open(dir.path(), 1 << 20).unwrap();
        let file = worlds.saves().join("same.sav");
        fs::write(&file, [4u8; 1000]).unwrap();
        let (a, _) = worlds.ingest_copy(&file).unwrap();
        let (b, _) = worlds.ingest_copy(&file).unwrap();
        assert_eq!(a.id(), b.id());
        worlds.discard_pending(a.id());
        worlds.collect().unwrap();
        assert!(worlds.store().missing(&b).unwrap().is_empty());
        assert!(worlds.store().manifest(&b.id()).is_ok());
        worlds.publish(&[b.id()], Some(b.id()));
        worlds.collect().unwrap();
        assert!(worlds.store().manifest(&b.id()).is_ok());
        worlds.set_keep(&[]);
        worlds.collect().unwrap();
        assert!(worlds.store().manifest(&b.id()).is_err());
    }
}
