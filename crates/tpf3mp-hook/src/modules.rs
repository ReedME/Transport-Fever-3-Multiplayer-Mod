//! The module editor's builds, read natively (docs/HOOKS.md, "The build
//! tools", "The module editor").
//!
//! The module editor (`UI::ModuleBuilder`, opened from a station's window)
//! tells game scripts nothing of its proposals on build 40408: the game
//! forwards `builder.proposalCreate` for six other tools only. It queues its
//! `WorldBuildProposal` itself, with its own call of `CommandList::Add`
//! (`UI::ModuleBuilder::MousePressed`, the call at 0x543b25, returning to
//! 0x543b2a). So at that call, where [`crate::builds`] counts the player's
//! click, the hook reads the queued proposal here and keeps it, as a Lua
//! table of the shape game scripts see a proposal in, under the count the
//! click had before it ([`record`]). The mod's GUI takes it for that click
//! (`tpf3mp_native.built(n)`, [`take`]) and makes the action of it as it
//! makes the construction tool's (`tpf3mp/capture.lua`): a
//! `BuildConstruction` that replaces the edited construction. The apply of
//! the click is stopped as every player's build is ([`crate::builds`]); the
//! room orders the edit for every game.
//!
//! What is read (build 40408; the layouts, the b-tree walk and its bounds
//! are those of `feat/capture-all`'s `conscap.rs`, by Juliansgith, and its
//! `investigation/TPF3_CONSTRUCTION_CAPTURE_2026-09-29.md`; the call site
//! and the entity getters re-checked with `tools/tpfre`):
//!
//! - `Proposal.toRemove` (`vector<int32>` at payload + 0x240), the
//!   entities the edit removes;
//! - `Proposal.toAdd` (`vector<ConstructionEntity>` at + 0x258, 0xe48 bytes
//!   each): the first one's file (`ResName` at + 0), parameters (the
//!   `lua::Table`, an abseil b-tree of variants, at + 0xa18), matrix (16
//!   `f32` at + 0xc98) and name (`std::string` at + 0xe18);
//! - the street part: the entity of each removed node (+ 0x30, 0x40 bytes
//!   each, entity at + 0) and removed segment (+ 0x48, 0x350 bytes, entity
//!   at + 0), and how many nodes, segments and edge objects it adds.
//!
//! Every read is checked readable, every vector for order and whole
//! elements, every count and text capped; a table must walk to exactly its
//! size. What does not read is kept as the reason, which the GUI logs and
//! refuses the click with (fail closed): nothing is guessed.

#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use tpf3mp_proto::lua::LuaValue;

/// The profile's name for the module editor's call of `CommandList::Add`.
/// `Add` returns 5 bytes past it.
pub const MODULE_ADD_CALL: &str = "ModuleBuilder::MousePressed/Add call";

pub use crate::build_data::native::modules::layout;

/// Bounds past which a read is a misread: an edit adds one construction.
pub const MAX_ADDED: usize = 16;
pub const MAX_REMOVED: usize = 64;
pub const MAX_STREET: usize = 1024;
pub const MAX_EDGE_OBJECTS: usize = 256;
pub const MAX_TABLE_ENTRIES: usize = 4096;
/// Deepest parameter nesting read (the mod refuses deeper than 8 anyway)
/// and deepest b-tree.
pub const MAX_DEPTH: usize = 8;
pub const MAX_TREE: usize = 32;
pub const MAX_TEXT: usize = 256;
/// Clicks whose builds are kept for the GUI to take.
pub const KEPT: usize = 16;

/// Memory to read from: the game's, or a test's.
pub trait Memory {
    /// `len` bytes at `address`, or `None` when they are not all readable.
    fn read(&self, address: usize, len: usize) -> Option<Vec<u8>>;

    /// `out.len()` bytes at `address` into `out`, without allocating where
    /// the memory can; `false` when they are not all readable.
    fn read_into(&self, address: usize, out: &mut [u8]) -> bool {
        match self.read(address, out.len()) {
            Some(bytes) => {
                out.copy_from_slice(&bytes);
                true
            }
            None => false,
        }
    }
}

/// The running game's memory, read only where [`crate::image::readable`]
/// says so.
pub struct Process;

impl Memory for Process {
    fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
        if address == 0 || !crate::image::readable(address, len) {
            return None;
        }
        let mut bytes = vec![0u8; len];
        // SAFETY: `len` bytes at `address` are committed readable memory,
        // checked just above, and `bytes` has room for them.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), len) };
        Some(bytes)
    }
}

pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(word)
}

pub(crate) fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    i32::from_le_bytes(word)
}

pub(crate) fn read(
    memory: &dyn Memory,
    address: usize,
    len: usize,
    what: &str,
) -> Result<Vec<u8>, String> {
    memory
        .read(address, len)
        .ok_or_else(|| format!("{what} does not read"))
}

/// The `std::vector` whose `{begin, end, capacity}` is at `offset` of
/// `head`: its begin and element count.
pub(crate) fn vector(
    head: &[u8],
    offset: usize,
    stride: usize,
    max: usize,
    what: &str,
) -> Result<(usize, usize), String> {
    let begin = u64_at(head, offset);
    let end = u64_at(head, offset + 8);
    let capacity = u64_at(head, offset + 16);
    if end < begin || capacity < end {
        return Err(format!(
            "{what}: its begin, end and capacity are out of order"
        ));
    }
    if begin == 0 && end != 0 {
        return Err(format!("{what}: a null begin with elements"));
    }
    let span = usize::try_from(end - begin).map_err(|_| format!("{what}: too large"))?;
    if !span.is_multiple_of(stride) {
        return Err(format!("{what}: not whole elements"));
    }
    let count = span / stride;
    if count > max {
        return Err(format!("{what}: {count}, more than the {max} a build has"));
    }
    let begin = usize::try_from(begin).map_err(|_| format!("{what}: too large"))?;
    Ok((begin, count))
}

/// The `int32` at `field` of each of a vector's `count` elements.
pub(crate) fn ids(
    memory: &dyn Memory,
    begin: usize,
    count: usize,
    stride: usize,
    field: usize,
    what: &str,
) -> Result<Vec<i32>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let raw = read(memory, begin, count * stride, what)?;
    Ok((0..count)
        .map(|i| i32_at(&raw, i * stride + field))
        .collect())
}

/// One MSVC `std::string` at `address`, at most `max` bytes.
pub(crate) fn string(
    memory: &dyn Memory,
    address: usize,
    max: usize,
    what: &str,
) -> Result<String, String> {
    let raw = read(memory, address, layout::STRING_SIZE, what)?;
    let len = u64_at(&raw, layout::STRING_LEN);
    let capacity = u64_at(&raw, layout::STRING_CAPACITY);
    if capacity < len {
        return Err(format!("{what}: its length is past its capacity"));
    }
    let len = usize::try_from(len)
        .ok()
        .filter(|len| *len <= max)
        .ok_or_else(|| format!("{what}: longer than {max} bytes"))?;
    let bytes = if capacity < layout::STRING_INLINE {
        if len >= layout::STRING_INLINE as usize {
            return Err(format!("{what}: an inline length that does not fit"));
        }
        raw[..len].to_vec()
    } else {
        let data = usize::try_from(u64_at(&raw, 0)).map_err(|_| format!("{what}: its data"))?;
        read(memory, data, len, what)?
    };
    String::from_utf8(bytes).map_err(|_| format!("{what}: not UTF-8"))
}

/// A `ResName` as the game names it to scripts: `first + "::/" + second`
/// (`::/depot/...` for the base game's), "" when `second` is empty.
pub(crate) fn res_name(memory: &dyn Memory, address: usize, what: &str) -> Result<String, String> {
    let first = string(memory, address, MAX_TEXT, what)?;
    let second = string(memory, address + layout::STRING_SIZE, MAX_TEXT, what)?;
    if second.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{first}::/{second}"))
}

/// A parameter table walk's shared bounds.
struct Walk {
    entries: usize,
    open: Vec<usize>,
}

fn variant(
    memory: &dyn Memory,
    address: usize,
    depth: usize,
    walk: &mut Walk,
    key: bool,
) -> Result<LuaValue, String> {
    let bytes = read(memory, address, layout::VARIANT_SIZE, "a parameter")?;
    match bytes[layout::VARIANT_TAG] as i8 {
        layout::TAG_BOOL if !key => Ok(LuaValue::Boolean(bytes[0] != 0)),
        layout::TAG_NUMBER => {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[..8]);
            let number = f64::from_le_bytes(word);
            if !number.is_finite() {
                return Err("a parameter number that is not finite".into());
            }
            Ok(LuaValue::Number(number))
        }
        layout::TAG_STRING => Ok(LuaValue::string(&string(
            memory,
            address,
            MAX_TEXT,
            "a parameter text",
        )?)),
        layout::TAG_TABLE if !key => {
            let table = usize::try_from(u64_at(&bytes, 0))
                .map_err(|_| "a nested table's pointer".to_string())?;
            Ok(LuaValue::Table(table_at(memory, table, depth + 1, walk)?))
        }
        other => Err(format!(
            "a parameter {} of variant tag {other}, which cannot travel",
            if key { "key" } else { "value" }
        )),
    }
}

/// The entries of one b-tree node and its subtrees, in order.
fn node(
    memory: &dyn Memory,
    address: usize,
    level: usize,
    depth: usize,
    walk: &mut Walk,
    out: &mut Vec<(LuaValue, LuaValue)>,
) -> Result<(), String> {
    if level > MAX_TREE {
        return Err("a parameter table's tree is deeper than any the game makes".into());
    }
    let head = read(
        memory,
        address,
        layout::NODE_HEADER,
        "a parameter table node",
    )?;
    let start = usize::from(head[layout::NODE_START]);
    let count = usize::from(head[layout::NODE_FINISH]);
    let max = usize::from(head[layout::NODE_MAX_COUNT]);
    let leaf = max != 0;
    if start != 0 || count > layout::NODE_SLOTS || (leaf && count > max) || (!leaf && count == 0) {
        return Err(format!(
            "a parameter table node with start {start}, {count} slot(s) of {max}"
        ));
    }
    let children = if leaf {
        Vec::new()
    } else {
        let raw = read(
            memory,
            address + layout::NODE_CHILDREN,
            8 * (count + 1),
            "a parameter table node's children",
        )?;
        (0..=count)
            .map(|i| usize::try_from(u64_at(&raw, 8 * i)).unwrap_or(0))
            .collect()
    };
    for i in 0..count {
        if let Some(child) = children.get(i) {
            node(memory, *child, level + 1, depth, walk, out)?;
        }
        walk.entries += 1;
        if walk.entries > MAX_TABLE_ENTRIES {
            return Err("a parameter table with more entries than any the game makes".into());
        }
        let slot = address + layout::NODE_HEADER + i * layout::SLOT_SIZE;
        let key = variant(memory, slot, depth, walk, true)?;
        let value = variant(memory, slot + layout::SLOT_VALUE, depth, walk, false)?;
        out.push((key, value));
    }
    if let Some(last) = children.get(count) {
        node(memory, *last, level + 1, depth, walk, out)?;
    }
    Ok(())
}

/// The `lua::Table` at `address`, walked whole.
fn table_at(
    memory: &dyn Memory,
    address: usize,
    depth: usize,
    walk: &mut Walk,
) -> Result<Vec<(LuaValue, LuaValue)>, String> {
    if depth > MAX_DEPTH {
        return Err(format!("parameters nested deeper than {MAX_DEPTH}"));
    }
    if walk.open.contains(&address) {
        return Err("a parameter table contains itself".into());
    }
    let head = read(memory, address, layout::TABLE_SIZE, "a parameter table")?;
    let size = usize::try_from(u64_at(&head, layout::TABLE_COUNT))
        .ok()
        .filter(|size| *size <= MAX_TABLE_ENTRIES)
        .ok_or_else(|| "a parameter table claims too many entries".to_string())?;
    let mut out = Vec::with_capacity(size);
    if size > 0 {
        let root = usize::try_from(u64_at(&head, layout::TABLE_ROOT))
            .map_err(|_| "a parameter table's root".to_string())?;
        walk.open.push(address);
        node(memory, root, 0, depth, walk, &mut out)?;
        walk.open.pop();
    }
    if out.len() != size {
        return Err(format!(
            "a parameter table of {size} entries walked to {}",
            out.len()
        ));
    }
    Ok(out)
}

/// What the module editor queued, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub file: String,
    pub params: Vec<(LuaValue, LuaValue)>,
    /// The game's matrix, column by column; the origin is 12..15.
    pub transf: [f32; 16],
    pub name: String,
    /// Constructions added (`toAdd`); only the first is read.
    pub added: usize,
    /// The entities removed (`toRemove`).
    pub removed: Vec<i32>,
    pub added_nodes: usize,
    pub added_segments: usize,
    pub removed_nodes: Vec<i32>,
    pub removed_segments: Vec<i32>,
    pub edge_objects_added: usize,
    pub edge_objects_removed: usize,
}

/// Reads the `Proposal` of a `WorldBuildProposal` payload at `payload`.
pub fn decode(memory: &dyn Memory, payload: usize) -> Result<Edit, String> {
    let head = read(memory, payload, layout::HEAD_LEN, "the proposal")?;
    let (begin, added) = vector(
        &head,
        layout::TO_ADD,
        layout::ENTITY_SIZE,
        MAX_ADDED,
        "the constructions added",
    )?;
    if added == 0 {
        return Err("the edit adds no construction".into());
    }
    let file = res_name(memory, begin + layout::ENTITY_FILE, "the file name")?;
    let mut walk = Walk {
        entries: 0,
        open: Vec::new(),
    };
    let params = table_at(memory, begin + layout::ENTITY_PARAMS, 0, &mut walk)?;
    let raw = read(memory, begin + layout::ENTITY_TRANSF, 64, "the matrix")?;
    let mut transf = [0f32; 16];
    for (i, value) in transf.iter_mut().enumerate() {
        let mut word = [0u8; 4];
        word.copy_from_slice(&raw[4 * i..4 * i + 4]);
        *value = f32::from_le_bytes(word);
    }
    if transf.iter().any(|v| !v.is_finite()) {
        return Err("the matrix is not finite".into());
    }
    let name = string(memory, begin + layout::ENTITY_NAME, MAX_TEXT, "the name")?;
    let (at, count) = vector(
        &head,
        layout::TO_REMOVE,
        4,
        MAX_REMOVED,
        "the entities removed",
    )?;
    let removed = ids(memory, at, count, 4, 0, "the entities removed")?;
    let (_, added_nodes) = vector(
        &head,
        layout::ADDED_NODES,
        layout::NODE_SIZE,
        MAX_STREET,
        "the nodes added",
    )?;
    let (_, added_segments) = vector(
        &head,
        layout::ADDED_SEGMENTS,
        layout::SEGMENT_SIZE,
        MAX_STREET,
        "the segments added",
    )?;
    let (at, count) = vector(
        &head,
        layout::REMOVED_NODES,
        layout::NODE_SIZE,
        MAX_STREET,
        "the nodes removed",
    )?;
    let removed_nodes = ids(memory, at, count, layout::NODE_SIZE, 0, "the nodes removed")?;
    let (at, count) = vector(
        &head,
        layout::REMOVED_SEGMENTS,
        layout::SEGMENT_SIZE,
        MAX_STREET,
        "the segments removed",
    )?;
    let removed_segments = ids(
        memory,
        at,
        count,
        layout::SEGMENT_SIZE,
        0,
        "the segments removed",
    )?;
    let (_, edge_objects_added) = vector(
        &head,
        layout::EDGE_OBJECTS_TO_ADD,
        layout::EDGE_OBJECT_SIZE,
        MAX_EDGE_OBJECTS,
        "the edge objects added",
    )?;
    let (_, edge_objects_removed) = vector(
        &head,
        layout::EDGE_OBJECTS_TO_REMOVE,
        4,
        MAX_EDGE_OBJECTS,
        "the edge objects removed",
    )?;
    Ok(Edit {
        file,
        params,
        transf,
        name,
        added,
        removed,
        added_nodes,
        added_segments,
        removed_nodes,
        removed_segments,
        edge_objects_added,
        edge_objects_removed,
    })
}

fn key(name: &str) -> LuaValue {
    LuaValue::string(name)
}

fn list(items: impl IntoIterator<Item = LuaValue>) -> LuaValue {
    LuaValue::Table(
        items
            .into_iter()
            .enumerate()
            .map(|(i, item)| (LuaValue::Integer(i as i64 + 1), item))
            .collect(),
    )
}

fn entity_record(entity: i32) -> LuaValue {
    LuaValue::Table(vec![(key("entity"), LuaValue::Integer(i64::from(entity)))])
}

impl Edit {
    /// The edit as game scripts see a proposal (`builder.proposalCreate`'s
    /// `param[1]`), as far as the mod reads one (`capture.construction`):
    /// `toRemove`, `toAdd[1]` with `fileName`, `name`, `params` and
    /// `transf`, and the street part's lists (removed nodes and segments by
    /// entity; what is added only counted, as empty records).
    pub fn to_lua(&self) -> LuaValue {
        let placeholders = |n: usize| list((0..n).map(|_| LuaValue::Table(Vec::new())));
        let construction = LuaValue::Table(vec![
            (key("fileName"), LuaValue::string(&self.file)),
            (key("name"), LuaValue::string(&self.name)),
            (key("params"), LuaValue::Table(self.params.clone())),
            (
                key("transf"),
                list(self.transf.iter().map(|v| LuaValue::Number(f64::from(*v)))),
            ),
        ]);
        let mut to_add = vec![construction];
        to_add.extend((1..self.added).map(|_| LuaValue::Table(Vec::new())));
        let street = LuaValue::Table(vec![
            (key("addedNodes"), placeholders(self.added_nodes)),
            (key("addedSegments"), placeholders(self.added_segments)),
            (
                key("removedNodes"),
                list(self.removed_nodes.iter().map(|e| entity_record(*e))),
            ),
            (
                key("removedSegments"),
                list(self.removed_segments.iter().map(|e| entity_record(*e))),
            ),
            (
                key("edgeObjectsToAdd"),
                placeholders(self.edge_objects_added),
            ),
            (
                key("edgeObjectsToRemove"),
                placeholders(self.edge_objects_removed),
            ),
        ]);
        LuaValue::Table(vec![
            (
                key("toRemove"),
                list(
                    self.removed
                        .iter()
                        .map(|e| LuaValue::Integer(i64::from(*e))),
                ),
            ),
            (key("toAdd"), list(to_add)),
            (key("proposal"), street),
        ])
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        format!(
            "{} at ({:.1},{:.1},{:.1}), {} parameter(s), name {:?}; {} added, removes {:?}; \
             street: {} node(s) and {} segment(s) added, nodes {:?} and segments {:?} removed; \
             edge objects {} added, {} removed",
            if self.file.is_empty() {
                "<no file>"
            } else {
                &self.file
            },
            self.transf[12],
            self.transf[13],
            self.transf[14],
            self.params.len(),
            self.name,
            self.added,
            self.removed,
            self.added_nodes,
            self.added_segments,
            self.removed_nodes,
            self.removed_segments,
            self.edge_objects_added,
            self.edge_objects_removed,
        )
    }
}

/// Where `Add` returns to from the module editor's call; 0 when the profile
/// does not name it.
static MODULE_RETURN: AtomicUsize = AtomicUsize::new(0);

/// The builds read, by the click count they were queued at, oldest first.
static KEPT_BUILDS: Mutex<VecDeque<(u64, Result<LuaValue, String>)>> = Mutex::new(VecDeque::new());

/// Notes the module editor's call of `Add`, at `call` (its absolute
/// address): `Add` returns 5 bytes past it.
pub fn set_call(call: usize) {
    MODULE_RETURN.store(call + 5, Ordering::Release);
}

/// Whether a call of `Add` returning to `return_address` is the module
/// editor's.
pub fn is_module_editor(return_address: usize) -> bool {
    let module = MODULE_RETURN.load(Ordering::Acquire);
    module != 0 && return_address == module
}

/// Keeps what the module editor queued at click `click` (the count before
/// it): the proposal read, or why it does not read. Logs one line.
pub fn record(memory: &dyn Memory, click: u64, payload: usize) {
    let read = decode(memory, payload);
    match &read {
        Ok(edit) => crate::log::line(&format!(
            "module editor: click {click} queued {}",
            edit.summary()
        )),
        Err(why) => crate::log::line(&format!(
            "module editor: click {click} does not read: {why}"
        )),
    }
    keep(click, read.map(|edit| edit.to_lua()));
}

/// Keeps `build` for click `click`, the newest [`KEPT`] only.
pub(crate) fn keep(click: u64, build: Result<LuaValue, String>) {
    let mut kept = KEPT_BUILDS.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|(n, _)| *n != click);
    kept.push_back((click, build));
    while kept.len() > KEPT {
        kept.pop_front();
    }
}

/// Serialises the tests that use the kept builds.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// The build read natively at click `click`, once: the module editor's, or
/// a terrain tool's ([`crate::terrain`]): `Some(Ok(table))`, `Some(Err(why))`
/// when it did not read, or `None` when the click was neither (or was taken
/// already).
pub fn take(click: u64) -> Option<Result<LuaValue, String>> {
    let mut kept = KEPT_BUILDS.lock().unwrap_or_else(PoisonError::into_inner);
    let at = kept.iter().position(|(n, _)| *n == click)?;
    kept.remove(at).map(|(_, build)| build)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory laid out by a test: regions by base address.
    #[derive(Default)]
    struct Fake {
        regions: Vec<(usize, Vec<u8>)>,
        next: usize,
    }

    impl Memory for Fake {
        fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            self.regions.iter().find_map(|(base, bytes)| {
                let offset = address.checked_sub(*base)?;
                bytes
                    .get(offset..offset.checked_add(len)?)
                    .map(<[u8]>::to_vec)
            })
        }
    }

    impl Fake {
        fn new() -> Self {
            Self {
                regions: Vec::new(),
                next: 0x100_0000,
            }
        }
        fn alloc(&mut self, bytes: Vec<u8>) -> usize {
            let at = self.next;
            self.next += ((bytes.len() + 0xfff) & !0xfff) + 0x1000;
            self.regions.push((at, bytes));
            at
        }
        fn bytes(&mut self, at: usize) -> &mut Vec<u8> {
            &mut self.regions.iter_mut().find(|(b, _)| *b == at).unwrap().1
        }
    }

    fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn put_i32(bytes: &mut [u8], at: usize, value: i32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_string(fake: &mut Fake, bytes: &mut [u8], at: usize, text: &str) {
        if text.len() < 16 {
            bytes[at..at + text.len()].copy_from_slice(text.as_bytes());
            put_u64(bytes, at + 0x18, 0xf);
        } else {
            let heap = fake.alloc(text.as_bytes().to_vec());
            put_u64(bytes, at, heap as u64);
            put_u64(bytes, at + 0x18, text.len() as u64 | 0xf);
        }
        put_u64(bytes, at + 0x10, text.len() as u64);
    }

    #[derive(Clone)]
    enum V {
        B(bool),
        N(f64),
        S(&'static str),
        T(Vec<(V, V)>),
        Nil,
    }

    fn put_variant(fake: &mut Fake, bytes: &mut [u8], at: usize, value: &V) {
        match value {
            V::Nil => bytes[at + 0x20] = 0,
            V::B(b) => {
                bytes[at] = u8::from(*b);
                bytes[at + 0x20] = 1;
            }
            V::N(n) => {
                bytes[at..at + 8].copy_from_slice(&n.to_le_bytes());
                bytes[at + 0x20] = 2;
            }
            V::S(s) => {
                put_string(fake, bytes, at, s);
                bytes[at + 0x20] = 3;
            }
            V::T(entries) => {
                let table = lay_table(fake, entries);
                put_u64(bytes, at, table as u64);
                bytes[at + 0x20] = 4;
            }
        }
    }

    /// A leaf of up to 3 entries, else an internal node with one separator.
    fn lay_node(fake: &mut Fake, entries: &[(V, V)]) -> usize {
        if entries.len() <= 3 {
            let mut bytes = vec![0u8; 0x10 + entries.len().max(1) * 0x50];
            bytes[0x0a] = entries.len() as u8;
            bytes[0x0b] = entries.len().max(1) as u8;
            for (i, (k, v)) in entries.iter().enumerate() {
                put_variant(fake, &mut bytes, 0x10 + i * 0x50, k);
                put_variant(fake, &mut bytes, 0x10 + i * 0x50 + 0x28, v);
            }
            return fake.alloc(bytes);
        }
        let mid = entries.len() / 2;
        let left = lay_node(fake, &entries[..mid]);
        let right = lay_node(fake, &entries[mid + 1..]);
        let mut bytes = vec![0u8; 0x120];
        bytes[0x0a] = 1;
        put_variant(fake, &mut bytes, 0x10, &entries[mid].0);
        put_variant(fake, &mut bytes, 0x38, &entries[mid].1);
        put_u64(&mut bytes, 0x100, left as u64);
        put_u64(&mut bytes, 0x108, right as u64);
        fake.alloc(bytes)
    }

    fn lay_table(fake: &mut Fake, entries: &[(V, V)]) -> usize {
        let mut bytes = vec![0u8; 0x18];
        if !entries.is_empty() {
            let root = lay_node(fake, entries);
            put_u64(&mut bytes, 0, root as u64);
        }
        put_u64(&mut bytes, 0x10, entries.len() as u64);
        fake.alloc(bytes)
    }

    fn put_vector(bytes: &mut [u8], at: usize, base: usize, count: usize, stride: usize) {
        let end = if count == 0 { 0 } else { base + count * stride };
        put_u64(bytes, at, if count == 0 { 0 } else { base as u64 });
        put_u64(bytes, at + 8, end as u64);
        put_u64(bytes, at + 16, end as u64);
    }

    fn put_ids(fake: &mut Fake, head: &mut [u8], at: usize, ids: &[i32], stride: usize) {
        if ids.is_empty() {
            return;
        }
        let mut raw = vec![0u8; ids.len() * stride];
        for (i, id) in ids.iter().enumerate() {
            put_i32(&mut raw, i * stride, *id);
        }
        let base = fake.alloc(raw);
        put_vector(head, at, base, ids.len(), stride);
    }

    /// A bus station 77 at (80, 0, 0) given a longer platform, as the module
    /// editor would queue it: the station removed, the new one added, its
    /// own entrance (segment 6000, node 6001) removed and made again.
    fn station_edit() -> (Fake, usize) {
        let mut fake = Fake::new();
        let mut entity = vec![0u8; layout::ENTITY_SIZE];
        put_string(&mut fake, &mut entity, 0, "");
        put_string(
            &mut fake,
            &mut entity,
            0x20,
            "stations/street/modular_street_station/modular_terminal.con",
        );
        let params = vec![
            (V::S("length"), V::N(3.0)),
            (
                V::S("modules"),
                V::T(vec![(
                    V::N(12.0),
                    V::T(vec![(V::S("name"), V::S("station/platform.module"))]),
                )]),
            ),
            (V::S("seed"), V::N(7.0)),
            (V::S("lit"), V::B(true)),
            (V::S("scale"), V::N(1.5)),
        ];
        let table = lay_table(&mut fake, &params);
        let table_bytes = fake.read(table, 0x18).unwrap();
        entity[layout::ENTITY_PARAMS..layout::ENTITY_PARAMS + 0x18].copy_from_slice(&table_bytes);
        let m: [f32; 16] = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 80.0, 0.0, 0.0, 1.0,
        ];
        for (i, v) in m.iter().enumerate() {
            let at = layout::ENTITY_TRANSF + 4 * i;
            entity[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        put_string(&mut fake, &mut entity, layout::ENTITY_NAME, "");
        let entity_at = fake.alloc(entity);
        let mut head = vec![0u8; layout::HEAD_LEN];
        put_vector(&mut head, layout::TO_ADD, entity_at, 1, layout::ENTITY_SIZE);
        put_ids(&mut fake, &mut head, layout::TO_REMOVE, &[77], 4);
        put_ids(
            &mut fake,
            &mut head,
            layout::REMOVED_SEGMENTS,
            &[6000],
            layout::SEGMENT_SIZE,
        );
        put_ids(
            &mut fake,
            &mut head,
            layout::REMOVED_NODES,
            &[6001],
            layout::NODE_SIZE,
        );
        put_ids(
            &mut fake,
            &mut head,
            layout::ADDED_SEGMENTS,
            &[-1],
            layout::SEGMENT_SIZE,
        );
        let payload = fake.alloc(head);
        (fake, payload)
    }

    #[test]
    fn a_module_edit_reads_as_the_proposal_game_scripts_would_see() {
        let (fake, payload) = station_edit();
        let edit = decode(&fake, payload).unwrap();
        assert_eq!(
            edit.file,
            "::/stations/street/modular_street_station/modular_terminal.con"
        );
        assert_eq!(edit.removed, [77]);
        assert_eq!(edit.removed_segments, [6000]);
        assert_eq!(edit.removed_nodes, [6001]);
        assert_eq!((edit.added_nodes, edit.added_segments), (0, 1));
        assert_eq!(edit.transf[12], 80.0);
        assert_eq!(edit.params.len(), 5, "every entry, through the tree");
        let lua = edit.to_lua();
        let first = |v: &LuaValue| match v {
            LuaValue::Table(entries) => entries[0].1.clone(),
            _ => panic!(),
        };
        let to_add = first(lua.get("toAdd").unwrap());
        assert_eq!(
            to_add.get("fileName"),
            Some(&LuaValue::string(
                "::/stations/street/modular_street_station/modular_terminal.con"
            ))
        );
        assert_eq!(
            first(lua.get("toRemove").unwrap()),
            LuaValue::Integer(77),
            "the construction the edit removes, by this game's entity"
        );
        let street = lua.get("proposal").unwrap();
        assert_eq!(
            first(street.get("removedSegments").unwrap()).get("entity"),
            Some(&LuaValue::Integer(6000))
        );
        assert!(
            edit.summary().contains("removes [77]"),
            "{}",
            edit.summary()
        );
    }

    #[test]
    fn what_does_not_read_is_a_reason_not_a_guess() {
        let (mut fake, payload) = station_edit();
        // The parameter table claims one entry more than its tree holds.
        let head = fake.read(payload, layout::HEAD_LEN).unwrap();
        let entity = u64_at(&head, layout::TO_ADD) as usize;
        let bytes = fake.bytes(entity);
        let size = u64_at(bytes, layout::ENTITY_PARAMS + 0x10);
        put_u64(bytes, layout::ENTITY_PARAMS + 0x10, size + 1);
        assert!(decode(&fake, payload).unwrap_err().contains("walked to"));
        // A nil parameter cannot travel.
        let mut fake = Fake::new();
        let table = lay_table(&mut fake, &[(V::S("x"), V::Nil)]);
        let mut walk = Walk {
            entries: 0,
            open: Vec::new(),
        };
        assert!(
            table_at(&fake, table, 0, &mut walk)
                .unwrap_err()
                .contains("variant tag 0")
        );
        // Vectors out of order, and memory that does not read.
        let (mut fake, payload) = station_edit();
        let head = fake.bytes(payload);
        put_u64(head, layout::TO_REMOVE + 8, 1);
        assert!(decode(&fake, payload).unwrap_err().contains("out of order"));
        assert!(decode(&Fake::new(), 0x1234).is_err());
        // A proposal that adds nothing.
        let mut fake = Fake::new();
        let payload = fake.alloc(vec![0u8; layout::HEAD_LEN]);
        assert_eq!(
            decode(&fake, payload).unwrap_err(),
            "the edit adds no construction"
        );
    }

    #[test]
    fn a_click_is_taken_once_and_only_the_module_editors_are_kept() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        set_call(0x1_4054_3b25);
        assert!(is_module_editor(0x1_4054_3b2a));
        assert!(!is_module_editor(0x1_4051_c11c), "the construction tool's");
        assert!(!is_module_editor(0));
        let (fake, payload) = station_edit();
        record(&fake, 41, payload);
        record(&Fake::new(), 42, 0x1234);
        assert!(take(40).is_none(), "not the module editor's click");
        assert!(matches!(take(41), Some(Ok(LuaValue::Table(_)))));
        assert!(take(41).is_none(), "taken once");
        assert!(matches!(take(42), Some(Err(why)) if why.contains("does not read")));
        for click in 100..100 + KEPT as u64 + 4 {
            keep(click, Err("x".into()));
        }
        assert!(take(100).is_none(), "only the newest are kept");
        assert!(take(100 + KEPT as u64 + 3).is_some());
    }
}
