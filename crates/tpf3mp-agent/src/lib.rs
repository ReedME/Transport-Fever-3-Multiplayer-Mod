//! The TPF3-MP client: connects to a server, proves the player's identity,
//! and exposes rooms and the turn stream to the game side. The shared-memory
//! link to the in-game hook builds on this (see `docs/ARCHITECTURE.md`).

pub mod about;
pub mod bridge;
pub mod content;
pub mod diagnostics;
mod follower;
pub mod game_logs;
pub mod launcher;
pub mod logs;
pub mod native_mods;
pub mod own_mod;
pub mod picker;
mod playout;
pub mod portraits;
pub mod save_check;
pub mod steam;
pub mod transfer;

use std::{
    collections::HashMap,
    fmt, io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use quinn::{RecvStream, SendStream};
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tpf3mp_net::{
    Identity, IdentityError, NetError, ServerTrust, TlsError, client_config, close, read_message,
    read_preamble,
    tunnel::{self, TunnelError, TunnelUrl},
    write_message, write_preamble,
};
use tpf3mp_proto::{
    CONTROL_MAX_FRAME, ChatText, ClientMessage, ContentDiff, ContentManifest, CreateRoom,
    GameMessage, Hello, IntentRejection, Invite, JoinRoom, LaneDigest, PROTOCOL_VERSION, Payload,
    Platform, PlayerId, RejectReason, Request, RequestError, Response, RoomView, SavedWorld,
    Secret, ServerMessage, SnapshotId, Speed, TURN_MAX_FRAME, Text, Turn, TurnMessage, TurnStart,
    Welcome,
};

pub use follower::{Action, FollowError, TurnFollower};
pub use playout::Playout;
pub use transfer::{BulkOpener, Worlds};

/// How long connecting and the handshake may take, so a server that never
/// answers cannot hang the client.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a request may wait for its response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long closing waits for the last lines of the player's log to go.
const LAST_DIAGNOSTICS: Duration = Duration::from_secs(2);
/// Messages queued for the server before senders wait.
const OUTGOING_QUEUE: usize = 256;
/// Events queued for the application before the client stops reading. The
/// server then sees a slow consumer and disconnects rather than buffer.
const EVENT_QUEUE: usize = 1024;
/// Bytes of turns queued for the application before the client stops
/// reading, weighed like [`TurnFollower`]'s backlog. Counting events alone,
/// a hostile server could make the client hold gigabytes.
const EVENT_BYTES: usize = 64 << 20;

/// How long UDP gets on its own before a tunnel joins the race. A QUIC
/// handshake takes a round trip or two; three seconds cover a lost packet
/// and its first resend.
pub const FALLBACK_AFTER: Duration = Duration::from_secs(3);

/// How a client reaches the server.
#[derive(Debug, Clone, Default)]
pub enum Route {
    /// QUIC over UDP.
    #[default]
    Udp,
    /// QUIC over UDP, and through this tunnel too if UDP has not connected
    /// after [`ConnectOptions::fallback_after`]; whichever connects first
    /// is kept.
    UdpOrTunnel(TunnelUrl),
    /// QUIC through this tunnel only, for networks known to block UDP.
    Tunnel(TunnelUrl),
}

/// Which tunnel, if any, a player takes when UDP does not get through.
#[derive(Debug, Clone, Default)]
pub enum TunnelChoice {
    /// Fall back to the one servers serve by default, `wss://<host>/tpf3mp`.
    #[default]
    Default,
    /// Fall back to this one.
    Url(TunnelUrl),
    /// A tunnel only, never UDP: this one, or the default one.
    Only(Option<TunnelUrl>),
    /// None: UDP only.
    Off,
}

impl TunnelChoice {
    /// The route to a server at `host`.
    pub fn route(&self, host: &str) -> Result<Route, TunnelError> {
        Ok(match self {
            // A host no URL can name gets no fallback, rather than no route.
            Self::Default => TunnelUrl::default_for(host).map_or(Route::Udp, Route::UdpOrTunnel),
            Self::Url(url) => Route::UdpOrTunnel(url.clone()),
            Self::Only(Some(url)) => Route::Tunnel(url.clone()),
            Self::Only(None) => Route::Tunnel(TunnelUrl::default_for(host)?),
            Self::Off => Route::Udp,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub server: SocketAddr,
    /// The name the server's certificate must be valid for.
    pub server_name: String,
    pub trust: ServerTrust,
    pub identity: Arc<Identity>,
    pub name: Text<32>,
    pub client_version: Text<64>,
    /// The protocol version announced in the preamble. Only tests change it.
    pub protocol_version: u32,
    /// UDP, a tunnel, or both. Through a tunnel, `server` only names the
    /// peer for QUIC; everything goes to the tunnel's host.
    pub route: Route,
    /// How long UDP has alone before a fallback tunnel joins the race.
    pub fallback_after: Duration,
    /// Where the lines of this player's log wait to go to the server
    /// ("Diagnostics" in PROTOCOL.md). `None` sends none.
    pub diagnostics: Option<diagnostics::Recorder>,
    /// The banner this player shows in rooms, told the server on every
    /// connection (`Request::SetBanner`); `None` for the default.
    pub banner: Option<tpf3mp_proto::BannerId>,
}

impl ConnectOptions {
    /// The options for connecting again after `client` connected with
    /// these: a network that needed the tunnel likely still does, so UDP
    /// and the tunnel start together, and whichever answers first wins.
    pub fn again_after(&self, client: &Client) -> Self {
        let mut options = self.clone();
        if client.tunneled() && matches!(options.route, Route::UdpOrTunnel(_)) {
            options.fallback_after = Duration::ZERO;
        }
        options
    }
}

impl ConnectOptions {
    pub fn new(
        server: SocketAddr,
        server_name: impl Into<String>,
        trust: ServerTrust,
        identity: Arc<Identity>,
        name: Text<32>,
    ) -> Self {
        Self {
            server,
            server_name: server_name.into(),
            trust,
            identity,
            name,
            client_version: Text::new(env!("CARGO_PKG_VERSION"))
                .expect("the crate version is short printable text"),
            protocol_version: PROTOCOL_VERSION,
            route: Route::Udp,
            fallback_after: FALLBACK_AFTER,
            diagnostics: None,
            banner: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("cannot open a UDP socket: {0}")]
    Socket(#[from] io::Error),
    #[error("cannot open the tunnel: {0}")]
    Tunnel(#[from] TunnelError),
    #[error("the server is out of reach over UDP ({udp}) and through {url} ({tunnel})")]
    NoRoute {
        url: String,
        udp: Box<ConnectError>,
        tunnel: Box<ConnectError>,
    },
    #[error("cannot start the connection: {0}")]
    Start(#[from] quinn::ConnectError),
    #[error("connection failed: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("{}", version_mismatch(.client, .server))]
    VersionMismatch { client: u32, server: u32 },
    #[error("the server declined: {0}")]
    Rejected(RejectReason),
    #[error("the server broke the protocol: {0}")]
    Protocol(#[from] NetError),
    #[error("the server answered the handshake with an unexpected message")]
    UnexpectedMessage,
    #[error("the server did not complete the handshake in time")]
    Timeout,
}

impl ConnectError {
    /// Whether the server speaks a newer protocol than this client: only an
    /// update lets this client play there, and trying again will not.
    pub fn client_is_older(&self) -> bool {
        match self {
            Self::VersionMismatch { client, server } => server > client,
            Self::NoRoute { udp, tunnel, .. } => udp.client_is_older() || tunnel.client_is_older(),
            _ => false,
        }
    }

    /// The protocols of a mismatch, this client's and the server's, also
    /// when it was one of the routes tried.
    pub fn mismatch(&self) -> Option<(u32, u32)> {
        match self {
            Self::VersionMismatch { client, server } => Some((*client, *server)),
            Self::NoRoute { udp, tunnel, .. } => udp.mismatch().or_else(|| tunnel.mismatch()),
            _ => None,
        }
    }

    /// What the player is told: for a protocol mismatch, which side is old,
    /// what to do and which file they started ([`about::protocol_mismatch`]);
    /// otherwise the error itself.
    pub fn for_player(&self) -> String {
        match self.mismatch() {
            Some((client, server)) => {
                about::protocol_mismatch(client, server, about::exe().as_deref())
            }
            None => self.to_string(),
        }
    }
}

fn version_mismatch(client: &u32, server: &u32) -> String {
    let advice = if server > client {
        "update TPF3-MP"
    } else {
        "the server has not been updated yet"
    };
    format!("this client speaks protocol {client} but the server speaks {server}: {advice}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClientError {
    #[error("the server refused: {0}")]
    Refused(RequestError),
    #[error("the connection to the server is gone")]
    Disconnected,
    #[error("the server did not answer in time")]
    Timeout,
    #[error("the server answered with an unexpected response")]
    UnexpectedResponse,
}

/// Something the server told this client.
#[derive(Debug)]
pub enum ClientEvent {
    RoomUpdate(RoomView),
    IntentRejected {
        client_seq: u64,
        reason: IntentRejection,
    },
    Diverged {
        step: u64,
        lanes: Vec<u16>,
    },
    /// A turn stream began; turns that follow continue from it.
    TurnStream(TurnStart),
    Turn(Turn),
    /// The room's owner removed this player, who cannot come back to it.
    Kicked,
    /// The room asks for the world this client saved at the save event
    /// `event`.
    Upload {
        event: u64,
        snapshot: SnapshotId,
    },
    /// A member of the room said something.
    Chat {
        from: PlayerId,
        text: ChatText,
    },
    /// How this player's game differs from the room's, or `None` once it
    /// no longer does.
    ContentDiff(Option<ContentDiff>),
    /// The server's operator says something to everyone connected.
    Notice(ChatText),
    /// The room's mods, as its owner declared them, or `None` while it has
    /// none (protocol 18's `ServerMessage::RoomMods`).
    RoomMods(Option<Box<tpf3mp_proto::RoomMods>>),
    /// What another member's build tool shows now, or `None` once it shows
    /// nothing. Dropped rather than queued when events are not taken fast
    /// enough: another comes within seconds.
    Preview {
        from: PlayerId,
        preview: Option<Payload>,
    },
    /// The connection ended.
    Closed(quinn::ConnectionError),
}

/// An event with the share of the byte budget it holds while queued.
type Queued = (ClientEvent, Option<OwnedSemaphorePermit>);

/// What the server tells this client, in order. Turns count against a byte
/// budget until received, so a client that reads slowly stops reading the
/// network instead of buffering without bound.
#[derive(Debug)]
pub struct Events {
    receiver: mpsc::Receiver<Queued>,
}

impl Events {
    /// The next event, or `None` once the connection is gone and every
    /// event has been received.
    pub async fn recv(&mut self) -> Option<ClientEvent> {
        self.receiver.recv().await.map(|(event, _budget)| event)
    }

    /// The next event if one is queued.
    pub fn try_recv(&mut self) -> Result<ClientEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv().map(|(event, _budget)| event)
    }

    /// Events queued now.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }
}

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Result<Response, RequestError>>>>>;

/// An open, authenticated session with a server.
pub struct Client {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    welcome: Welcome,
    player: PlayerId,
    requests: Requests,
    tunneled: bool,
    /// The lines of this player's log this connection sends, if any.
    diagnostics: Option<diagnostics::Recorder>,
}

/// Sends requests on a client's control stream and matches the responses.
/// Cheap to clone, so work that must not wait on a round trip can hand
/// requests to a task of their own.
#[derive(Clone)]
pub struct Requests {
    outgoing: mpsc::Sender<ClientMessage>,
    pending: Pending,
    reader_done: Arc<AtomicBool>,
    next_request: Arc<AtomicU32>,
}

impl fmt::Debug for Requests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Requests").finish_non_exhaustive()
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("player", &self.player)
            .field("session_id", &self.welcome.session_id)
            .finish_non_exhaustive()
    }
}

/// Finds the address of a server given as `host:port`: an IPv4 one when
/// there is one, as servers listen on IPv4 by default while `localhost` and
/// names with both kinds of record may list IPv6 first.
pub async fn resolve(server: &str) -> io::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host(server).await?.collect();
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or(addresses.first())
        .copied()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the name has no address"))
}

/// Connects to a server and completes the handshake. Server messages arrive
/// on the returned receiver.
pub async fn connect(options: ConnectOptions) -> Result<(Client, Events), ConnectError> {
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    tokio::time::timeout_at(deadline, connect_within(options, deadline))
        .await
        .map_err(|_| ConnectError::Timeout)?
}

/// A QUIC connection and the endpoint it runs on.
type Opened = (quinn::Endpoint, quinn::Connection);

/// Opens the QUIC connection by the options' route, by `deadline`. Says
/// whether it runs through a tunnel. When neither UDP nor the tunnel
/// connects, the error names what went wrong with both.
async fn open_connection(
    options: &ConnectOptions,
    deadline: tokio::time::Instant,
) -> Result<(Opened, bool), ConnectError> {
    let url = match &options.route {
        Route::Udp => return Ok((Box::pin(over_udp(options)).await?, false)),
        Route::Tunnel(url) => return Ok((Box::pin(through_tunnel(options, url)).await?, true)),
        Route::UdpOrTunnel(url) => url,
    };
    // Boxed: the handshakes' state would make every future that awaits a
    // connection too large for a thread's stack.
    let mut udp = Box::pin(over_udp(options));
    let failed_early = tokio::select! {
        result = &mut udp => match result {
            Ok(opened) => return Ok((opened, false)),
            Err(error) => Some(error),
        },
        () = tokio::time::sleep(options.fallback_after) => None,
    };
    if let Some(udp_error) = failed_early {
        return match by_deadline(deadline, Box::pin(through_tunnel(options, url))).await {
            Ok(opened) => Ok((opened, true)),
            Err(tunnel_error) => Err(no_route(url, udp_error, tunnel_error)),
        };
    }
    // UDP is slow to answer: race it against the tunnel.
    let mut tunnel = Box::pin(through_tunnel(options, url));
    tokio::select! {
        result = &mut udp => match result {
            Ok(opened) => Ok((opened, false)),
            Err(udp_error) => match by_deadline(deadline, tunnel).await {
                Ok(opened) => Ok((opened, true)),
                Err(tunnel_error) => Err(no_route(url, udp_error, tunnel_error)),
            },
        },
        result = &mut tunnel => match result {
            Ok(opened) => Ok((opened, true)),
            Err(tunnel_error) => match by_deadline(deadline, udp).await {
                Ok(opened) => Ok((opened, false)),
                Err(udp_error) => Err(no_route(url, udp_error, tunnel_error)),
            },
        },
    }
}

/// Runs a connection attempt until `deadline`. One that never answers fails
/// with `Timeout`, to be reported next to the other route's failure.
async fn by_deadline(
    deadline: tokio::time::Instant,
    attempt: impl Future<Output = Result<Opened, ConnectError>>,
) -> Result<Opened, ConnectError> {
    tokio::time::timeout_at(deadline, attempt)
        .await
        .unwrap_or(Err(ConnectError::Timeout))
}

fn no_route(url: &TunnelUrl, udp: ConnectError, tunnel: ConnectError) -> ConnectError {
    ConnectError::NoRoute {
        url: url.to_string(),
        udp: Box::new(udp),
        tunnel: Box::new(tunnel),
    }
}

async fn over_udp(options: &ConnectOptions) -> Result<Opened, ConnectError> {
    let local: SocketAddr = if options.server.is_ipv6() {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    };
    // Through tpf3mp_net::udp, which falls back to a plain socket where the
    // network stack refuses quinn's socket options (Wine and Proton).
    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        tpf3mp_net::udp::bind(local)?,
        Arc::new(quinn::TokioRuntime),
    )?;
    endpoint.set_default_client_config(client_config(options.trust.clone())?);
    let connection = endpoint
        .connect(options.server, &options.server_name)?
        .await?;
    Ok((endpoint, connection))
}

async fn through_tunnel(options: &ConnectOptions, url: &TunnelUrl) -> Result<Opened, ConnectError> {
    let socket = tunnel::connect(url, &options.trust, options.server).await?;
    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        socket,
        Arc::new(quinn::TokioRuntime),
    )?;
    endpoint.set_default_client_config(client_config(options.trust.clone())?);
    let connection = endpoint
        .connect(options.server, &options.server_name)?
        .await?;
    Ok((endpoint, connection))
}

async fn connect_within(
    options: ConnectOptions,
    deadline: tokio::time::Instant,
) -> Result<(Client, Events), ConnectError> {
    let ((endpoint, connection), tunneled) = open_connection(&options, deadline).await?;
    let (welcome, send, recv) = match handshake(&connection, &options).await {
        Ok(opened) => opened,
        Err(error) => {
            let code = match &error {
                ConnectError::VersionMismatch { .. } => close::VERSION_MISMATCH,
                ConnectError::Rejected(_) => close::NORMAL,
                _ => close::PROTOCOL_VIOLATION,
            };
            connection.close(code, b"");
            endpoint.wait_idle().await;
            return Err(error);
        }
    };

    let (outgoing, outgoing_rx) = mpsc::channel(OUTGOING_QUEUE);
    let (events, events_rx) = mpsc::channel(EVENT_QUEUE);
    let pending = Pending::default();
    let reader_done = Arc::new(AtomicBool::new(false));
    tokio::spawn(write_control(send, outgoing_rx));
    tokio::spawn(read_control(
        recv,
        connection.clone(),
        Arc::clone(&pending),
        Arc::clone(&reader_done),
        events.clone(),
    ));
    tokio::spawn(read_turns(
        connection.clone(),
        events.clone(),
        Arc::new(Semaphore::new(EVENT_BYTES)),
    ));
    tokio::spawn({
        let connection = connection.clone();
        async move {
            let reason = connection.closed().await;
            let _ = events.send((ClientEvent::Closed(reason), None)).await;
        }
    });

    let requests = Requests {
        outgoing,
        pending,
        reader_done,
        next_request: Arc::new(AtomicU32::new(1)),
    };
    if let Some(recorder) = options.diagnostics.clone() {
        let connection = connection.clone();
        tokio::spawn(diagnostics::upload(
            recorder,
            requests.clone(),
            async move {
                connection.closed().await;
            },
        ));
    }
    if let Some(banner) = options.banner.clone() {
        // Before anything else this connection asks, on the same stream.
        let requests = requests.clone();
        tokio::spawn(async move {
            if let Err(error) = requests
                .request(tpf3mp_proto::Request::SetBanner(Some(banner)))
                .await
            {
                tracing::debug!(%error, "the server did not take the banner");
            }
        });
    }
    Ok((
        Client {
            endpoint,
            connection,
            welcome,
            player: options.identity.player(),
            requests,
            tunneled,
            diagnostics: options.diagnostics,
        },
        Events {
            receiver: events_rx,
        },
    ))
}

async fn handshake(
    connection: &quinn::Connection,
    options: &ConnectOptions,
) -> Result<(Welcome, SendStream, RecvStream), ConnectError> {
    let failed = |error| stream_failure(connection, error);
    let (mut send, mut recv) = connection.open_bi().await?;
    write_preamble(&mut send, options.protocol_version)
        .await
        .map_err(failed)?;
    let server_protocol = read_preamble(&mut recv).await.map_err(failed)?;
    if server_protocol != options.protocol_version {
        return Err(ConnectError::VersionMismatch {
            client: options.protocol_version,
            server: server_protocol,
        });
    }
    let hello = ClientMessage::Hello(Hello {
        client_version: options.client_version.clone(),
        platform: Platform::current(),
        name: options.name.clone(),
        identity: options.identity.player(),
        proof: options.identity.prove(connection)?,
    });
    write_message(&mut send, &hello, CONTROL_MAX_FRAME)
        .await
        .map_err(failed)?;
    match read_message::<ServerMessage>(&mut recv, CONTROL_MAX_FRAME)
        .await
        .map_err(failed)?
    {
        ServerMessage::Welcome(welcome) => Ok((welcome, send, recv)),
        ServerMessage::Reject(reject) => Err(ConnectError::Rejected(reject.reason)),
        _ => Err(ConnectError::UnexpectedMessage),
    }
}

/// A stream error caused by the server closing the connection is reported as
/// the server's close reason, which says why.
fn stream_failure(connection: &quinn::Connection, error: NetError) -> ConnectError {
    match connection.close_reason() {
        Some(reason) => ConnectError::Connection(reason),
        None => ConnectError::Protocol(error),
    }
}

impl Client {
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }

    pub fn player(&self) -> PlayerId {
        self.player
    }

    pub fn rtt(&self) -> Duration {
        self.connection.rtt()
    }

    /// Whether the session runs through a tunnel rather than over UDP.
    pub fn tunneled(&self) -> bool {
        self.tunneled
    }

    /// Waits for the connection to close, and says why it did.
    pub async fn closed(&self) -> quinn::ConnectionError {
        self.connection.closed().await
    }

    /// Sends a request and waits for its response.
    pub async fn request(&self, request: Request) -> Result<Response, ClientError> {
        self.requests.request(request).await
    }

    /// A handle that sends requests on this client's connection.
    pub fn requests(&self) -> Requests {
        self.requests.clone()
    }

    pub async fn create_room(&self, create: CreateRoom) -> Result<(Invite, RoomView), ClientError> {
        match self.request(Request::CreateRoom(create)).await? {
            Response::RoomCreated { invite, room } => Ok((invite, room)),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    /// Page `page` of the server's public rooms.
    pub async fn list_rooms(&self, page: u16) -> Result<tpf3mp_proto::RoomPage, ClientError> {
        match self
            .request(tpf3mp_proto::Request::ListRooms { page })
            .await?
        {
            tpf3mp_proto::Response::Rooms(page) => Ok(page),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    pub async fn join_room(&self, join: JoinRoom) -> Result<RoomView, ClientError> {
        match self.request(Request::JoinRoom(join)).await? {
            Response::RoomJoined(room) => Ok(room),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    pub async fn leave_room(&self) -> Result<(), ClientError> {
        self.done(Request::LeaveRoom).await
    }

    pub async fn set_ready(&self, ready: bool) -> Result<(), ClientError> {
        self.done(Request::SetReady(ready)).await
    }

    /// Declares what this player's game runs: for this connection, and to
    /// the room it is in. A running game can only be joined after this.
    pub async fn declare_content(&self, content: ContentManifest) -> Result<(), ClientError> {
        self.done(Request::DeclareContent(content)).await
    }

    /// Declares what this player's game runs, or as the room's owner, that
    /// and the room's mods ([`picker::Declaration`]).
    pub async fn declare(&self, declaration: picker::Declaration) -> Result<(), ClientError> {
        self.done(declaration.request()).await
    }

    /// The room's owner declares what their game runs and the room's mods,
    /// together: for this connection, and to the room it owns.
    pub async fn declare_room(
        &self,
        declaration: tpf3mp_proto::RoomDeclaration,
    ) -> Result<(), ClientError> {
        self.done(Request::DeclareRoom(Box::new(declaration))).await
    }

    pub async fn start_game(&self) -> Result<(), ClientError> {
        self.done(Request::StartGame).await
    }

    pub async fn set_speed(&self, speed: Speed) -> Result<(), ClientError> {
        self.done(Request::SetSpeed(speed)).await
    }

    /// Removes a player from the room for good; only the owner may.
    pub async fn kick(&self, player: PlayerId) -> Result<(), ClientError> {
        self.done(Request::Kick(player)).await
    }

    /// Says something to everyone in the room.
    pub async fn chat(&self, text: ChatText) -> Result<(), ClientError> {
        self.done(Request::Chat(text)).await
    }

    async fn done(&self, request: Request) -> Result<(), ClientError> {
        self.requests.done(request).await
    }

    pub async fn send_intent(&self, client_seq: u64, payload: Payload) -> Result<(), ClientError> {
        self.send_intent_with(client_seq, payload, None).await
    }

    /// An intent with the password it needs, such as a company's: the room
    /// orders it with the password's seal, never the password (PROTOCOL.md,
    /// "Secrets").
    pub async fn send_intent_with(
        &self,
        client_seq: u64,
        payload: Payload,
        secret: Option<Secret>,
    ) -> Result<(), ClientError> {
        self.send(GameMessage::Intent {
            client_seq,
            payload,
            secret,
        })
        .await
    }

    /// Shows the room's other members what this player's build tool shows
    /// now, an action's payload, or that it shows nothing.
    pub async fn send_preview(&self, preview: Option<Payload>) -> Result<(), ClientError> {
        self.send(GameMessage::Preview(preview)).await
    }

    /// Tells the room where this player's game is with its world while it
    /// comes in; `None` once it plays.
    pub async fn report_loading(
        &self,
        stage: Option<tpf3mp_proto::LoadingStage>,
    ) -> Result<(), ClientError> {
        self.send(GameMessage::Loading(stage)).await
    }

    pub async fn report_progress(&self, world_epoch: u64, step: u64) -> Result<(), ClientError> {
        self.send(GameMessage::Progress { world_epoch, step }).await
    }

    pub async fn report_loaded(&self, world_epoch: u64, step: u64) -> Result<(), ClientError> {
        self.send(GameMessage::Loaded { world_epoch, step }).await
    }

    pub async fn report_checkpoint(
        &self,
        world_epoch: u64,
        step: u64,
        lanes: Vec<LaneDigest>,
    ) -> Result<(), ClientError> {
        self.send(GameMessage::Checkpoint {
            world_epoch,
            step,
            lanes,
        })
        .await
    }

    /// Reports the world saved at the save event `event`: its lanes, and
    /// the snapshot this client holds of it, if saving worked.
    pub async fn report_saved(
        &self,
        world_epoch: u64,
        event: u64,
        lanes: Vec<LaneDigest>,
        world: Option<SavedWorld>,
    ) -> Result<(), ClientError> {
        self.send(GameMessage::Saved {
            world_epoch,
            event,
            lanes,
            world,
        })
        .await
    }

    /// Opens bulk streams on this connection, for moving worlds.
    pub fn bulk(&self) -> BulkOpener {
        BulkOpener {
            connection: self.connection.clone(),
        }
    }

    async fn send(&self, message: GameMessage) -> Result<(), ClientError> {
        self.requests
            .outgoing
            .send(ClientMessage::Game(message))
            .await
            .map_err(|_| ClientError::Disconnected)
    }

    /// Ends the session and waits until the server has been told. The last
    /// lines of this player's log go first, when it sends them.
    pub async fn close(self) {
        if let Some(recorder) = &self.diagnostics {
            let _ = tokio::time::timeout(
                LAST_DIAGNOSTICS,
                diagnostics::send(recorder, &self.requests),
            )
            .await;
        }
        self.connection.close(close::NORMAL, b"client leaving");
        self.endpoint.wait_idle().await;
    }
}

impl Requests {
    /// Sends a request and waits for its response.
    pub async fn request(&self, request: Request) -> Result<Response, ClientError> {
        self.request_with_timeout(request, REQUEST_TIMEOUT).await
    }

    /// Sends a request with its own bounded wait. On timeout, forgets its
    /// pending response before returning so a slow server cannot leave a
    /// cancelled request behind on a reused connection.
    pub async fn request_with_timeout(
        &self,
        request: Request,
        timeout: Duration,
    ) -> Result<Response, ClientError> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (reply, answer) = oneshot::channel();
        self.pending_map().insert(id, reply);
        // The reader marks itself done before failing every pending request,
        // so a request registered after that is caught here.
        if self.reader_done.load(Ordering::SeqCst) {
            self.pending_map().remove(&id);
            return Err(ClientError::Disconnected);
        }
        if self
            .outgoing
            .send(ClientMessage::Request { id, request })
            .await
            .is_err()
        {
            self.pending_map().remove(&id);
            return Err(ClientError::Disconnected);
        }
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(Ok(response))) => Ok(response),
            Ok(Ok(Err(error))) => Err(ClientError::Refused(error)),
            Ok(Err(_)) => Err(ClientError::Disconnected),
            Err(_) => {
                self.pending_map().remove(&id);
                Err(ClientError::Timeout)
            }
        }
    }

    /// Sends a request whose only answer is that it was done.
    pub async fn done(&self, request: Request) -> Result<(), ClientError> {
        match self.request(request).await? {
            Response::Done => Ok(()),
            _ => Err(ClientError::UnexpectedResponse),
        }
    }

    fn pending_map(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<u32, oneshot::Sender<Result<Response, RequestError>>>>
    {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Background tasks hold the connection; without this, a dropped
        // client would linger until the idle timeout.
        self.connection.close(close::NORMAL, b"client dropped");
    }
}

async fn write_control(mut send: SendStream, mut outgoing: mpsc::Receiver<ClientMessage>) {
    while let Some(message) = outgoing.recv().await {
        if write_message(&mut send, &message, CONTROL_MAX_FRAME)
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn read_control(
    mut recv: RecvStream,
    connection: quinn::Connection,
    pending: Pending,
    done: Arc<AtomicBool>,
    events: mpsc::Sender<Queued>,
) {
    loop {
        let message = match read_message::<ServerMessage>(&mut recv, CONTROL_MAX_FRAME).await {
            Ok(message) => message,
            Err(error) => {
                if !error.is_disconnect() {
                    connection.close(close::PROTOCOL_VIOLATION, b"malformed control message");
                }
                break;
            }
        };
        let event = match message {
            ServerMessage::Response { id, result } => {
                let reply = pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
                continue;
            }
            ServerMessage::RoomUpdate(room) => ClientEvent::RoomUpdate(room),
            ServerMessage::IntentRejected { client_seq, reason } => {
                ClientEvent::IntentRejected { client_seq, reason }
            }
            ServerMessage::Diverged { step, lanes } => ClientEvent::Diverged { step, lanes },
            ServerMessage::Kicked => ClientEvent::Kicked,
            ServerMessage::Upload { event, snapshot } => ClientEvent::Upload { event, snapshot },
            ServerMessage::Chat { from, text } => ClientEvent::Chat { from, text },
            ServerMessage::ContentDiff(diff) => ClientEvent::ContentDiff(diff),
            ServerMessage::Notice(text) => ClientEvent::Notice(text),
            ServerMessage::RoomMods(room) => ClientEvent::RoomMods(room),
            ServerMessage::Preview { from, preview } => {
                // Advisory: never a reason to stop reading the control
                // stream, which carries the responses.
                match events.try_send((ClientEvent::Preview { from, preview }, None)) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => continue,
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
            ServerMessage::Welcome(_) | ServerMessage::Reject(_) => {
                connection.close(close::PROTOCOL_VIOLATION, b"unexpected handshake message");
                break;
            }
        };
        if events.send((event, None)).await.is_err() {
            break;
        }
    }
    done.store(true, Ordering::SeqCst);
    // Dropping the reply senders fails every waiting request.
    pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Reads turn streams one after another, so a stream that replaces an older
/// one is only read once the older one has ended.
async fn read_turns(
    connection: quinn::Connection,
    events: mpsc::Sender<Queued>,
    budget: Arc<Semaphore>,
) {
    while let Ok(mut recv) = connection.accept_uni().await {
        match read_turn_stream(&mut recv, &events, &budget).await {
            Ok(()) => {}
            Err(TurnStreamError::Violation) => {
                connection.close(close::PROTOCOL_VIOLATION, b"malformed turn stream");
                return;
            }
            Err(TurnStreamError::Receiver) => return,
        }
    }
}

enum TurnStreamError {
    Violation,
    Receiver,
}

async fn read_turn_stream(
    recv: &mut RecvStream,
    events: &mpsc::Sender<Queued>,
    budget: &Arc<Semaphore>,
) -> Result<(), TurnStreamError> {
    match read_preamble(recv).await {
        Ok(PROTOCOL_VERSION) => {}
        Ok(_) => return Err(TurnStreamError::Violation),
        Err(error) if error.is_disconnect() => return Ok(()),
        Err(_) => return Err(TurnStreamError::Violation),
    }
    let start = match read_message::<TurnMessage>(recv, TURN_MAX_FRAME).await {
        Ok(TurnMessage::Start(start)) => start,
        Ok(TurnMessage::Turn(_)) => return Err(TurnStreamError::Violation),
        Err(error) if error.is_disconnect() => return Ok(()),
        Err(_) => return Err(TurnStreamError::Violation),
    };
    events
        .send((ClientEvent::TurnStream(start), None))
        .await
        .map_err(|_| TurnStreamError::Receiver)?;
    loop {
        match read_message::<TurnMessage>(recv, TURN_MAX_FRAME).await {
            Ok(TurnMessage::Turn(turn)) => {
                // Waits while the application holds the whole budget, which
                // stops reading and lets QUIC flow control reach the server.
                let weight = u32::try_from(follower::turn_weight(&turn).min(EVENT_BYTES))
                    .unwrap_or(u32::MAX);
                let share = Arc::clone(budget)
                    .acquire_many_owned(weight)
                    .await
                    .map_err(|_| TurnStreamError::Receiver)?;
                events
                    .send((ClientEvent::Turn(turn), Some(share)))
                    .await
                    .map_err(|_| TurnStreamError::Receiver)?;
            }
            Ok(TurnMessage::Start(_)) => return Err(TurnStreamError::Violation),
            // The server finished the stream, or the connection ended.
            Err(error) if error.is_disconnect() => return Ok(()),
            Err(_) => return Err(TurnStreamError::Violation),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_request_timeout_removes_its_pending_response() {
        let (outgoing, _sent) = mpsc::channel(1);
        let requests = Requests {
            outgoing,
            pending: Arc::new(Mutex::new(HashMap::new())),
            reader_done: Arc::new(AtomicBool::new(false)),
            next_request: Arc::new(AtomicU32::new(0)),
        };
        assert_eq!(
            requests
                .request_with_timeout(Request::LeaveRoom, Duration::from_millis(1))
                .await,
            Err(ClientError::Timeout)
        );
        assert!(requests.pending_map().is_empty());
    }

    #[test]
    fn only_a_newer_server_calls_for_an_update() {
        let newer = ConnectError::VersionMismatch {
            client: 3,
            server: 4,
        };
        let older = ConnectError::VersionMismatch {
            client: 4,
            server: 3,
        };
        assert!(newer.client_is_older());
        assert!(!older.client_is_older());
        assert!(!ConnectError::Timeout.client_is_older());
        let routes = ConnectError::NoRoute {
            url: "wss://example.org/tpf3mp".into(),
            udp: Box::new(ConnectError::Timeout),
            tunnel: Box::new(newer),
        };
        assert!(routes.client_is_older());
        assert_eq!(routes.mismatch(), Some((3, 4)));
        assert!(
            routes.for_player().starts_with(
                "This launcher is too old for the server (it speaks protocol 3, the server 4)."
            ),
            "{}",
            routes.for_player()
        );
        assert!(older.for_player().contains("the server needs updating"));
        assert_eq!(ConnectError::Timeout.mismatch(), None);
        assert_eq!(
            ConnectError::Timeout.for_player(),
            ConnectError::Timeout.to_string()
        );
    }
}
