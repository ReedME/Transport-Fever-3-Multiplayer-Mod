//! Bulk transfers over a real QUIC connection on localhost.

#![allow(clippy::unwrap_used)]

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use quinn::{Connection, Endpoint, RecvStream, SendStream};
use tpf3mp_net::{
    ServerIdentity, ServerTrust,
    bulk::{self, BulkError, Completion},
    client_config, read_message, server_config, write_message,
};
use tpf3mp_proto::{
    BULK_REQUEST_MAX_FRAME, BULK_RESPONSE_MAX_FRAME, BulkRequest, BulkResponse, ChunkHash,
    FixedBytes,
};
use tpf3mp_snapshot::{ChunkParams, ChunkSink, ChunkStore, Manifest, ManifestId, StoreConfig};
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{Layer, layer::SubscriberExt};

const IDLE: Duration = Duration::from_secs(10);

/// SplitMix64 bytes: deterministic and incompressible.
fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed;
    let mut data = Vec::with_capacity(len + 8);
    while data.len() < len {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        data.extend_from_slice(&(z ^ (z >> 31)).to_le_bytes());
    }
    data.truncate(len);
    data
}

/// Many chunks from little data, which keeps debug builds fast.
fn params() -> ChunkParams {
    ChunkParams::new(16 << 10, 64 << 10, 256 << 10).unwrap()
}

fn store(dir: &tempfile::TempDir) -> ChunkStore {
    let mut config = StoreConfig::new(1 << 30);
    config.sync_chunks = false;
    ChunkStore::open(dir.path(), config).unwrap()
}

/// Both ends of one bidirectional stream, with what keeps it alive: quinn
/// closes a connection once every handle to it is gone.
struct Pipe {
    fetcher: (SendStream, RecvStream),
    server: (SendStream, RecvStream),
    _alive: (Endpoint, Endpoint, Connection, Connection),
}

/// Opens a stream from a client to a server.
async fn pipe() -> Pipe {
    let identity = ServerIdentity::self_signed(&["localhost"]).unwrap();
    let leaf = identity.leaf().clone();
    let server = Endpoint::server(
        server_config(identity).unwrap(),
        SocketAddr::from(([127, 0, 0, 1], 0)),
    )
    .unwrap();
    let mut client = Endpoint::client(SocketAddr::from(([127, 0, 0, 1], 0))).unwrap();
    client.set_default_client_config(client_config(ServerTrust::Pinned(leaf)).unwrap());
    let address = server.local_addr().unwrap();
    let accepting = tokio::spawn({
        let server = server.clone();
        async move { server.accept().await.unwrap().await.unwrap() }
    });
    let connection = client.connect(address, "localhost").unwrap().await.unwrap();
    let server_connection = accepting.await.unwrap();
    let (mut send, recv) = connection.open_bi().await.unwrap();
    // A stream exists for the peer once something is sent on it.
    send.write_all(b"x").await.unwrap();
    let (server_send, mut server_recv) = server_connection.accept_bi().await.unwrap();
    let mut first = [0; 1];
    server_recv.read_exact(&mut first).await.unwrap();
    Pipe {
        fetcher: (send, recv),
        server: (server_send, server_recv),
        _alive: (client, server, connection, server_connection),
    }
}

/// Serves `manifest` from `from` and fetches it into `into`.
async fn transfer(
    from: &ChunkStore,
    into: &ChunkStore,
    manifest: &Manifest,
    expected: ManifestId,
    completion: Completion,
) -> (Result<Manifest, BulkError>, Result<bulk::Served, BulkError>) {
    transfer_with_idle(from, into, manifest, expected, completion, IDLE).await
}

async fn transfer_with_idle(
    from: &ChunkStore,
    into: &ChunkStore,
    manifest: &Manifest,
    expected: ManifestId,
    completion: Completion,
    idle: Duration,
) -> (Result<Manifest, BulkError>, Result<bulk::Served, BulkError>) {
    let Pipe {
        fetcher: (mut send, mut recv),
        server: (mut server_send, mut server_recv),
        _alive,
    } = pipe().await;
    let serving = tokio::spawn({
        let from = from.clone();
        let manifest = manifest.clone();
        async move {
            let served =
                bulk::serve(&mut server_send, &mut server_recv, &from, &manifest, idle).await;
            let _ = server_send.finish();
            served
        }
    });
    let fetched = bulk::fetch(
        &mut send,
        &mut recv,
        into,
        &expected,
        completion,
        idle,
        |_| {},
    )
    .await;
    // The server stops serving once the fetching side finishes, or fails.
    let _ = send.finish();
    (fetched, serving.await.unwrap())
}

/// Tokio cannot stop a blocking disk worker by aborting the async fetch that
/// awaits it. Keep a real sink in exactly that state until the test releases it.
struct DetachedSink {
    release: mpsc::Sender<bool>,
    finished: tokio::sync::oneshot::Receiver<()>,
}

impl DetachedSink {
    async fn start(store: &ChunkStore, manifest: &Manifest) -> Self {
        let (release, released) = mpsc::channel();
        let (claimed, claiming) = tokio::sync::oneshot::channel();
        let (finished, finishing) = tokio::sync::oneshot::channel();
        let store = store.clone();
        let manifest = manifest.clone();
        let original = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                let sink = ChunkSink::open(&store, manifest).unwrap();
                let _ = claimed.send(());
                // A failed assertion drops the sender too, so no worker leaks.
                if released.recv().unwrap_or(false) {
                    sink.abandon().unwrap();
                } else {
                    drop(sink);
                }
                let _ = finished.send(());
            })
            .await
            .unwrap();
        });
        tokio::time::timeout(IDLE, claiming).await.unwrap().unwrap();
        original.abort();
        assert!(original.await.unwrap_err().is_cancelled());
        Self {
            release,
            finished: finishing,
        }
    }

    async fn release(self, abandon: bool) {
        self.release.send(abandon).unwrap();
        tokio::time::timeout(IDLE, self.finished)
            .await
            .unwrap()
            .unwrap();
    }
}

/// Observe the actual first AlreadyOpen retry on this fetch's own subscriber.
/// A scheduling delay or slow handshake cannot make the regression pass.
struct SinkWaitObserver(Mutex<Option<tokio::sync::oneshot::Sender<()>>>);

impl<S: tracing::Subscriber> Layer<S> for SinkWaitObserver {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "tpf3mp_net::bulk" {
            return;
        }
        struct Message(bool);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}")
                        == "waiting for a previous local snapshot transfer to finish";
                }
            }
        }
        let mut message = Message(false);
        event.record(&mut message);
        if message.0
            && let Some(notify) = self.0.lock().unwrap().take()
        {
            let _ = notify.send(());
        }
    }
}

#[tokio::test]
async fn a_replacement_fetch_waits_for_cancelled_disk_work_without_blocking_other_snapshots() {
    let (server_dir, client_dir, out_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (server, client) = (store(&server_dir), store(&client_dir));
    let data = random_bytes(21, 700_000);
    let manifest = server.ingest(data.as_slice(), params()).unwrap();
    let other_data = random_bytes(22, 400_000);
    let other = server.ingest(other_data.as_slice(), params()).unwrap();
    // Verified chunks survive an old owner's abandonment even when the
    // pending manifest disappears. The replacement should reuse this chunk.
    let cached = manifest.unique_chunks()[0].id;
    let mut partial = ChunkSink::open(&client, manifest.clone()).unwrap();
    partial
        .put(&cached, &server.read_compressed(&cached).unwrap())
        .unwrap();
    drop(partial);
    let detached = DetachedSink::start(&client, &manifest).await;
    let out = out_dir.path().join("replacement.sav");
    let (noticed, noticing) = tokio::sync::oneshot::channel();
    let replacement = tokio::spawn({
        let (server, client, manifest, out) = (
            server.clone(),
            client.clone(),
            manifest.clone(),
            out.clone(),
        );
        async move {
            transfer(
                &server,
                &client,
                &manifest,
                manifest.id(),
                Completion::File(out),
            )
            .await
        }
        .with_subscriber(
            tracing_subscriber::registry().with(SinkWaitObserver(Mutex::new(Some(noticed)))),
        )
    });
    tokio::time::timeout(IDLE, noticing).await.unwrap().unwrap();
    assert!(
        !replacement.is_finished(),
        "the held sink still owns this snapshot"
    );
    assert!(
        !out.exists(),
        "no stale worker or replacement completed a file"
    );

    // No global transfer lock: a different ID completes while the old disk
    // worker is still blocked and the replacement is waiting on its claim.
    let other_out = out_dir.path().join("other.sav");
    let (fetched, served) = transfer(
        &server,
        &client,
        &other,
        other.id(),
        Completion::File(other_out.clone()),
    )
    .await;
    assert_eq!(fetched.unwrap().id(), other.id());
    served.unwrap();
    assert_eq!(std::fs::read(other_out).unwrap(), other_data);
    assert!(!replacement.is_finished());

    // The old owner explicitly removes its pending record before dropping the
    // claim. The current offer must recreate it, preserving cached chunks.
    detached.release(true).await;
    let (fetched, served) = tokio::time::timeout(IDLE, replacement)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.unwrap().id(), manifest.id());
    assert_eq!(
        served.unwrap().chunks,
        manifest.unique_chunks().len() as u64 - 1
    );
    assert_eq!(std::fs::read(out).unwrap(), data);
    assert_eq!(client.pending().unwrap(), []);
    assert_eq!(client.retained().unwrap().len(), 2);
}

#[tokio::test]
async fn a_local_sink_that_never_releases_times_out_without_blame_or_abandoning_its_owner() {
    let (server_dir, client_dir, out_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (server, client) = (store(&server_dir), store(&client_dir));
    let manifest = server
        .ingest(random_bytes(23, 300_000).as_slice(), params())
        .unwrap();
    let detached = DetachedSink::start(&client, &manifest).await;
    let out = out_dir.path().join("timed-out.sav");
    let idle = Duration::from_millis(250);
    let (noticed, noticing) = tokio::sync::oneshot::channel();
    let fetching = tokio::spawn({
        let (server, client, manifest, out) = (
            server.clone(),
            client.clone(),
            manifest.clone(),
            out.clone(),
        );
        async move {
            transfer_with_idle(
                &server,
                &client,
                &manifest,
                manifest.id(),
                Completion::File(out),
                idle,
            )
            .await
        }
        .with_subscriber(
            tracing_subscriber::registry().with(SinkWaitObserver(Mutex::new(Some(noticed)))),
        )
    });
    tokio::time::timeout(IDLE, noticing).await.unwrap().unwrap();
    let (fetched, _) = tokio::time::timeout(IDLE, fetching).await.unwrap().unwrap();
    let error = fetched.unwrap_err();
    assert!(
        matches!(error, BulkError::LocalBusy { snapshot, waited }
        if snapshot == manifest.id() && waited == idle),
        "{error}"
    );
    assert!(
        !error.is_violation(),
        "local contention is not a hostile peer"
    );
    assert!(!out.exists());
    assert_eq!(
        client.pending().unwrap(),
        [manifest.id()],
        "another task still owns the transfer"
    );
    assert_eq!(client.retained().unwrap(), []);
    detached.release(false).await;
    // Failure did not damage the resumable state or leave its own claim open.
    ChunkSink::resume(&client, &manifest.id())
        .unwrap()
        .abandon()
        .unwrap();
    assert_eq!(client.pending().unwrap(), []);
}

#[tokio::test]
async fn a_snapshot_arrives_identical() {
    let (server_dir, client_dir, out_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (server, client) = (store(&server_dir), store(&client_dir));
    let data = random_bytes(1, 1_500_000);
    let manifest = server.ingest(data.as_slice(), params()).unwrap();
    let out = out_dir.path().join("world.sav");
    let (fetched, served) = transfer(
        &server,
        &client,
        &manifest,
        manifest.id(),
        Completion::File(out.clone()),
    )
    .await;
    assert_eq!(fetched.unwrap().id(), manifest.id());
    let served = served.unwrap();
    assert_eq!(served.chunks, manifest.unique_chunks().len() as u64);
    assert!(std::fs::read(&out).unwrap() == data);
    assert_eq!(client.retained().unwrap(), [manifest.id()]);
}

#[tokio::test]
async fn a_returning_client_fetches_only_what_changed() {
    let (server_dir, client_dir, out_dir) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (server, client) = (store(&server_dir), store(&client_dir));
    let mut data = random_bytes(2, 2_000_000);
    let old = server.ingest(data.as_slice(), params()).unwrap();
    client.ingest(data.as_slice(), params()).unwrap();
    // A small change in the middle of the world.
    data[1_000_000..1_000_100].fill(7);
    let new = server.ingest(data.as_slice(), params()).unwrap();
    assert_ne!(old.id(), new.id());
    let (fetched, served) = transfer(
        &server,
        &client,
        &new,
        new.id(),
        Completion::File(out_dir.path().join("world.sav")),
    )
    .await;
    fetched.unwrap();
    let served = served.unwrap();
    assert!(
        served.chunks <= 2,
        "{} of {} chunks moved",
        served.chunks,
        new.unique_chunks().len()
    );
}

#[tokio::test]
async fn retaining_keeps_the_snapshot_without_a_file() {
    let (server_dir, client_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (server, client) = (store(&server_dir), store(&client_dir));
    let manifest = server
        .ingest(random_bytes(3, 700_000).as_slice(), params())
        .unwrap();
    let (fetched, served) = transfer(
        &server,
        &client,
        &manifest,
        manifest.id(),
        Completion::Retain,
    )
    .await;
    fetched.unwrap();
    served.unwrap();
    assert_eq!(client.retained().unwrap(), [manifest.id()]);
    assert_eq!(client.pending().unwrap(), []);
}

#[tokio::test]
async fn the_manifest_of_another_snapshot_is_refused() {
    let (server_dir, client_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (server, client) = (store(&server_dir), store(&client_dir));
    let manifest = server
        .ingest(random_bytes(4, 300_000).as_slice(), params())
        .unwrap();
    let (fetched, _) = transfer(
        &server,
        &client,
        &manifest,
        ManifestId([9; 32]),
        Completion::Retain,
    )
    .await;
    let error = fetched.unwrap_err();
    assert!(matches!(error, BulkError::WrongManifest), "{error}");
    assert!(error.is_violation());
    assert_eq!(client.pending().unwrap(), [], "nothing was started");
}

#[tokio::test]
async fn a_server_that_lost_a_chunk_says_so() {
    let (server_dir, client_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (server, client) = (store(&server_dir), store(&client_dir));
    let manifest = server
        .ingest(random_bytes(5, 600_000).as_slice(), params())
        .unwrap();
    // The server's copy loses its chunks.
    server.release(&manifest.id()).unwrap();
    server.gc([]).unwrap();
    let (fetched, served) = transfer(
        &server,
        &client,
        &manifest,
        manifest.id(),
        Completion::Retain,
    )
    .await;
    assert!(
        matches!(fetched, Err(BulkError::Unavailable)),
        "{fetched:?}"
    );
    assert!(served.is_err());
}

#[tokio::test]
async fn serving_refuses_chunks_outside_its_manifest() {
    let server_dir = tempfile::tempdir().unwrap();
    let server = store(&server_dir);
    let offered = server
        .ingest(random_bytes(6, 300_000).as_slice(), params())
        .unwrap();
    // Another snapshot in the same store, which must not leak.
    let other = server
        .ingest(random_bytes(7, 300_000).as_slice(), params())
        .unwrap();
    let Pipe {
        fetcher: (mut send, mut recv),
        server: (mut server_send, mut server_recv),
        _alive,
    } = pipe().await;
    let serving = tokio::spawn({
        let server = server.clone();
        async move { bulk::serve(&mut server_send, &mut server_recv, &server, &offered, IDLE).await }
    });
    let probe = BulkRequest::Chunks {
        ids: vec![ChunkHash(FixedBytes(other.chunks()[0].id.0))],
    };
    write_message(&mut send, &probe, BULK_REQUEST_MAX_FRAME)
        .await
        .unwrap();
    let error = serving.await.unwrap().unwrap_err();
    assert!(matches!(error, BulkError::Violation(_)), "{error}");
    // Nothing was sent back.
    let answer = read_message::<BulkResponse>(&mut recv, BULK_RESPONSE_MAX_FRAME).await;
    assert!(answer.is_err());
}

#[tokio::test]
async fn an_idle_peer_times_out() {
    let (server_dir, client_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let server = store(&server_dir);
    let client = store(&client_dir);
    let manifest = Arc::new(
        server
            .ingest(random_bytes(8, 100_000).as_slice(), params())
            .unwrap(),
    );
    let Pipe {
        fetcher: (mut send, mut recv),
        server: (_server_send, _server_recv),
        _alive,
    } = pipe().await;
    // Nobody serves.
    let fetched = bulk::fetch(
        &mut send,
        &mut recv,
        &client,
        &manifest.id(),
        Completion::Retain,
        Duration::from_millis(200),
        |_| {},
    )
    .await;
    assert!(matches!(fetched, Err(BulkError::Idle(_))), "{fetched:?}");
}

/// From the snapshot review: a fetch that failed partway used to leave its
/// transfer pending, and garbage collection keeps pending transfers' chunks,
/// so they stayed forever.
#[tokio::test]
async fn a_failed_fetch_leaves_nothing_pinned() {
    let (server_dir, client_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (server, client) = (store(&server_dir), store(&client_dir));
    let manifest = server
        .ingest(random_bytes(9, 900_000).as_slice(), params())
        .unwrap();
    // The server can serve every chunk but the last.
    let last = manifest.unique_chunks().last().unwrap().id.to_string();
    std::fs::remove_file(server.root().join("chunks").join(&last[..2]).join(&last)).unwrap();
    let (fetched, _) = transfer(
        &server,
        &client,
        &manifest,
        manifest.id(),
        Completion::Retain,
    )
    .await;
    assert!(fetched.is_err());
    assert_eq!(client.pending().unwrap(), [], "the transfer was given up");
    client.gc([]).unwrap();
    assert_eq!(
        client.used_bytes(),
        0,
        "its chunks went at the next collection"
    );
}

/// A transfer cut off without a word (its task aborted, its process killed)
/// is dropped by the sweep, unless it is running.
#[tokio::test]
async fn idle_transfers_can_be_swept_but_running_ones_stay() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let source = tempfile::tempdir().unwrap();
    let other = self::store(&source);
    let idle = other
        .ingest(random_bytes(10, 300_000).as_slice(), params())
        .unwrap();
    let running = other
        .ingest(random_bytes(11, 300_000).as_slice(), params())
        .unwrap();
    drop(tpf3mp_snapshot::ChunkSink::open(&store, idle).unwrap());
    let _open = tpf3mp_snapshot::ChunkSink::open(&store, running.clone()).unwrap();
    assert_eq!(store.abandon_idle_transfers().unwrap(), 1);
    assert_eq!(store.pending().unwrap(), [running.id()]);
}

/// A server that trickles a chunk is given up on rather than holding the
/// transfer, and what waits on it, indefinitely.
#[tokio::test]
async fn a_trickling_server_is_given_up_on() {
    let (server_dir, client_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (server, client) = (store(&server_dir), store(&client_dir));
    let manifest = server
        .ingest(random_bytes(12, 300_000).as_slice(), params())
        .unwrap();
    let Pipe {
        fetcher: (mut send, mut recv),
        server: (mut server_send, mut server_recv),
        _alive,
    } = pipe().await;
    let trickling = tokio::spawn(async move {
        let request = read_message::<BulkRequest>(&mut server_recv, BULK_REQUEST_MAX_FRAME)
            .await
            .unwrap();
        assert_eq!(request, BulkRequest::Manifest);
        let answer = BulkResponse::Manifest {
            bytes: manifest.to_bytes(),
        };
        write_message(&mut server_send, &answer, BULK_RESPONSE_MAX_FRAME)
            .await
            .unwrap();
        // The first chunk's frame header, then a byte now and then.
        let _ = read_message::<BulkRequest>(&mut server_recv, BULK_REQUEST_MAX_FRAME).await;
        let _ = server_send.write_all(&60_000u32.to_le_bytes()).await;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if server_send.write_all(&[0]).await.is_err() {
                break;
            }
        }
    });
    let started = tokio::time::Instant::now();
    let fetched = bulk::fetch(
        &mut send,
        &mut recv,
        &client,
        &manifest_id_of(&server),
        Completion::Retain,
        IDLE,
        |_| {},
    )
    .await;
    assert!(matches!(fetched, Err(BulkError::Idle(_))), "{fetched:?}");
    assert!(
        started.elapsed() < Duration::from_secs(9),
        "given up promptly"
    );
    trickling.abort();
}

fn manifest_id_of(store: &ChunkStore) -> ManifestId {
    store.retained().unwrap()[0]
}
