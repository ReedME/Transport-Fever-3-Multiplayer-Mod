//! Wire protocol shared by `tpf3mp-server` and `tpf3mp-agent`. The semantics
//! are specified in `docs/PROTOCOL.md`.
//!
//! Every stream opens with a fixed-layout [preamble](encode_preamble) that
//! carries the protocol version. Its layout never changes, so any two releases
//! can tell each other which version they speak, however much else differs.
//! After the preamble, every message is one frame: a little-endian `u32`
//! payload length followed by a postcard-encoded message. A frame above the
//! stream's cap is a protocol violation. Decoding enforces the bounds of every
//! field (see [`Text`] and [`Payload`]), so a decoded message is within limits.
//!
//! What a player's action carries inside an intent's payload is the
//! [`action`] schema; the network layer never looks inside it.

pub mod action;
mod bounded;
mod bytes;
mod content;
mod control;
mod diagnostics;
mod ids;
pub mod lua;
mod room_mods;
mod snapshot;
mod text;
mod turn;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub use bounded::{BoundedVec, TooMany};
pub use bytes::{FixedBytes, MAX_PAYLOAD, Payload, PayloadTooLarge};
pub use content::{
    ContentDiff, ContentManifest, GameBuilds, MAX_DIFF_LISTED, MAX_LISTED_MODS, MAX_MANIFEST_BYTES,
    ModChange, ModId, ModRef, ModVersion, OWN_MOD, Unlisted, own_mod_short,
};
pub use control::{
    AUTH_DOMAIN, AUTH_EXPORTER_LABEL, BANNERS, BannerId, ChatText, ClientMessage,
    ContentFingerprint, ContentStatus, CreateRoom, GameMessage, Hello, IntentRejection, JoinRoom,
    LaneDigest, ListedRoom, LoadingStage, MAX_CHECKPOINT_LANES, MAX_PREVIEW, MAX_ROOM_MEMBERS,
    MemberView, PORTRAITS, ROOMS_PER_PAGE, Reject, RejectReason, Request, RequestError, Response,
    Resume, RoomListing, RoomPage, RoomPhase, RoomSettings, RoomView, RulesName, RulesOffer,
    Secret, ServerMessage, Speed, StartSave, StartView, Welcome, is_banner, is_portrait,
};
pub use diagnostics::{
    DiagnosticBatch, DiagnosticEvent, DiagnosticLevel, DiagnosticTarget, DiagnosticText, LogSource,
    MAX_DIAGNOSTIC_EVENTS, Telemetry, TelemetryLine, TelemetryLines, redact,
};
pub use ids::{
    CODE_LEN, Code, CodeError, Invite, InviteError, LogSession, PlayerId, RoomId, SessionId,
    Signature,
};
pub use room_mods::{
    GAME_SETTINGS, MAX_ROOM_DECLARATION_BYTES, MAX_ROOM_MODS, MAX_ROOM_PARAMS, MODIO_SOURCE,
    ModInfo, ModParam, ModParams, RoomConfig, RoomDeclaration, RoomMod, RoomMods, RoomModsError,
    is_mod_id,
};
pub use snapshot::{
    BULK_REQUEST_MAX_FRAME, BULK_RESPONSE_MAX_FRAME, BulkOpen, BulkRequest, BulkResponse,
    ChunkHash, MAX_CHUNKS_PER_REQUEST, SavedWorld, SnapshotId, WorldOffer,
};
pub use text::{Text, TextError};
pub use turn::{Event, EventBody, Seal, Turn, TurnMessage, TurnStart};

/// Protocol version. Client and server must match exactly. Version 2 lets
/// hosts choose the rules a room is played by; version 3 declares a game's
/// mods by name, so players learn which differ; version 4 carries the
/// operator's notices; version 5 lets clients send their diagnostics;
/// version 6 makes invites and session IDs six-character codes; version 7
/// lets a room's owner hand the room the world it starts from
/// ([`Request::StartWorld`]); version 8 carries a password beside the intent
/// that needs it ([`Secret`]) and orders the intent with the password's seal
/// ([`Seal`]), for companies' passwords; version 9 lists public rooms
/// ([`Request::ListRooms`], [`CreateRoom::listing`]); version 10 carries
/// each member's banner ([`Request::SetBanner`], [`MemberView::banner`]);
/// version 11 a room's play style ([`CreateRoom::competitive`],
/// [`RoomView::competitive`]); version 12 each member's loading progress
/// ([`GameMessage::Loading`], [`MemberView::loading`]); version 13 lets a
/// player show a campaign character's portrait ([`PORTRAITS`]) as their
/// banner, with banner ids of up to 32 bytes ([`BannerId`]); version 14
/// lets the owner change or clear the world the room starts from while it
/// is in its lobby, names that save to every member
/// ([`Request::StartWorld`]'s `save`, [`Request::ClearStartWorld`],
/// [`RoomView::start`]) and marks everyone not ready when it changes;
/// version 16 carries each diagnostics line's source, the hook's and the
/// game's logs among them, and the launcher's run ([`Request::Telemetry`],
/// [`LogSession`]); version 17 relays what each player's build tool shows
/// to the other members ([`GameMessage::Preview`],
/// [`ServerMessage::Preview`]); version 18 lets the room's owner declare the
/// room's mods with what players are told of them and their settings
/// ([`Request::DeclareRoom`]), tells every member the room's mods
/// ([`ServerMessage::RoomMods`]) and how each member's game differs
/// ([`MemberView::differs`]); version 19 checks an invite on a server without
/// joining its room ([`Request::ResolveInvite`], [`Response::InviteMatch`]).
pub const PROTOCOL_VERSION: u32 = 20;

/// Application protocol name negotiated during the TLS handshake.
pub const ALPN: &[u8] = b"tpf3mp";

/// Largest frame accepted on the control stream. An intent with a
/// [`MAX_PAYLOAD`] payload fits.
pub const CONTROL_MAX_FRAME: usize = 64 * 1024;

/// Largest frame accepted on the turn stream. The server splits a tick's
/// events over several turns rather than exceed it.
pub const TURN_MAX_FRAME: usize = 1024 * 1024;

const MAGIC: [u8; 6] = *b"TPF3MP";

/// Length of the version preamble: six magic bytes and a little-endian `u32`.
pub const PREAMBLE_LEN: usize = MAGIC.len() + 4;

/// Length of a frame header: the little-endian `u32` payload length.
pub const FRAME_HEADER_LEN: usize = 4;

pub fn encode_preamble(version: u32) -> [u8; PREAMBLE_LEN] {
    let mut bytes = [0; PREAMBLE_LEN];
    bytes[..MAGIC.len()].copy_from_slice(&MAGIC);
    bytes[MAGIC.len()..].copy_from_slice(&version.to_le_bytes());
    bytes
}

pub fn decode_preamble(bytes: [u8; PREAMBLE_LEN]) -> Result<u32, PreambleError> {
    let (magic, version) = bytes.split_at(MAGIC.len());
    if magic != MAGIC {
        return Err(PreambleError::BadMagic);
    }
    let mut version_bytes = [0; 4];
    version_bytes.copy_from_slice(version);
    Ok(u32::from_le_bytes(version_bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PreambleError {
    #[error("the stream does not start with the TPF3-MP preamble")]
    BadMagic,
}

/// The operating system and CPU architecture a client runs on. Rooms mix
/// platforms, and the server uses this to choose a room's anchor replica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "windows" => Os::Windows,
            "linux" => Os::Linux,
            "macos" => Os::MacOs,
            _ => Os::Other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Aarch64,
            _ => Arch::Other,
        };
        Self { os, arch }
    }
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    TooLarge { len: usize, max: usize },
    #[error("empty frame")]
    Empty,
    #[error("malformed message: {0}")]
    Malformed(#[from] postcard::Error),
    #[error("{0} unexpected bytes after the message")]
    TrailingBytes(usize),
}

/// Encodes `message` as one frame, refusing to produce a frame the receiver
/// would reject.
pub fn encode_frame<T: Serialize>(message: &T, max: usize) -> Result<Vec<u8>, FrameError> {
    let payload = postcard::to_stdvec(message)?;
    let too_large = FrameError::TooLarge {
        len: payload.len(),
        max,
    };
    if payload.len() > max {
        return Err(too_large);
    }
    let len = u32::try_from(payload.len()).map_err(|_| too_large)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Reads a frame header and returns the payload length, which is at most `max`.
pub fn frame_len(header: [u8; FRAME_HEADER_LEN], max: usize) -> Result<usize, FrameError> {
    let len = usize::try_from(u32::from_le_bytes(header)).unwrap_or(usize::MAX);
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    Ok(len)
}

/// Decodes one frame payload. The payload must contain exactly one message.
pub fn decode_frame<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FrameError> {
    let (message, rest) = postcard::take_from_bytes(payload)?;
    if !rest.is_empty() {
        return Err(FrameError::TrailingBytes(rest.len()));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> ClientMessage {
        ClientMessage::Hello(Hello {
            client_version: Text::new("0.1").unwrap(),
            platform: Platform {
                os: Os::MacOs,
                arch: Arch::Aarch64,
            },
            name: Text::new("Al").unwrap(),
            identity: PlayerId(FixedBytes([1; 32])),
            proof: Signature(FixedBytes([2; 64])),
        })
    }

    fn payload(frame: &[u8]) -> &[u8] {
        &frame[FRAME_HEADER_LEN..]
    }

    #[test]
    fn preamble_round_trips() {
        let bytes = encode_preamble(PROTOCOL_VERSION);
        assert_eq!(decode_preamble(bytes), Ok(PROTOCOL_VERSION));
    }

    #[test]
    fn preamble_rejects_foreign_streams() {
        let mut bytes = encode_preamble(7);
        bytes[0] = b'X';
        assert_eq!(decode_preamble(bytes), Err(PreambleError::BadMagic));
    }

    /// The preamble layout is the one thing every future release must still
    /// understand. Changing these bytes breaks version-mismatch reporting.
    #[test]
    fn preamble_layout_is_frozen() {
        assert_eq!(
            encode_preamble(0x0102_0304),
            [b'T', b'P', b'F', b'3', b'M', b'P', 0x04, 0x03, 0x02, 0x01]
        );
    }

    /// Guards against accidental wire changes such as reordered variants or
    /// fields. Update deliberately, together with `PROTOCOL_VERSION`.
    #[test]
    fn hello_wire_format_is_stable() {
        let mut expected = vec![
            0, // ClientMessage::Hello
            3, b'0', b'.', b'1', // client_version
            2, 1, // Os::MacOs, Arch::Aarch64
            2, b'A', b'l', // name
        ];
        expected.extend([1; 32]); // identity, no length prefix
        expected.extend([2; 64]); // proof, no length prefix
        let frame = encode_frame(&hello(), CONTROL_MAX_FRAME).unwrap();
        assert_eq!(payload(&frame), expected.as_slice());
        assert_eq!(
            frame[..FRAME_HEADER_LEN],
            u32::try_from(expected.len()).unwrap().to_le_bytes()
        );
    }

    #[test]
    fn game_message_wire_format_is_stable() {
        let progress = ClientMessage::Game(GameMessage::Progress {
            world_epoch: 1,
            step: 300,
        });
        let frame = encode_frame(&progress, CONTROL_MAX_FRAME).unwrap();
        // Game variant, Progress variant, varint 300.
        assert_eq!(frame, [5, 0, 0, 0, 2, 1, 1, 0xac, 0x02]);
        // Version 12: a member's loading progress, the last variant.
        let loading = ClientMessage::Game(GameMessage::Loading(Some(LoadingStage::Fetching {
            percent: 42,
        })));
        let frame = encode_frame(&loading, CONTROL_MAX_FRAME).unwrap();
        assert_eq!(
            payload(&frame),
            [2, 4, 1, 0, 42],
            "Game, Loading, Some, Fetching, percent"
        );
        // Version 8: an intent carries an optional secret after its payload.
        let intent = ClientMessage::Game(GameMessage::Intent {
            client_seq: 1,
            payload: Payload::new(vec![7]).unwrap(),
            secret: Some(Secret {
                scope: 3,
                password: Text::new("pw").unwrap(),
            }),
        });
        let frame = encode_frame(&intent, CONTROL_MAX_FRAME).unwrap();
        assert_eq!(
            payload(&frame),
            [2, 0, 1, 1, 7, 1, 3, 2, b'p', b'w'],
            "Game, Intent, seq, payload, Some, scope, password"
        );
    }

    #[test]
    fn a_secret_never_shows_its_password() {
        let secret = Secret {
            scope: 3,
            password: Text::new("hunter2").unwrap(),
        };
        let shown = format!(
            "{:?}",
            GameMessage::Intent {
                client_seq: 1,
                payload: Payload::new(vec![7]).unwrap(),
                secret: Some(secret),
            }
        );
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("<hidden>"), "{shown}");
    }

    #[test]
    fn messages_round_trip() {
        let turn = TurnMessage::Turn(Turn {
            number: 9,
            sealed_through: 120,
            speed: Speed::NORMAL,
            events: vec![Event {
                seq: 41,
                step: 118,
                body: EventBody::Command {
                    player: PlayerId(FixedBytes([3; 32])),
                    client_seq: 7,
                    payload: Payload::new(vec![1, 2, 3]).unwrap(),
                    seal: Some(Seal {
                        scope: 2,
                        tag: FixedBytes([9; 32]),
                    }),
                },
            }],
        });
        let frame = encode_frame(&turn, TURN_MAX_FRAME).unwrap();
        assert_eq!(decode_frame::<TurnMessage>(payload(&frame)).unwrap(), turn);

        let frame = encode_frame(&hello(), CONTROL_MAX_FRAME).unwrap();
        assert_eq!(
            decode_frame::<ClientMessage>(payload(&frame)).unwrap(),
            hello()
        );

        let response = ServerMessage::Response {
            id: 3,
            result: Err(RequestError::BadInvite),
        };
        let frame = encode_frame(&response, CONTROL_MAX_FRAME).unwrap();
        assert_eq!(
            decode_frame::<ServerMessage>(payload(&frame)).unwrap(),
            response
        );
    }

    #[test]
    fn invite_resolution_round_trips_at_protocol_20() {
        assert_eq!(PROTOCOL_VERSION, 20);
        let request = ClientMessage::Request {
            id: 7,
            request: Request::ResolveInvite {
                invite: Invite(Code::random()),
                password: Some(Text::new("secret").unwrap()),
            },
        };
        let frame = encode_frame(&request, CONTROL_MAX_FRAME).unwrap();
        assert_eq!(
            decode_frame::<ClientMessage>(payload(&frame)).unwrap(),
            request
        );

        let response = ServerMessage::Response {
            id: 7,
            result: Ok(Response::InviteMatch),
        };
        let frame = encode_frame(&response, CONTROL_MAX_FRAME).unwrap();
        assert_eq!(
            decode_frame::<ServerMessage>(payload(&frame)).unwrap(),
            response
        );
    }

    #[test]
    fn frame_len_enforces_the_cap() {
        let max = 16;
        assert!(matches!(
            frame_len(17u32.to_le_bytes(), max),
            Err(FrameError::TooLarge { len: 17, max: 16 })
        ));
        assert!(matches!(
            frame_len(u32::MAX.to_le_bytes(), max),
            Err(FrameError::TooLarge { .. })
        ));
        assert!(matches!(
            frame_len(0u32.to_le_bytes(), max),
            Err(FrameError::Empty)
        ));
        assert_eq!(frame_len(16u32.to_le_bytes(), max).unwrap(), 16);
    }

    #[test]
    fn encoder_refuses_frames_above_the_cap() {
        assert!(matches!(
            encode_frame(&hello(), 4),
            Err(FrameError::TooLarge { max: 4, .. })
        ));
    }

    #[test]
    fn decoder_rejects_truncated_and_padded_payloads() {
        let frame = encode_frame(&hello(), CONTROL_MAX_FRAME).unwrap();
        let body = payload(&frame);
        assert!(matches!(
            decode_frame::<ClientMessage>(&body[..body.len() - 1]),
            Err(FrameError::Malformed(_))
        ));
        let mut padded = body.to_vec();
        padded.push(0);
        assert!(matches!(
            decode_frame::<ClientMessage>(&padded),
            Err(FrameError::TrailingBytes(1))
        ));
    }

    #[test]
    fn decoder_enforces_text_rules() {
        // A hello whose client_version carries a terminal escape sequence.
        let mut body = vec![0, 4];
        body.extend_from_slice(b"\x1b[2J");
        assert!(matches!(
            decode_frame::<ClientMessage>(&body),
            Err(FrameError::Malformed(_))
        ));
    }

    #[test]
    fn decoder_rejects_unknown_variants() {
        assert!(matches!(
            decode_frame::<ClientMessage>(&[200, 0]),
            Err(FrameError::Malformed(_))
        ));
    }

    #[test]
    fn session_id_displays_as_its_code() {
        let code: Code = "K7QM2X".parse().unwrap();
        assert_eq!(SessionId(code).to_string(), "K7QM2X");
    }

    #[test]
    fn default_room_settings_are_valid() {
        assert!(RoomSettings::DEFAULT.is_valid());
        let mut settings = RoomSettings::DEFAULT;
        settings.steps_per_second = 0;
        assert!(!settings.is_valid());
    }

    /// Version 13: every portrait id is a banner id that fits, apart from
    /// the banners, so either travels as one `SetBanner`.
    #[test]
    fn portraits_are_banner_ids_of_their_own() {
        for id in PORTRAITS {
            assert!(is_banner(id) && is_portrait(id), "{id}");
            assert!(!BANNERS.contains(id), "{id} is a banner too");
            assert!(BannerId::new(*id).is_ok(), "{id} is too long");
            assert!(
                id.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{id} is a file name"
            );
        }
        assert_eq!(PORTRAITS.len(), 25);
        assert!(!is_banner("none"), "the dialogue's empty speaker");
        assert!(!is_portrait(BANNERS[0]));
    }
}
