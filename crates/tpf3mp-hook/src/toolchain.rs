//! The street and track tools continue from the segment the room built
//! (docs/HOOKS.md, "The street and track tools continue").
//!
//! In single player, one click of the street or track tool
//! (`UI::StreetBuilder::UpdateEngine`) queues the build, resets the tool's
//! proposal (`ResetProposal`, start point and all) and hands `Add` a
//! callback. Once the simulation applied the build, the callback finds the
//! new edge that ends where the click did, in the command's own result, and
//! starts the next segment there (`StartAt`): the player keeps dragging
//! from the segment just built. It does so only when the command's result
//! says the build succeeded. In the room's game the click's own build is
//! refused at its apply ([`crate::builds`]) and the room builds it later in
//! every game, so the callback hears a failed build, and the player is back
//! at picking a start point after every segment.
//!
//! The hook does what the callback would have done, later, from what the
//! room really built, and never hands the game's callback a result of its
//! own making (docs/HOOKS.md, "Three rules the cancel point taught"):
//!
//! 1. **At the click** (main thread, the add's detour, before `Add` takes
//!    the callback): the click's callback is read as `UpdateEngine` builds
//!    it ([`read_click`]): the tool, the clicked end, the start point the
//!    next segment takes and the two settings bytes the callback reads.
//!    One click is kept; a later click of the street or track tool
//!    replaces it (a script's build, say a mod's follow-up, does not).
//! 2. **At the hand-over** (the mod's GUI script hands the click's build
//!    to the room): the click is bound to the ticket the room's answer will
//!    carry ([`bind`]), or dropped when it was not handed over.
//! 3. **At the replay** (simulation thread): while that ticket's action is
//!    applied, the edges each of its builds made are copied out of the
//!    command's payload ([`collect`], [`read_edges`]), and kept once the
//!    action is reported applied with that ticket ([`applied`]).
//! 4. **In the GUI's state** (main thread, the mod's `guiUpdate`): the
//!    built edge that ends within [`TOLERANCE`] of the clicked end
//!    ([`find`]; the room carries positions in millimetres) is checked
//!    against the GUI's own state (that edge, with those nodes, its end
//!    there), which also shows the GUI's state has the room's build, and its
//!    end node's edges are counted for the direction ([`seen`]).
//! 5. **At the tool's frame** (main thread, `UI::StreetBuilder::Step`'s
//!    pre-hook, with the tool itself): if the tool still waits for a start
//!    point, the edge exists in the tool's engine and it was seen in the
//!    GUI's state a frame ago at most, the game's `StartAt` gets the start
//!    point the callback would have built ([`snap_bytes`],
//!    [`direction_bytes`]).
//!
//! The click is dropped, and the tool left as the game left it, when the
//! player starts a segment (even one let go again before the room
//! built), the tool is closed (it missed frames, as when another tool is
//! picked), the room refuses the action or applies it without such an edge, the game
//! leaves the room, or [`LIMIT`] passes. Nothing is built and no command is
//! queued, so lockstep is untouched.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::modules::{Memory, i32_at, u64_at};

pub use crate::build_data::native::toolchain::{
    ADD_CALL, BUSY, CAPTURE, CAPTURE_TO_ADD_CALL, DRAGGING, ENTITY_EXISTS, ENTRY_EDGE, ENTRY_NODE0,
    ENTRY_NODE1, ENTRY_POS0, ENTRY_POS1, ENTRY_SIZE, ENTRY_TAN0, ENTRY_TAN1, FUNCTION_IMPL,
    FUNCTOR_CONTINUE, FUNCTOR_DIRECTION, FUNCTOR_END, FUNCTOR_START, FUNCTOR_TOOL, HAS_START, KIND,
    PROVIDER, PROVIDER_ENGINE, RESULT_SCAN, RESULTS, REVISION_EXISTS_CALL, SCAN_TO_SNAP, SETTINGS,
    SETTINGS_CONTINUE, SNAP, SNAP_CALL, START_AT, START_AT_REVISION_CALL,
};

/// How far the room's end of the segment may lie from the clicked end:
/// the room carries positions in whole millimetres, and an f32 a few
/// kilometres out is good to about half of one.
pub const TOLERANCE: f32 = 0.005;
/// How long a click waits for the room's build.
pub const LIMIT: Duration = Duration::from_secs(15);
/// GUI frames the tool may miss before it counts as closed.
pub const GAP: u64 = 3;
/// GUI frames a check of the GUI's state stays good for.
pub const FRESH: u64 = 2;
/// Edges read of one action's builds, at most.
pub const MAX_EDGES: usize = 4096;
/// The start point's size, as `StartAt` copies it.
pub const SNAP_SIZE: usize = 0x34;
/// The direction's size, as `StartAt` reads it.
pub const DIRECTION_SIZE: usize = 0x14;
/// Explicit opt-in until ordinary-start input cancellation and track
/// direction match the native callback in a real-game check.
pub const ENV: &str = "TPF3MP_TOOL_CONTINUE";

fn wanted(value: Option<&str>) -> bool {
    value == Some("1")
}

/// A click of the street or track tool, as its callback holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    /// The tool (`UI::StreetBuilder`).
    pub tool: usize,
    /// Its settings, which the callback reads.
    pub settings: usize,
    /// The point `StartAt` takes as the next segment's start.
    pub start: [f32; 3],
    /// Where the click ended the segment.
    pub end: [f32; 3],
    /// The settings byte that lets the tool continue at all.
    pub continues: bool,
    /// The settings byte that lets it take the segment's direction.
    pub direction: bool,
    /// The tool's first parameter byte: which edges the direction counts.
    pub kind: u8,
}

/// An edge a build made, as the command's result holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub edge: i32,
    pub nodes: [i32; 2],
    pub ends: [[f32; 3]; 2],
    pub tangents: [[f32; 3]; 2],
}

/// The built edge's end where the click ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub edge: i32,
    /// Which end: 0 (node0) or 1 (node1).
    pub end: usize,
    pub nodes: [i32; 2],
    pub at: [f32; 3],
    pub tangent: [f32; 3],
}

impl Found {
    /// The end's node.
    pub fn node(&self) -> i32 {
        self.nodes[self.end]
    }
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn vec3_at(bytes: &[u8], offset: usize) -> [f32; 3] {
    [
        f32_at(bytes, offset),
        f32_at(bytes, offset + 4),
        f32_at(bytes, offset + 8),
    ]
}

/// The click whose callback is `callback`, if it reads as `UpdateEngine`
/// builds it: inline in its `std::function` (impl pointer to itself), the
/// tool's busy byte just set, its settings where the tool keeps them.
pub fn read_click(memory: &dyn Memory, callback: usize) -> Result<Click, String> {
    let function = memory
        .read(callback, FUNCTION_IMPL + 8)
        .ok_or("the click's callback does not read")?;
    let impl_pointer = u64_at(&function, FUNCTION_IMPL) as usize;
    if impl_pointer != callback {
        return Err(format!(
            "the click's callback keeps its functor elsewhere ({impl_pointer:#x})"
        ));
    }
    let tool = u64_at(&function, FUNCTOR_TOOL) as usize;
    if tool == 0 {
        return Err("the click's callback names no tool".into());
    }
    let busy = memory
        .read(tool.wrapping_add(BUSY), 1)
        .ok_or("the tool does not read")?;
    if busy[0] != 1 {
        return Err(format!("the tool's busy byte is {}", busy[0]));
    }
    let captures = memory
        .read(
            callback + FUNCTOR_START,
            FUNCTOR_DIRECTION + 1 - FUNCTOR_START,
        )
        .ok_or("the click's callback does not read")?;
    let at = |offset: usize| offset - FUNCTOR_START;
    let settings = memory
        .read(tool.wrapping_add(SETTINGS), 8)
        .map(|b| u64_at(&b, 0) as usize)
        .filter(|s| *s != 0)
        .ok_or("the tool has no settings")?;
    let kind = memory
        .read(tool.wrapping_add(KIND), 1)
        .ok_or("the tool's parameters do not read")?;
    let click = Click {
        tool,
        settings,
        start: vec3_at(&captures, at(FUNCTOR_START)),
        end: vec3_at(&captures, at(FUNCTOR_END)),
        continues: captures[at(FUNCTOR_CONTINUE)] != 0,
        direction: captures[at(FUNCTOR_DIRECTION)] != 0,
        kind: kind[0],
    };
    if !click.start.iter().chain(&click.end).all(|v| v.is_finite()) {
        return Err("the click's points are not numbers".into());
    }
    Ok(click)
}

/// The edges a build made, from its command's payload, appended to `out`:
/// those with a real id and both nodes (the apply gives them theirs).
pub fn read_edges(memory: &dyn Memory, payload: usize, out: &mut Vec<Edge>) -> Result<(), String> {
    let vector = memory
        .read(payload.wrapping_add(RESULTS), 16)
        .ok_or("the build's edges do not read")?;
    let (begin, end) = (u64_at(&vector, 0) as usize, u64_at(&vector, 8) as usize);
    if begin == end {
        return Ok(());
    }
    if begin == 0 || end < begin || (end - begin) % ENTRY_SIZE != 0 {
        return Err(format!(
            "the build's edges are not a vector ({begin:#x}..{end:#x})"
        ));
    }
    let count = (end - begin) / ENTRY_SIZE;
    if out.len() + count > MAX_EDGES {
        return Err(format!("the build made {count} edges, more than are read"));
    }
    let len = ENTRY_TAN1 + 12;
    for i in 0..count {
        let entry = memory
            .read(begin + i * ENTRY_SIZE, len)
            .ok_or("an edge of the build does not read")?;
        let edge = Edge {
            edge: i32_at(&entry, ENTRY_EDGE),
            nodes: [i32_at(&entry, ENTRY_NODE0), i32_at(&entry, ENTRY_NODE1)],
            ends: [vec3_at(&entry, ENTRY_POS0), vec3_at(&entry, ENTRY_POS1)],
            tangents: [vec3_at(&entry, ENTRY_TAN0), vec3_at(&entry, ENTRY_TAN1)],
        };
        let finite = edge
            .ends
            .iter()
            .chain(&edge.tangents)
            .flatten()
            .all(|v| v.is_finite());
        if edge.edge >= 0 && edge.nodes.iter().all(|n| *n >= 0) && finite {
            out.push(edge);
        }
    }
    Ok(())
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    let d: f32 = a.iter().zip(&b).map(|(x, y)| (x - y) * (x - y)).sum();
    d <= TOLERANCE * TOLERANCE
}

/// The first built edge that ends where the click did, node0's end before
/// node1's, as the game's callback looks.
pub fn find(edges: &[Edge], end: [f32; 3]) -> Option<Found> {
    edges.iter().find_map(|e| {
        (0..2).find(|&i| near(e.ends[i], end)).map(|i| Found {
            edge: e.edge,
            end: i,
            nodes: e.nodes,
            at: e.ends[i],
            tangent: e.tangents[i],
        })
    })
}

/// The direction the next segment starts in, as the game's callback gives
/// it: when the settings ask for it and the end node has exactly one edge
/// of the tool's network (a street tool counts streets, any other edges
/// that are no street), the edge's tangent there, pointing on, normalized.
/// `streets` and `others` are counted in the GUI's state.
pub fn direction(click: &Click, found: &Found, streets: u32, others: u32) -> Option<[f32; 3]> {
    if !click.direction {
        return None;
    }
    let count = if click.kind != 0 { streets } else { others };
    if count != 1 {
        return None;
    }
    let sign = if found.end == 1 { 1.0 } else { -1.0 };
    let t = found.tangent.map(|v| v * sign);
    let len = t.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !len.is_finite() || len < 1e-6 {
        return None;
    }
    Some(t.map(|v| v / len))
}

/// The start point (`street_util::SnapPointBaseEdge`) the game's callback
/// builds for `found`: the edge, 0 or 1 along it, 0, the word 1; the bytes
/// the game leaves unset are 0.
pub fn snap_bytes(found: &Found) -> [u8; SNAP_SIZE] {
    let mut snap = [0u8; SNAP_SIZE];
    snap[0..4].copy_from_slice(&found.edge.to_le_bytes());
    let t: f32 = if found.end == 1 { 1.0 } else { 0.0 };
    snap[4..8].copy_from_slice(&t.to_le_bytes());
    snap[0xc..0xe].copy_from_slice(&1u16.to_le_bytes());
    snap
}

/// The optional direction the game's callback hands `StartAt`: the vector,
/// a 0 byte, the tool's parameter byte, and whether there is one at +0x10.
pub fn direction_bytes(direction: Option<[f32; 3]>, kind: u8) -> [u8; DIRECTION_SIZE] {
    let mut bytes = [0u8; DIRECTION_SIZE];
    if let Some(d) = direction {
        for (i, v) in d.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        bytes[0xd] = kind;
        bytes[0x10] = 1;
    }
    bytes
}

/// What the tool shows at its frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolNow {
    pub busy: u8,
    pub has_start: u8,
    pub dragging: u8,
    pub settings: usize,
    /// The settings' continue byte.
    pub continues: u8,
}

/// The tool's state, read.
pub fn read_tool(memory: &dyn Memory, tool: usize) -> Option<ToolNow> {
    let byte = |offset: usize| memory.read(tool.wrapping_add(offset), 1).map(|b| b[0]);
    let settings = memory
        .read(tool.wrapping_add(SETTINGS), 8)
        .map(|b| u64_at(&b, 0) as usize)?;
    let continues = memory.read(settings.wrapping_add(SETTINGS_CONTINUE), 1)?[0];
    Some(ToolNow {
        busy: byte(BUSY)?,
        has_start: byte(HAS_START)?,
        dragging: byte(DRAGGING)?,
        settings,
        continues,
    })
}

/// The tool's engine, as `StartAt` reads it.
pub fn read_engine(memory: &dyn Memory, tool: usize) -> Option<usize> {
    let provider = memory
        .read(tool.wrapping_add(PROVIDER), 8)
        .map(|b| u64_at(&b, 0) as usize)
        .filter(|p| *p != 0)?;
    memory
        .read(provider.wrapping_add(PROVIDER_ENGINE), 8)
        .map(|b| u64_at(&b, 0) as usize)
        .filter(|e| *e != 0)
}

/// Where a click stands.
#[derive(Clone, Debug, PartialEq)]
enum Stage {
    /// Queued; not yet handed to the room.
    Queued,
    /// Handed to the room with this ticket.
    Ordered(u64),
    /// Applied by the room: what its builds made, and the edge's last check
    /// in the GUI's state (the direction, the frame).
    Built {
        edges: Vec<Edge>,
        seen: Option<(Option<[f32; 3]>, u64)>,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    click: Click,
    /// The click's number (the count of clicks before it).
    number: u64,
    stage: Stage,
    since: Instant,
    /// The GUI frame of the tool's last frame.
    stepped: Option<u64>,
}

/// What the tool's frame is to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// Nothing (another tool, or nothing kept).
    Nothing,
    /// Wait: the click's build is not there yet.
    Wait,
    /// Drop the click, for this reason.
    Drop(&'static str),
    /// Start the next segment, if the edge exists in the tool's engine.
    Start {
        number: u64,
        tool: usize,
        found: Found,
        start: [f32; 3],
        direction: Option<[f32; 3]>,
        kind: u8,
    },
}

/// The click kept, and where it stands. The hook keeps one, behind a lock;
/// the tests drive their own.
#[derive(Default)]
pub struct Chain {
    pending: Option<Pending>,
}

impl Chain {
    /// A click of the street or track tool: `click` as read, or `None` when
    /// it did not read (which drops the kept one too). The click comes
    /// inside the tool's frame `frame`, which counts as its last: a tool
    /// closed before its next frame misses frames too.
    pub fn click(&mut self, number: u64, click: Option<Click>, now: Instant, frame: u64) {
        self.pending = click.map(|click| Pending {
            click,
            number,
            stage: Stage::Queued,
            since: now,
            stepped: Some(frame),
        });
    }

    /// The game's proposal reset follows its own click before `Add` calls
    /// [`before_add`]. Any later reset of this builder withdraws its kept
    /// click, including an abort before the room replies.
    fn reset(&mut self, builder: usize) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.click.tool == builder)
        {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// The GUI handed click `number` to the room with `ticket`, or did not
    /// hand it over (`None`).
    pub fn bind(&mut self, number: u64, ticket: Option<u64>) -> Option<&'static str> {
        let p = self.pending.as_mut()?;
        if p.number != number || p.stage != Stage::Queued {
            return None;
        }
        match ticket {
            Some(ticket) => {
                p.stage = Stage::Ordered(ticket);
                None
            }
            None => {
                self.pending = None;
                Some("its build was not handed to the room")
            }
        }
    }

    /// The ticket whose builds are to be read at the replay.
    pub fn ordered(&self) -> Option<u64> {
        match self.pending.as_ref()?.stage {
            Stage::Ordered(ticket) => Some(ticket),
            _ => None,
        }
    }

    /// An action was applied: `ticket` is its, when it is the player's;
    /// `edges` what its builds made.
    pub fn applied(
        &mut self,
        ticket: Option<u64>,
        ok: bool,
        edges: Vec<Edge>,
    ) -> Option<&'static str> {
        let ordered = self.ordered()?;
        if ticket != Some(ordered) {
            return None;
        }
        if !ok {
            self.pending = None;
            return Some("the room did not apply its build");
        }
        if edges.is_empty() {
            self.pending = None;
            return Some("its build made no edge the hook could read");
        }
        if let Some(p) = self.pending.as_mut() {
            p.stage = Stage::Built { edges, seen: None };
        }
        None
    }

    /// The room refused the action with `ticket`.
    pub fn refused(&mut self, ticket: u64) -> Option<&'static str> {
        if self.ordered() == Some(ticket) {
            self.pending = None;
            return Some("the room refused its build");
        }
        None
    }

    fn expired(&mut self, now: Instant, in_room: bool) -> Option<&'static str> {
        let p = self.pending.as_ref()?;
        let why = if !in_room {
            "the game left the room"
        } else if now.duration_since(p.since) > LIMIT {
            "the room did not build it in time"
        } else {
            return None;
        };
        self.pending = None;
        Some(why)
    }

    /// The GUI's frame: the built edge to check in the GUI's state, if any.
    pub fn poll(&mut self, now: Instant, in_room: bool) -> (Option<Found>, Option<&'static str>) {
        if let Some(why) = self.expired(now, in_room) {
            return (None, Some(why));
        }
        let Some(p) = self.pending.as_ref() else {
            return (None, None);
        };
        let Stage::Built { edges, .. } = &p.stage else {
            return (None, None);
        };
        match find(edges, p.click.end) {
            Some(found) => (Some(found), None),
            None => {
                self.pending = None;
                (
                    None,
                    Some("none of the room's edges ends where the click did"),
                )
            }
        }
    }

    /// The GUI's state has `edge` as the room built it, with `streets` and
    /// `others` edges at its end node, at GUI frame `frame`.
    pub fn seen(&mut self, edge: i32, streets: u32, others: u32, frame: u64) {
        let Some(p) = self.pending.as_mut() else {
            return;
        };
        let click = p.click;
        let Stage::Built { edges, seen } = &mut p.stage else {
            return;
        };
        if let Some(found) = find(edges, click.end).filter(|f| f.edge == edge) {
            *seen = Some((direction(&click, &found, streets, others), frame));
        }
    }

    /// The tool `this` runs its frame, at GUI frame `frame`; `read` reads
    /// its state.
    pub fn step(
        &mut self,
        this: usize,
        frame: u64,
        now: Instant,
        in_room: bool,
        read: &dyn Fn(usize) -> Option<ToolNow>,
    ) -> Step {
        if self.pending.as_ref().is_none_or(|p| p.click.tool != this) {
            return Step::Nothing;
        }
        if let Some(why) = self.expired(now, in_room) {
            return Step::Drop(why);
        }
        let Some(p) = self.pending.as_mut() else {
            return Step::Nothing;
        };
        let missed = p
            .stepped
            .is_some_and(|last| frame.saturating_sub(last) >= GAP);
        p.stepped = Some(frame);
        if missed {
            self.pending = None;
            return Step::Drop("the tool was closed");
        }
        // The tool's own state, from the click's callback on, whatever the
        // room has done: a start point of the player's ends the click for
        // good, even if the player lets it go again before the room built.
        let Some(tool) = read(this) else {
            self.pending = None;
            return Step::Drop("the tool does not read");
        };
        if tool.busy != 0 {
            return Step::Wait;
        }
        let why = if tool.has_start != 0 || tool.dragging != 0 {
            "the player started a segment"
        } else if tool.settings != p.click.settings {
            "the tool's settings changed"
        } else {
            let Stage::Built {
                edges,
                seen: Some((direction, at)),
            } = &p.stage
            else {
                return Step::Wait;
            };
            if frame.saturating_sub(*at) > FRESH {
                return Step::Wait;
            }
            let Some(found) = find(edges, p.click.end) else {
                return Step::Wait;
            };
            if tool.continues == 0 || !p.click.continues {
                "the tool's settings do not continue"
            } else {
                return Step::Start {
                    number: p.number,
                    tool: this,
                    found,
                    start: p.click.start,
                    direction: *direction,
                    kind: p.click.kind,
                };
            }
        };
        self.pending = None;
        Step::Drop(why)
    }

    /// Takes click `number` for its start, if it is still the one kept.
    pub fn take(&mut self, number: u64) -> bool {
        if self.pending.as_ref().is_some_and(|p| p.number == number) {
            self.pending = None;
            return true;
        }
        false
    }
}

/// Where `UpdateEngine`'s `Add` returns to; 0 when off.
static ADD_RETURN: AtomicUsize = AtomicUsize::new(0);
/// The game's `StartAt` and `EntityExists`.
static START_AT_FN: AtomicUsize = AtomicUsize::new(0);
static EXISTS_FN: AtomicUsize = AtomicUsize::new(0);
/// The street builder's frame runs the hook's pre-hook.
static STEP_HOOKED: AtomicBool = AtomicBool::new(false);
/// A missed proposal-reset hook must leave continuation off.
static RESET_HOOKED: AtomicBool = AtomicBool::new(false);
/// The GUI's frames, counted by the mod's `guiUpdate`.
static FRAME: AtomicU64 = AtomicU64::new(0);
/// A click waits for its ticket's builds: the replay reads them.
static ARMED: AtomicBool = AtomicBool::new(false);
static CHAIN: Mutex<Chain> = Mutex::new(Chain { pending: None });
/// What the builds of the action being applied made so far (simulation
/// thread).
static SCRATCH: Mutex<Scratch> = Mutex::new(Scratch {
    edges: Vec::new(),
    unreadable: false,
});

/// The edges an action's builds made, and whether one build's did not
/// read (then none is taken).
struct Scratch {
    edges: Vec<Edge>,
    unreadable: bool,
}

fn chain() -> std::sync::MutexGuard<'static, Chain> {
    CHAIN.lock().unwrap_or_else(PoisonError::into_inner)
}

fn rearm(chain: &Chain) {
    ARMED.store(chain.ordered().is_some(), Ordering::Release);
}

/// Each reason is logged once, so a playtest's log says what happened
/// without a line a click.
static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn log_once(line: String) {
    let new = match LOGGED.lock() {
        Ok(mut logged) if !logged.contains(&line) && logged.len() < 24 => {
            logged.push(line.clone());
            true
        }
        _ => false,
    };
    if new {
        crate::log::line(&line);
    }
}

fn dropped(why: Option<&'static str>) {
    if let Some(why) = why {
        log_once(format!(
            "street tool: does not continue from the clicked segment: {why}"
        ));
    }
}

/// Whether the tools continue: the targets resolved and the street
/// builder's frame is hooked.
pub fn enabled() -> bool {
    ready(
        ADD_RETURN.load(Ordering::Acquire) != 0,
        STEP_HOOKED.load(Ordering::Acquire),
        RESET_HOOKED.load(Ordering::Acquire),
    )
}

fn ready(add: bool, step: bool, reset: bool) -> bool {
    add && step && reset
}

/// The street builder's frame runs [`on_step`] (crate::toolplayer).
pub fn set_step_hooked(hooked: bool) {
    STEP_HOOKED.store(hooked, Ordering::Release);
}

/// Called only after the native proposal-reset detour is installed.
pub fn set_reset_hooked(hooked: bool) {
    RESET_HOOKED.store(hooked, Ordering::Release);
}

/// Called by the add's detour for each of the player's builds in the room's
/// game, click `number`, before `Add` takes `callback`. Only a click of the
/// street or track tool replaces the kept one: a script's build (a mod's
/// follow-up, also the player's) leaves it, and another tool's click comes
/// from another tool, so the street builder misses frames and drops it.
pub fn before_add(return_address: usize, callback: usize, number: u64) {
    if !enabled() || return_address != ADD_RETURN.load(Ordering::Acquire) {
        return;
    }
    let click = match read_click(&crate::modules::Process, callback) {
        Ok(click) => Some(click),
        Err(why) => {
            log_once(format!(
                "street tool: does not continue, the click does not read as the game's: {why}"
            ));
            None
        }
    };
    let mut chain = chain();
    chain.click(number, click, Instant::now(), FRAME.load(Ordering::Acquire));
    rearm(&chain);
}

/// Called by the already-installed proposal-reset detour. A reset after the
/// click's Add callback means the player (or game) abandoned that proposal;
/// continuing it after the room replies would revive an old start point.
pub fn proposal_reset(builder: usize) {
    let reset = {
        let mut chain = chain();
        let reset = chain.reset(builder);
        rearm(&chain);
        reset
    };
    if reset {
        dropped(Some("the tool's proposal was reset"));
    }
}

/// The mod's GUI handed click `number` to the room with `ticket`, or did
/// not (`None`).
pub fn bind(number: u64, ticket: Option<u64>) {
    let mut chain = chain();
    dropped(chain.bind(number, ticket));
    rearm(&chain);
}

/// Called by the build apply's detour after the room's build applied,
/// with its payload (simulation thread).
pub fn collect(payload: usize) {
    if !ARMED.load(Ordering::Acquire) {
        return;
    }
    let mut scratch = SCRATCH.lock().unwrap_or_else(PoisonError::into_inner);
    if let Err(why) = read_edges(&crate::modules::Process, payload, &mut scratch.edges) {
        log_once(format!(
            "street tool: the room's build does not read: {why}"
        ));
        scratch.unreadable = true;
    }
}

/// An action of the room's was reported applied (simulation thread):
/// `ticket` is its, when it is the player's own.
pub fn applied(ticket: Option<u64>, ok: bool) {
    let edges = {
        let mut scratch = SCRATCH.lock().unwrap_or_else(PoisonError::into_inner);
        let edges = std::mem::take(&mut scratch.edges);
        let unreadable = std::mem::replace(&mut scratch.unreadable, false);
        if unreadable { Vec::new() } else { edges }
    };
    let mut chain = chain();
    dropped(chain.applied(ticket, ok, edges));
    rearm(&chain);
}

/// The game script begins or ends applying the room's actions.
pub fn replaying(_on: bool) {
    let mut scratch = SCRATCH.lock().unwrap_or_else(PoisonError::into_inner);
    scratch.edges.clear();
    scratch.unreadable = false;
}

/// The room refused the player's action with `ticket`.
pub fn refused(ticket: u64) {
    let mut chain = chain();
    dropped(chain.refused(ticket));
    rearm(&chain);
}

/// The mod's `guiUpdate`, each GUI frame: the built edge to check in the
/// GUI's state, if any.
pub fn poll() -> Option<Found> {
    FRAME.fetch_add(1, Ordering::AcqRel);
    let mut chain = chain();
    let (found, why) = chain.poll(Instant::now(), crate::lua::in_room());
    dropped(why);
    rearm(&chain);
    found
}

/// The GUI's state has `edge` as the room built it.
pub fn seen(edge: i32, streets: u32, others: u32) {
    chain().seen(edge, streets, others, FRAME.load(Ordering::Acquire));
}

/// `bool ecs::Engine::EntityExists(Engine*, int)`.
type ExistsFn = unsafe extern "C-unwind" fn(usize, i32) -> u8;
/// `StartAt(tool, snap, 1, start, direction)`.
type StartAtFn = unsafe extern "C-unwind" fn(usize, *const u8, u8, *const f32, *const u8) -> usize;

/// The street builder `this` begins its frame (main thread): starts the
/// next segment from the room's edge when everything says so.
pub fn on_step(this: usize) {
    if !enabled() || this == 0 {
        return;
    }
    let frame = FRAME.load(Ordering::Acquire);
    let step = chain().step(
        this,
        frame,
        Instant::now(),
        crate::lua::in_room(),
        &|tool| read_tool(&crate::modules::Process, tool),
    );
    let (number, tool, found, start, direction, kind) = match step {
        Step::Nothing | Step::Wait => return,
        Step::Drop(why) => {
            dropped(Some(why));
            rearm(&chain());
            return;
        }
        Step::Start {
            number,
            tool,
            found,
            start,
            direction,
            kind,
        } => (number, tool, found, start, direction, kind),
    };
    // The edge must exist in the tool's own engine: StartAt asserts it.
    let Some(engine) = read_engine(&crate::modules::Process, tool) else {
        let mut chain = chain();
        chain.take(number);
        rearm(&chain);
        dropped(Some("the tool's engine does not read"));
        return;
    };
    let exists = EXISTS_FN.load(Ordering::Acquire);
    let start_at = START_AT_FN.load(Ordering::Acquire);
    if exists == 0 || start_at == 0 {
        return;
    }
    // SAFETY: ecs::Engine::EntityExists as the profile resolved it and
    // install checked GetRevisionIfSet calls it; pure reads of the tool's
    // own engine for an id >= 0, on the main thread that owns it.
    let exists: ExistsFn = unsafe { std::mem::transmute::<usize, ExistsFn>(exists) };
    if unsafe { exists(engine, found.edge) } == 0 {
        return;
    }
    {
        let mut chain = chain();
        let taken = chain.take(number);
        rearm(&chain);
        if !taken {
            return;
        }
    }
    let snap = snap_bytes(&found);
    let direction_bytes = direction_bytes(direction, kind);
    // SAFETY: StartAt as the profile resolved it (its whole body is the
    // signature, and the callback's own call of it checked), called as the
    // game's callback calls it: the tool, which runs its frame on this
    // thread now, a start point and a direction laid out as the callback
    // lays them out, for an edge that exists in the tool's engine, and the
    // start point the click's callback captured. It copies what it is
    // handed before it returns.
    let start_at: StartAtFn = unsafe { std::mem::transmute::<usize, StartAtFn>(start_at) };
    unsafe {
        start_at(
            tool,
            snap.as_ptr(),
            1,
            start.as_ptr(),
            direction_bytes.as_ptr(),
        )
    };
    log_once(format!(
        "street tool: continues from the room's segment, {} direction",
        if direction.is_some() {
            "with its"
        } else {
            "without a"
        }
    ));
}

/// The call's target at `call`, if `code` reads a `call rel32` there.
fn call_target(code: &dyn Fn(usize) -> Option<[u8; 5]>, call: usize) -> Option<usize> {
    let bytes = code(call)?;
    if bytes[0] != 0xE8 {
        return None;
    }
    let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    Some(call.wrapping_add(5).wrapping_add_signed(rel as isize))
}

/// The resolved targets, checked against each other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Targets {
    pub add_call: usize,
    pub start_at: usize,
    pub exists: usize,
}

/// Resolves and checks the targets: the click's call goes to `add`, the
/// capture lies before it in the same function, the callback's two pieces
/// lie apart as they do, its call goes to `StartAt`, and `StartAt`'s
/// revision read calls `EntityExists`.
pub fn check(
    at: &dyn Fn(&str) -> Result<usize, String>,
    code: &dyn Fn(usize) -> Option<[u8; 5]>,
    add: usize,
) -> Result<Targets, String> {
    let add_call = at(ADD_CALL)?;
    let capture = at(CAPTURE)?;
    let scan = at(RESULT_SCAN)?;
    let snap = at(SNAP)?;
    let start_at = at(START_AT)?;
    let exists = at(ENTITY_EXISTS)?;
    if call_target(code, add_call) != Some(add) {
        return Err(format!("{ADD_CALL} does not call CommandList::Add"));
    }
    if capture.wrapping_add(CAPTURE_TO_ADD_CALL) != add_call {
        return Err(format!("{CAPTURE} is not where {ADD_CALL} expects it"));
    }
    if scan.wrapping_add(SCAN_TO_SNAP) != snap {
        return Err(format!("{RESULT_SCAN} and {SNAP} are not one callback"));
    }
    if call_target(code, snap + SNAP_CALL) != Some(start_at) {
        return Err(format!("the callback does not call {START_AT}"));
    }
    let revision = call_target(code, start_at + START_AT_REVISION_CALL)
        .ok_or_else(|| format!("{START_AT} does not read the edge's revision"))?;
    if call_target(code, revision + REVISION_EXISTS_CALL) != Some(exists) {
        return Err(format!(
            "{START_AT}'s revision read does not call {ENTITY_EXISTS}"
        ));
    }
    Ok(Targets {
        add_call,
        start_at,
        exists,
    })
}

/// Installs the tools' continuing where the profile has every target;
/// returns the hook log's line.
pub fn install(at: &dyn Fn(&str) -> Result<usize, String>, add: usize) -> String {
    if !wanted(std::env::var(ENV).ok().as_deref()) {
        return format!(
            "the street and track tools' continuation is off ({ENV}=1 opts in for testing)"
        );
    }
    let code = |address: usize| -> Option<[u8; 5]> {
        if !crate::image::readable(address, 5) {
            return None;
        }
        // SAFETY: five readable bytes of the game's mapped code, only read.
        Some(unsafe { std::ptr::read_unaligned(address as *const [u8; 5]) })
    };
    match check(at, &code, add) {
        Ok(targets) => {
            START_AT_FN.store(targets.start_at, Ordering::Release);
            EXISTS_FN.store(targets.exists, Ordering::Release);
            ADD_RETURN.store(targets.add_call + 5, Ordering::Release);
            format!(
                "the street and track tools continue from the room's segment where Add returns to \
                 {:#x}, once the street builder's frame is hooked (the tools' player)",
                targets.add_call + 5
            )
        }
        Err(why) => {
            format!("the street and track tools do not continue from the room's segment: {why}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[test]
    fn continuation_requires_explicit_test_opt_in() {
        assert!(!wanted(None));
        assert!(!wanted(Some("0")));
        assert!(!wanted(Some("true")));
        assert!(wanted(Some("1")));
    }

    #[test]
    fn continuation_requires_both_native_tool_hooks() {
        assert!(!ready(true, true, false));
        assert!(!ready(true, false, true));
        assert!(!ready(false, true, true));
        assert!(ready(true, true, true));
    }

    #[derive(Default)]
    struct Fake {
        bytes: RefCell<BTreeMap<usize, u8>>,
    }

    impl Fake {
        fn put(&self, address: usize, bytes: &[u8]) {
            let mut map = self.bytes.borrow_mut();
            for (i, b) in bytes.iter().enumerate() {
                map.insert(address + i, *b);
            }
        }
        fn f32s(&self, address: usize, values: &[f32]) {
            for (i, v) in values.iter().enumerate() {
                self.put(address + 4 * i, &v.to_le_bytes());
            }
        }
    }

    impl Memory for Fake {
        fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            let map = self.bytes.borrow();
            (0..len).map(|i| map.get(&(address + i)).copied()).collect()
        }
    }

    const TOOL: usize = 0x0275_2000_0000;
    const SETTINGS_AT: usize = 0x0275_3000_0000;
    const CALLBACK: usize = 0x00e5_ff68;
    const END: [f32; 3] = [1200.25, -340.5, 81.75];
    const START: [f32; 3] = [1100.0, -300.0, 80.0];

    /// The tool and the click's callback as `UpdateEngine` leaves them just
    /// before `Add`: busy set, the functor inline, its captures.
    fn clicked(memory: &Fake) {
        memory.put(TOOL, &[0; 0x9f0]);
        memory.put(TOOL + BUSY, &[1]);
        memory.put(TOOL + SETTINGS, &(SETTINGS_AT as u64).to_le_bytes());
        memory.put(TOOL + KIND, &[1]);
        memory.put(SETTINGS_AT, &[0; 0x900]);
        memory.put(SETTINGS_AT + SETTINGS_CONTINUE, &[1]);
        memory.put(CALLBACK, &[0; FUNCTION_IMPL + 8]);
        memory.put(CALLBACK, &0x1_436b_f6f8u64.to_le_bytes());
        memory.put(CALLBACK + FUNCTOR_TOOL, &(TOOL as u64).to_le_bytes());
        memory.f32s(CALLBACK + FUNCTOR_START, &START);
        memory.f32s(CALLBACK + FUNCTOR_END, &END);
        memory.put(CALLBACK + FUNCTOR_CONTINUE, &[1, 1]);
        memory.put(CALLBACK + FUNCTION_IMPL, &(CALLBACK as u64).to_le_bytes());
    }

    #[test]
    fn a_click_reads_as_update_engine_builds_it() {
        let memory = Fake::default();
        clicked(&memory);
        let click = read_click(&memory, CALLBACK).unwrap();
        assert_eq!(
            click,
            Click {
                tool: TOOL,
                settings: SETTINGS_AT,
                start: START,
                end: END,
                continues: true,
                direction: true,
                kind: 1,
            }
        );
    }

    #[test]
    fn a_click_that_does_not_read_as_the_games_is_not_kept() {
        let memory = Fake::default();
        assert!(read_click(&memory, CALLBACK).is_err(), "nothing there");
        clicked(&memory);
        memory.put(CALLBACK + FUNCTION_IMPL, &0x1234u64.to_le_bytes());
        assert!(
            read_click(&memory, CALLBACK)
                .unwrap_err()
                .contains("elsewhere")
        );
        clicked(&memory);
        memory.put(TOOL + BUSY, &[0]);
        assert!(
            read_click(&memory, CALLBACK)
                .unwrap_err()
                .contains("busy byte is 0")
        );
        clicked(&memory);
        memory.put(TOOL + SETTINGS, &0u64.to_le_bytes());
        assert!(
            read_click(&memory, CALLBACK)
                .unwrap_err()
                .contains("no settings")
        );
        clicked(&memory);
        memory.f32s(CALLBACK + FUNCTOR_END, &[f32::NAN, 0.0, 0.0]);
        assert!(read_click(&memory, CALLBACK).is_err());
    }

    const PAYLOAD: usize = 0x0300_0000_0000;
    const ENTRIES: usize = 0x0300_1000_0000;

    fn entry(memory: &Fake, i: usize, edge: i32, nodes: [i32; 2], ends: [[f32; 3]; 2]) {
        let at = ENTRIES + i * ENTRY_SIZE;
        memory.put(at, &[0; ENTRY_SIZE]);
        memory.put(at + ENTRY_EDGE, &edge.to_le_bytes());
        memory.put(at + ENTRY_NODE0, &nodes[0].to_le_bytes());
        memory.put(at + ENTRY_NODE1, &nodes[1].to_le_bytes());
        memory.f32s(at + ENTRY_POS0, &ends[0]);
        memory.f32s(at + ENTRY_POS1, &ends[1]);
        memory.f32s(at + ENTRY_TAN0, &[30.0, 40.0, 0.0]);
        memory.f32s(at + ENTRY_TAN1, &[0.0, 50.0, 0.0]);
    }

    fn vector(memory: &Fake, count: usize) {
        memory.put(PAYLOAD + RESULTS, &(ENTRIES as u64).to_le_bytes());
        memory.put(
            PAYLOAD + RESULTS + 8,
            &((ENTRIES + count * ENTRY_SIZE) as u64).to_le_bytes(),
        );
    }

    #[test]
    fn the_builds_edges_read_with_their_ids_and_ends() {
        let memory = Fake::default();
        entry(&memory, 0, -3, [5, 6], [START, END]);
        entry(&memory, 1, 41, [7, 8], [START, END]);
        entry(&memory, 2, 42, [8, -1], [END, START]);
        vector(&memory, 3);
        let mut edges = Vec::new();
        read_edges(&memory, PAYLOAD, &mut edges).unwrap();
        assert_eq!(edges.len(), 1, "placeholder ids and nodes are left out");
        assert_eq!(edges[0].edge, 41);
        assert_eq!(edges[0].nodes, [7, 8]);
        assert_eq!(edges[0].ends, [START, END]);
        assert_eq!(edges[0].tangents, [[30.0, 40.0, 0.0], [0.0, 50.0, 0.0]]);

        // Not a vector of whole entries: nothing is taken.
        memory.put(
            PAYLOAD + RESULTS + 8,
            &((ENTRIES + 10) as u64).to_le_bytes(),
        );
        assert!(read_edges(&memory, PAYLOAD, &mut Vec::new()).is_err());
        // Empty.
        memory.put(PAYLOAD + RESULTS + 8, &(ENTRIES as u64).to_le_bytes());
        let mut none = Vec::new();
        read_edges(&memory, PAYLOAD, &mut none).unwrap();
        assert!(none.is_empty());
        // Beyond what is read.
        vector(&memory, MAX_EDGES + 1);
        assert!(read_edges(&memory, PAYLOAD, &mut Vec::new()).is_err());
    }

    fn edge(id: i32, nodes: [i32; 2], ends: [[f32; 3]; 2]) -> Edge {
        Edge {
            edge: id,
            nodes,
            ends,
            tangents: [[30.0, 40.0, 0.0], [0.0, 50.0, 0.0]],
        }
    }

    #[test]
    fn the_edge_ending_where_the_click_did_is_found_within_a_few_millimetres() {
        // The room's millimetres: 0.4 mm off.
        let room_end = [END[0] + 0.0004, END[1], END[2] - 0.0002];
        let edges = [
            edge(10, [1, 2], [[0.0; 3], [5.0, 0.0, 0.0]]),
            edge(11, [2, 3], [START, room_end]),
            edge(12, [3, 4], [room_end, [9.0; 3]]),
        ];
        let found = find(&edges, END).unwrap();
        assert_eq!(
            (found.edge, found.end, found.node()),
            (11, 1, 3),
            "the first, as the game looks"
        );
        assert_eq!(
            find(&edges[2..], END).map(|f| (f.edge, f.end)),
            Some((12, 0))
        );
        // A centimetre off is another place.
        let far = [END[0] + 0.01, END[1], END[2]];
        assert_eq!(find(&[edge(13, [1, 2], [START, far])], END), None);
    }

    fn click(direction: bool, kind: u8) -> Click {
        Click {
            tool: TOOL,
            settings: SETTINGS_AT,
            start: START,
            end: END,
            continues: true,
            direction,
            kind,
        }
    }

    #[test]
    fn the_direction_follows_the_games_rule() {
        let at_end1 = find(&[edge(11, [2, 3], [START, END])], END).unwrap();
        let at_end0 = find(&[edge(12, [3, 4], [END, START])], END).unwrap();
        // A street tool, a new node with only the new street: the tangent
        // at node1 onwards, and at node0 reversed; normalized.
        assert_eq!(
            direction(&click(true, 1), &at_end1, 1, 0),
            Some([0.0, 1.0, 0.0])
        );
        assert_eq!(
            direction(&click(true, 1), &at_end0, 1, 0),
            Some([-0.6, -0.8, 0.0])
        );
        // A track tool counts the other edges.
        assert_eq!(
            direction(&click(true, 0), &at_end1, 0, 1),
            Some([0.0, 1.0, 0.0])
        );
        assert_eq!(direction(&click(true, 0), &at_end1, 1, 0), None);
        // A track whose end snapped to a street beside it, as a guide: the
        // new node has only the track, and it goes on in its direction
        // (seen 2026-10-15: it started at an angle).
        assert_eq!(
            direction(&click(true, 0), &at_end1, 0, 1),
            Some([0.0, 1.0, 0.0])
        );
        // More than one edge there, or the settings off.
        assert_eq!(direction(&click(true, 1), &at_end1, 2, 0), None);
        assert_eq!(direction(&click(false, 1), &at_end1, 1, 0), None);
        // No tangent to normalize.
        let mut flat = at_end1;
        flat.tangent = [0.0; 3];
        assert_eq!(direction(&click(true, 1), &flat, 1, 0), None);
    }

    #[test]
    fn the_start_point_and_direction_are_laid_out_as_the_callbacks() {
        let found = find(&[edge(77, [2, 3], [START, END])], END).unwrap();
        let snap = snap_bytes(&found);
        assert_eq!(&snap[0..4], &77i32.to_le_bytes());
        assert_eq!(&snap[4..8], &1.0f32.to_le_bytes());
        assert_eq!(&snap[8..0xc], &0.0f32.to_le_bytes());
        assert_eq!(&snap[0xc..0xe], &1u16.to_le_bytes());
        assert!(snap[0xe..].iter().all(|b| *b == 0));
        let none = direction_bytes(None, 1);
        assert_eq!(none, [0; DIRECTION_SIZE], "no direction: has_value 0");
        let some = direction_bytes(Some([0.0, 1.0, 0.0]), 1);
        assert_eq!(&some[4..8], &1.0f32.to_le_bytes());
        assert_eq!((some[0xc], some[0xd], some[0x10]), (0, 1, 1));
    }

    fn tool_now() -> ToolNow {
        ToolNow {
            busy: 0,
            has_start: 0,
            dragging: 0,
            settings: SETTINGS_AT,
            continues: 1,
        }
    }

    /// One click at GUI frame 0, handed over and applied.
    fn built(chain: &mut Chain, now: Instant) {
        built_at(chain, now, 0);
    }

    fn built_at(chain: &mut Chain, now: Instant, frame: u64) {
        chain.click(7, Some(click(true, 1)), now, frame);
        assert_eq!(chain.bind(7, Some(900)), None);
        assert_eq!(chain.ordered(), Some(900));
        assert_eq!(
            chain.applied(Some(900), true, vec![edge(11, [2, 3], [START, END])]),
            None
        );
    }

    #[test]
    fn a_later_proposal_reset_cancels_continuation_before_or_after_room_ack() {
        let now = Instant::now();
        let mut chain = Chain::default();
        // The build's own reset happens before Add captures a click.
        assert!(!chain.reset(TOOL));
        chain.click(7, Some(click(true, 1)), now, 0);
        assert!(!chain.reset(TOOL + 1), "another builder cannot cancel it");
        chain.bind(7, Some(900));
        assert!(
            chain.reset(TOOL),
            "a later native reset cancels while ack is pending"
        );
        assert_eq!(chain.ordered(), None);
        chain.applied(Some(900), true, vec![edge(11, [2, 3], [START, END])]);
        chain.seen(11, 1, 0, 1);
        assert_eq!(
            chain.step(TOOL, 1, now, true, &|_| Some(tool_now())),
            Step::Nothing
        );

        built(&mut chain, now);
        assert!(chain.reset(TOOL), "a reset after ack also cancels");
        assert_eq!(chain.poll(now, true), (None, None));
    }

    #[test]
    fn a_click_continues_once_its_build_is_in_the_guis_state() {
        let now = Instant::now();
        let mut chain = Chain::default();
        let read = |_: usize| Some(tool_now());
        chain.click(7, Some(click(true, 1)), now, 0);
        assert_eq!(
            chain.step(TOOL, 1, now, true, &read),
            Step::Wait,
            "not handed over yet"
        );
        chain.bind(7, Some(900));
        // Another player's action, and one of this player's other ones.
        assert_eq!(
            chain.applied(None, true, vec![edge(5, [1, 2], [START, END])]),
            None
        );
        assert_eq!(
            chain.applied(Some(899), true, vec![edge(6, [1, 2], [START, END])]),
            None
        );
        assert_eq!(
            chain.poll(now, true),
            (None, None),
            "neither is the click's"
        );
        chain.applied(Some(900), true, vec![edge(11, [2, 3], [START, END])]);
        let (found, _) = chain.poll(now, true);
        assert_eq!(found.map(|f| f.edge), Some(11));
        assert_eq!(
            chain.step(TOOL, 2, now, true, &read),
            Step::Wait,
            "not seen in the GUI yet"
        );
        chain.seen(11, 1, 0, 2);
        // Another tool's frame changes nothing.
        assert_eq!(chain.step(TOOL + 8, 3, now, true, &read), Step::Nothing);
        let busy = |_: usize| {
            Some(ToolNow {
                busy: 1,
                ..tool_now()
            })
        };
        assert_eq!(
            chain.step(TOOL, 3, now, true, &busy),
            Step::Wait,
            "the callback has not run"
        );
        let Step::Start {
            number,
            found,
            start,
            direction,
            ..
        } = chain.step(TOOL, 4, now, true, &read)
        else {
            panic!("starts");
        };
        assert_eq!((number, found.edge, found.end, start), (7, 11, 1, START));
        assert_eq!(direction, Some([0.0, 1.0, 0.0]));
        assert!(chain.take(7));
        assert_eq!(chain.step(TOOL, 5, now, true, &read), Step::Nothing, "once");
    }

    #[test]
    fn a_stale_check_of_the_guis_state_waits_for_a_new_one() {
        let now = Instant::now();
        let mut chain = Chain::default();
        let read = |_: usize| Some(tool_now());
        built_at(&mut chain, now, 11);
        chain.seen(11, 1, 0, 10);
        chain.step(TOOL, 12, now, true, &read);
        assert_eq!(
            chain.step(TOOL, 13, now, true, &read),
            Step::Wait,
            "seen 3 frames ago"
        );
        chain.seen(99, 1, 0, 13);
        assert_eq!(
            chain.step(TOOL, 14, now, true, &read),
            Step::Wait,
            "another edge"
        );
        chain.seen(11, 1, 0, 14);
        assert!(matches!(
            chain.step(TOOL, 15, now, true, &read),
            Step::Start { .. }
        ));
    }

    #[test]
    fn a_click_is_dropped_when_the_player_or_the_room_moves_on() {
        let now = Instant::now();
        let read = |_: usize| Some(tool_now());

        // A newer click of the tool that did not read.
        let mut chain = Chain::default();
        built(&mut chain, now);
        chain.click(8, None, now, 0);
        assert_eq!(chain.step(TOOL, 1, now, true, &read), Step::Nothing);

        // Not handed to the room.
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        assert_eq!(chain.bind(6, None), None, "another click's hand-over");
        assert!(chain.bind(7, None).is_some());
        assert_eq!(chain.ordered(), None);

        // Refused, or not applied.
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        chain.bind(7, Some(900));
        assert_eq!(chain.refused(901), None);
        assert!(chain.refused(900).is_some());
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        chain.bind(7, Some(900));
        assert!(chain.applied(Some(900), false, Vec::new()).is_some());
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        chain.bind(7, Some(900));
        assert!(
            chain.applied(Some(900), true, Vec::new()).is_some(),
            "no edge read"
        );

        // Applied without an edge ending there.
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        chain.bind(7, Some(900));
        chain.applied(Some(900), true, vec![edge(5, [1, 2], [[0.0; 3], [1.0; 3]])]);
        assert!(chain.poll(now, true).1.is_some());
        assert_eq!(chain.ordered(), None);

        // Out of the room, or too late.
        let mut chain = Chain::default();
        built(&mut chain, now);
        assert_eq!(chain.poll(now, false).1, Some("the game left the room"));
        let mut chain = Chain::default();
        built(&mut chain, now);
        let late = now + LIMIT + Duration::from_secs(1);
        assert!(matches!(
            chain.step(TOOL, 1, late, true, &read),
            Step::Drop(_)
        ));

        // The tool closed (missed frames) and opened again.
        let mut chain = Chain::default();
        built(&mut chain, now);
        chain.step(TOOL, 1, now, true, &read);
        chain.seen(11, 1, 0, 5);
        assert_eq!(
            chain.step(TOOL, 5, now, true, &read),
            Step::Drop("the tool was closed")
        );
        // Closed right after the click, inside the click's own frame, and
        // opened again later: no frame of the tool in between at all.
        let mut chain = Chain::default();
        built_at(&mut chain, now, 5);
        chain.seen(11, 1, 0, 20);
        assert_eq!(
            chain.step(TOOL, 20, now, true, &read),
            Step::Drop("the tool was closed")
        );

        // The player picked a start point, the settings changed or say no.
        for (state, why) in [
            (
                ToolNow {
                    has_start: 1,
                    ..tool_now()
                },
                "the player started a segment",
            ),
            (
                ToolNow {
                    dragging: 1,
                    ..tool_now()
                },
                "the player started a segment",
            ),
            (
                ToolNow {
                    settings: 1,
                    ..tool_now()
                },
                "the tool's settings changed",
            ),
            (
                ToolNow {
                    continues: 0,
                    ..tool_now()
                },
                "the tool's settings do not continue",
            ),
        ] {
            let mut chain = Chain::default();
            built(&mut chain, now);
            chain.seen(11, 1, 0, 1);
            assert_eq!(
                chain.step(TOOL, 1, now, true, &|_| Some(state)),
                Step::Drop(why)
            );
            assert_eq!(chain.step(TOOL, 2, now, true, &read), Step::Nothing);
        }
    }

    #[test]
    fn a_start_the_player_picked_and_let_go_while_the_room_built_ends_the_click() {
        // The room is slow: the player picks another start point, drags a
        // few frames and right-clicks it away before the click's build
        // comes. The tool then has no start point again, but the player
        // moved on: the old segment must not come back.
        let now = Instant::now();
        let mut chain = Chain::default();
        let idle = |_: usize| Some(tool_now());
        let started = |_: usize| {
            Some(ToolNow {
                has_start: 1,
                dragging: 1,
                ..tool_now()
            })
        };
        chain.click(7, Some(click(true, 1)), now, 0);
        chain.bind(7, Some(900));
        assert_eq!(chain.step(TOOL, 1, now, true, &idle), Step::Wait);
        assert_eq!(
            chain.step(TOOL, 2, now, true, &started),
            Step::Drop("the player started a segment")
        );
        assert_eq!(chain.ordered(), None, "the replay reads nothing more");
        chain.applied(Some(900), true, vec![edge(11, [2, 3], [START, END])]);
        assert_eq!(chain.poll(now, true), (None, None));
        chain.seen(11, 1, 0, 3);
        assert_eq!(chain.step(TOOL, 3, now, true, &idle), Step::Nothing);
        // Before the click's callback ran (busy), the tool's state is not
        // yet the player's.
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        let busy = |_: usize| {
            Some(ToolNow {
                busy: 1,
                has_start: 1,
                ..tool_now()
            })
        };
        assert_eq!(chain.step(TOOL, 1, now, true, &busy), Step::Wait);
        assert_eq!(chain.step(TOOL, 2, now, true, &idle), Step::Wait);
    }

    #[test]
    fn only_the_street_tools_click_replaces_the_kept_one() {
        // A mod's follow-up build (playerInitiated too) reaches the add's
        // detour from elsewhere: the street tool's click stays kept.
        ADD_RETURN.store(0x1_4058_91b4, Ordering::Release);
        STEP_HOOKED.store(true, Ordering::Release);
        RESET_HOOKED.store(true, Ordering::Release);
        {
            let mut chain = chain();
            built(&mut chain, Instant::now());
        }
        before_add(0x1_4000_1234, CALLBACK, 8);
        assert!(chain().take(7), "kept through the script's build");
        // The street tool's own click replaces it, even one that does not
        // read (nothing at the callback here).
        built(&mut chain(), Instant::now());
        before_add(0x1_4058_91b4, CALLBACK, 9);
        assert!(!chain().take(7));
        ADD_RETURN.store(0, Ordering::Release);
        STEP_HOOKED.store(false, Ordering::Release);
        RESET_HOOKED.store(false, Ordering::Release);
    }

    #[test]
    fn a_click_handed_over_earlier_is_not_taken_for_a_later_one() {
        // Click 7 ends at a junction and is handed over; before the room
        // builds it, click 8 ends at the same junction. Click 7's build is
        // not click 8's.
        let now = Instant::now();
        let mut chain = Chain::default();
        chain.click(7, Some(click(true, 1)), now, 0);
        chain.bind(7, Some(900));
        chain.click(8, Some(click(true, 1)), now, 0);
        chain.bind(8, Some(901));
        assert_eq!(
            chain.applied(Some(900), true, vec![edge(11, [2, 3], [START, END])]),
            None
        );
        assert_eq!(chain.poll(now, true), (None, None));
        chain.applied(Some(901), true, vec![edge(12, [4, 3], [START, END])]);
        assert_eq!(chain.poll(now, true).0.map(|f| f.edge), Some(12));
    }

    #[test]
    fn the_targets_check_each_other() {
        let add = 0x1_409d_4040usize;
        let mut names: BTreeMap<&str, usize> = BTreeMap::new();
        names.insert(CAPTURE, 0x1_4058_8f65);
        names.insert(ADD_CALL, 0x1_4058_8f65 + CAPTURE_TO_ADD_CALL);
        names.insert(RESULT_SCAN, 0x1_4056_d949);
        names.insert(SNAP, 0x1_4056_d949 + SCAN_TO_SNAP);
        names.insert(START_AT, 0x1_4057_0a30);
        names.insert(ENTITY_EXISTS, 0x1_404f_7170);
        let revision = 0x1_42bb_af00usize;
        let calls: BTreeMap<usize, usize> = [
            (names[ADD_CALL], add),
            (names[SNAP] + SNAP_CALL, names[START_AT]),
            (names[START_AT] + START_AT_REVISION_CALL, revision),
            (revision + REVISION_EXISTS_CALL, names[ENTITY_EXISTS]),
        ]
        .into_iter()
        .collect();
        let code = |calls: &BTreeMap<usize, usize>, at: usize| -> Option<[u8; 5]> {
            let target = *calls.get(&at)?;
            let rel = (target as i64 - (at as i64 + 5)) as i32;
            let r = rel.to_le_bytes();
            Some([0xE8, r[0], r[1], r[2], r[3]])
        };
        let at = |names: &BTreeMap<&str, usize>, name: &str| {
            names
                .get(name)
                .copied()
                .ok_or_else(|| format!("the profile has no {name}"))
        };
        let targets = check(&|n| at(&names, n), &|a| code(&calls, a), add).unwrap();
        assert_eq!(targets.add_call, names[ADD_CALL]);
        assert_eq!(targets.start_at, names[START_AT]);
        assert_eq!(targets.exists, names[ENTITY_EXISTS]);

        // Another Add, a missing target, a callback that calls elsewhere,
        // or a revision read that checks with another function.
        assert!(check(&|n| at(&names, n), &|a| code(&calls, a), add + 1).is_err());
        let mut fewer = names.clone();
        fewer.remove(ENTITY_EXISTS);
        assert!(check(&|n| at(&fewer, n), &|a| code(&calls, a), add).is_err());
        let mut other = calls.clone();
        other.insert(names[SNAP] + SNAP_CALL, 0x1_4000_1000);
        assert!(check(&|n| at(&names, n), &|a| code(&other, a), add).is_err());
        let mut other = calls.clone();
        other.insert(revision + REVISION_EXISTS_CALL, 0x1_4000_2000);
        assert!(check(&|n| at(&names, n), &|a| code(&other, a), add).is_err());
        let mut moved = names.clone();
        moved.insert(CAPTURE, names[CAPTURE] + 1);
        assert!(check(&|n| at(&moved, n), &|a| code(&calls, a), add).is_err());
    }
}
