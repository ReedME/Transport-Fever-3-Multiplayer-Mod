//! Messages on the turn stream: a running room's ordered event log. See
//! `docs/PROTOCOL.md` for the invariants clients rely on. Variants are
//! identified by position: append, never reorder.

use serde::{Deserialize, Serialize};

use crate::{
    Platform, Text,
    bytes::{FixedBytes, Payload},
    control::{RulesName, Speed},
    ids::{PlayerId, RoomId},
    snapshot::WorldOffer,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnMessage {
    Start(TurnStart),
    Turn(Turn),
}

/// The first message on a turn stream: where the log continues and how the
/// room is paced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStart {
    pub room: RoomId,
    /// The rules the room is played by, which the game needs to know: with
    /// `native`, its own economy runs as it always does.
    pub rules: RulesName,
    /// Number of the next turn on this stream.
    pub next_turn: u64,
    /// Sequence number of the next event on this stream.
    pub next_event: u64,
    /// The frontier of the turn before this stream's first: the world this
    /// stream continues has run every step up to and including this one,
    /// and applied every event before `next_event`.
    pub sealed_through: u64,
    pub steps_per_second: u16,
    pub checkpoint_interval: u32,
    /// The history this stream's turns belong to, to name when resuming.
    pub history: u64,
    /// The world to load before following this stream: for a player who
    /// joins a running game, one who can no longer resume, or one being
    /// rebased after diverging. `None` continues the world the client has,
    /// or, at the start of a game, the world every player loads.
    pub world: Option<WorldOffer>,
    /// Identifies reports produced by this world; unchanged on exact resume,
    /// replaced whenever a new load is ordered (even at the same frontier).
    pub world_epoch: u64,
}

/// One sealed turn. See the invariants in `docs/PROTOCOL.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub number: u64,
    /// Clients may execute every step up to and including this one.
    pub sealed_through: u64,
    pub speed: Speed,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Room-global, gap-free sequence number.
    pub seq: u64,
    /// The event applies after step `step - 1` and before step `step`.
    pub step: u64,
    pub body: EventBody,
}

/// What the server made of a [`crate::Secret`]: an HMAC-SHA256 of the
/// password under the server's key, bound to the room and to the secret's
/// scope. Games compare seals and never see the password; without the
/// server's key a seal cannot be turned back into one or checked against a
/// guess, so it may be logged and kept in a save. Only the server makes
/// seals: a client cannot send one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seal {
    /// The scope the player named, as [`crate::Secret::scope`].
    pub scope: u64,
    pub tag: FixedBytes<32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventBody {
    /// A player's accepted intent.
    Command {
        player: PlayerId,
        client_seq: u64,
        payload: Payload,
        /// The seal of the password the player sent with it, if any: only
        /// the server makes one ([`crate::Secret`]).
        seal: Option<Seal>,
    },
    PlayerJoined {
        player: PlayerId,
        name: Text<32>,
        platform: Platform,
    },
    PlayerLeft {
        player: PlayerId,
        /// The room's owner removed the player, who cannot come back.
        kicked: bool,
    },
    /// Every client saves its world here: with every earlier event applied
    /// and before step `step` runs. It reports the save with the world's
    /// lane digests (`GameMessage::Saved`). A save is always the last event
    /// of its turn, and that turn seals no new steps, so a stream that
    /// starts from the save starts at a turn boundary.
    Save,
}
