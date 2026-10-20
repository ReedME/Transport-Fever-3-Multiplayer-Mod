//! One client connection: the handshake, then requests and game messages
//! until the connection ends.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use quinn::{RecvStream, SendStream};
use thiserror::Error;
use tokio::{
    sync::{OwnedSemaphorePermit, mpsc, watch},
    time::Instant,
};
use tpf3mp_net::{
    NetError,
    bulk::{self, BulkError, Completion},
    close, read_message, read_preamble, verify_proof, write_frame, write_message, write_preamble,
};
use tpf3mp_proto::{
    BULK_REQUEST_MAX_FRAME, BULK_RESPONSE_MAX_FRAME, BulkOpen, BulkResponse, CONTROL_MAX_FRAME,
    ChatText, ClientMessage, Code, GameMessage, Hello, IntentRejection, JoinRoom,
    MAX_CHECKPOINT_LANES, MAX_PREVIEW, PROTOCOL_VERSION, PlayerId, Reject, RejectReason, Request,
    RequestError, Response, RoomView, ServerMessage, SessionId, TURN_MAX_FRAME, TurnMessage,
    TurnStart, Welcome,
};
use tracing::{debug, info};

use crate::{
    Shared,
    admission::{self, Handshake, Origin},
    limit::TokenBucket,
    metrics,
    room::{Declared, MemberLink, NewMember, Reply, RoomCommand, RoomHandle, TurnFeed},
    snapshots::{BULK_IDLE, Snapshots},
};

/// How long a peer gets to acknowledge a final message (a `Reject`, or the
/// preamble after a version mismatch) before the server closes the connection.
const LINGER: Duration = Duration::from_secs(2);
/// Control messages queued for one client before it counts as too slow.
const CONTROL_QUEUE: usize = 256;
/// Turns queued for one client before it counts as too slow: about 100 s of
/// turns at the default tick.
const TURN_QUEUE: usize = 1024;
/// Other members' build previews queued for one client. A full queue drops
/// the newest: previews are advisory and sent again, so a slow client
/// misses some and is never disconnected for them.
const ADVISORY_QUEUE: usize = 64;
/// Bytes of other members' previews written to one client per second, and
/// the burst: a preview's bytes share the control stream with the client's
/// own messages, and once written they stay ahead of them, so previews over
/// this are dropped rather than let a flood of them (many members, 16 KiB
/// each) fill the stream's window. A preview that hides costs nothing.
const ADVISORY_BYTES_PER_SECOND: u32 = 48 * 1024;
const ADVISORY_BYTES_BURST: u32 = 64 * 1024;
/// Requests one connection may make per second, and the burst on top. A
/// client makes a handful per game.
const REQUESTS_PER_SECOND: u32 = 10;
const REQUEST_BURST: u32 = 20;
/// Of those, room joins and invite-resolution attempts.
const JOINS_PER_SECOND: u32 = 1;
/// Pages of the room list a connection may ask for: one a second, with a
/// burst of five, so a browser can page ahead without scraping the server.
const LISTS_PER_SECOND: u32 = 1;
const LIST_BURST: u32 = 5;
const JOIN_BURST: u32 = 5;
/// Diagnostics requests, on a budget of their own so that they never make
/// a player's other requests wait or fail.
/// They carry the hook's and the game's logs too: two a second, with a
/// burst of sixteen, at most 32 lines each.
const DIAGNOSTICS_PER_SECOND: u32 = 2;
const DIAGNOSTICS_BURST: u32 = 16;
/// Game messages one connection may send per second, and the burst on top,
/// each kind on its own so a flood of one never starves another: dropping
/// a member's progress reports would make its room wait for it.
///
/// Checkpoints have no limit here. A flood would starve the sender's own
/// honest reports, and every round would then wait for them; the room
/// instead ignores reports for closed rounds at almost no cost.
const PROGRESS_PER_SECOND: u32 = 200;
const PROGRESS_BURST: u32 = 400;
/// Above the room's own per-player limit, which answers with reasons; this
/// only keeps a flood out of the room's queue.
const INTENTS_PER_SECOND: u32 = 40;
const INTENT_BURST: u32 = 80;
/// Build previews: the agent sends at most five a second while the player's
/// tool changes what it shows, and one every few seconds while it does not.
/// Excess ones are dropped.
const PREVIEWS_PER_SECOND: u32 = 5;
const PREVIEW_BURST: u32 = 10;
/// Time a client gets to say what a new bulk stream is for.
const BULK_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

static NEXT_LINK: AtomicU64 = AtomicU64::new(1);

pub(crate) async fn serve(incoming: quinn::Incoming, ticket: Handshake, shared: Arc<Shared>) {
    let connection = match incoming.await {
        Ok(connection) => connection,
        Err(error) => {
            debug!(%error, "connection attempt failed");
            return;
        }
    };
    // Addresses stay out of the logs; the stable ID correlates log lines.
    let connection_id = connection.stable_id();
    let admitted = match tokio::time::timeout(
        shared.handshake_timeout,
        handshake(&connection, ticket, &shared),
    )
    .await
    {
        Ok(Ok(admitted)) => admitted,
        Ok(Err(refusal)) => {
            debug!(connection = connection_id, %refusal, "handshake refused");
            metrics::increment(&shared.metrics.handshakes_refused);
            refusal.close(&connection);
            return;
        }
        Err(_) => {
            debug!(connection = connection_id, "handshake timed out");
            metrics::increment(&shared.metrics.handshakes_refused);
            connection.close(close::HANDSHAKE_TIMEOUT, b"handshake timed out");
            return;
        }
    };
    let Admitted {
        hello,
        session_id,
        issued,
        slot,
        address_share,
        send,
        recv,
    } = admitted;
    info!(
        connection = connection_id,
        session = %session_id,
        player = %hello.identity,
        client = %hello.client_version,
        platform = ?hello.platform,
        "session started"
    );
    metrics::increment(&shared.metrics.sessions_opened);
    Client::new(connection.clone(), shared, hello, session_id)
        .run(send, recv)
        .await;
    // The client may end its control stream and keep the connection; the
    // session is over either way.
    connection.close(close::NORMAL, b"session ended");
    let reason = connection.closed().await;
    info!(session = %session_id, %reason, "session ended");
    drop(address_share);
    drop(slot);
    drop(issued);
}

struct Admitted {
    hello: Hello,
    session_id: SessionId,
    /// Keeps `session_id` from being given to anyone else meanwhile.
    issued: IssuedSessionId,
    slot: OwnedSemaphorePermit,
    address_share: admission::Session,
    send: SendStream,
    recv: RecvStream,
}

/// Why the server ended a connection during the handshake.
#[derive(Debug, Error)]
enum Refusal {
    #[error("client speaks protocol {0}")]
    VersionMismatch(u32),
    #[error("no free session slot")]
    ServerFull,
    #[error("the address holds its share of sessions")]
    TooManyConnections,
    #[error("the identity proof did not verify")]
    BadProof,
    #[error("the client broke the protocol: {0}")]
    Violation(#[from] NetError),
    #[error("the first message was not a Hello")]
    UnexpectedMessage,
    #[error("the connection was lost: {0}")]
    Lost(#[from] quinn::ConnectionError),
}

impl Refusal {
    fn close(&self, connection: &quinn::Connection) {
        let (code, reason): (quinn::VarInt, &[u8]) = match self {
            Self::VersionMismatch(_) => (close::VERSION_MISMATCH, b"protocol version mismatch"),
            Self::ServerFull => (close::REJECTED, b"server full"),
            Self::TooManyConnections => (close::REJECTED, b"too many connections"),
            Self::BadProof => (close::REJECTED, b"identity not verified"),
            Self::Violation(_) | Self::UnexpectedMessage => {
                (close::PROTOCOL_VIOLATION, b"protocol violation")
            }
            Self::Lost(_) => return,
        };
        connection.close(code, reason);
    }
}

async fn handshake(
    connection: &quinn::Connection,
    ticket: Handshake,
    shared: &Shared,
) -> Result<Admitted, Refusal> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let client_protocol = read_preamble(&mut recv).await?;
    // Always answer with our version, so the client can say which side is old.
    write_preamble(&mut send, PROTOCOL_VERSION).await?;
    if client_protocol != PROTOCOL_VERSION {
        linger(&mut send).await;
        return Err(Refusal::VersionMismatch(client_protocol));
    }
    let ClientMessage::Hello(hello) = read_message(&mut recv, CONTROL_MAX_FRAME).await? else {
        return Err(Refusal::UnexpectedMessage);
    };
    if !verify_proof(connection, &hello.identity, &hello.proof) {
        reject(&mut send, RejectReason::BadProof).await?;
        return Err(Refusal::BadProof);
    }
    let Ok(slot) = Arc::clone(&shared.sessions).try_acquire_owned() else {
        reject(&mut send, RejectReason::ServerFull).await?;
        return Err(Refusal::ServerFull);
    };
    let Some(address_share) = ticket.into_session() else {
        reject(&mut send, RejectReason::TooManyConnections).await?;
        return Err(Refusal::TooManyConnections);
    };
    let issued = shared.session_ids.issue(|id| {
        shared
            .diagnostics
            .as_ref()
            .is_some_and(|diagnostics| diagnostics.has(id))
    });
    let session_id = issued.id;
    let welcome = ServerMessage::Welcome(Welcome {
        server_version: shared.server_version.clone(),
        session_id,
        rules: shared.directory.offers(),
    });
    write_message(&mut send, &welcome, CONTROL_MAX_FRAME).await?;
    Ok(Admitted {
        hello,
        session_id,
        issued,
        slot,
        address_share,
        send,
        recv,
    })
}

async fn reject(send: &mut SendStream, reason: RejectReason) -> Result<(), NetError> {
    let reject = ServerMessage::Reject(Reject { reason });
    write_message(send, &reject, CONTROL_MAX_FRAME).await?;
    linger(send).await;
    Ok(())
}

/// Finishes the stream and waits, briefly, until the peer has received all of
/// it, so a final message is not lost when the connection closes.
async fn linger(send: &mut SendStream) {
    if send.finish().is_ok() {
        // Timing out only means the peer may miss the reason; the connection
        // closes either way.
        let _ = tokio::time::timeout(LINGER, send.stopped()).await;
    }
}

/// The session IDs of the sessions open now.
#[derive(Default)]
pub(crate) struct SessionIds(Mutex<HashSet<SessionId>>);

impl SessionIds {
    /// A session ID no open session has, nor one for which `kept` holds:
    /// one whose diagnostics are still kept. A support code then names one
    /// session alone.
    pub(crate) fn issue(self: &Arc<Self>, kept: impl Fn(&SessionId) -> bool) -> IssuedSessionId {
        let mut open = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let id = loop {
            let id = SessionId(Code::random());
            if !open.contains(&id) && !kept(&id) {
                break id;
            }
        };
        open.insert(id);
        IssuedSessionId {
            ids: Arc::clone(self),
            id,
        }
    }
}

/// A session ID in use; it is free again when dropped.
pub(crate) struct IssuedSessionId {
    ids: Arc<SessionIds>,
    pub(crate) id: SessionId,
}

impl Drop for IssuedSessionId {
    fn drop(&mut self) {
        self.ids
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// Why the server stopped serving a client after the handshake.
#[derive(Debug, Error)]
enum Violation {
    #[error(transparent)]
    Stream(NetError),
    #[error("a second Hello")]
    SecondHello,
    #[error("a checkpoint or save with more than {MAX_CHECKPOINT_LANES} lanes")]
    TooManyLanes,
    #[error("the room could not queue a load acknowledgement")]
    LoadQueueFull,
}

/// The server side of one admitted client.
struct Client {
    connection: quinn::Connection,
    shared: Arc<Shared>,
    origin: Origin,
    player: PlayerId,
    hello: Hello,
    /// This connection's session: the support code its player sees.
    session: SessionId,
    link: MemberLink,
    /// What this player's game runs, once declared.
    content: Option<Arc<Declared>>,
    room: Option<RoomHandle>,
    /// The room, for the task that serves bulk streams.
    rooms: watch::Sender<Option<RoomHandle>>,
    control: Option<mpsc::Receiver<ServerMessage>>,
    advisory: Option<mpsc::Receiver<ServerMessage>>,
    turns: Option<mpsc::Receiver<TurnFeed>>,
    requests: TokenBucket,
    joins: TokenBucket,
    lists: TokenBucket,
    progress: TokenBucket,
    intents: TokenBucket,
    previews: TokenBucket,
    diagnostics: TokenBucket,
    /// Bytes of diagnostics this session has had kept.
    diagnostics_kept: u64,
    /// The launcher runs this session has sent lines of, indexed under
    /// them: one, unless a client changes its run.
    runs: Vec<tpf3mp_proto::LogSession>,
    /// The banner this player picked, for the rooms it joins.
    banner: Option<tpf3mp_proto::BannerId>,
}

impl Client {
    fn new(
        connection: quinn::Connection,
        shared: Arc<Shared>,
        hello: Hello,
        session: SessionId,
    ) -> Self {
        let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
        let (advisory_tx, advisory_rx) = mpsc::channel(ADVISORY_QUEUE);
        let (turns_tx, turns_rx) = mpsc::channel(TURN_QUEUE);
        let link = MemberLink {
            id: NEXT_LINK.fetch_add(1, Ordering::Relaxed),
            control: control_tx,
            advisory: advisory_tx,
            turns: turns_tx,
            connection: connection.clone(),
        };
        Self {
            origin: shared.origin(connection.remote_address()),
            connection,
            shared,
            player: hello.identity,
            hello,
            session,
            link,
            content: None,
            room: None,
            rooms: watch::Sender::new(None),
            control: Some(control_rx),
            advisory: Some(advisory_rx),
            turns: Some(turns_rx),
            requests: TokenBucket::new(REQUESTS_PER_SECOND, REQUEST_BURST),
            joins: TokenBucket::new(JOINS_PER_SECOND, JOIN_BURST),
            lists: TokenBucket::new(LISTS_PER_SECOND, LIST_BURST),
            progress: TokenBucket::new(PROGRESS_PER_SECOND, PROGRESS_BURST),
            intents: TokenBucket::new(INTENTS_PER_SECOND, INTENT_BURST),
            previews: TokenBucket::new(PREVIEWS_PER_SECOND, PREVIEW_BURST),
            diagnostics: TokenBucket::new(DIAGNOSTICS_PER_SECOND, DIAGNOSTICS_BURST),
            diagnostics_kept: 0,
            runs: Vec::new(),
            banner: None,
        }
    }

    async fn run(mut self, send: SendStream, mut recv: RecvStream) {
        let (Some(control), Some(advisory), Some(turns)) =
            (self.control.take(), self.advisory.take(), self.turns.take())
        else {
            return;
        };
        let control_writer = tokio::spawn(write_control(send, control, advisory));
        let turn_writer = tokio::spawn(write_turns(self.connection.clone(), turns));
        let bulk = tokio::spawn(accept_bulk(BulkPeer {
            connection: self.connection.clone(),
            shared: Arc::clone(&self.shared),
            player: self.player,
            link: self.link.id,
            rooms: self.rooms.subscribe(),
        }));
        let notices = tokio::spawn(forward_announcements(
            self.shared.announcements.subscribe(),
            self.link.control.clone(),
        ));
        if let Err(violation) = self.serve(&mut recv).await {
            debug!(player = %self.player, %violation, "closing a client that broke the protocol");
            metrics::increment(&self.shared.metrics.protocol_violations);
            self.connection
                .close(close::PROTOCOL_VIOLATION, b"protocol violation");
        }
        if let Some(room) = self.set_room(None) {
            room.notify(RoomCommand::Disconnected {
                player: self.player,
                link: self.link.id,
            });
        }
        control_writer.abort();
        turn_writer.abort();
        bulk.abort();
        notices.abort();
        // Each holds the connection, and with it the server's socket: the
        // connection's task ends only once they have.
        let _ = tokio::join!(control_writer, turn_writer, bulk, notices);
    }

    /// Enters or leaves a room, and returns the room left.
    fn set_room(&mut self, room: Option<RoomHandle>) -> Option<RoomHandle> {
        self.rooms.send_replace(room.clone());
        std::mem::replace(&mut self.room, room)
    }

    async fn serve(&mut self, recv: &mut RecvStream) -> Result<(), Violation> {
        let mut roomless_since = None;
        loop {
            let read = read_message::<ClientMessage>(recv, CONTROL_MAX_FRAME);
            let read = if self.room.is_some() {
                roomless_since = None;
                read.await
            } else {
                // A session outside any room holds a slot for nothing, so it
                // gets a deadline that requests do not extend.
                let since = *roomless_since.get_or_insert_with(Instant::now);
                match tokio::time::timeout_at(since + self.shared.roomless_timeout, read).await {
                    Ok(read) => read,
                    Err(_) => {
                        debug!(player = %self.player, "closing a session idle outside any room");
                        metrics::increment(&self.shared.metrics.idle_sessions_closed);
                        self.connection.close(close::IDLE, b"idle outside a room");
                        return Ok(());
                    }
                }
            };
            let message = match read {
                Ok(message) => message,
                Err(error) if error.is_disconnect() => return Ok(()),
                Err(error) => return Err(Violation::Stream(error)),
            };
            let now = std::time::Instant::now();
            match message {
                ClientMessage::Hello(_) => return Err(Violation::SecondHello),
                ClientMessage::Request { id, request } => {
                    let joining = matches!(
                        request,
                        Request::JoinRoom(_) | Request::ResolveInvite { .. }
                    );
                    let listing = matches!(request, Request::ListRooms { .. });
                    let diagnostics =
                        matches!(request, Request::Diagnostics(_) | Request::Telemetry(_));
                    let result = if diagnostics {
                        if self.diagnostics.take(now, 1) {
                            self.request(request).await
                        } else {
                            Err(RequestError::RateLimited)
                        }
                    } else if !self.requests.take(now, 1)
                        || (joining && !self.joins.take(now, 1))
                        || (listing && !self.lists.take(now, 1))
                    {
                        Err(RequestError::RateLimited)
                    } else {
                        self.request(request).await
                    };
                    let response = ServerMessage::Response { id, result };
                    if self.link.control.send(response).await.is_err() {
                        return Ok(());
                    }
                }
                ClientMessage::Game(message) => {
                    let allowed = match &message {
                        GameMessage::Intent { .. } => self.intents.take(now, 1),
                        GameMessage::Progress { .. } => self.progress.take(now, 1),
                        // The room keeps at most about two a second; more
                        // are never needed.
                        GameMessage::Loading(_) => self.progress.take(now, 1),
                        GameMessage::Preview(_) => self.previews.take(now, 1),
                        // Like checkpoints: the room ignores reports of
                        // saves it is not deciding.
                        GameMessage::Checkpoint { .. }
                        | GameMessage::Saved { .. }
                        | GameMessage::Loaded { .. } => true,
                    };
                    if allowed {
                        self.game(message)?;
                    } else if let GameMessage::Intent { client_seq, .. } = message {
                        self.reject_intent(client_seq, IntentRejection::RateLimited);
                    }
                }
            }
        }
    }

    fn new_member(&self) -> NewMember {
        NewMember {
            player: self.player,
            name: self.hello.name.clone(),
            platform: self.hello.platform,
            link: self.link.clone(),
            content: self.content.clone(),
            banner: self.banner.clone(),
        }
    }

    /// Joins the room of `join`'s invite.
    async fn join(&mut self, join: JoinRoom) -> Result<RoomView, RequestError> {
        let handle = self
            .shared
            .directory
            .find(&join.invite)
            .ok_or(RequestError::BadInvite)?;
        let member = self.new_member();
        let view = handle
            .request(|reply| RoomCommand::Join {
                member,
                invite: join.invite,
                password: join.password,
                resume: join.resume,
                scan_plan: join.scan_plan,
                reply,
            })
            .await?;
        self.set_room(Some(handle));
        Ok(view)
    }

    async fn request(&mut self, request: Request) -> Result<Response, RequestError> {
        match request {
            Request::CreateRoom(create) => {
                if self.still_in_room().await {
                    return Err(RequestError::AlreadyInRoom);
                }
                let share = self
                    .shared
                    .admission
                    .room(self.origin)
                    .ok_or(RequestError::TooManyRooms)?;
                let (handle, invite, room) =
                    self.shared
                        .directory
                        .create(self.new_member(), create, share)?;
                self.set_room(Some(handle));
                Ok(Response::RoomCreated { invite, room })
            }
            Request::JoinRoom(join) => {
                if self.still_in_room().await {
                    return Err(RequestError::AlreadyInRoom);
                }
                let admission = Arc::clone(&self.shared.admission);
                if !admission.may_join(self.origin, std::time::Instant::now()) {
                    return Err(RequestError::RateLimited);
                }
                let joined = self.join(join).await;
                if joined == Err(RequestError::BadInvite) {
                    admission.wrong_invite(self.origin, std::time::Instant::now());
                }
                joined.map(Response::RoomJoined)
            }
            Request::ResolveInvite { invite, password } => {
                let admission = Arc::clone(&self.shared.admission);
                let now = std::time::Instant::now();
                if !admission.may_join(self.origin, now) {
                    return Err(RequestError::RateLimited);
                }
                let result = self
                    .shared
                    .directory
                    .resolve_invite(self.player, invite, password)
                    .await;
                if result == Err(RequestError::BadInvite) {
                    admission.wrong_invite(self.origin, std::time::Instant::now());
                }
                result.map(|()| Response::InviteMatch)
            }
            Request::NativeIndex { proposed, page } => Ok(Response::NativeIndex(
                self.shared.native.page(proposed, page),
            )),
            Request::LeaveRoom => {
                let handle = self.set_room(None).ok_or(RequestError::NotInRoom)?;
                let player = self.player;
                handle
                    .request(|reply| RoomCommand::Leave { player, reply })
                    .await?;
                Ok(Response::Done)
            }
            Request::SetReady(ready) => {
                self.in_room(|player, reply| RoomCommand::SetReady {
                    player,
                    ready,
                    reply,
                })
                .await
            }
            Request::DeclareContent(_) | Request::DeclareRoom(_) => {
                let content = match request {
                    Request::DeclareContent(manifest) if manifest.is_valid() => {
                        Declared::new(manifest)
                    }
                    Request::DeclareRoom(declaration) if declaration.validate().is_ok() => {
                        Declared::with_room(*declaration)
                    }
                    _ => return Err(RequestError::InvalidContent),
                };
                let content = Arc::new(content);
                self.content = Some(Arc::clone(&content));
                if self.room.is_none() {
                    return Ok(Response::Done);
                }
                match self
                    .in_room(|player, reply| RoomCommand::DeclareContent {
                        player,
                        content,
                        reply,
                    })
                    .await
                {
                    // The room closed meanwhile: the declaration still holds
                    // for the next room.
                    Err(RequestError::NotInRoom) => Ok(Response::Done),
                    result => result,
                }
            }
            Request::StartGame => {
                self.in_room(|player, reply| RoomCommand::Start { player, reply })
                    .await
            }
            Request::SetSpeed(speed) => {
                self.in_room(|player, reply| RoomCommand::SetSpeed {
                    player,
                    speed,
                    reply,
                })
                .await
            }
            Request::Kick(target) => {
                self.in_room(|player, reply| RoomCommand::Kick {
                    player,
                    target,
                    reply,
                })
                .await
            }
            Request::Chat(text) => {
                self.in_room(|player, reply| RoomCommand::Chat {
                    player,
                    text,
                    reply,
                })
                .await
            }
            Request::Diagnostics(batch) => self.keep_diagnostics(&batch),
            Request::Telemetry(telemetry) => self.keep_telemetry(&telemetry),
            Request::SetBanner(banner) => {
                if banner
                    .as_ref()
                    .is_some_and(|id| !tpf3mp_proto::is_banner(id.as_str()))
                {
                    return Err(RequestError::UnknownBanner);
                }
                self.banner.clone_from(&banner);
                if self.room.is_none() {
                    return Ok(Response::Done);
                }
                match self
                    .in_room(|player, reply| RoomCommand::SetBanner {
                        player,
                        banner,
                        reply,
                    })
                    .await
                {
                    // The room closed meanwhile: kept for the next.
                    Err(RequestError::NotInRoom) => Ok(Response::Done),
                    result => result,
                }
            }
            Request::ListRooms { page } => Ok(Response::Rooms(self.shared.directory.list(page))),
            Request::DescribeRoom(listing) => {
                self.in_room(|player, reply| RoomCommand::Describe {
                    player,
                    listing,
                    reply,
                })
                .await
            }
            Request::StartWorld { world, save } => {
                if self.shared.snapshots.is_none() {
                    return Err(RequestError::WorldsNotKept);
                }
                self.in_room(|player, reply| RoomCommand::StartWorld {
                    player,
                    world,
                    save,
                    reply,
                })
                .await
            }
            Request::ClearStartWorld => {
                self.in_room(|player, reply| RoomCommand::ClearStartWorld { player, reply })
                    .await
            }
        }
    }

    /// Hands a batch of this session's diagnostics to the writer, without
    /// waiting on it.
    fn keep_diagnostics(
        &mut self,
        batch: &tpf3mp_proto::DiagnosticBatch,
    ) -> Result<Response, RequestError> {
        let diagnostics = self
            .shared
            .diagnostics
            .as_ref()
            .ok_or(RequestError::DiagnosticsNotKept)?;
        let who = crate::diagnostics::Who::new(self.player, self.hello.name.as_str());
        match diagnostics.submit(self.session, &who, self.diagnostics_kept, batch) {
            Ok(bytes) => {
                self.diagnostics_kept += bytes;
                Ok(Response::Done)
            }
            Err(crate::diagnostics::NotKept::Quota) => Err(RequestError::DiagnosticsNotKept),
            // The client keeps them and tries again later.
            Err(crate::diagnostics::NotKept::Busy) => Err(RequestError::RateLimited),
        }
    }

    /// Keeps a batch of a launcher's lines, every source, under this
    /// session, each with the launcher's run. Never waits on the writer.
    fn keep_telemetry(
        &mut self,
        telemetry: &tpf3mp_proto::Telemetry,
    ) -> Result<Response, RequestError> {
        /// Runs one session indexes at most: a client has one.
        const MAX_RUNS: usize = 4;
        let diagnostics = self
            .shared
            .diagnostics
            .as_ref()
            .ok_or(RequestError::DiagnosticsNotKept)?;
        let first = !self.runs.contains(&telemetry.run);
        if first && self.runs.len() >= MAX_RUNS {
            return Err(RequestError::DiagnosticsNotKept);
        }
        let who = crate::diagnostics::Who::new(self.player, self.hello.name.as_str());
        match diagnostics.submit_telemetry(
            self.session,
            &who,
            telemetry.run,
            first,
            self.diagnostics_kept,
            &telemetry.lines,
        ) {
            Ok(bytes) => {
                self.diagnostics_kept += bytes;
                if first {
                    self.runs.push(telemetry.run);
                }
                Ok(Response::Done)
            }
            Err(crate::diagnostics::NotKept::Quota) => Err(RequestError::DiagnosticsNotKept),
            // The client keeps them and tries again later.
            Err(crate::diagnostics::NotKept::Busy) => Err(RequestError::RateLimited),
        }
    }

    /// Whether this client is still in the room it last entered. A kick or
    /// a closed room leaves it without the connection being told, so the
    /// room is asked.
    async fn still_in_room(&mut self) -> bool {
        let Some(handle) = &self.room else {
            return false;
        };
        let player = self.player;
        let member = handle
            .request(|reply| RoomCommand::IsMember { player, reply })
            .await
            .is_ok();
        if !member {
            self.set_room(None);
        }
        member
    }

    async fn in_room(
        &mut self,
        make: impl FnOnce(PlayerId, Reply) -> RoomCommand,
    ) -> Result<Response, RequestError> {
        let handle = self.room.as_ref().ok_or(RequestError::NotInRoom)?;
        let player = self.player;
        let result = handle.request(|reply| make(player, reply)).await;
        if result == Err(RequestError::NotInRoom) {
            // The room closed or no longer counts us as a member.
            self.set_room(None);
        }
        result.map(|()| Response::Done)
    }

    fn game(&mut self, message: GameMessage) -> Result<(), Violation> {
        let Some(room) = &self.room else {
            // Game traffic can still be in flight just after a player left.
            if let GameMessage::Intent { client_seq, .. } = message {
                self.reject_intent(client_seq, IntentRejection::GameNotRunning);
            }
            return Ok(());
        };
        match message {
            GameMessage::Intent {
                client_seq,
                payload,
                secret,
            } => {
                let queued = room.notify(RoomCommand::Intent {
                    player: self.player,
                    client_seq,
                    payload,
                    secret,
                });
                if !queued {
                    self.reject_intent(client_seq, IntentRejection::RateLimited);
                }
            }
            GameMessage::Loading(stage) => {
                room.notify(RoomCommand::Loading {
                    player: self.player,
                    stage,
                });
            }
            GameMessage::Preview(preview) => {
                // Larger than any client sends: not shown, and no harm done.
                if preview.as_ref().is_none_or(|p| p.len() <= MAX_PREVIEW) {
                    room.notify(RoomCommand::Preview {
                        player: self.player,
                        preview,
                    });
                }
            }
            GameMessage::Progress { step, world_epoch } => {
                room.notify(RoomCommand::Progress {
                    world_epoch,
                    player: self.player,
                    link: self.link.id,
                    step,
                });
            }
            GameMessage::Loaded { step, world_epoch } => {
                if !room.notify(RoomCommand::Loaded {
                    player: self.player,
                    link: self.link.id,
                    world_epoch,
                    step,
                }) {
                    return Err(Violation::LoadQueueFull);
                }
            }
            GameMessage::Checkpoint {
                step,
                lanes,
                world_epoch,
            } => {
                if lanes.len() > MAX_CHECKPOINT_LANES {
                    return Err(Violation::TooManyLanes);
                }
                room.notify(RoomCommand::Checkpoint {
                    world_epoch,
                    player: self.player,
                    link: self.link.id,
                    step,
                    lanes,
                });
            }
            GameMessage::Saved {
                world_epoch,
                event,
                lanes,
                world,
            } => {
                if lanes.len() > MAX_CHECKPOINT_LANES {
                    return Err(Violation::TooManyLanes);
                }
                room.notify(RoomCommand::Saved {
                    world_epoch,
                    player: self.player,
                    link: self.link.id,
                    event,
                    lanes,
                    world,
                });
            }
        }
        Ok(())
    }

    fn reject_intent(&self, client_seq: u64, reason: IntentRejection) {
        let _ = self
            .link
            .control
            .try_send(ServerMessage::IntentRejected { client_seq, reason });
    }
}

/// Writes the client's control stream: its own messages first, other
/// members' previews when none wait, so a preview never holds up a
/// response, a chat line or a rejected intent. Ends when the control
/// messages end.
async fn write_control(
    mut send: SendStream,
    mut messages: mpsc::Receiver<ServerMessage>,
    mut advisory: mpsc::Receiver<ServerMessage>,
) {
    let mut budget = TokenBucket::new(ADVISORY_BYTES_PER_SECOND, ADVISORY_BYTES_BURST);
    loop {
        let message = tokio::select! {
            biased;
            message = messages.recv() => match message {
                Some(message) => message,
                None => return,
            },
            Some(message) = advisory.recv() => {
                if !budget.take(std::time::Instant::now(), advisory_cost(&message)) {
                    continue;
                }
                message
            }
        };
        if write_message(&mut send, &message, CONTROL_MAX_FRAME)
            .await
            .is_err()
        {
            return;
        }
    }
}

/// What a relayed preview costs a client's advisory byte budget: its
/// payload. One that hides costs nothing, so the budget never keeps a
/// preview drawn that its member's tool no longer shows.
fn advisory_cost(message: &ServerMessage) -> u64 {
    match message {
        ServerMessage::Preview {
            preview: Some(preview),
            ..
        } => u64::try_from(preview.len()).unwrap_or(u64::MAX),
        _ => 0,
    }
}

/// Writes the client's turn stream. A new stream replaces the old one, which
/// is finished first so the client reads them in order.
async fn write_turns(connection: quinn::Connection, mut feed: mpsc::Receiver<TurnFeed>) {
    let mut stream: Option<SendStream> = None;
    while let Some(item) = feed.recv().await {
        let written = match item {
            TurnFeed::Open { start, backlog } => {
                if let Some(mut old) = stream.take() {
                    let _ = old.finish();
                }
                match open_turn_stream(&connection, start, &backlog).await {
                    Ok(opened) => {
                        stream = Some(opened);
                        true
                    }
                    Err(()) => false,
                }
            }
            TurnFeed::Frame(frame) => match stream.as_mut() {
                Some(send) => write_frame(send, &frame).await.is_ok(),
                None => true,
            },
            TurnFeed::Close => {
                if let Some(mut old) = stream.take() {
                    let _ = old.finish();
                }
                true
            }
        };
        if !written {
            return;
        }
    }
}

async fn open_turn_stream(
    connection: &quinn::Connection,
    start: TurnStart,
    backlog: &[Arc<[u8]>],
) -> Result<SendStream, ()> {
    let mut send = connection.open_uni().await.map_err(|_| ())?;
    write_preamble(&mut send, PROTOCOL_VERSION)
        .await
        .map_err(|_| ())?;
    write_message(&mut send, &TurnMessage::Start(start), TURN_MAX_FRAME)
        .await
        .map_err(|_| ())?;
    for frame in backlog {
        write_frame(&mut send, frame).await.map_err(|_| ())?;
    }
    Ok(send)
}

/// What the task serving a client's bulk streams knows of the client.
struct BulkPeer {
    connection: quinn::Connection,
    shared: Arc<Shared>,
    player: PlayerId,
    link: u64,
    rooms: watch::Receiver<Option<RoomHandle>>,
}

/// Serves the client's bulk streams, one at a time: the transport lets a
/// client open no more than its control stream and one other.
async fn accept_bulk(peer: BulkPeer) {
    while let Ok((send, recv)) = peer.connection.accept_bi().await {
        if let Err(error) = bulk_stream(&peer, send, recv).await
            && error.is_violation()
        {
            debug!(player = %peer.player, %error, "closing a client that broke the bulk protocol");
            metrics::increment(&peer.shared.metrics.protocol_violations);
            peer.connection
                .close(close::PROTOCOL_VIOLATION, b"protocol violation");
            return;
        }
    }
}

/// One bulk stream: the client fetches a snapshot the room offered it, or
/// uploads a save the room asked it for.
async fn bulk_stream(
    peer: &BulkPeer,
    mut send: SendStream,
    mut recv: RecvStream,
) -> Result<(), BulkError> {
    let opening = async {
        let version = read_preamble(&mut recv).await?;
        write_preamble(&mut send, PROTOCOL_VERSION).await?;
        if version != PROTOCOL_VERSION {
            return Err(BulkError::Violation("a bulk stream of another version"));
        }
        Ok(read_message::<BulkOpen>(&mut recv, BULK_REQUEST_MAX_FRAME).await?)
    };
    let open = tokio::time::timeout(BULK_OPEN_TIMEOUT, opening)
        .await
        .map_err(|_| BulkError::Idle(BULK_OPEN_TIMEOUT))??;
    let room = peer.rooms.borrow().clone();
    let (Some(snapshots), Some(room)) = (&peer.shared.snapshots, room) else {
        return refuse(&mut send, open).await;
    };
    let (player, link) = (peer.player, peer.link);
    match open {
        BulkOpen::Fetch { snapshot } => {
            let manifest = room
                .ask(|reply| RoomCommand::Fetch {
                    player,
                    link,
                    snapshot,
                    reply,
                })
                .await
                .flatten();
            let Some(manifest) = manifest else {
                return refuse(&mut send, open).await;
            };
            let _permit = transfer_permit(snapshots).await;
            let served =
                bulk::serve(&mut send, &mut recv, &snapshots.store, &manifest, BULK_IDLE).await;
            let _ = send.finish();
            let served = served?;
            metrics::add(&peer.shared.metrics.snapshot_bytes_served, served.bytes);
            debug!(%player, %snapshot, chunks = served.chunks, "served a snapshot");
            Ok(())
        }
        BulkOpen::Serve { snapshot } => {
            let asked = room
                .ask(|reply| RoomCommand::Upload {
                    player,
                    link,
                    snapshot,
                    reply,
                })
                .await
                .unwrap_or(false);
            if !asked {
                return refuse(&mut send, open).await;
            }
            let _permit = transfer_permit(snapshots).await;
            // Held from before it arrives, so no room letting go of the same
            // world can take it; the room takes the hold over.
            let id = bulk::manifest_id(&snapshot);
            snapshots.hold(id);
            let fetched = bulk::fetch(
                &mut send,
                &mut recv,
                &snapshots.store,
                &id,
                Completion::Retain,
                BULK_IDLE,
                |_| {},
            )
            .await;
            let violation = fetched.as_ref().err().is_some_and(BulkError::is_violation);
            let failed = fetched.is_err();
            let result = match fetched {
                Ok(manifest) => Ok(Arc::new(manifest)),
                Err(error) => Err(error.to_string()),
            };
            let told = room
                .tell(RoomCommand::Uploaded {
                    player,
                    snapshot,
                    result,
                })
                .await;
            if failed || !told {
                // Nothing arrived, or the room closed meanwhile: the hold
                // goes back.
                let snapshots = Arc::clone(snapshots);
                let _ = tokio::task::spawn_blocking(move || snapshots.release(&[id])).await;
            }
            if violation {
                return Err(BulkError::Violation("a broken upload"));
            }
            Ok(())
        }
    }
}

/// Ends a bulk stream the server will not serve or receive. A client that
/// wanted to fetch hears that the snapshot is unavailable; one that wanted
/// to upload sees the stream end without a request.
async fn refuse(send: &mut SendStream, open: BulkOpen) -> Result<(), BulkError> {
    if let BulkOpen::Fetch { .. } = open {
        write_message(send, &BulkResponse::Unavailable, BULK_RESPONSE_MAX_FRAME).await?;
    }
    let _ = send.finish();
    Ok(())
}

async fn transfer_permit(snapshots: &Snapshots) -> Option<OwnedSemaphorePermit> {
    Arc::clone(&snapshots.transfers).acquire_owned().await.ok()
}

/// Passes the operator's notices to this client. A notice that finds the
/// client's queue full is skipped rather than waited for: it is advice,
/// and the queue carries the game.
async fn forward_announcements(
    mut announcements: tokio::sync::broadcast::Receiver<ChatText>,
    control: mpsc::Sender<ServerMessage>,
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match announcements.recv().await {
            Ok(text) => {
                let _ = control.try_send(ServerMessage::Notice(text));
            }
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use tpf3mp_proto::{FixedBytes, MAX_PREVIEW, Payload, PlayerId, ServerMessage};

    use super::*;

    fn preview(len: usize) -> ServerMessage {
        ServerMessage::Preview {
            from: PlayerId(FixedBytes([7; 32])),
            preview: (len > 0).then(|| Payload::new(vec![0; len]).unwrap()),
        }
    }

    #[test]
    fn a_flood_of_previews_is_cut_to_the_byte_budget_and_hides_always_pass() {
        let now = std::time::Instant::now();
        let mut budget = TokenBucket::new(ADVISORY_BYTES_PER_SECOND, ADVISORY_BYTES_BURST);
        let largest = preview(MAX_PREVIEW);
        let mut written = 0;
        for _ in 0..20 {
            if budget.take(now, advisory_cost(&largest)) {
                written += 1;
            }
        }
        assert_eq!(written, 4, "64 KiB of 16 KiB previews at once, no more");
        assert!(
            budget.take(now, advisory_cost(&preview(0))),
            "a hide is never dropped for the budget"
        );
        assert_eq!(advisory_cost(&ServerMessage::Kicked), 0);
        // A second on, three more fit.
        let later = now + Duration::from_secs(1);
        let refilled = (0..20)
            .filter(|_| budget.take(later, advisory_cost(&largest)))
            .count();
        assert_eq!(refilled, 3);
    }
}
