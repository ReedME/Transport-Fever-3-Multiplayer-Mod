//! Read the level crossing window's type change at `CommandList::Add`.
//!
//! Clicking a crossing opens `RailroadCrossingContent`, whose native
//! `RailroadCrossingComp` changes the crossing's type with a
//! `WorldBuildProposal` it queues itself (playerInitiated 1): no Lua runs and
//! game scripts hear of no proposal, so in a room the click was stopped at
//! the apply with nothing said (2026-10-16). The window's call of `Add` is
//! told by where it returns to; there its proposal is read, checked for the
//! one shape the room carries, and kept for the click (the same queue as the
//! module editor's, [`crate::modules::keep`]), as the crossing and its new
//! type or why it is not carried. A recognised click always keeps an answer,
//! so the GUI never hands a preview of another tool for it.
//!
//! The shape carried (build 40420, read statically; docs/HOOKS.md, "Level
//! crossing types"): no geometry, no node configuration, an empty
//! `node2rcType`, and one entry in the window's own crossing -> type map.
//! The window's other shape rebuilds the crossing's segments and maps its
//! nodes instead; that one is refused, as is anything else.
//!
//! The street and track tools' toolbar type is read here too, at every
//! other player build ([`record_tool`]): the tool maps its crossing nodes
//! in the proposal's `node2rcType`, which the Lua binding does not reach.

#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use tpf3mp_proto::lua::LuaValue;

use crate::modules::{self, Memory, i32_at, read, u64_at, vector};
use crate::terrain::Heap;

pub use crate::build_data::native::crossings::{
    CROSSING_ADD_CALL, INSERT_HEAD, MAP_INSERT_CALL, MAPS_LAYOUT, layout,
};

/// Where `Add` returns to from the window's call; 0 when the profile does
/// not name it.
static CROSSING_RETURN: AtomicUsize = AtomicUsize::new(0);
/// Whether the profile found the proposal's crossing maps where they were.
static LAYOUT: AtomicBool = AtomicBool::new(false);

/// Most entries a crossing map holds before it is a misread.
const MAX_ENTRIES: u64 = 64;

/// Notes the window's call of `Add`, at `call` (its absolute address), and
/// whether the maps' layout resolved.
pub fn set_call(call: usize, layout: bool) {
    CROSSING_RETURN.store(call + 5, Ordering::Release);
    LAYOUT.store(layout, Ordering::Release);
}

/// Whether a call of `Add` returning to `return_address` is the crossing
/// window's.
pub fn is_crossing_window(return_address: usize) -> bool {
    let window = CROSSING_RETURN.load(Ordering::Acquire);
    window != 0 && return_address == window
}

/// What the window's click changes, read from its proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The crossing (this game's entity) to the type, in the window's own
    /// map: no geometry at all.
    Crossing { entity: i32, kind: i32 },
}

/// Bound counts while distinguishing the window's type-only shape from a
/// rebuild; non-empty rebuild vectors are refused.
const MAX_SEGMENTS: usize = 16;
const MAX_JUNCTIONS: usize = 32;

/// Keeps what the window queued at click `click` (the count before it): the
/// crossing and its type, or why the room does not carry it. Logs one line.
pub fn record(memory: &dyn Memory, click: u64, payload: usize) {
    let read = if LAYOUT.load(Ordering::Acquire) {
        decode(memory, payload)
    } else {
        Err("the profile does not find this build's crossing maps".to_owned())
    };
    match &read {
        Ok(Change::Crossing { entity, kind }) => crate::log::line(&format!(
            "crossing window: click {click} sets crossing {entity} to type {kind}"
        )),
        Err(why) => crate::log::line(&format!(
            "crossing window: click {click} is not carried: {why}"
        )),
    }
    keep(click, read);
}

/// What every reason this keeps begins with, so the GUI tells the window's
/// click from the module editor's.
pub const WHY_PREFIX: &str = "crossing window: ";

/// Keeps `read` for click `click` as the GUI takes it: `{ crossingType =
/// true, type = t, entity = e }`, or why not, after [`WHY_PREFIX`].
fn keep(click: u64, read: Result<Change, String>) {
    modules::keep(
        click,
        read.map(|change| {
            let mut fields = vec![(LuaValue::string("crossingType"), LuaValue::Boolean(true))];
            let Change::Crossing { entity, kind } = change;
            fields.push((LuaValue::string("type"), LuaValue::Integer(i64::from(kind))));
            fields.push((
                LuaValue::string("entity"),
                LuaValue::Integer(i64::from(entity)),
            ));
            LuaValue::Table(fields)
        })
        .map_err(|why| format!("{WHY_PREFIX}{why}")),
    );
}

/// What the window's proposal at `payload` changes, when it is a shape the
/// room carries; why not otherwise. The only carried shape changes the
/// crossing map and nothing else. A rebuild with `node2rcType` also lays
/// segments and may replace junction settings; a type-only action cannot
/// reproduce those changes, so every such rebuild is refused.
pub fn decode(memory: &dyn Memory, payload: usize) -> Result<Change, String> {
    use crate::modules::layout as street;
    let head = read(memory, payload, street::HEAD_LEN, "the crossing proposal")?;
    for (offset, stride, what) in [
        (street::ADDED_NODES, street::NODE_SIZE, "nodes"),
        (street::REMOVED_NODES, street::NODE_SIZE, "nodes"),
        (street::EDGE_OBJECTS_TO_REMOVE, 4, "edge objects"),
        (
            street::EDGE_OBJECTS_TO_ADD,
            street::EDGE_OBJECT_SIZE,
            "edge objects",
        ),
        (street::TO_REMOVE, 4, "constructions"),
        (street::TO_ADD, street::ENTITY_SIZE, "constructions"),
    ] {
        if vector(&head, offset, stride, usize::MAX, "the crossing proposal")?.1 != 0 {
            return Err(format!("a crossing change that also changes {what}"));
        }
    }
    let (_, configs) = vector(
        &head,
        layout::CONFIGS_ADDED,
        layout::CONFIG_SIZE,
        MAX_JUNCTIONS,
        "the junctions laid",
    )?;
    let (_, resets) = vector(
        &head,
        layout::CONFIGS_REMOVED,
        4,
        MAX_JUNCTIONS,
        "the junctions removed",
    )?;
    let (_, added) = vector(
        &head,
        street::ADDED_SEGMENTS,
        street::SEGMENT_SIZE,
        MAX_SEGMENTS,
        "the segments laid",
    )?;
    let (_, removed) = vector(
        &head,
        street::REMOVED_SEGMENTS,
        street::SEGMENT_SIZE,
        MAX_SEGMENTS,
        "the segments removed",
    )?;
    let nodes = entries(memory, &head, layout::NODE_TYPES, "node2rcType")?;
    let crossings = entries(memory, &head, layout::CROSSING_TYPES, "the crossing map")?;
    if nodes.is_empty() {
        if added != 0 || removed != 0 {
            return Err("a crossing change that lays segments and types no crossing".to_owned());
        }
        if configs != 0 || resets != 0 {
            return Err("a crossing change that also changes junctions".to_owned());
        }
        return match crossings[..] {
            [(entity, kind)] if entity >= 0 && kind >= 0 => Ok(Change::Crossing { entity, kind }),
            [_] => Err("a crossing or type that is not one".to_owned()),
            [] => Err("a crossing change that changes no crossing".to_owned()),
            _ => Err("a change of more than one crossing".to_owned()),
        };
    }
    if !crossings.is_empty() {
        return Err("a crossing change in both of the proposal's maps".to_owned());
    }
    Err("a crossing rebuild with segments or junctions is not carried".to_owned())
}

/// Refuses a carrier `head` with anything in it but the window's own
/// crossing map: geometry, edge objects, constructions, junctions, or
/// `node2rcType`.
fn nothing_else(memory: &dyn Memory, head: &[u8]) -> Result<(), String> {
    use crate::modules::layout as street;
    for (offset, stride, what) in [
        (street::ADDED_NODES, street::NODE_SIZE, "nodes"),
        (street::ADDED_SEGMENTS, street::SEGMENT_SIZE, "segments"),
        (street::REMOVED_NODES, street::NODE_SIZE, "nodes"),
        (street::REMOVED_SEGMENTS, street::SEGMENT_SIZE, "segments"),
        (street::EDGE_OBJECTS_TO_REMOVE, 4, "edge objects"),
        (
            street::EDGE_OBJECTS_TO_ADD,
            street::EDGE_OBJECT_SIZE,
            "edge objects",
        ),
        (street::TO_REMOVE, 4, "constructions"),
        (street::TO_ADD, street::ENTITY_SIZE, "constructions"),
        (layout::CONFIGS_ADDED, layout::CONFIG_SIZE, "junctions"),
        (layout::CONFIGS_REMOVED, 4, "junctions"),
    ] {
        if vector(head, offset, stride, usize::MAX, "the crossing proposal")?.1 != 0 {
            return Err(format!("the carrier also changes {what}"));
        }
    }
    if !entries(memory, head, layout::NODE_TYPES, "node2rcType")?.is_empty() {
        return Err("the carrier has node2rcType".to_owned());
    }
    Ok(())
}

// ---------- the street and track tools' toolbar type ----------

/// What a street or track tool's click says of the level crossings it lays
/// (docs/HOOKS.md, "Level crossing types"): the tool (`UI::StreetBuilder`)
/// maps each crossing node of its own polyline to `-2 - rcType`, the
/// toolbar's type "preferred" (an existing crossing keeps its own), and
/// maps nothing for an automatic choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolType {
    /// No node mapped: the engine picks each new crossing's type.
    None,
    /// Every node mapped to this type (this game's index), preferred.
    Type(i32),
    /// The profile does not find the maps, so what the tool picked is not
    /// known: the build goes as it did before the reader.
    Unknown,
}

/// The tools' types read, by the click count they were queued at, oldest
/// first: a queue of its own, so the GUI's routing of the module editor's,
/// the terrain tools' and the crossing window's records is untouched.
static TOOL_KEPT: Mutex<VecDeque<(u64, Result<ToolType, String>)>> = Mutex::new(VecDeque::new());

/// Whether the profile found the proposal's crossing maps, for the tools'
/// reader: set at install whether or not the window's call is found.
static TOOL_LAYOUT: AtomicBool = AtomicBool::new(false);

/// Notes whether the profile found the proposal's crossing maps.
pub fn set_layout(layout: bool) {
    TOOL_LAYOUT.store(layout, Ordering::Release);
}

/// The toolbar's type in the proposal at `payload`: `node2rcType` read
/// whole (every link checked), its "ignored" entries (-1) left out. Every
/// other entry must be the tool's preferred encoding (`-2 - type`), all of
/// one type; a forced type (>= 0, as the crossing window's rebuild writes)
/// or types that differ are refused.
pub fn decode_tool(memory: &dyn Memory, payload: usize) -> Result<ToolType, String> {
    let head = read(
        memory,
        payload,
        crate::modules::layout::HEAD_LEN,
        "the build's proposal",
    )?;
    let values: Vec<i32> = entries(memory, &head, layout::NODE_TYPES, "node2rcType")?
        .into_iter()
        .map(|(_, value)| value)
        .filter(|value| *value != -1)
        .collect();
    let Some(&first) = values.first() else {
        return Ok(ToolType::None);
    };
    if values.iter().any(|value| *value >= 0) {
        return Err("a build that forces a level crossing's type".to_owned());
    }
    if values.iter().any(|value| *value != first) {
        return Err("a build that gives its level crossings different types".to_owned());
    }
    // first <= -2, so the type is 0 or more and fits.
    Ok(ToolType::Type(-2 - first))
}

/// Keeps what the tool's click `click` (the count before it) says of its
/// crossings: read at every player build that is not the module editor's,
/// a terrain tool's or the crossing window's. Logs a line when it picked a
/// type or did not read.
pub fn record_tool(memory: &dyn Memory, click: u64, payload: usize) {
    let read = if TOOL_LAYOUT.load(Ordering::Acquire) {
        decode_tool(memory, payload)
    } else {
        Ok(ToolType::Unknown)
    };
    match &read {
        Ok(ToolType::Type(kind)) => crate::log::line(&format!(
            "build tool: click {click} lays its level crossings as type {kind}"
        )),
        Err(why) => crate::log::line(&format!(
            "build tool: click {click}'s level crossing type does not read: {why}"
        )),
        Ok(_) => {}
    }
    keep_tool(click, read);
}

/// Keeps `read` for click `click`, the newest [`modules::KEPT`] only.
pub(crate) fn keep_tool(click: u64, read: Result<ToolType, String>) {
    let mut kept = TOOL_KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|(n, _)| *n != click);
    kept.push_back((click, read));
    while kept.len() > modules::KEPT {
        kept.pop_front();
    }
}

/// What the tool's click `click` said of its crossings, once: `None` when
/// nothing was kept for it (not a player build, or taken already).
pub fn take_tool(click: u64) -> Option<Result<ToolType, String>> {
    let mut kept = TOOL_KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let at = kept.iter().position(|(n, _)| *n == click)?;
    kept.remove(at).map(|(_, read)| read)
}

// ---------- the room's replay: a carrier filled with the type ----------

/// The game's insert into a crossing map, as [`fill`] calls it: handed the
/// heap, the map and the key, it answers the entry's list node.
pub type Insert<'a> = dyn FnMut(&mut dyn Heap, usize, i32) -> Result<usize, String> + 'a;

/// The crossing and type armed for the next build the room's actions send,
/// and whether a build was filled with them.
struct Armed {
    change: Option<(i32, i32)>,
    filled: bool,
}

static ARMED: Mutex<Option<Armed>> = Mutex::new(None);
/// The game's insert into a crossing map, found through the window's call
/// of it; 0 when the profile does not lead to it.
static INSERT: AtomicUsize = AtomicUsize::new(0);

/// Notes the game's insert into a crossing map, at `insert`.
pub fn set_insert(insert: usize) {
    INSERT.store(insert, Ordering::Release);
}

/// Arms the next build applied while the room's actions run with crossing
/// `entity` (this game's) taking type `kind` (this game's index). Refuses
/// where this hook cannot fill a build with it.
pub fn arm(entity: i32, kind: i32) -> Result<(), String> {
    if entity < 0 || kind < 0 {
        return Err("a crossing or type that is not one".to_owned());
    }
    if INSERT.load(Ordering::Acquire) == 0 || !LAYOUT.load(Ordering::Acquire) {
        return Err("the profile does not lead to this build's crossing map".to_owned());
    }
    *ARMED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Armed {
        change: Some((entity, kind)),
        filled: false,
    });
    Ok(())
}

/// Disarms: whether a build was filled with the crossing armed, or `None`
/// when none was armed.
pub fn disarm() -> Option<bool> {
    ARMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .map(|armed| armed.filled)
}

/// Fills the carrier at `payload`, an empty proposal, so that it sets
/// crossing `entity` to type `kind`, as the window's own build does: the
/// crossing into the proposal's crossing map through `insert` (the game's,
/// handed the heap, the map and the key, which answers the entry's list
/// node), its type into that node. Refuses a
/// carrier with anything in it, and checks the map holds exactly the one
/// entry after.
pub fn fill(
    heap: &mut dyn Heap,
    insert: &mut Insert<'_>,
    payload: usize,
    entity: i32,
    kind: i32,
) -> Result<(), String> {
    let len = crate::modules::layout::HEAD_LEN;
    let head = read(heap, payload, len, "the carrier")?;
    nothing_else(heap, &head).map_err(|why| format!("the carrier is not empty: {why}"))?;
    if !entries(
        heap,
        &head,
        layout::CROSSING_TYPES,
        "the carrier's crossing map",
    )?
    .is_empty()
    {
        return Err("the carrier's crossing map is not empty".to_owned());
    }
    let node = insert(heap, payload + layout::CROSSING_TYPES, entity)?;
    if node == 0 || !heap.write(node + layout::NODE_VALUE, &kind.to_le_bytes()) {
        return Err("the crossing's type could not be written".to_owned());
    }
    let head = read(heap, payload, len, "the carrier")?;
    if entries(
        heap,
        &head,
        layout::CROSSING_TYPES,
        "the carrier's crossing map",
    )? != [(entity, kind)]
    {
        return Err("the carrier's crossing map does not hold the crossing".to_owned());
    }
    Ok(())
}

/// At the apply of a build the room's actions send: fills it with the
/// crossing armed, if one is. `Ok(true)` when it filled, `Ok(false)` when
/// nothing was armed, `Err(why)` when the build is refused (the change is
/// then spent).
pub fn inject(
    heap: &mut dyn Heap,
    insert: &mut Insert<'_>,
    payload: usize,
) -> Result<bool, String> {
    let (entity, kind) = {
        let mut armed = ARMED.lock().unwrap_or_else(PoisonError::into_inner);
        match armed.as_mut().and_then(|a| a.change.take()) {
            Some(change) => change,
            None => return Ok(false),
        }
    };
    fill(heap, insert, payload, entity, kind)?;
    if let Some(armed) = ARMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        armed.filled = true;
    }
    crate::log::line(&format!(
        "crossing: filled the room's carrier: crossing {entity} to type {kind}"
    ));
    Ok(true)
}

/// [`inject`] in the running game: its memory, and its insert.
#[cfg(all(windows, target_arch = "x86_64"))]
pub fn inject_game(payload: usize) -> Result<bool, String> {
    /// `unordered_map<int,int>::try_emplace`: the map, the `pair<iterator,
    /// bool>` it answers in, the key; returns that pair.
    type InsertFn = unsafe extern "C" fn(usize, *mut [usize; 2], *const i32) -> *mut [usize; 2];
    let address = INSERT.load(Ordering::Acquire);
    let mut insert = |_: &mut dyn Heap, map: usize, key: i32| -> Result<usize, String> {
        if address == 0 {
            return Err("the profile does not lead to the crossing map's insert".to_owned());
        }
        // SAFETY: the game's insert, found through the window's call of it
        // and its head checked at install.
        let insert: InsertFn = unsafe { std::mem::transmute::<usize, InsertFn>(address) };
        let mut out = [0usize; 2];
        // SAFETY: `map` is the carrier's crossing map, read whole and empty
        // just before, on the game's thread inside the apply that owns it;
        // `out` and `key` live across the call.
        let answered = unsafe { insert(map, &mut out, &key) };
        if answered.is_null() {
            return Err("the game's insert answered nothing".to_owned());
        }
        // SAFETY: the insert answers the pair it was handed.
        Ok(unsafe { (*answered)[0] })
    };
    inject(&mut crate::terrain::GameHeap, &mut insert, payload)
}

/// The `(key, value)` pairs of the MSVC `std::unordered_map<int,int>` at
/// `offset` of `head`, in list order. Its list must close on its sentinel
/// after exactly its size, every link answered by its back link.
fn entries(
    memory: &dyn Memory,
    head: &[u8],
    offset: usize,
    what: &str,
) -> Result<Vec<(i32, i32)>, String> {
    let sentinel = u64_at(head, offset + layout::MAP_HEAD);
    let size = u64_at(head, offset + layout::MAP_SIZE);
    if size > MAX_ENTRIES {
        return Err(format!("{what}: {size} entries, more than a crossing has"));
    }
    let sentinel =
        usize::try_from(sentinel).map_err(|_| format!("{what}: its list is out of range"))?;
    if sentinel == 0 || !sentinel.is_multiple_of(8) {
        return Err(format!("{what}: no list"));
    }
    let mut out = Vec::new();
    let mut previous = sentinel;
    let mut node = link(memory, sentinel, layout::NODE_NEXT, what)?;
    while node != sentinel {
        if out.len() as u64 == size {
            return Err(format!("{what}: its list is longer than its size"));
        }
        if node == 0 || !node.is_multiple_of(8) {
            return Err(format!("{what}: a broken link"));
        }
        let raw = read(memory, node, layout::NODE_LEN, what)?;
        if u64_at(&raw, layout::NODE_PREV) != previous as u64 {
            return Err(format!("{what}: a link its neighbour does not answer"));
        }
        let key = i32_at(&raw, layout::NODE_KEY);
        if out.iter().any(|(k, _)| *k == key) {
            return Err(format!("{what}: a key twice"));
        }
        out.push((key, i32_at(&raw, layout::NODE_VALUE)));
        previous = node;
        node = usize::try_from(u64_at(&raw, layout::NODE_NEXT))
            .map_err(|_| format!("{what}: a broken link"))?;
    }
    if out.len() as u64 != size {
        return Err(format!("{what}: its list is shorter than its size"));
    }
    if link(memory, sentinel, layout::NODE_PREV, what)? != previous {
        return Err(format!("{what}: its list does not close"));
    }
    Ok(out)
}

/// The pointer at `field` of the list node at `node`.
fn link(memory: &dyn Memory, node: usize, field: usize, what: &str) -> Result<usize, String> {
    let raw = read(memory, node + field, 8, what)?;
    usize::try_from(u64_at(&raw, 0)).map_err(|_| format!("{what}: a broken link"))
}
