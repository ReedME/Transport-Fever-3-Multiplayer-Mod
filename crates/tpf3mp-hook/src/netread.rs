//! The rolling world checks' static lanes read natively, a part at a time
//! (docs/HOOKS.md, "The network lane read natively"): the rows
//! `tpf3mp/lanes.lua` builds from the street and track edges, junctions and
//! constructions, the same text, from the engine's memory instead of a
//! `getComponent` call and a dozen field reads an object ([`read_part`]).
//!
//! [`ENV`] sets it: off (the default), `on`, where a room's rolling history
//! is kept in native parts, or `compare`, where the mod also reads each
//! part in Lua, compares the rows and hashes its own. The full readers
//! ([`network`], [`construction_rows`]) remain for the tests that hold the
//! rows to the mod's Lua.
//!
//! Where it reads (investigation/TF3_NATIVE_NETWORK_2026-10-04.md; the
//! offsets are the build's, in its native bundle):
//!
//! - the engine, `[CGameTime+8]`, from the `CGameTime` the game's own step
//!   called its speed getter on; only while that step runs, so only inside
//!   the game script's `postUpdate`, where the mod reads its lanes, and
//!   never on another thread's call;
//! - the `BaseEdge` pool, the one of the engine's pools whose vtable is
//!   `CompVec<BaseEdge>`'s, its type id its index there (and its own
//!   record of it, which must agree);
//! - every entity whose component bits hold that type id, its data index
//!   from its component list, its `BaseEdge` from the pool's dense vector
//!   or its pages.
//!
//! Fail closed: anything that does not read as the layout says (a pointer
//! out of order, a count past its bound, an entity whose bits and list
//! disagree, a number that is not finite) fails the whole read with why,
//! and the room's history of parts holds. Nothing is written, nothing of
//! the game's is called.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::build_data::native::netread as layout;
use crate::modules::{Memory, i32_at, res_name, u64_at};

/// `off` (unset), `compare` or `on`.
pub const ENV: &str = "TPF3MP_HOOK_NATIVE_LANES";

/// Entities looked up together ([`Store::locate`]).
const BUNCH: usize = 64;

/// An entity and its data index of each kind looked up ([`Store::locate`]).
type Located<const N: usize> = (usize, [Option<i32>; N]);

/// A hint to the processor to fetch the cache line at `address` before it
/// is read: it reads nothing into the program and cannot fault, whatever
/// the address.
#[inline]
fn prefetch(address: usize) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: a prefetch is only a hint to the cache; it never faults, and
    // nothing is read from it.
    unsafe {
        std::arch::x86_64::_mm_prefetch::<{ std::arch::x86_64::_MM_HINT_T0 }>(address as *const i8);
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = address;
}

/// Most component pools, entities, lane configs an edge and components an
/// entity read; past any of them the read fails.
pub const MAX_POOLS: usize = 4096;
pub const MAX_ENTITIES: usize = 1 << 23;
pub const MAX_LANES: usize = 256;
pub const MAX_COMPONENTS: usize = 256;
/// Type ids the component bits hold: 16 bytes an entity.
const MAX_TYPE: usize = layout::BITS_PER_ENTITY * 8;

/// What [`ENV`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Compare,
    On,
}

impl Mode {
    /// From [`ENV`]'s value; anything else than `compare` or `on` is off,
    /// with why when it was set to something.
    pub fn from_env(value: Option<&str>) -> (Self, Option<String>) {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            None => (Self::Off, None),
            Some(v) if v.eq_ignore_ascii_case("compare") => (Self::Compare, None),
            Some(v) if v.eq_ignore_ascii_case("on") => (Self::On, None),
            Some(v) if v.eq_ignore_ascii_case("off") => (Self::Off, None),
            Some(v) => (
                Self::Off,
                Some(format!(
                    "{ENV}={v} is none of off, compare and on; the native network read is off"
                )),
            ),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Compare => "compare",
            Self::On => "on",
        }
    }
}

/// The same validated contract the launcher advertises at create/join.
pub fn configured_plan() -> Result<tpf3mp_proto::ScanPlan, &'static str> {
    static CONFIG: OnceLock<Result<tpf3mp_proto::ScanPlan, &'static str>> = OnceLock::new();
    *CONFIG.get_or_init(tpf3mp_proto::ScanPlan::from_environment)
}

pub fn mode() -> Mode {
    match configured_plan().map(|plan| plan.mode) {
        Ok(tpf3mp_proto::ScanMode::NativeCompareV2) => Mode::Compare,
        Ok(tpf3mp_proto::ScanMode::NativeV2) => Mode::On,
        _ => Mode::Off,
    }
}

/// The running game's memory, each range checked before it is copied,
/// through a cache of its own: the read jumps between many more heap
/// regions (the components' strings, lane configs and lists) than the
/// shared per-thread cache keeps ([`crate::image::Readable`], 8), and each
/// region it forgets is a `VirtualQuery` again. One lives for one read, in
/// which the engine frees nothing.
#[derive(Default)]
pub struct Process {
    /// Readable regions `[base, end)`, sorted and apart.
    regions: std::cell::RefCell<Vec<(usize, usize)>>,
    /// The region the last check found, asked first: reads come in runs
    /// through the same table or pool.
    last: std::cell::Cell<(usize, usize)>,
    /// Checks made and regions asked of the system, for the log.
    checks: std::cell::Cell<usize>,
    queries: std::cell::Cell<usize>,
}

impl Process {
    pub fn new() -> Self {
        Self::default()
    }

    /// The checks made and the regions asked of the system so far.
    pub fn counts(&self) -> (usize, usize) {
        (self.checks.get(), self.queries.get())
    }

    /// Whether `len` bytes at `address` are readable, asking the system
    /// only for the parts no region known covers.
    fn readable(&self, address: usize, len: usize) -> bool {
        let Some(end) = address.checked_add(len) else {
            return false;
        };
        self.checks.set(self.checks.get() + 1);
        let (lo, hi) = self.last.get();
        if lo <= address && end <= hi {
            return true;
        }
        let mut regions = self.regions.borrow_mut();
        let mut at = address;
        while at < end {
            // The last region beginning at or before `at`.
            let i = regions.partition_point(|(base, _)| *base <= at);
            if i > 0 && at < regions[i - 1].1 {
                if at == address {
                    self.last.set(regions[i - 1]);
                }
                at = regions[i - 1].1;
                continue;
            }
            self.queries.set(self.queries.get() + 1);
            let Some((base, region_end)) = crate::image::region(at) else {
                return false;
            };
            if base > at || region_end <= at {
                return false;
            }
            // Merge it with every known region it touches.
            let (mut lo, mut hi) = (base, region_end);
            regions.retain(|(b, e)| {
                let apart = *e < lo || hi < *b;
                if !apart {
                    lo = lo.min(*b);
                    hi = hi.max(*e);
                }
                apart
            });
            let j = regions.partition_point(|(b, _)| *b < lo);
            regions.insert(j, (lo, hi));
            if lo <= address {
                self.last.set((lo, hi));
            }
            at = hi;
        }
        true
    }
}

impl Memory for Process {
    fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
        let mut bytes = vec![0u8; len];
        self.read_into(address, &mut bytes).then_some(bytes)
    }

    fn read_into(&self, address: usize, out: &mut [u8]) -> bool {
        if address == 0 || cfg!(not(windows)) || !self.readable(address, out.len()) {
            return false;
        }
        // SAFETY: `out.len()` bytes at `address` are committed readable
        // memory, checked just above in this read, in which the engine
        // frees nothing, and `out` has room for them.
        unsafe { std::ptr::copy_nonoverlapping(address as *const u8, out.as_mut_ptr(), out.len()) };
        true
    }
}

/// The network lane of the world the game's step is running now, from the
/// game's memory: `Err(why)` outside the step, without the image, or when
/// the edges do not read.
pub fn read_now() -> Result<Network, String> {
    let (memory, engine, image) = engine_now()?;
    network(&memory, engine, image)
}

/// The memory, the engine and the image of the game's step running now.
fn engine_now() -> Result<(Process, usize, usize), String> {
    let game_time = crate::install::game_time_now();
    if game_time == 0 {
        return Err("no game step is running".into());
    }
    let image = image_base();
    if image == 0 {
        return Err("the game's image was not found".into());
    }
    let memory = Process::new();
    let head = read(
        &memory,
        game_time,
        layout::GAME_TIME_ENGINE + 8,
        "the CGameTime",
    )?;
    let engine = usize::try_from(u64_at(&head, layout::GAME_TIME_ENGINE))
        .map_err(|_| "the engine's address".to_string())?;
    Ok((memory, engine, image))
}

#[cfg(windows)]
fn image_base() -> usize {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    static BASE: OnceLock<usize> = OnceLock::new();
    // SAFETY: a null name asks for the process's own executable, which
    // stays loaded for the life of the process.
    *BASE.get_or_init(|| unsafe { GetModuleHandleW(std::ptr::null()) } as usize)
}

#[cfg(not(windows))]
fn image_base() -> usize {
    0
}

/// `len` bytes at `address`, or why not; nothing to read for none.
fn read(memory: &dyn Memory, address: usize, len: usize, what: &str) -> Result<Vec<u8>, String> {
    if len == 0 {
        return Ok(Vec::new());
    }
    crate::modules::read(memory, address, len, what)
}

/// `N` bytes at `address`, or why not, without allocating.
fn read_array<const N: usize>(
    memory: &dyn Memory,
    address: usize,
    what: &str,
) -> Result<[u8; N], String> {
    let mut out = [0u8; N];
    if memory.read_into(address, &mut out) {
        Ok(out)
    } else {
        Err(format!("{what} does not read"))
    }
}

/// A `std::vector`'s `{begin, end}` at `offset` of `head`: its begin and
/// its count of `stride`-byte elements, at most `max`.
fn vector(
    head: &[u8],
    offset: usize,
    stride: usize,
    max: usize,
    what: &str,
) -> Result<(usize, usize), String> {
    let begin = u64_at(head, offset);
    let end = u64_at(head, offset + 8);
    if end < begin {
        return Err(format!("{what}: its end is before its begin"));
    }
    if begin == 0 && end != 0 {
        return Err(format!("{what}: no begin but an end"));
    }
    let bytes = usize::try_from(end - begin).map_err(|_| format!("{what}: its size"))?;
    if bytes % stride != 0 {
        return Err(format!(
            "{what}: a size that is not a whole number of elements"
        ));
    }
    let count = bytes / stride;
    if count > max {
        return Err(format!("{what}: {count} elements, more than {max}"));
    }
    let begin = usize::try_from(begin).map_err(|_| format!("{what}: its begin"))?;
    Ok((begin, count))
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    f32::from_le_bytes(word)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(word)
}

/// A component pool: where its elements of `size` bytes are.
struct Pool {
    dense: (usize, usize),
    pages: (usize, usize),
    size: usize,
}

impl Pool {
    fn read(memory: &dyn Memory, pool: usize, size: usize, what: &str) -> Result<Self, String> {
        let head = read(memory, pool, layout::POOL_HEAD, what)?;
        let dense = vector(&head, layout::POOL_DENSE, size, MAX_ENTITIES, what)?;
        let pages = vector(
            &head,
            layout::POOL_PAGES,
            layout::PAGE_ENTRY,
            MAX_ENTITIES,
            what,
        )?;
        Ok(Self { dense, pages, size })
    }

    /// The address of the element at data index `index`.
    fn element(&self, memory: &dyn Memory, index: i32) -> Result<usize, String> {
        let index = u32::try_from(index).map_err(|_| format!("a negative data index {index}"))?;
        if index < layout::PAGED_FROM {
            let i = index as usize;
            if i >= self.dense.1 {
                return Err(format!(
                    "data index {i} past the pool's {} elements",
                    self.dense.1
                ));
            }
            return Ok(self.dense.0 + i * self.size);
        }
        let i = (index - layout::PAGED_FROM) as usize;
        let page = i / layout::PAGE_SLOTS;
        // The game's own accessor (`sub_2806a0`) bounds nothing; the page
        // table's length does here.
        if page >= self.pages.1 {
            return Err(format!("paged slot {i} past the pool's pages"));
        }
        let entry: [u8; 8] = read_array(
            memory,
            self.pages.0 + page * layout::PAGE_ENTRY,
            "a pool page",
        )?;
        let data =
            usize::try_from(u64_at(&entry, 0)).map_err(|_| "a pool page's address".to_string())?;
        if data == 0 {
            return Err(format!("paged slot {i} on a page that is not there"));
        }
        Ok(data + (i % layout::PAGE_SLOTS) * self.size)
    }
}

/// The type id of the pool whose vtable is at `vtable`: its index among
/// the engine's pools (`sub_94770` appends a type's pool as it registers
/// the type with the id `pools.size() + 1`, stored less one). Every entity
/// read through it must list that id ([`Store::index`]).
fn type_id(
    memory: &dyn Memory,
    engine: usize,
    vtable: usize,
    what: &str,
) -> Result<(usize, usize), String> {
    find_type(memory, engine, vtable, what, false)?.ok_or_else(|| format!("no pool of {what}"))
}

/// [`type_id`], or `None` where the engine has no pool of that type: one
/// it registers at the type's first component, so a world with none of
/// them may have no pool. `strict`, a pool whose head does not read fails
/// the search instead of being passed over: an absence is then certain.
fn find_type(
    memory: &dyn Memory,
    engine: usize,
    vtable: usize,
    what: &str,
    strict: bool,
) -> Result<Option<(usize, usize)>, String> {
    let head = read(memory, engine + layout::POOLS, 16, "the engine's pools")?;
    let (begin, count) = vector(&head, 0, 8, MAX_POOLS, "the engine's pools")?;
    let pools = read(memory, begin, count * 8, "the engine's pools")?;
    let mut found = None;
    for i in 0..count {
        let pool = usize::try_from(u64_at(&pools, i * 8)).unwrap_or(0);
        if pool == 0 {
            continue;
        }
        let Some(head) = memory.read(pool, 16) else {
            if strict {
                return Err(format!("pool {i} does not read, looking for {what}"));
            }
            continue;
        };
        if usize::try_from(u64_at(&head, 0)).ok() == Some(vtable) {
            if found.is_some() {
                return Err(format!("two pools of {what}"));
            }
            found = Some((i, pool));
        }
    }
    let Some((id, pool)) = found else {
        return Ok(None);
    };
    if id >= MAX_TYPE {
        return Err(format!("{what}'s type id {id} is past the component bits"));
    }
    Ok(Some((id, pool)))
}

/// Entities a scan reads the component bits of at once: 16 KiB, which
/// stays in the processor's first cache while it is looked through.
const CHUNK: usize = 1 << 10;

/// The engine's entities: their table and their component bits.
struct Store<'m> {
    memory: &'m dyn Memory,
    engine: usize,
    image: usize,
    records: usize,
    entities: usize,
    bits: usize,
}

/// One component type: its id and its pool.
struct Kind {
    id: usize,
    pool: Pool,
    name: &'static str,
}

impl<'m> Store<'m> {
    fn new(memory: &'m dyn Memory, engine: usize, image: usize) -> Result<Self, String> {
        let head = read(memory, engine, layout::BITS + 8, "the engine")?;
        let (records, entities) = vector(
            &head,
            layout::ENTITIES,
            layout::ENTITY_RECORD,
            MAX_ENTITIES,
            "the entity table",
        )?;
        let bits = usize::try_from(u64_at(&head, layout::BITS))
            .map_err(|_| "the component bits".to_string())?;
        if entities > 0 && bits == 0 {
            return Err("entities without component bits".into());
        }
        Ok(Self {
            memory,
            engine,
            image,
            records,
            entities,
            bits,
        })
    }

    /// The component type whose pool's vtable is at `vtable` (an RVA), its
    /// elements `size` bytes.
    fn kind(&self, vtable: usize, size: usize, name: &'static str) -> Result<Kind, String> {
        let (id, pool) = type_id(self.memory, self.engine, self.image + vtable, name)?;
        let pool = Pool::read(self.memory, pool, size, name)?;
        Ok(Kind { id, pool, name })
    }

    /// [`Store::kind`], or `None` where the engine has no pool of it yet.
    fn kind_if_any(
        &self,
        vtable: usize,
        size: usize,
        name: &'static str,
    ) -> Result<Option<Kind>, String> {
        let Some((id, pool)) =
            find_type(self.memory, self.engine, self.image + vtable, name, true)?
        else {
            return Ok(None);
        };
        let pool = Pool::read(self.memory, pool, size, name)?;
        Ok(Some(Kind { id, pool, name }))
    }

    /// The data index of `kind` in `entity`'s component list: `None` for a
    /// removed entity, `Err` when the list lacks it or lists it twice.
    fn index(&self, entity: usize, kind: &Kind) -> Result<Option<i32>, String> {
        let Some([index]) = self.indices(entity, [kind])? else {
            return Ok(None);
        };
        index
            .map(Some)
            .ok_or_else(|| format!("entity {entity} has the {0} bit but no {0}", kind.name))
    }

    /// The data index of each of `kinds` in `entity`'s component list, from
    /// one read of it: `None` for a removed entity, each `None` where the
    /// list lacks that kind, `Err` when it lists one twice.
    fn indices<const N: usize>(
        &self,
        entity: usize,
        kinds: [&Kind; N],
    ) -> Result<Option<[Option<i32>; N]>, String> {
        let list = self.list_of(entity)?;
        self.indices_in(entity, list, kinds)
    }

    /// Where `entity`'s component list is, and how many pairs it holds.
    fn list_of(&self, entity: usize) -> Result<(usize, usize), String> {
        let record: [u8; layout::ENTITY_RECORD] = read_array(
            self.memory,
            self.records + entity * layout::ENTITY_RECORD,
            "an entity's record",
        )?;
        vector(
            &record,
            0,
            layout::COMPONENT_PAIR,
            MAX_COMPONENTS,
            "an entity's components",
        )
    }

    /// [`Store::indices`] from `entity`'s list at `(pairs, count)`.
    fn indices_in<const N: usize>(
        &self,
        entity: usize,
        (pairs, count): (usize, usize),
        kinds: [&Kind; N],
    ) -> Result<Option<[Option<i32>; N]>, String> {
        // Most entities list a few components: no heap and little to clear.
        let mut small = [0u8; 32 * layout::COMPONENT_PAIR];
        let mut large = Vec::new();
        let list: &mut [u8] = if count <= 32 {
            &mut small[..count * layout::COMPONENT_PAIR]
        } else {
            large.resize(count * layout::COMPONENT_PAIR, 0);
            &mut large
        };
        if count > 0 && !self.memory.read_into(pairs, list) {
            return Err("an entity's components does not read".into());
        }
        let list = &*list;
        // A removed entity keeps one pair {-1, -1} (`sub_4f7db0`).
        if count == 1 && i32_at(list, 0) < 0 {
            return Ok(None);
        }
        let mut found = [None; N];
        for p in 0..count {
            let id = i32_at(list, p * layout::COMPONENT_PAIR);
            for (kind, index) in kinds.iter().zip(found.iter_mut()) {
                if usize::try_from(id).ok() == Some(kind.id) {
                    if index.is_some() {
                        return Err(format!("entity {entity} lists {} twice", kind.name));
                    }
                    *index = Some(i32_at(list, p * layout::COMPONENT_PAIR + 4));
                }
            }
        }
        Ok(Some(found))
    }

    /// Each of `entities`' data indices of `kinds`, as [`Store::indices`]
    /// finds them, removed entities left out: read a bunch at a time, the
    /// bunch's records asked of the processor first, then their component
    /// lists, so that their waits for memory overlap instead of following
    /// one another.
    fn locate<const N: usize>(
        &self,
        entities: &[usize],
        kinds: [&Kind; N],
    ) -> Result<Vec<Located<N>>, String> {
        let mut out = Vec::with_capacity(entities.len());
        let mut lists = [(0usize, 0usize); BUNCH];
        for bunch in entities.chunks(BUNCH) {
            for &entity in bunch {
                prefetch(self.records + entity * layout::ENTITY_RECORD);
            }
            for (list, &entity) in lists.iter_mut().zip(bunch) {
                *list = self.list_of(entity)?;
                prefetch(list.0);
            }
            for (list, &entity) in lists.iter().zip(bunch) {
                if let Some(found) = self.indices_in(entity, *list, kinds)? {
                    out.push((entity, found));
                }
            }
        }
        Ok(out)
    }

    /// Whether `entity`'s component bits hold `kind`.
    fn has(&self, entity: usize, kind: &Kind) -> Result<bool, String> {
        if entity >= self.entities {
            return Ok(false);
        }
        let word: [u8; 8] = read_array(
            self.memory,
            self.bits + entity * layout::BITS_PER_ENTITY + (kind.id / 64) * 8,
            "the component bits",
        )?;
        Ok(u64_at(&word, 0) >> (kind.id % 64) & 1 == 1)
    }

    /// `entity`'s `kind` into `out` (its first `out.len()` bytes): `false`
    /// when it has none or is removed.
    fn component_into(&self, entity: usize, kind: &Kind, out: &mut [u8]) -> Result<bool, String> {
        if out.len() > kind.pool.size {
            return Err(format!("more of {} than it has", kind.name));
        }
        if !self.has(entity, kind)? {
            return Ok(false);
        }
        let Some(index) = self.index(entity, kind)? else {
            return Ok(false);
        };
        let at = kind.pool.element(self.memory, index)?;
        if !self.memory.read_into(at, out) {
            return Err(format!("{} does not read", kind.name));
        }
        Ok(true)
    }

    /// Every entity whose component bits hold `kind`, in id order.
    fn with(&self, kind: &Kind) -> Result<Vec<usize>, String> {
        let [out] = self.with_each([kind])?;
        Ok(out)
    }

    /// For each of `kinds`, every entity whose component bits hold it, in
    /// id order: one pass over the bits for all.
    fn with_each<const N: usize>(&self, kinds: [&Kind; N]) -> Result<[Vec<usize>; N], String> {
        let (lists, _) = self.with_all(&kinds)?;
        let mut lists = lists.into_iter();
        Ok(std::array::from_fn(|_| lists.next().unwrap_or_default()))
    }

    /// [`Store::with_each`] for as many kinds as `kinds` holds.
    /// The count is entity slots whose component bits were examined, whether
    /// or not any requested kind was present.
    fn with_all(&self, kinds: &[&Kind]) -> Result<(Vec<Vec<usize>>, usize), String> {
        let at: Vec<(usize, usize)> = kinds
            .iter()
            .map(|kind| ((kind.id / 64) * 8, kind.id % 64))
            .collect();
        let mut out: Vec<Vec<usize>> = vec![Vec::new(); kinds.len()];
        let mut bits = vec![0u8; CHUNK * layout::BITS_PER_ENTITY];
        let mut first = 0;
        let mut scanned_entity_slots = 0;
        while first < self.entities {
            let n = CHUNK.min(self.entities - first);
            let chunk = &mut bits[..n * layout::BITS_PER_ENTITY];
            if !self
                .memory
                .read_into(self.bits + first * layout::BITS_PER_ENTITY, chunk)
            {
                return Err("the component bits does not read".into());
            }
            for i in 0..n {
                let entity = &chunk[i * layout::BITS_PER_ENTITY..(i + 1) * layout::BITS_PER_ENTITY];
                for ((word, bit), list) in at.iter().zip(out.iter_mut()) {
                    if u64_at(entity, *word) >> bit & 1 == 1 {
                        list.push(first + i);
                    }
                }
            }
            first += n;
            scanned_entity_slots += n;
        }
        Ok((out, scanned_entity_slots))
    }
}

/// What a junction row needs of an edge: its nodes and its network.
#[derive(Debug, Clone, Copy)]
struct EdgeEnds {
    node0: usize,
    node1: usize,
    street: bool,
}

/// The network lane's rows, read natively: the edges' rows, and apart, the
/// junctions' (which can fail on their own).
pub struct Network {
    pub edges: Vec<String>,
    pub junctions: Result<Vec<Junction>, String>,
    /// The junctions whose rows only the game's Lua can make (see
    /// [`read_junctions`]), by node entity, for the mod to make them.
    pub deferred: Vec<usize>,
    /// What the read took, for the log: the entities, and the time of its
    /// parts.
    pub timing: String,
}

/// A junction's row but for the two names only the game's Lua gives: its
/// traffic light preference (an enum value) and its light's resource
/// (`-1` for the default). The row is `head|preference|light|tail`
/// (tpf3mp/junctions.lua, `rows`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Junction {
    pub head: String,
    pub preference: i32,
    pub light: i32,
    pub tail: String,
}

/// The edge rows of the world in `engine`, as lanes.lua makes them
/// (`lanes.edgeRow`), unsorted. `image` is where the game's executable is
/// loaded.
pub fn edge_rows(memory: &dyn Memory, engine: usize, image: usize) -> Result<Vec<String>, String> {
    let store = Store::new(memory, engine, image)?;
    let edges = store.kind(
        layout::BASE_EDGE_POOL_VTABLE,
        layout::BASE_EDGE_SIZE,
        "BaseEdge",
    )?;
    Ok(read_edges(&store, &edges)?.0)
}

/// Maps by entity: hashed by one multiply, not SipHash, for the engine's
/// own numbers, which no one chooses.
#[derive(Default, Clone, Copy)]
struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(*b)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }
    fn write_usize(&mut self, n: usize) {
        self.0 = (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

type IdMap<V> = HashMap<usize, V, std::hash::BuildHasherDefault<IdHasher>>;

type Ends = IdMap<EdgeEnds>;

/// The edges' rows and their ends, by entity.
fn read_edges(store: &Store, kind: &Kind) -> Result<(Vec<String>, Ends), String> {
    let mut rows = Vec::new();
    let mut ends = Ends::default();
    let mut memo = Memo::default();
    for entity in store.with(kind)? {
        let Some(index) = store.index(entity, kind)? else {
            continue;
        };
        let at = kind.pool.element(store.memory, index)?;
        let edge: [u8; layout::BASE_EDGE_SIZE] = read_array(store.memory, at, "a BaseEdge")?;
        rows.push(
            edge_row(store.memory, &edge, at, &mut memo)
                .map_err(|why| format!("entity {entity}: {why}"))?,
        );
        ends.insert(entity, edge_ends(&edge, entity)?);
    }
    Ok((rows, ends))
}

/// What a junction row needs of the edge `entity` (its `BaseEdge`).
fn edge_ends(edge: &[u8], entity: usize) -> Result<EdgeEnds, String> {
    let node = |offset: usize| {
        usize::try_from(i32_at(edge, offset))
            .map_err(|_| format!("entity {entity}: a negative node"))
    };
    let street = match i32_at(edge, layout::EDGE_ROAD_TYPE) {
        layout::ROAD_TYPE_STREET => true,
        layout::ROAD_TYPE_TRACK => false,
        other => return Err(format!("entity {entity}: road type {other}")),
    };
    Ok(EdgeEnds {
        node0: node(layout::EDGE_NODE0)?,
        node1: node(layout::EDGE_NODE1)?,
        street,
    })
}

/// The network lane of the world in `engine`, read natively: `Err` when
/// the edges did not read, the junctions' own `Err` when only they did not.
pub fn network(memory: &dyn Memory, engine: usize, image: usize) -> Result<Network, String> {
    let t0 = std::time::Instant::now();
    let store = Store::new(memory, engine, image)?;
    let edges = store.kind(
        layout::BASE_EDGE_POOL_VTABLE,
        layout::BASE_EDGE_SIZE,
        "BaseEdge",
    )?;
    let t1 = std::time::Instant::now();
    let (rows, ends) = read_edges(&store, &edges)?;
    let t2 = std::time::Instant::now();
    let (junctions, deferred) = match read_junctions(&store, &ends) {
        Ok((parts, deferred)) => (Ok(parts), deferred),
        Err(why) => (Err(why), Vec::new()),
    };
    let t3 = std::time::Instant::now();
    let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1000.0;
    Ok(Network {
        edges: rows,
        junctions,
        timing: format!(
            "{} entities; pools {:.1} ms, edges {:.1} ms, junctions {:.1} ms, {} junctions left to Lua",
            store.entities,
            ms(t0, t1),
            ms(t1, t2),
            ms(t2, t3),
            deferred.len()
        ),
        deferred,
    })
}

/// The constructions lane's rows, as lanes.lua makes them: each
/// construction's file and its place to 0.1 m, `file@x,y`, unsorted.
pub fn construction_rows(
    memory: &dyn Memory,
    engine: usize,
    image: usize,
) -> Result<Vec<String>, String> {
    let store = Store::new(memory, engine, image)?;
    let kind = store.kind(
        layout::CONSTRUCTION_POOL_VTABLE,
        layout::CONSTRUCTION_SIZE,
        "Construction",
    )?;
    let mut memo = Memo::default();
    let mut rows = Vec::new();
    for entity in store.with(&kind)? {
        let Some(index) = store.index(entity, &kind)? else {
            continue;
        };
        let at = kind.pool.element(memory, index)?;
        // Its file and its place, not the whole component.
        let bytes = read(memory, at, layout::CONSTRUCTION_Y + 4, "a Construction")?;
        let file = memo.name(
            memory,
            &bytes,
            layout::CONSTRUCTION_FILE,
            at + layout::CONSTRUCTION_FILE,
            "a construction's file",
        )?;
        let x = q01_text(f32_at(&bytes, layout::CONSTRUCTION_X))?;
        let y = q01_text(f32_at(&bytes, layout::CONSTRUCTION_Y))?;
        rows.push(format!("{file}@{x},{y}"));
    }
    Ok(rows)
}

/// PROTOTYPE: how many parts a room's history takes and every how many
/// updates one is read, `NxS` (default `10x1`), for measuring; a room
/// keeps what its first update found.
pub const PLAN_ENV: &str = "TPF3MP_HOOK_PARTS";

/// Parts and stride from the one cached, validated configuration.
pub fn plan() -> (u32, u32) {
    let plan = configured_plan().unwrap_or(tpf3mp_proto::ScanPlan::DEFAULT);
    (plan.parts, plan.stride)
}

/// The side of a part's cell (lanes.lua, `lanes.rowPart`): 256 m, in the
/// whole tenths of a metre edge and construction rows place their objects
/// at, and in the millimetres of a junction row's node key.
pub const CELL_TENTHS: i64 = 2_560;
pub const CELL_MM: i64 = 256_000;
/// Most parts a rolling check splits the static lanes into.
pub const MAX_PARTS: u32 = 1024;

/// The part of `n` a place `(x, y)` is in, its coordinates whole units of
/// which `side` makes a cell: cells `floor(x / side)`, `floor(y / side)`,
/// and of them `(cx + 3 cy) mod n`, never negative, so that every part has
/// cells all over the map. lanes.lua's `partOf` computes the same in
/// doubles, exact for any place a map has.
pub fn part_of(x: i64, y: i64, side: i64, n: u32) -> u32 {
    let (cx, cy) = (x.div_euclid(side), y.div_euclid(side));
    // A non-negative remainder below `n`, which fits.
    cx.wrapping_add(cy.wrapping_mul(3))
        .rem_euclid(i64::from(n.max(1))) as u32
}

/// `floor(v * 10 + 0.5)`: a value's whole tenths, as `q01` rounds it.
fn tenths(v: f32) -> Result<i64, String> {
    let t = (f64::from(v) * 10.0 + 0.5).floor();
    if !t.is_finite() || t.abs() >= 1e15 {
        return Err(format!("a place that is not on a map ({v})"));
    }
    Ok(t as i64)
}

/// A value's whole millimetres, as `pointKey`'s `%.0f` rounds it.
fn millimetres(v: f32) -> Result<i64, String> {
    let t = (f64::from(v) * 1000.0).round_ties_even();
    if !t.is_finite() || t.abs() >= 1e15 {
        return Err(format!("a place that is not on a map ({v})"));
    }
    Ok(t as i64)
}

/// The place an edge's row puts it at in whole tenths: of its two ends (to
/// 0.1 m, as the row gives them), the lower by x, then y, then z.
fn edge_anchor(edge: &[u8]) -> Result<(i64, i64), String> {
    let end = |offset: usize| -> Result<(i64, i64, i64), String> {
        Ok((
            tenths(f32_at(edge, offset))?,
            tenths(f32_at(edge, offset + 4))?,
            tenths(f32_at(edge, offset + 8))?,
        ))
    };
    let (x, y, _) = end(layout::EDGE_POSITION0)?.min(end(layout::EDGE_POSITION1)?);
    Ok((x, y))
}

/// Which objects a part reads: its edges, its junctions, its constructions,
/// or several of them (the edges and junctions are both the network lane's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kinds {
    pub edges: bool,
    pub junctions: bool,
    pub constructions: bool,
}

impl Kinds {
    pub const ALL: Self = Self {
        edges: true,
        junctions: true,
        constructions: true,
    };

    /// `edges`, `junctions`, `constructions` or `all`.
    pub fn from_name(name: &str) -> Option<Self> {
        let none = Self {
            edges: false,
            junctions: false,
            constructions: false,
        };
        match name {
            "edges" => Some(Self {
                edges: true,
                ..none
            }),
            "junctions" => Some(Self {
                junctions: true,
                ..none
            }),
            "constructions" => Some(Self {
                constructions: true,
                ..none
            }),
            "all" => Some(Self::ALL),
            _ => None,
        }
    }

    /// Whether it reads any of the network lane.
    pub fn network(self) -> bool {
        self.edges || self.junctions
    }
}

/// One part of the static lanes, read natively for a rolling check: of the
/// objects in part `k` of `n` ([`part_of`]), those of `kinds`: an edge by
/// the lower of the ends its row names, a junction by its node's place, a
/// construction by its own, each as its row rounds it.
pub struct Part {
    pub n: u32,
    pub k: u32,
    pub kinds: Kinds,
    /// The room's step it was read at, when it was read in one.
    pub step: Option<u64>,
    pub edges: Vec<String>,
    pub junctions: Vec<Junction>,
    /// The part's junctions only the game's Lua can make rows of
    /// ([`read_junctions`]), by node entity.
    pub deferred: Vec<usize>,
    pub constructions: Vec<String>,
    /// The part's level crossings, read with its junctions.
    pub crossings: Vec<Crossing>,
    /// Entity slots whose component bits were examined to find this part's
    /// objects. This is the whole entity table for every part read.
    pub component_bit_entity_slots_scanned: usize,
    /// What the read found and took, for the log.
    pub timing: String,
}

/// A level crossing of a part: its row but for its type's name, which only
/// the game's Lua gives (tpf3mp/crossings.lua, `row`): `crossing:` and its
/// nodes' places in whole millimetres, sorted; and its type's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crossing {
    pub head: String,
    pub type_index: i32,
}

/// Most nodes one level crossing has before its read is a misread
/// (tpf3mp_proto action::MAX_CROSSING_NODES).
const MAX_CROSSING_NODES: usize = 8;

/// `floor(v * 1000 + 0.5)`: a value's whole millimetres, as crossings.lua's
/// `mm` rounds it (half up; a junction's `pointKey` rounds ties to even).
fn crossing_mm(v: f32) -> Result<i64, String> {
    let t = (f64::from(v) * 1000.0 + 0.5).floor();
    if !t.is_finite() || t.abs() >= 1e15 {
        return Err(format!("a place that is not on a map ({v})"));
    }
    Ok(t as i64)
}

/// Reads each of `located` (entity and data index) element of `kind`, its
/// first `N` bytes, the cache line of the one `AHEAD` places on asked of
/// the processor first; `each` takes the entity, the element's address and
/// its bytes.
fn each_element<const N: usize>(
    store: &Store,
    kind: &Kind,
    located: &[(usize, i32)],
    mut each: impl FnMut(usize, usize, &[u8; N]) -> Result<(), String>,
) -> Result<(), String> {
    const AHEAD: usize = 16;
    let mut at = Vec::with_capacity(located.len());
    for (_, index) in located {
        at.push(kind.pool.element(store.memory, *index)?);
    }
    for (i, (entity, _)) in located.iter().enumerate() {
        if let Some(next) = at.get(i + AHEAD) {
            prefetch(*next);
        }
        let bytes: [u8; N] = read_array(store.memory, at[i], kind.name)?;
        each(*entity, at[i], &bytes)?;
    }
    Ok(())
}

/// The entities of `kind` among `entities` and their data indices, looked
/// up in bunches ([`Store::locate`]).
fn located(store: &Store, kind: &Kind, entities: &[usize]) -> Result<Vec<(usize, i32)>, String> {
    store
        .locate(entities, [kind])?
        .into_iter()
        .map(|(entity, [index])| {
            index
                .map(|i| (entity, i))
                .ok_or_else(|| format!("entity {entity} has the {0} bit but no {0}", kind.name))
        })
        .collect()
}

/// Part `k` of `n` of the static lanes of the world in `engine`, its
/// objects of `kinds`. Every object of those kinds is looked at to find
/// its part (and for junctions every edge's ends kept, which their rows
/// name); only the part's are made rows of.
pub fn read_part(
    memory: &dyn Memory,
    engine: usize,
    image: usize,
    n: u32,
    k: u32,
    kinds: Kinds,
) -> Result<Part, String> {
    if n == 0 || n > MAX_PARTS || k >= n {
        return Err(format!("no part {k} of {n}"));
    }
    let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1000.0;
    let t0 = std::time::Instant::now();
    let store = Store::new(memory, engine, image)?;
    // Finding a type walks the pool table and reads each pool's head. Most
    // scheduled batches read just one kind, so leave the other pools alone.
    let edge_kind = kinds
        .network()
        .then(|| {
            store.kind(
                layout::BASE_EDGE_POOL_VTABLE,
                layout::BASE_EDGE_SIZE,
                "BaseEdge",
            )
        })
        .transpose()?;
    let node_kind = kinds
        .junctions
        .then(|| {
            store.kind(
                layout::BASE_NODE_POOL_VTABLE,
                layout::BASE_NODE_SIZE,
                "BaseNode",
            )
        })
        .transpose()?;
    let config_kind = kinds
        .junctions
        .then(|| {
            store.kind(
                layout::BASE_NODE_CONFIG_POOL_VTABLE,
                layout::BASE_NODE_CONFIG_SIZE,
                "BaseNodeConfig",
            )
        })
        .transpose()?;
    let cons = kinds
        .constructions
        .then(|| {
            store.kind(
                layout::CONSTRUCTION_POOL_VTABLE,
                layout::CONSTRUCTION_SIZE,
                "Construction",
            )
        })
        .transpose()?;
    // Level crossings go with the junctions; a world that never had one has
    // no pool of them, and so none.
    let crossing_kind = if kinds.junctions {
        store.kind_if_any(
            layout::RAILROAD_CROSSING_POOL_VTABLE,
            layout::RAILROAD_CROSSING_SIZE,
            "RailroadCrossing",
        )?
    } else {
        None
    };
    // One pass over the bits for the kinds read.
    let mut wanted: Vec<&Kind> = Vec::new();
    if let Some(kind) = edge_kind.as_ref() {
        wanted.push(kind);
    }
    if let Some(kind) = config_kind.as_ref() {
        wanted.push(kind);
    }
    if let Some(kind) = cons.as_ref() {
        wanted.push(kind);
    }
    if let Some(kind) = crossing_kind.as_ref() {
        wanted.push(kind);
    }
    let (lists, component_bit_entity_slots_scanned) = store.with_all(&wanted)?;
    let mut lists = lists.into_iter();
    let mut next = |wanted: bool| {
        if wanted {
            lists.next().unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    let edge_ids = next(kinds.network());
    let config_ids = next(kinds.junctions);
    let cons_ids = next(kinds.constructions);
    let crossing_ids = next(crossing_kind.is_some());
    let t1 = std::time::Instant::now();
    let mut memo = Memo::default();
    let mut edges = Vec::new();
    let mut ends = Ends::default();
    if kinds.network() && !kinds.edges {
        let edge_kind = edge_kind.as_ref().ok_or("no BaseEdge kind for network")?;
        // Only the ends junction rows name: an edge's nodes and network,
        // the front of its BaseEdge.
        let found = located(&store, edge_kind, &edge_ids)?;
        ends.reserve(found.len());
        each_element::<{ layout::EDGE_ROAD_TYPE + 4 }>(
            &store,
            edge_kind,
            &found,
            |entity, _, edge| {
                ends.insert(entity, edge_ends(edge, entity)?);
                Ok(())
            },
        )?;
    } else if kinds.network() {
        let edge_kind = edge_kind.as_ref().ok_or("no BaseEdge kind for network")?;
        let found = located(&store, edge_kind, &edge_ids)?;
        if kinds.junctions {
            ends.reserve(found.len());
        }
        each_element::<{ layout::BASE_EDGE_SIZE }>(
            &store,
            edge_kind,
            &found,
            |entity, at, edge| {
                if kinds.edges {
                    let (x, y) =
                        edge_anchor(edge).map_err(|why| format!("entity {entity}: {why}"))?;
                    if part_of(x, y, CELL_TENTHS, n) == k {
                        edges.push(
                            edge_row(memory, edge, at, &mut memo)
                                .map_err(|why| format!("entity {entity}: {why}"))?,
                        );
                    }
                }
                if kinds.junctions {
                    ends.insert(entity, edge_ends(edge, entity)?);
                }
                Ok(())
            },
        )?;
    }
    let t2 = std::time::Instant::now();
    let mut junctions = Vec::new();
    let mut deferred = Vec::new();
    let mut nodes_seen = 0;
    let mut crossings = Vec::new();
    let mut crossings_seen = 0;
    if kinds.junctions {
        let config_kind = config_kind
            .as_ref()
            .ok_or("no BaseNodeConfig kind for junctions")?;
        let node_kind = node_kind.as_ref().ok_or("no BaseNode kind for junctions")?;
        let (mut j, configs, street_node) = junction_reader(&store, &ends)?;
        // Only nodes of the street or track network; their configuration's
        // and their place's index from one read of their list, in bunches;
        // a removed node's bits can stay set, and read_junction skips it.
        let ours: Vec<usize> = config_ids
            .into_iter()
            .filter(|node| street_node.contains_key(node))
            .collect();
        let mut places = Vec::with_capacity(ours.len());
        for (node, [config, place]) in store.locate(&ours, [config_kind, node_kind])? {
            if config.is_none() {
                return Err(format!(
                    "entity {node} has the BaseNodeConfig bit but no BaseNodeConfig"
                ));
            }
            places.push((
                node,
                place.ok_or_else(|| format!("node {node} has no position"))?,
            ));
        }
        nodes_seen = places.len();
        let mut in_part = Vec::new();
        each_element::<{ layout::BASE_NODE_SIZE }>(&store, node_kind, &places, |node, _, raw| {
            let p = j.keep_position(node, raw)?;
            if part_of(millimetres(p[0])?, millimetres(p[1])?, CELL_MM, n) == k {
                in_part.push(node);
            }
            Ok(())
        })?;
        for node in in_part {
            let street = street_node[&node];
            match read_junction(&mut j, &configs, node, street)? {
                Some(JunctionRead::Row(row)) => junctions.push(row),
                Some(JunctionRead::Deferred) => deferred.push(node),
                None => {}
            }
        }
        if let Some(kind) = crossing_kind.as_ref() {
            let found = located(&store, kind, &crossing_ids)?;
            crossings_seen = found.len();
            crossings = read_crossings(&store, &mut j, kind, &found, n, k)?;
        }
    }
    let t3 = std::time::Instant::now();
    let mut constructions = Vec::new();
    let mut cons_seen = 0;
    if kinds.constructions {
        let cons = cons
            .as_ref()
            .ok_or("no Construction kind for constructions")?;
        let found = located(&store, cons, &cons_ids)?;
        cons_seen = found.len();
        each_element::<{ layout::CONSTRUCTION_Y + 4 }>(&store, cons, &found, |_, at, bytes| {
            let (x, y) = (
                f32_at(bytes, layout::CONSTRUCTION_X),
                f32_at(bytes, layout::CONSTRUCTION_Y),
            );
            if part_of(tenths(x)?, tenths(y)?, CELL_TENTHS, n) != k {
                return Ok(());
            }
            let file = memo.name(
                memory,
                bytes,
                layout::CONSTRUCTION_FILE,
                at + layout::CONSTRUCTION_FILE,
                "a construction's file",
            )?;
            constructions.push(format!("{file}@{},{}", q01_text(x)?, q01_text(y)?));
            Ok(())
        })?;
    }
    let t4 = std::time::Instant::now();
    let timing = format!(
        "part {k}/{n}: component bits {component_bit_entity_slots_scanned}/{} entity slots {:.2} ms, {} of {} edges {:.2} ms, {}+{} of {} junctions and {} of {} crossings {:.2} ms, {} of {} constructions {:.2} ms",
        store.entities,
        ms(t0, t1),
        edges.len(),
        edge_ids.len(),
        ms(t1, t2),
        junctions.len(),
        deferred.len(),
        nodes_seen,
        crossings.len(),
        crossings_seen,
        ms(t2, t3),
        constructions.len(),
        cons_seen,
        ms(t3, t4),
    );
    Ok(Part {
        n,
        k,
        kinds,
        step: None,
        edges,
        junctions,
        deferred,
        constructions,
        crossings,
        component_bit_entity_slots_scanned,
        timing,
    })
}

/// The crossings of `found` (entity and data index of `kind`) in part `k`
/// of `n`, by their first node's place as lanes.lua's `rowPart` puts a
/// crossing row: each crossing's nodes (a `std::vector` of entities at
/// `CROSSING_NODES`, at most [`MAX_CROSSING_NODES`]), their places from
/// `j` in whole millimetres ([`crossing_mm`]), sorted, none twice; and its
/// type's index. Fails as crossings.lua's `places` does: a crossing with no
/// node, a node whose place does not read, two nodes in one place.
fn read_crossings(
    store: &Store,
    j: &mut Junctions<'_, '_>,
    kind: &Kind,
    found: &[(usize, i32)],
    n: u32,
    k: u32,
) -> Result<Vec<Crossing>, String> {
    let mut heads = Vec::with_capacity(found.len());
    each_element::<{ layout::CROSSING_TYPE_INDEX + 4 }>(store, kind, found, |entity, _, raw| {
        let (begin, count) = vector(
            raw,
            layout::CROSSING_NODES,
            4,
            MAX_CROSSING_NODES,
            "a level crossing's nodes",
        )
        .map_err(|why| format!("level crossing {entity}: {why}"))?;
        // A std::vector of 4-byte entities: aligned, and its capacity past
        // its end.
        let end = u64_at(raw, layout::CROSSING_NODES + 8);
        let capacity = u64_at(raw, layout::CROSSING_NODES + 16);
        if begin % 4 != 0 || capacity < end {
            return Err(format!("level crossing {entity}: its nodes are no vector"));
        }
        let index = &raw[layout::CROSSING_TYPE_INDEX..layout::CROSSING_TYPE_INDEX + 4];
        let type_index = i32::from_le_bytes([index[0], index[1], index[2], index[3]]);
        heads.push((entity, begin, count, type_index));
        Ok(())
    })?;
    let mut out = Vec::new();
    for (entity, begin, count, type_index) in heads {
        if count == 0 {
            return Err(format!("level crossing {entity} has no node"));
        }
        let ids = read(store.memory, begin, count * 4, "a level crossing's nodes")?;
        let mut places = Vec::with_capacity(count);
        for i in 0..count {
            let node =
                i32::from_le_bytes([ids[i * 4], ids[i * 4 + 1], ids[i * 4 + 2], ids[i * 4 + 3]]);
            let node = usize::try_from(node)
                .map_err(|_| format!("level crossing {entity} names node {node}"))?;
            let p = j.position(node)?;
            places.push([crossing_mm(p[0])?, crossing_mm(p[1])?, crossing_mm(p[2])?]);
        }
        places.sort_unstable();
        if places.windows(2).any(|w| w[0] == w[1]) {
            return Err(format!(
                "level crossing {entity} has two nodes in one place"
            ));
        }
        if part_of(places[0][0], places[0][1], CELL_MM, n) != k {
            continue;
        }
        let keys: Vec<String> = places
            .iter()
            .map(|[x, y, z]| format!("{x},{y},{z}"))
            .collect();
        out.push(Crossing {
            head: format!("crossing:{}", keys.join(";")),
            type_index,
        });
    }
    Ok(out)
}

/// Part `k` of `n` of the world the game's step is running now.
pub fn part_now(n: u32, k: u32, kinds: Kinds) -> Result<Part, String> {
    let (memory, engine, image) = engine_now()?;
    let mut part = read_part(&memory, engine, image, n, k, kinds)?;
    part.step = crate::seeds::current_step();
    let (checks, queries) = memory.counts();
    part.timing += &format!(", {checks} checks, {queries} regions asked");
    Ok(part)
}

thread_local! {
    /// The last part read on this thread, for its texts ([`part_texts`]):
    /// the mod reads it and asks for its texts in the same `postUpdate`.
    static LAST_PART: std::cell::RefCell<Option<Part>> = const { std::cell::RefCell::new(None) };
}

/// Keeps `part` for [`part_texts`], in place of any part kept before.
pub fn keep_part(part: Option<Part>) {
    LAST_PART.with(|last| *last.borrow_mut() = part);
}

/// The two static lanes' texts of the part [`keep_part`] kept last on this
/// thread, once, when it is part `k` of `n` read at `step`: the network lane's (its edge rows, and its junction rows
/// named as [`summary`] names them, with `deferred`, the rows the game's
/// Lua made of the part's junctions left to it) and the constructions
/// lane's, each `count:hash` as lanes.lua's `summary` makes it; and with
/// `rows`, their rows, sorted.
pub fn part_texts(
    (n, k, kinds): (u32, u32, Kinds),
    step: Option<u64>,
    preferences: &HashMap<i32, String>,
    lights: &HashMap<i32, String>,
    deferred: &[String],
    crossing_types: &HashMap<i32, String>,
    rows: bool,
) -> Result<PartTexts, String> {
    let part = LAST_PART
        .with(|last| last.borrow_mut().take())
        .ok_or("no part was read on this thread")?;
    // Only the part asked for, read in this same update.
    if (part.n, part.k, part.kinds) != (n, k, kinds) || part.step != step {
        return Err(format!(
            "the part kept is {}/{} ({:?}) of step {:?}, not {k}/{n} ({kinds:?}) of step {step:?}",
            part.k, part.n, part.kinds, part.step
        ));
    }
    let mut network = network_rows(
        &part.edges,
        &part.junctions,
        part.deferred.len(),
        preferences,
        lights,
        deferred,
    )?;
    network.sort_unstable();
    let mut constructions = part.constructions;
    constructions.sort_unstable();
    let text = |rows: &[String]| {
        format!(
            "{}:{}",
            rows.len(),
            crate::lanehash::hash(rows.join("\x1e").as_bytes())
        )
    };
    // The level crossings, each with its type's name, which only the game's
    // Lua gives: after the network's own text, as lanes.lua sums them up
    // (`|crossings count:hash`), on a turn that reads the junctions.
    let mut crossings = Vec::with_capacity(part.crossings.len());
    for c in &part.crossings {
        let name = crossing_types
            .get(&c.type_index)
            .filter(|name| !name.is_empty())
            .ok_or("unknown level crossing type")?;
        crossings.push(format!("{}|{name}", c.head));
    }
    crossings.sort_unstable();
    let network_text = kinds.network().then(|| {
        if kinds.junctions {
            format!("{}|crossings {}", text(&network), text(&crossings))
        } else {
            text(&network)
        }
    });
    network.extend(crossings);
    network.sort_unstable();
    // Only the lanes it read: an edges part says nothing of junctions'
    // rows or of constructions, and their lanes are not touched.
    Ok(PartTexts {
        network: network_text,
        constructions: kinds.constructions.then(|| text(&constructions)),
        rows: rows.then_some((network, constructions)),
    })
}

/// [`part_texts`]' answer.
pub struct PartTexts {
    pub network: Option<String>,
    pub constructions: Option<String>,
    pub rows: Option<(Vec<String>, Vec<String>)>,
}

/// lanes.lua's `summary` of `rows`: their count and the hash of their
/// sorted text joined by `\x1e`.
pub fn rows_summary(mut rows: Vec<String>) -> String {
    rows.sort_unstable();
    let text = rows.join("\x1e");
    format!("{}:{}", rows.len(), crate::lanehash::hash(text.as_bytes()))
}

/// The constructions lane of the world the game's step is running now:
/// its rows, read natively.
pub fn constructions_now() -> Result<Vec<String>, String> {
    let (memory, engine, image) = engine_now()?;
    construction_rows(&memory, engine, image)
}

thread_local! {
    /// The last network read on this thread, for its summary
    /// ([`summary_of_last`]): the mod reads it and asks for its summary in
    /// the same `postUpdate`.
    static LAST: std::cell::RefCell<Option<Network>> = const { std::cell::RefCell::new(None) };
}

/// Keeps `network` for [`summary_of_last`].
pub fn keep(network: Network) {
    LAST.with(|last| *last.borrow_mut() = Some(network));
}

/// [`summary`] of the network [`keep`] kept last on this thread, once.
pub fn summary_of_last(
    preferences: &HashMap<i32, String>,
    lights: &HashMap<i32, String>,
    deferred: &[String],
) -> Result<String, String> {
    let network = LAST
        .with(|last| last.borrow_mut().take())
        .ok_or("no network was read on this thread")?;
    summary(&network, preferences, lights, deferred)
}

/// The network lane's text as lanes.lua's `summary` makes it from its rows
/// (the edges' rows and each junction's row after `junction:`): their
/// count and the hash of their sorted text joined by `\x1e`. The junction
/// rows take their preference's and light's names from `preferences` and
/// `lights`, as `junctions.rowsFromParts` does; `deferred` are the rows
/// the game's Lua made for the junctions the read left to it
/// ([`Network::deferred`]), one each.
pub fn summary(
    network: &Network,
    preferences: &HashMap<i32, String>,
    lights: &HashMap<i32, String>,
    deferred: &[String],
) -> Result<String, String> {
    let junctions = network.junctions.as_ref().map_err(Clone::clone)?;
    let mut rows = network_rows(
        &network.edges,
        junctions,
        network.deferred.len(),
        preferences,
        lights,
        deferred,
    )?;
    rows.sort_unstable();
    let text = rows.join("\x1e");
    Ok(format!(
        "{}:{}",
        rows.len(),
        crate::lanehash::hash(text.as_bytes())
    ))
}

/// The network lane's rows, unsorted: `edges`, and each of `junctions`
/// and `deferred` (the Lua's rows of the `left` junctions left to it)
/// after `junction:`, named from `preferences` and `lights`.
fn network_rows(
    edges: &[String],
    junctions: &[Junction],
    left: usize,
    preferences: &HashMap<i32, String>,
    lights: &HashMap<i32, String>,
    deferred: &[String],
) -> Result<Vec<String>, String> {
    if deferred.len() != left {
        return Err(format!(
            "{} junction rows for the {left} junctions left to the mod",
            deferred.len(),
        ));
    }
    let mut rows: Vec<String> = Vec::with_capacity(edges.len() + junctions.len() + deferred.len());
    rows.extend(deferred.iter().map(|row| format!("junction:{row}")));
    rows.extend(edges.iter().cloned());
    for j in junctions {
        let preference = preferences
            .get(&j.preference)
            .ok_or("unknown traffic light preference")?;
        let light = if j.light == -1 {
            "default"
        } else {
            lights
                .get(&j.light)
                .map(String::as_str)
                .ok_or("unknown traffic light resource")?
        };
        rows.push(format!(
            "junction:{}|{preference}|{light}|{}",
            j.head, j.tail
        ));
    }
    Ok(rows)
}

/// C's `%.0f`, as `string.format` makes it. Only finite numbers.
pub fn fixed0(v: f64) -> Result<String, String> {
    if !v.is_finite() {
        return Err(format!("a number that is not finite ({v})"));
    }
    // C rounds the exact value, ties to even, and keeps a negative zero's
    // sign: below 2^52 that is `round_ties_even`, without the slow exact
    // formatting.
    if v.abs() < 4.0e15 {
        let r = v.round_ties_even();
        if r == 0.0 && v.is_sign_negative() {
            return Ok("-0".into());
        }
        return Ok((r as i64).to_string());
    }
    Ok(format!("{v:.0}"))
}

/// junctions.lua's `pointKey`: a position in millimetres.
fn point_key(p: [f32; 3]) -> Result<String, String> {
    Ok(format!(
        "{},{},{}",
        fixed0(f64::from(p[0]) * 1000.0)?,
        fixed0(f64::from(p[1]) * 1000.0)?,
        fixed0(f64::from(p[2]) * 1000.0)?
    ))
}

fn flag(raw: &[u8], at: usize, what: &str) -> Result<bool, String> {
    match raw[at] {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(format!("{what} reads {other}")),
    }
}

/// What makes the junctions' rows: the nodes' positions and the edges'
/// keys, each read and made once.
struct Junctions<'s, 'm> {
    store: &'s Store<'m>,
    ends: &'s Ends,
    nodes: Kind,
    positions: IdMap<[f32; 3]>,
    keys: IdMap<std::rc::Rc<str>>,
}

impl Junctions<'_, '_> {
    fn position(&mut self, node: usize) -> Result<[f32; 3], String> {
        if let Some(p) = self.positions.get(&node) {
            return Ok(*p);
        }
        let mut raw = [0u8; layout::BASE_NODE_SIZE];
        if !self.store.component_into(node, &self.nodes, &mut raw)? {
            return Err(format!("node {node} has no position"));
        }
        self.keep_position(node, &raw)
    }

    /// Keeps and gives `node`'s place from its `BaseNode` bytes.
    fn keep_position(&mut self, node: usize, raw: &[u8]) -> Result<[f32; 3], String> {
        let p = [
            f32_at(raw, layout::NODE_POSITION),
            f32_at(raw, layout::NODE_POSITION + 4),
            f32_at(raw, layout::NODE_POSITION + 8),
        ];
        if p.iter().any(|v| !v.is_finite()) {
            return Err(format!("node {node}: an invalid junction position"));
        }
        self.positions.insert(node, p);
        Ok(p)
    }

    /// junctions.lua's `edgeKey`: the edge's network and its nodes' places.
    fn edge_key(&mut self, edge: i32) -> Result<std::rc::Rc<str>, String> {
        let edge = usize::try_from(edge).map_err(|_| format!("a junction names edge {edge}"))?;
        if let Some(k) = self.keys.get(&edge) {
            return Ok(k.clone());
        }
        let e = *self
            .ends
            .get(&edge)
            .ok_or_else(|| format!("a junction edge {edge} no longer exists"))?;
        let mut a = point_key(self.position(e.node0)?)?;
        let mut b = point_key(self.position(e.node1)?)?;
        if a.as_bytes() > b.as_bytes() {
            std::mem::swap(&mut a, &mut b);
        }
        let k: std::rc::Rc<str> =
            format!("{}:{a}>{b}", if e.street { "Street" } else { "Track" }).into();
        self.keys.insert(edge, k.clone());
        Ok(k)
    }
}

/// Every junction's row parts, as junctions.lua's `rows` makes them: the
/// nodes of the edges in `ends` that have a `BaseNodeConfig`, in id order.
///
/// A junction with two crosswalks or more, one of whose phases locks some
/// of its crosswalks but not all, is left to the game's Lua (its node in
/// the second list): a phase names its lanes by index, the crosswalks'
/// part of them in the order the game's Lua lists the crosswalk set, and
/// the Lua lists a copy of the component, whose hash set a copy can lay
/// out anew, in another order than the engine's own set here.
fn read_junctions(store: &Store, ends: &Ends) -> Result<(Vec<Junction>, Vec<usize>), String> {
    let (mut j, configs, street_node) = junction_reader(store, ends)?;
    let mut out = Vec::new();
    let mut deferred = Vec::new();
    for node in store.with(&configs)? {
        let Some(&street) = street_node.get(&node) else {
            // In neither network's node map: junctions.lua never reads it.
            continue;
        };
        match read_junction(&mut j, &configs, node, street)? {
            Some(JunctionRead::Row(row)) => out.push(row),
            Some(JunctionRead::Deferred) => deferred.push(node),
            None => {}
        }
    }
    Ok((out, deferred))
}

/// What reads junctions: their nodes' places and edges' keys, the
/// configurations' pool, and each node's network: a street edge at it
/// makes it a street node, as getNodeStreetSegments being first does in
/// junctions.lua. Only nodes of an edge in `ends` are in either network.
fn junction_reader<'s, 'm>(
    store: &'s Store<'m>,
    ends: &'s Ends,
) -> Result<(Junctions<'s, 'm>, Kind, IdMap<bool>), String> {
    let nodes = store.kind(
        layout::BASE_NODE_POOL_VTABLE,
        layout::BASE_NODE_SIZE,
        "BaseNode",
    )?;
    let configs = store.kind(
        layout::BASE_NODE_CONFIG_POOL_VTABLE,
        layout::BASE_NODE_CONFIG_SIZE,
        "BaseNodeConfig",
    )?;
    let mut street_node = IdMap::<bool>::default();
    for e in ends.values() {
        for n in [e.node0, e.node1] {
            *street_node.entry(n).or_insert(false) |= e.street;
        }
    }
    let j = Junctions {
        store,
        ends,
        nodes,
        positions: IdMap::default(),
        keys: IdMap::default(),
    };
    Ok((j, configs, street_node))
}

/// One junction's read: its row's parts, or left to the game's Lua.
enum JunctionRead {
    Row(Junction),
    Deferred,
}

/// The junction at `node` (of the street network or not), as
/// [`read_junctions`] reads each: `None` where it has no configuration.
fn read_junction(
    j: &mut Junctions,
    configs: &Kind,
    node: usize,
    street: bool,
) -> Result<Option<JunctionRead>, String> {
    let store = j.store;
    {
        let mut raw = [0u8; layout::BASE_NODE_CONFIG_SIZE];
        if !store.component_into(node, configs, &mut raw)? {
            return Ok(None);
        }
        let mut lanes = Vec::new();
        let (turns_at, turns) = vector(
            &raw,
            layout::CONFIG_TURNS,
            layout::TURN_SIZE,
            MAX_LANES,
            "a junction's turns",
        )?;
        let turn_bytes = read(
            store.memory,
            turns_at,
            turns * layout::TURN_SIZE,
            "a junction's turns",
        )?;
        for t in 0..turns {
            let turn = &turn_bytes[t * layout::TURN_SIZE..(t + 1) * layout::TURN_SIZE];
            let incoming = j.edge_key(i32_at(turn, layout::TURN_SEGMENT0))?;
            let outgoing = j.edge_key(i32_at(turn, layout::TURN_SEGMENT1))?;
            let mut lane = String::with_capacity(incoming.len() + outgoing.len() + 32);
            lane.push_str(&incoming);
            lane.push(':');
            lane.push_str(&i32_at(turn, layout::TURN_LANE0).to_string());
            lane.push('>');
            lane.push_str(&outgoing);
            lane.push(':');
            lane.push_str(&i32_at(turn, layout::TURN_LANE1).to_string());
            lane.push_str(if flag(turn, layout::TURN_ROAD, "a turn's road flag")? {
                ":true"
            } else {
                ":false"
            });
            lane.push_str(if flag(turn, layout::TURN_TRAM, "a turn's tram flag")? {
                ":true"
            } else {
                ":false"
            });
            lanes.push(lane);
        }
        for e in crate::junctions::crosswalk_ids(store.memory, &raw)? {
            lanes.push(format!("walk:{}", j.edge_key(e)?));
        }
        let mut sorted: Vec<&str> = lanes.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        let (phases_at, phases) = vector(
            &raw,
            layout::CONFIG_PHASES,
            layout::PHASE_SIZE,
            MAX_LANES,
            "a junction's phases",
        )?;
        let phase_bytes = read(
            store.memory,
            phases_at,
            phases * layout::PHASE_SIZE,
            "a junction's phases",
        )?;
        let mut phase_rows = Vec::with_capacity(phases);
        let walks = turns..lanes.len();
        let mut ambiguous = false;
        for i in 0..phases {
            let phase = &phase_bytes[i * layout::PHASE_SIZE..(i + 1) * layout::PHASE_SIZE];
            let (locked_at, count) = vector(
                phase,
                layout::PHASE_LOCKED,
                4,
                MAX_LANES,
                "a phase's locked lanes",
            )?;
            let locked_bytes = read(store.memory, locked_at, count * 4, "a phase's locked lanes")?;
            let mut locked = Vec::with_capacity(count);
            let mut locked_walks = std::collections::BTreeSet::new();
            let mut walk_locks = 0;
            for k in 0..count {
                let index = usize::try_from(i32_at(&locked_bytes, k * 4))
                    .ok()
                    .filter(|l| *l < lanes.len())
                    .ok_or("traffic phase references no lane")?;
                if walks.contains(&index) {
                    locked_walks.insert(index);
                    walk_locks += 1;
                }
                locked.push(lanes[index].as_str());
            }
            // Some of several crosswalks, or one of them twice: which ones
            // the row names depends on the crosswalk set's order.
            if walks.len() >= 2
                && !locked_walks.is_empty()
                && (locked_walks.len() < walks.len() || walk_locks > locked_walks.len())
            {
                ambiguous = true;
            }
            locked.sort_unstable();
            phase_rows.push(format!(
                "{}/{}/{}:{}",
                fixed3(f64::from(f32_at(phase, layout::PHASE_DURATION)))?,
                fixed3(f64::from(f32_at(phase, layout::PHASE_MINIMUM)))?,
                flag(phase, layout::PHASE_SKIP, "a phase's skip flag")?,
                locked.join(","),
            ));
        }
        if ambiguous {
            return Ok(Some(JunctionRead::Deferred));
        }
        let head = format!(
            "{}:{}|{}",
            if street { "Street" } else { "Track" },
            point_key(j.position(node)?)?,
            sorted.join(";")
        );
        let tail = format!(
            "{}|{}|{}",
            flag(
                &raw,
                layout::CONFIG_DOUBLE_SLIP,
                "a junction's double slip flag"
            )?,
            flag(
                &raw,
                layout::CONFIG_CUSTOM_PHASES,
                "a junction's custom phases flag"
            )?,
            phase_rows.join(";")
        );
        Ok(Some(JunctionRead::Row(Junction {
            head,
            preference: i32_at(&raw, layout::CONFIG_PREFERENCE),
            light: i32_at(&raw, layout::CONFIG_LIGHT_TYPE),
            tail,
        })))
    }
}

/// One edge's row from its `BaseEdge` (`edge`, read at `at`).
fn edge_row(
    memory: &dyn Memory,
    edge: &[u8],
    at: usize,
    memo: &mut Memo,
) -> Result<String, String> {
    let point = |offset: usize| -> Result<String, String> {
        let mut parts = Vec::with_capacity(3);
        for k in 0..3 {
            parts.push(q01_text(f32_at(edge, offset + 4 * k))?);
        }
        Ok(parts.join(","))
    };
    let (mut a, mut b) = (
        point(layout::EDGE_POSITION0)?,
        point(layout::EDGE_POSITION1)?,
    );
    let reversed = a.as_bytes() > b.as_bytes();
    if reversed {
        std::mem::swap(&mut a, &mut b);
    }
    let template = memo.name(
        memory,
        edge,
        layout::EDGE_ROAD_TEMPLATE,
        at + layout::EDGE_ROAD_TEMPLATE,
        "the edge's road template",
    )?;
    let (lanes_at, lanes) = vector(
        edge,
        layout::EDGE_LANE_CONFIGS,
        layout::LANE_CONFIG_SIZE,
        MAX_LANES,
        "the edge's lane configs",
    )?;
    let mut buffer = [0u8; MAX_LANES * layout::LANE_CONFIG_SIZE];
    let configs = &mut buffer[..lanes * layout::LANE_CONFIG_SIZE];
    if lanes > 0 && !memory.read_into(lanes_at, configs) {
        return Err("the edge's lane configs does not read".into());
    }
    let mut lane_rows: Vec<&str> = Vec::with_capacity(lanes);
    // Each distinct config made once a read: the same few repeat over and
    // over (a road type's lanes).
    for i in 0..lanes {
        let mut key = [0u8; layout::LANE_CONFIG_SIZE];
        key.copy_from_slice(
            &configs[i * layout::LANE_CONFIG_SIZE..(i + 1) * layout::LANE_CONFIG_SIZE],
        );
        if let std::collections::hash_map::Entry::Vacant(entry) = memo.lanes.entry((key, reversed))
        {
            entry.insert(lane_row(&key, reversed)?);
        }
    }
    for i in 0..lanes {
        let mut key = [0u8; layout::LANE_CONFIG_SIZE];
        key.copy_from_slice(
            &configs[i * layout::LANE_CONFIG_SIZE..(i + 1) * layout::LANE_CONFIG_SIZE],
        );
        lane_rows.push(memo.lanes[&(key, reversed)].as_str());
    }
    lane_rows.sort_unstable();
    let mut row = String::with_capacity(a.len() + b.len() + template.len() + 8 + lanes * 48);
    row.push_str(&a);
    row.push('>');
    row.push_str(&b);
    row.push(':');
    row.push_str(&template);
    row.push_str("|lanes:");
    for (i, lane) in lane_rows.iter().enumerate() {
        if i > 0 {
            row.push(';');
        }
        row.push_str(lane);
    }
    Ok(row)
}

/// One lane config's text, as lanes.edgeRow makes it: its speed, width,
/// height and offset (turned with a `reversed` edge) to 1 mm, whether it
/// runs forward along the row, and its 16 transport modes.
fn lane_row(c: &[u8], reversed: bool) -> Result<String, String> {
    let sign = if reversed { -1.0 } else { 1.0 };
    let forward = match c[layout::LANE_FORWARD] {
        0 => false,
        1 => true,
        other => return Err(format!("a lane's forward flag reads {other}")),
    };
    let modes = u32_at(c, layout::LANE_MODES);
    let modes: String = (0..16)
        .map(|m| if modes >> m & 1 == 1 { '1' } else { '0' })
        .collect();
    Ok(format!(
        "{}/{}/{}/{}/{}/{modes}",
        fixed3(f64::from(f32_at(c, layout::LANE_SPEED)))?,
        fixed3(f64::from(f32_at(c, layout::LANE_WIDTH)))?,
        fixed3(f64::from(f32_at(c, layout::LANE_HEIGHT)))?,
        fixed3(f64::from(f32_at(c, layout::LANE_OFFSET)) * sign)?,
        forward != reversed,
    ))
}

/// What one read makes again and again, made once: each distinct lane
/// config's text, by its bytes and the edge's direction, and the road
/// templates' names, by their `ResName`'s bytes (two `std::string`s: the
/// same bytes are the same text while the engine frees nothing).
#[derive(Default)]
struct Memo {
    lanes: HashMap<([u8; layout::LANE_CONFIG_SIZE], bool), String>,
    names: HashMap<[u8; 0x40], String>,
}

impl Memo {
    /// The `ResName` at `offset` of `bytes`, read at `at`.
    fn name(
        &mut self,
        memory: &dyn Memory,
        bytes: &[u8],
        offset: usize,
        at: usize,
        what: &str,
    ) -> Result<String, String> {
        let mut key = [0u8; 0x40];
        key.copy_from_slice(&bytes[offset..offset + 0x40]);
        if let Some(text) = self.names.get(&key) {
            return Ok(text.clone());
        }
        let text = res_name(memory, at, what)?;
        self.names.insert(key, text.clone());
        Ok(text)
    }
}

/// `tostring(q01(v))`: a value to 0.1 as the game's Lua prints it. For the
/// tenths of anything below 10^12 that is the integer of tenths with its
/// last digit after a point (`%.14g` of the double nearest k/10 rounds to
/// it exactly); [`lua_number`] for anything else.
fn q01_text(v: f32) -> Result<String, String> {
    let tenths = (f64::from(v) * 10.0 + 0.5).floor();
    if !tenths.is_finite() {
        return Err(format!("a number that is not finite ({v})"));
    }
    if tenths.abs() >= 1e13 {
        return lua_number(tenths / 10.0);
    }
    let k = tenths as i64;
    let (whole, tenth) = (k.unsigned_abs() / 10, k.unsigned_abs() % 10);
    let sign = if k < 0 { "-" } else { "" };
    Ok(if tenth == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{tenth}")
    })
}

/// lanes.lua's `q01`: `math.floor(v * 10 + 0.5) / 10`, in doubles.
#[cfg(test)]
fn q01(v: f32) -> f64 {
    (f64::from(v) * 10.0 + 0.5).floor() / 10.0
}

/// A number as the game's Lua (5.2) makes it text, `tostring` and `%s`:
/// C's `%.14g`. Only finite numbers.
pub fn lua_number(v: f64) -> Result<String, String> {
    if !v.is_finite() {
        return Err(format!("a number that is not finite ({v})"));
    }
    const P: i32 = 14;
    if v == 0.0 {
        return Ok(if v.is_sign_negative() { "-0" } else { "0" }.into());
    }
    // The exponent of the value rounded to P significant digits, as
    // `%.13e` gives it.
    let e = format!("{:.*e}", (P - 1) as usize, v);
    let (mantissa, exponent) = e.split_once('e').ok_or("a number's exponent")?;
    let x: i32 = exponent.parse().map_err(|_| "a number's exponent")?;
    let text = if (-4..P).contains(&x) {
        let precision = usize::try_from(P - 1 - x).unwrap_or(0);
        trim_zeros(format!("{v:.precision$}"))
    } else {
        format!(
            "{}e{}{:02}",
            trim_zeros(mantissa.to_string()),
            if x < 0 { '-' } else { '+' },
            x.abs()
        )
    };
    Ok(text)
}

fn trim_zeros(mut text: String) -> String {
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// C's `%.3f`, as `string.format` makes it. Only finite numbers.
pub fn fixed3(v: f64) -> Result<String, String> {
    if !v.is_finite() {
        return Err(format!("a number that is not finite ({v})"));
    }
    Ok(format!("{v:.3}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Memory made of blocks at addresses.
    #[derive(Default)]
    struct Fake {
        blocks: BTreeMap<usize, Vec<u8>>,
        next: usize,
        reads: std::cell::Cell<usize>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                blocks: BTreeMap::new(),
                next: 0x10_0000,
                reads: std::cell::Cell::new(0),
            }
        }
        fn alloc(&mut self, bytes: Vec<u8>) -> usize {
            let at = self.next;
            self.next += (bytes.len().max(1) + 0xfff) & !0xfff;
            self.blocks.insert(at, bytes);
            at
        }
        fn put(&mut self, at: usize, bytes: Vec<u8>) {
            self.blocks.insert(at, bytes);
        }
    }

    impl Memory for Fake {
        fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            let (base, block) = self.blocks.range(..=address).next_back()?;
            let start = address - base;
            block.get(start..start + len).map(<[u8]>::to_vec)
        }
    }

    fn put_u64(bytes: &mut [u8], at: usize, v: u64) {
        bytes[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn put_f32(bytes: &mut [u8], at: usize, v: f32) {
        bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// An MSVC `std::string` holding `text`, inline or on the heap.
    fn string(fake: &mut Fake, text: &str) -> Vec<u8> {
        let mut s = vec![0u8; 0x20];
        if text.len() < 16 {
            s[..text.len()].copy_from_slice(text.as_bytes());
            put_u64(&mut s, 0x18, 15);
        } else {
            let heap = fake.alloc(text.as_bytes().to_vec());
            put_u64(&mut s, 0, heap as u64);
            put_u64(&mut s, 0x18, text.len() as u64);
        }
        put_u64(&mut s, 0x10, text.len() as u64);
        s
    }

    struct Lane {
        speed: f32,
        width: f32,
        height: f32,
        forward: bool,
        modes: u32,
        offset: f32,
    }

    struct Edge {
        p0: [f32; 3],
        p1: [f32; 3],
        template: (&'static str, &'static str),
        lanes: Vec<Lane>,
    }

    const IMAGE: usize = 0x1_4000_0000;

    /// An engine with `edges` as entities 1, 3, 4, ... (entity 0 a person,
    /// entity 2 a removed edge), the BaseEdge pool at type id `edge_type`,
    /// the edges from `paged` on in pages.
    fn engine(fake: &mut Fake, edges: &[Edge], edge_type: usize, paged: usize) -> usize {
        let mut data = Vec::new();
        let mut page = vec![0u8; layout::PAGE_SLOTS * layout::BASE_EDGE_SIZE];
        let mut indices = Vec::new();
        for (i, e) in edges.iter().enumerate() {
            let mut b = vec![0u8; layout::BASE_EDGE_SIZE];
            for k in 0..3 {
                put_f32(&mut b, layout::EDGE_POSITION0 + 4 * k, e.p0[k]);
                put_f32(&mut b, layout::EDGE_POSITION1 + 4 * k, e.p1[k]);
            }
            let mut configs = vec![0u8; e.lanes.len() * layout::LANE_CONFIG_SIZE];
            for (j, l) in e.lanes.iter().enumerate() {
                let c = &mut configs[j * layout::LANE_CONFIG_SIZE..];
                put_f32(c, layout::LANE_SPEED, l.speed);
                put_f32(c, layout::LANE_WIDTH, l.width);
                put_f32(c, layout::LANE_HEIGHT, l.height);
                c[layout::LANE_FORWARD] = u8::from(l.forward);
                c[layout::LANE_MODES..layout::LANE_MODES + 4]
                    .copy_from_slice(&l.modes.to_le_bytes());
                put_f32(c, layout::LANE_OFFSET, l.offset);
            }
            let len = configs.len();
            let at = if len == 0 { 0 } else { fake.alloc(configs) };
            put_u64(&mut b, layout::EDGE_LANE_CONFIGS, at as u64);
            put_u64(&mut b, layout::EDGE_LANE_CONFIGS + 8, (at + len) as u64);
            put_u64(&mut b, layout::EDGE_LANE_CONFIGS + 16, (at + len) as u64);
            let first = string(fake, e.template.0);
            let second = string(fake, e.template.1);
            b[layout::EDGE_ROAD_TEMPLATE..layout::EDGE_ROAD_TEMPLATE + 0x20]
                .copy_from_slice(&first);
            b[layout::EDGE_ROAD_TEMPLATE + 0x20..layout::EDGE_ROAD_TEMPLATE + 0x40]
                .copy_from_slice(&second);
            if i < paged {
                indices.push((data.len() / layout::BASE_EDGE_SIZE) as u32);
                data.extend_from_slice(&b);
            } else {
                let slot = i - paged;
                page[slot * layout::BASE_EDGE_SIZE..(slot + 1) * layout::BASE_EDGE_SIZE]
                    .copy_from_slice(&b);
                indices.push(layout::PAGED_FROM + slot as u32);
            }
        }
        let dense_len = data.len();
        let dense = fake.alloc(data);
        let page_at = fake.alloc(page);
        let mut page_table = vec![0u8; layout::PAGE_ENTRY];
        put_u64(&mut page_table, 0, page_at as u64);
        let page_table_at = fake.alloc(page_table);
        let mut pool = vec![0u8; layout::POOL_HEAD];
        put_u64(&mut pool, 0, (IMAGE + layout::BASE_EDGE_POOL_VTABLE) as u64);
        put_u64(&mut pool, layout::POOL_DENSE, dense as u64);
        put_u64(
            &mut pool,
            layout::POOL_DENSE + 8,
            (dense + dense_len) as u64,
        );
        put_u64(&mut pool, layout::POOL_PAGES, page_table_at as u64);
        put_u64(
            &mut pool,
            layout::POOL_PAGES + 8,
            (page_table_at + layout::PAGE_ENTRY) as u64,
        );
        // What the game keeps at +0x98 is no slot count it bounds by
        // (seen in the game: values past any count): never read.
        put_u64(&mut pool, 0x98, 0xd66b_0d48_0000_0001);
        let pool_at = fake.alloc(pool);
        // Another pool, of something else, before it.
        let mut other = vec![0u8; layout::POOL_HEAD];
        put_u64(&mut other, 0, (IMAGE + 0x100) as u64);
        let other_at = fake.alloc(other);
        let mut pools = vec![0u8; (edge_type + 1) * 8];
        put_u64(&mut pools, 0, other_at as u64);
        put_u64(&mut pools, edge_type * 8, pool_at as u64);
        let pools_len = pools.len();
        let pools_at = fake.alloc(pools);
        // Entities: 0 a person, then the edges, with a removed edge at 2.
        let mut records: Vec<Vec<(i32, i32)>> = vec![vec![(0, 7)]];
        let mut bits: Vec<u128> = vec![1];
        for (i, index) in indices.iter().enumerate() {
            if i == 1 {
                // Removed, its bits left as they were.
                records.push(vec![(-1, -1)]);
                bits.push(1 << edge_type);
            }
            records.push(vec![(0, 3), (edge_type as i32, *index as i32)]);
            bits.push(1 | 1u128 << edge_type);
        }
        let mut table = vec![0u8; records.len() * layout::ENTITY_RECORD];
        for (e, pairs) in records.iter().enumerate() {
            let mut list = Vec::new();
            for (t, d) in pairs {
                list.extend_from_slice(&t.to_le_bytes());
                list.extend_from_slice(&d.to_le_bytes());
            }
            let len = list.len();
            let at = fake.alloc(list);
            put_u64(&mut table, e * 24, at as u64);
            put_u64(&mut table, e * 24 + 8, (at + len) as u64);
            put_u64(&mut table, e * 24 + 16, (at + len) as u64);
        }
        let table_len = table.len();
        let table_at = fake.alloc(table);
        let bits_bytes: Vec<u8> = bits.iter().flat_map(|b| b.to_le_bytes()).collect();
        let bits_at = fake.alloc(bits_bytes);
        let mut head = vec![0u8; 0x100];
        put_u64(&mut head, layout::POOLS, pools_at as u64);
        put_u64(&mut head, layout::POOLS + 8, (pools_at + pools_len) as u64);
        put_u64(&mut head, layout::ENTITIES, table_at as u64);
        put_u64(
            &mut head,
            layout::ENTITIES + 8,
            (table_at + table_len) as u64,
        );
        put_u64(&mut head, layout::BITS, bits_at as u64);
        fake.alloc(head)
    }

    fn lane(speed: f32, offset: f32, forward: bool, modes: u32) -> Lane {
        Lane {
            speed,
            width: 3.5,
            height: 0.0,
            forward,
            modes,
            offset,
        }
    }

    fn sample() -> Vec<Edge> {
        vec![
            Edge {
                p0: [10.04, -3.06, 0.0],
                p1: [100.0, 2.25, 1.0],
                template: ("", "street/town_medium_new.lua"),
                lanes: vec![
                    lane(13.888_889, -2.0, false, 0b1),
                    lane(13.888_889, 2.0, true, 0b11),
                ],
            },
            Edge {
                p0: [500.15, 200.0, 12.3456],
                p1: [400.0, 199.95, 12.0],
                template: ("mymod_1", "street/x.lua"),
                lanes: vec![
                    lane(0.0625, 0.0, true, 0x8000),
                    lane(22.2, 1.0625, false, 0),
                ],
            },
            Edge {
                p0: [-0.04, 0.05, -0.05],
                p1: [0.0, 0.0, 0.0],
                template: ("", ""),
                lanes: vec![],
            },
            Edge {
                p0: [123_456.78, -98_765.43, 1e-5],
                p1: [123_456.7, -98_765.4, 2.5],
                template: ("", "track/standard.lua"),
                lanes: vec![lane(83.333_33, -0.7175, true, 1 << 11)],
            },
        ]
    }

    /// lanes.lua's own row, run in Lua, for each of `edges`.
    fn lua_rows(edges: &[Edge]) -> Vec<String> {
        let lua = mlua::Lua::new();
        let scripts = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mod/tpf3mp_1/content/scripts/"
        );
        lua.load(format!(
            "package.path = {:?} .. '?.lua;' .. package.path",
            scripts
        ))
        .exec()
        .unwrap();
        let lanes: mlua::Table = lua.load("return require('tpf3mp.lanes')").eval().unwrap();
        let row: mlua::Function = lanes.get("edgeRow").unwrap();
        let mut out = Vec::new();
        for e in edges {
            let edge = lua.create_table().unwrap();
            let vec = |p: [f32; 3]| {
                let t = lua.create_table().unwrap();
                t.set("x", f64::from(p[0])).unwrap();
                t.set("y", f64::from(p[1])).unwrap();
                t.set("z", f64::from(p[2])).unwrap();
                t
            };
            edge.set("position0", vec(e.p0)).unwrap();
            edge.set("position1", vec(e.p1)).unwrap();
            let template = if e.template.1.is_empty() {
                String::new()
            } else {
                format!("{}::/{}", e.template.0, e.template.1)
            };
            edge.set("roadTemplate", template).unwrap();
            let configs = lua.create_table().unwrap();
            for (i, l) in e.lanes.iter().enumerate() {
                let c = lua.create_table().unwrap();
                c.set("speed", f64::from(l.speed)).unwrap();
                c.set("width", f64::from(l.width)).unwrap();
                c.set("height", f64::from(l.height)).unwrap();
                c.set("offset", f64::from(l.offset)).unwrap();
                c.set("forward", l.forward).unwrap();
                let modes = lua.create_table().unwrap();
                for m in 0..16 {
                    modes.set(m, l.modes >> m & 1 == 1).unwrap();
                }
                c.set("transportModes", modes).unwrap();
                configs.set(i + 1, c).unwrap();
            }
            edge.set("laneConfigs", configs).unwrap();
            out.push(row.call::<String>(edge).unwrap());
        }
        out
    }

    #[test]
    fn rows_read_as_the_mods_lua_makes_them() {
        let edges = sample();
        for (edge_type, paged) in [(5, edges.len()), (64, 1), (127, 0)] {
            let mut fake = Fake::new();
            let engine = engine(&mut fake, &edges, edge_type, paged);
            let native = edge_rows(&fake, engine, IMAGE).unwrap();
            assert_eq!(
                native,
                lua_rows(&edges),
                "type {edge_type}, paged from {paged}"
            );
        }
    }

    #[test]
    fn numbers_print_as_lua_prints_them() {
        let lua = mlua::Lua::new();
        let g: mlua::Function = lua
            .load("return function(v) return tostring(v) end")
            .eval()
            .unwrap();
        let f: mlua::Function = lua
            .load("return function(v) return string.format('%.3f', v) end")
            .eval()
            .unwrap();
        let mut samples = vec![
            0.0,
            -0.0,
            0.1,
            -0.1,
            1.0,
            12.5,
            -3.1,
            1e-5,
            1.25e-5,
            123_456.7,
            -98_765.4,
            1e14,
            1e15,
            123_456_789_012_345.0,
            0.000_1,
            0.000_099_99,
            0.0625,
            1.0625,
            2.5,
            0.0005,
            0.0015,
            0.0025,
            -0.0005,
            13.888_889_312_744_14,
            83.333_33,
            1e100,
            -1e-100,
            5e-324,
        ];
        // Every float32 lanes.lua reads at 0.1 m, and many others.
        let mut x: u32 = 12345;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let v = f32::from_bits(x);
            if v.is_finite() {
                samples.push(f64::from(v));
                samples.push(q01(v));
                let lua_q: String = g.call(q01(v)).unwrap();
                assert_eq!(q01_text(v).unwrap(), lua_q, "tostring(q01({v:e}))");
            }
            // Positions as a map has them.
            let p = (x % 4_000_000) as f32 / 97.0 - 20_000.0;
            let lua_q: String = g.call(q01(p)).unwrap();
            assert_eq!(q01_text(p).unwrap(), lua_q, "tostring(q01({p:e}))");
            samples.push(f64::from(x % 100_000) / 16.0 - 3000.0);
        }
        let z: mlua::Function = lua
            .load("return function(v) return string.format('%.0f', v) end")
            .eval()
            .unwrap();
        for v in samples {
            let lua_z: String = z.call(v * 1000.0).unwrap();
            assert_eq!(
                fixed0(v * 1000.0).unwrap(),
                lua_z,
                "%.0f of {:e}",
                v * 1000.0
            );
            let lua_g: String = g.call(v).unwrap();
            assert_eq!(lua_number(v).unwrap(), lua_g, "tostring({v:e})");
            let lua_f: String = f.call(v).unwrap();
            assert_eq!(fixed3(v).unwrap(), lua_f, "%.3f of {v:e}");
        }
    }

    #[test]
    fn a_layout_that_does_not_read_fails_the_read() {
        let edges = sample();
        // No pool of BaseEdge: another image.
        let mut fake = Fake::new();
        let at = engine(&mut fake, &edges, 5, 4);
        assert!(
            edge_rows(&fake, at, IMAGE + 0x1000)
                .unwrap_err()
                .contains("no pool")
        );
        // A lane's flag that is no bool.
        let mut fake = Fake::new();
        let mut odd = sample();
        odd.truncate(1);
        let at = engine(&mut fake, &odd, 5, 1);
        let pools = u64_at(&fake.read(at + layout::POOLS, 8).unwrap(), 0) as usize;
        let pool = u64_at(&fake.read(pools + 5 * 8, 8).unwrap(), 0) as usize;
        let dense = u64_at(&fake.read(pool + layout::POOL_DENSE, 8).unwrap(), 0) as usize;
        let edge = fake.read(dense, layout::BASE_EDGE_SIZE).unwrap();
        let configs = u64_at(&edge, layout::EDGE_LANE_CONFIGS) as usize;
        let mut c = fake.read(configs, 2 * layout::LANE_CONFIG_SIZE).unwrap();
        c[layout::LANE_FORWARD] = 7;
        fake.put(configs, c);
        assert!(
            edge_rows(&fake, at, IMAGE)
                .unwrap_err()
                .contains("forward flag")
        );
        // A number that is not finite.
        let mut fake = Fake::new();
        let mut nan = sample();
        nan[0].p1[1] = f32::NAN;
        let at = engine(&mut fake, &nan, 5, 4);
        assert!(
            edge_rows(&fake, at, IMAGE)
                .unwrap_err()
                .contains("not finite")
        );
        // An entity whose bits say BaseEdge and whose list does not.
        let mut fake = Fake::new();
        let at = engine(&mut fake, &edges, 5, 4);
        let bits = u64_at(&fake.read(at + layout::BITS, 8).unwrap(), 0) as usize;
        fake.blocks.get_mut(&bits).unwrap()[0] |= 1 << 5;
        assert!(
            edge_rows(&fake, at, IMAGE)
                .unwrap_err()
                .contains("no BaseEdge")
        );
    }

    /// One pool of a world built for a test: its vtable, type id, element
    /// size and elements by entity, all dense.
    struct Spec {
        vtable: usize,
        id: usize,
        size: usize,
        elements: Vec<(usize, Vec<u8>)>,
    }

    #[test]
    fn a_part_reads_only_the_component_pools_its_kind_needs() {
        for (vtable, id, size, kinds) in [
            (
                layout::BASE_EDGE_POOL_VTABLE,
                3,
                layout::BASE_EDGE_SIZE,
                Kinds {
                    edges: true,
                    junctions: false,
                    constructions: false,
                },
            ),
            (
                layout::CONSTRUCTION_POOL_VTABLE,
                66,
                layout::CONSTRUCTION_SIZE,
                Kinds {
                    edges: false,
                    junctions: false,
                    constructions: true,
                },
            ),
        ] {
            let mut fake = Fake::new();
            let engine = build(
                &mut fake,
                &[Spec {
                    vtable,
                    id,
                    size,
                    elements: Vec::new(),
                }],
                0,
            );
            let part = read_part(&fake, engine, IMAGE, 8, 0, kinds).unwrap();
            assert!(part.edges.is_empty());
            assert!(part.junctions.is_empty());
            assert!(part.constructions.is_empty());
        }
    }

    #[test]
    fn a_single_kind_part_does_one_pool_search_not_four() {
        let mut fake = Fake::new();
        let specs = [
            (layout::BASE_EDGE_POOL_VTABLE, 3, layout::BASE_EDGE_SIZE),
            (layout::BASE_NODE_POOL_VTABLE, 9, layout::BASE_NODE_SIZE),
            (
                layout::BASE_NODE_CONFIG_POOL_VTABLE,
                70,
                layout::BASE_NODE_CONFIG_SIZE,
            ),
            (
                layout::CONSTRUCTION_POOL_VTABLE,
                66,
                layout::CONSTRUCTION_SIZE,
            ),
        ]
        .map(|(vtable, id, size)| Spec {
            vtable,
            id,
            size,
            elements: Vec::new(),
        });
        let engine = build(&mut fake, &specs, 0);
        for kinds in [
            Kinds {
                edges: true,
                junctions: false,
                constructions: false,
            },
            Kinds {
                edges: false,
                junctions: false,
                constructions: true,
            },
        ] {
            fake.reads.set(0);
            read_part(&fake, engine, IMAGE, 8, 0, kinds).unwrap();
            // Store head once; pool table and four pool heads once; the
            // chosen pool's full head once. The old eager four searches
            // made 29 reads of this fixture before scanning any entities.
            assert_eq!(fake.reads.get(), 8);
        }
    }

    /// An engine with the pools of `specs` and `entities` entities.
    fn build(fake: &mut Fake, specs: &[Spec], entities: usize) -> usize {
        let top = specs.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        let mut pools = vec![0u8; top * 8];
        let mut lists: Vec<Vec<(i32, i32)>> = vec![Vec::new(); entities];
        let mut bits = vec![0u128; entities];
        for spec in specs {
            let mut data = Vec::new();
            for (i, (entity, bytes)) in spec.elements.iter().enumerate() {
                assert_eq!(bytes.len(), spec.size);
                data.extend_from_slice(bytes);
                lists[*entity].push((spec.id as i32, i as i32));
                bits[*entity] |= 1 << spec.id;
            }
            let len = data.len();
            let dense = fake.alloc(data);
            let mut pool = vec![0u8; layout::POOL_HEAD];
            put_u64(&mut pool, 0, (IMAGE + spec.vtable) as u64);
            put_u64(&mut pool, layout::POOL_DENSE, dense as u64);
            put_u64(&mut pool, layout::POOL_DENSE + 8, (dense + len) as u64);
            let at = fake.alloc(pool);
            put_u64(&mut pools, spec.id * 8, at as u64);
        }
        let pools_len = pools.len();
        let pools_at = fake.alloc(pools);
        let mut table = vec![0u8; entities * layout::ENTITY_RECORD];
        for (e, pairs) in lists.iter().enumerate() {
            let mut list = Vec::new();
            for (t, d) in pairs {
                list.extend_from_slice(&t.to_le_bytes());
                list.extend_from_slice(&d.to_le_bytes());
            }
            let len = list.len();
            let at = if len == 0 { 0 } else { fake.alloc(list) };
            put_u64(&mut table, e * 24, at as u64);
            put_u64(&mut table, e * 24 + 8, (at + len) as u64);
            put_u64(&mut table, e * 24 + 16, (at + len) as u64);
        }
        let table_len = table.len();
        let table_at = fake.alloc(table);
        let bits_at = fake.alloc(bits.iter().flat_map(|b| b.to_le_bytes()).collect());
        let mut head = vec![0u8; 0x100];
        put_u64(&mut head, layout::POOLS, pools_at as u64);
        put_u64(&mut head, layout::POOLS + 8, (pools_at + pools_len) as u64);
        put_u64(&mut head, layout::ENTITIES, table_at as u64);
        put_u64(
            &mut head,
            layout::ENTITIES + 8,
            (table_at + table_len) as u64,
        );
        put_u64(&mut head, layout::BITS, bits_at as u64);
        fake.alloc(head)
    }

    struct Turn {
        from: usize,
        lane_in: i32,
        to: usize,
        lane_out: i32,
        road: bool,
        tram: bool,
    }

    struct Phase {
        locked: Vec<i32>,
        duration: f32,
        minimum: f32,
        skip: bool,
    }

    struct Config {
        node: usize,
        turns: Vec<Turn>,
        crosswalks: Vec<usize>,
        preference: i32,
        light: i32,
        double_slip: bool,
        custom: bool,
        phases: Vec<Phase>,
    }

    /// Nodes 10-13 and 20 (by position), edges 1-3 (1 and 2 streets, 3 a
    /// track) and 4 (a track from 13 to 20), and the junction configs.
    /// Nodes by entity and position; edges as entity, node0, node1, street.
    type World = (
        Vec<(usize, [f32; 3])>,
        Vec<(usize, usize, usize, bool)>,
        Vec<Config>,
    );

    fn junction_world() -> World {
        // Spread over several 256 m cells, on both sides of 0. Node 13's x
        // is 255.95 m to 1 mm (its junction's cell 0) but 256.0 m to 0.1 m
        // (the edge row starting there: cell 1).
        let nodes = vec![
            (10, [0.0, 0.0, 0.0]),
            (11, [3000.0004, -20.0005, 3.25]),
            (12, [-2600.5, 5000.0015, -0.0004]),
            (13, [255.950_01, 300.0, 1.0]),
            (20, [-50.0, -7000.0, 0.0]),
            (30, [9.0, 9.0, 9.0]),
        ];
        let edges = vec![
            (1, 10, 11, true),
            (2, 11, 12, true),
            (3, 12, 13, false),
            (4, 20, 13, false),
        ];
        let configs = vec![
            Config {
                node: 11,
                turns: vec![
                    Turn {
                        from: 1,
                        lane_in: 0,
                        to: 2,
                        lane_out: 1,
                        road: true,
                        tram: false,
                    },
                    Turn {
                        from: 2,
                        lane_in: 1,
                        to: 1,
                        lane_out: 0,
                        road: true,
                        tram: true,
                    },
                ],
                crosswalks: vec![2, 1],
                preference: 2,
                light: -1,
                double_slip: false,
                custom: true,
                phases: vec![
                    Phase {
                        locked: vec![2, 0],
                        duration: 30.0,
                        minimum: 5.5,
                        skip: true,
                    },
                    Phase {
                        locked: vec![3, 1],
                        duration: 25.0625,
                        minimum: 0.0,
                        skip: false,
                    },
                ],
            },
            Config {
                node: 12,
                turns: vec![],
                // Both locked together: their order makes no difference.
                crosswalks: vec![3, 2],
                preference: 0,
                light: 3,
                double_slip: true,
                custom: false,
                phases: vec![
                    Phase {
                        locked: vec![1, 0],
                        duration: 12.0,
                        minimum: 0.0,
                        skip: false,
                    },
                    Phase {
                        locked: vec![],
                        duration: 20.0,
                        minimum: 4.0,
                        skip: true,
                    },
                ],
            },
            Config {
                node: 13,
                turns: vec![Turn {
                    from: 3,
                    lane_in: 0,
                    to: 4,
                    lane_out: 0,
                    road: false,
                    tram: false,
                }],
                crosswalks: vec![],
                preference: 1,
                light: -1,
                double_slip: false,
                custom: false,
                phases: vec![],
            },
            // Two crosswalks, one of them locked twice: which one the row
            // names twice depends on the set's order, so it is left to the
            // game's Lua.
            Config {
                node: 10,
                turns: vec![],
                crosswalks: vec![2, 1],
                preference: 0,
                light: -1,
                double_slip: false,
                custom: false,
                phases: vec![Phase {
                    locked: vec![0, 1, 1],
                    duration: 10.0,
                    minimum: 0.0,
                    skip: false,
                }],
            },
            // At no edge: junctions.lua never reads it.
            Config {
                node: 30,
                turns: vec![],
                crosswalks: vec![],
                preference: 0,
                light: -1,
                double_slip: false,
                custom: false,
                phases: vec![],
            },
        ];
        (nodes, edges, configs)
    }

    /// A phmap flat_hash_set<int> of `ids` in that slot order, as
    /// `crate::junctions` reads it, written into `raw` at 0x18.
    fn crosswalk_set(fake: &mut Fake, raw: &mut [u8], ids: &[usize]) {
        if ids.is_empty() {
            return;
        }
        let capacity = 7;
        let mut tags = vec![0x80u8; capacity + 1];
        tags[capacity] = 0xff;
        let mut slots = vec![0u8; capacity * 4];
        for (i, id) in ids.iter().enumerate() {
            tags[i] = 0x11;
            slots[i * 4..i * 4 + 4].copy_from_slice(&(*id as i32).to_le_bytes());
        }
        put_u64(raw, 0x18, fake.alloc(tags) as u64);
        put_u64(raw, 0x20, fake.alloc(slots) as u64);
        put_u64(raw, 0x28, ids.len() as u64);
        put_u64(raw, 0x30, capacity as u64);
    }

    fn vector_of(fake: &mut Fake, raw: &mut [u8], at: usize, bytes: Vec<u8>) {
        let len = bytes.len();
        let begin = if len == 0 { 0 } else { fake.alloc(bytes) };
        put_u64(raw, at, begin as u64);
        put_u64(raw, at + 8, (begin + len) as u64);
        put_u64(raw, at + 16, (begin + len) as u64);
    }

    fn junction_engine(fake: &mut Fake) -> usize {
        junction_engine_in(fake, 40, false, None)
    }

    fn junction_engine_in(
        fake: &mut Fake,
        entity_slots: usize,
        dense_cell: bool,
        sparse_construction: Option<usize>,
    ) -> usize {
        let (mut nodes, edges, configs) = junction_world();
        if dense_cell {
            for (i, (_, p)) in nodes.iter_mut().enumerate() {
                *p = [10.0 + i as f32, 10.0, 0.0];
            }
        }
        let mut edge_elements = Vec::new();
        for (entity, n0, n1, street) in &edges {
            let mut b = vec![0u8; layout::BASE_EDGE_SIZE];
            b[layout::EDGE_NODE0..layout::EDGE_NODE0 + 4]
                .copy_from_slice(&(*n0 as i32).to_le_bytes());
            b[layout::EDGE_NODE1..layout::EDGE_NODE1 + 4]
                .copy_from_slice(&(*n1 as i32).to_le_bytes());
            let road = if *street {
                layout::ROAD_TYPE_STREET
            } else {
                layout::ROAD_TYPE_TRACK
            };
            b[layout::EDGE_ROAD_TYPE..layout::EDGE_ROAD_TYPE + 4]
                .copy_from_slice(&road.to_le_bytes());
            let p = |n: usize| nodes.iter().find(|(e, _)| *e == n).unwrap().1;
            for k in 0..3 {
                put_f32(&mut b, layout::EDGE_POSITION0 + 4 * k, p(*n0)[k]);
                put_f32(&mut b, layout::EDGE_POSITION1 + 4 * k, p(*n1)[k]);
            }
            edge_elements.push((*entity, b));
        }
        let node_elements = nodes
            .iter()
            .map(|(entity, p)| {
                let mut b = vec![0u8; layout::BASE_NODE_SIZE];
                for (k, v) in p.iter().enumerate() {
                    put_f32(&mut b, layout::NODE_POSITION + 4 * k, *v);
                }
                (*entity, b)
            })
            .collect();
        let mut config_elements = Vec::new();
        for c in &configs {
            let mut raw = vec![0u8; layout::BASE_NODE_CONFIG_SIZE];
            let mut turns = Vec::new();
            for t in &c.turns {
                let mut b = vec![0u8; layout::TURN_SIZE];
                b[layout::TURN_SEGMENT0..][..4].copy_from_slice(&(t.from as i32).to_le_bytes());
                b[layout::TURN_LANE0..][..4].copy_from_slice(&t.lane_in.to_le_bytes());
                b[layout::TURN_SEGMENT1..][..4].copy_from_slice(&(t.to as i32).to_le_bytes());
                b[layout::TURN_LANE1..][..4].copy_from_slice(&t.lane_out.to_le_bytes());
                b[layout::TURN_ROAD] = u8::from(t.road);
                b[layout::TURN_TRAM] = u8::from(t.tram);
                turns.extend(b);
            }
            vector_of(fake, &mut raw, layout::CONFIG_TURNS, turns);
            crosswalk_set(fake, &mut raw, &c.crosswalks);
            let mut phases = Vec::new();
            for p in &c.phases {
                let mut b = vec![0u8; layout::PHASE_SIZE];
                vector_of(
                    fake,
                    &mut b,
                    layout::PHASE_LOCKED,
                    p.locked.iter().flat_map(|l| l.to_le_bytes()).collect(),
                );
                put_f32(&mut b, layout::PHASE_DURATION, p.duration);
                put_f32(&mut b, layout::PHASE_MINIMUM, p.minimum);
                b[layout::PHASE_SKIP] = u8::from(p.skip);
                phases.extend(b);
            }
            vector_of(fake, &mut raw, layout::CONFIG_PHASES, phases);
            raw[layout::CONFIG_DOUBLE_SLIP] = u8::from(c.double_slip);
            raw[layout::CONFIG_PREFERENCE..][..4].copy_from_slice(&c.preference.to_le_bytes());
            raw[layout::CONFIG_LIGHT_TYPE..][..4].copy_from_slice(&c.light.to_le_bytes());
            raw[layout::CONFIG_CUSTOM_PHASES] = u8::from(c.custom);
            config_elements.push((c.node, raw));
        }
        let mut constructions = construction_elements(fake);
        if let Some(entity) = sparse_construction {
            let mut raw = vec![0u8; layout::CONSTRUCTION_SIZE];
            let first = string(fake, "");
            let second = string(fake, "station/high_id.con");
            raw[0..0x20].copy_from_slice(&first);
            raw[0x20..0x40].copy_from_slice(&second);
            put_f32(&mut raw, layout::CONSTRUCTION_X, 10.0);
            put_f32(&mut raw, layout::CONSTRUCTION_Y, 10.0);
            constructions.push((entity, raw));
        }
        if dense_cell {
            for (_, raw) in &mut constructions {
                put_f32(raw, layout::CONSTRUCTION_X, 10.0);
                put_f32(raw, layout::CONSTRUCTION_Y, 10.0);
            }
        }
        let specs = [
            Spec {
                vtable: layout::BASE_EDGE_POOL_VTABLE,
                id: 3,
                size: layout::BASE_EDGE_SIZE,
                elements: edge_elements,
            },
            Spec {
                vtable: layout::BASE_NODE_POOL_VTABLE,
                id: 9,
                size: layout::BASE_NODE_SIZE,
                elements: node_elements,
            },
            Spec {
                vtable: layout::BASE_NODE_CONFIG_POOL_VTABLE,
                id: 70,
                size: layout::BASE_NODE_CONFIG_SIZE,
                elements: config_elements,
            },
            Spec {
                vtable: layout::CONSTRUCTION_POOL_VTABLE,
                id: 66,
                size: layout::CONSTRUCTION_SIZE,
                elements: constructions,
            },
        ];
        build(fake, &specs, entity_slots)
    }

    /// Constructions 31-35 (file and place) spread over cells.
    fn construction_elements(fake: &mut Fake) -> Vec<(usize, Vec<u8>)> {
        let constructions: [(usize, &str, [f32; 2]); 5] = [
            (31, "station/rail/modular_station.con", [1234.56, -98.04]),
            (32, "building/x.con", [-0.04, 255.95]),
            (33, "station/rail/modular_station.con", [-20000.15, 7.0]),
            (34, "industry/farm.con", [5000.0, 5000.0]),
            (35, "industry/farm.con", [-2560.0, -2560.04]),
        ];
        constructions
            .iter()
            .map(|(entity, file, [x, y])| {
                let mut b = vec![0u8; layout::CONSTRUCTION_SIZE];
                let first = string(fake, "");
                let second = string(fake, file);
                b[0..0x20].copy_from_slice(&first);
                b[0x20..0x40].copy_from_slice(&second);
                put_f32(&mut b, layout::CONSTRUCTION_X, *x);
                put_f32(&mut b, layout::CONSTRUCTION_Y, *y);
                (*entity, b)
            })
            .collect()
    }

    /// lanes.lua's `rowPart` and `partOf`, in a real Lua.
    fn lua_parts() -> (mlua::Lua, mlua::Function, mlua::Function) {
        let lua = mlua::Lua::new();
        let scripts = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mod/tpf3mp_1/content/scripts/"
        );
        lua.load(format!(
            "package.path = {:?} .. '?.lua;' .. package.path",
            scripts
        ))
        .exec()
        .unwrap();
        let (row_part, part_of): (mlua::Function, mlua::Function) = lua
            .load("local lanes = require('tpf3mp.lanes') return lanes.rowPart, lanes.partOf")
            .eval()
            .unwrap();
        (lua, row_part, part_of)
    }

    /// Nodes (entity and place) and level crossings (entity, nodes in the
    /// order the component holds them, type index) of a crossing world:
    /// places that round half up where ties go to even (0.0625 m, -0.1875 m),
    /// one at a cell's edge, nodes stored out of order.
    type Crossings = (Vec<(usize, [f32; 3])>, Vec<(usize, Vec<i32>, i32)>);

    fn crossing_world() -> Crossings {
        (
            vec![
                (10, [0.0625, 20.0, 1.0]),
                (11, [8.0, 20.0, 1.0]),
                (12, [-0.1875, -300.5, 0.0004]),
                (13, [-8.1875, -300.5, -0.0004]),
                (14, [255.9995, 7.0, 0.0]),
                (15, [263.9995, 7.0, 0.0]),
            ],
            vec![
                (30, vec![11, 10], 3),
                (31, vec![12, 13], 5),
                (32, vec![15, 14], 3),
            ],
        )
    }

    /// An engine of `world`: its nodes, its crossings (unless `no_pool`),
    /// and empty edge and junction pools.
    fn crossing_engine(fake: &mut Fake, world: &Crossings, no_pool: bool) -> usize {
        let (nodes, crossings) = world;
        let node_elements = nodes
            .iter()
            .map(|(entity, p)| {
                let mut b = vec![0u8; layout::BASE_NODE_SIZE];
                for (k, v) in p.iter().enumerate() {
                    put_f32(&mut b, layout::NODE_POSITION + 4 * k, *v);
                }
                (*entity, b)
            })
            .collect();
        let mut crossing_elements = Vec::new();
        for (entity, ids, type_index) in crossings {
            let mut b = vec![0u8; layout::RAILROAD_CROSSING_SIZE];
            vector_of(
                fake,
                &mut b,
                layout::CROSSING_NODES,
                ids.iter().flat_map(|n| n.to_le_bytes()).collect(),
            );
            b[layout::CROSSING_TYPE_INDEX..][..4].copy_from_slice(&type_index.to_le_bytes());
            crossing_elements.push((*entity, b));
        }
        let mut specs = vec![
            Spec {
                vtable: layout::BASE_EDGE_POOL_VTABLE,
                id: 3,
                size: layout::BASE_EDGE_SIZE,
                elements: Vec::new(),
            },
            Spec {
                vtable: layout::BASE_NODE_POOL_VTABLE,
                id: 9,
                size: layout::BASE_NODE_SIZE,
                elements: node_elements,
            },
            Spec {
                vtable: layout::BASE_NODE_CONFIG_POOL_VTABLE,
                id: 70,
                size: layout::BASE_NODE_CONFIG_SIZE,
                elements: Vec::new(),
            },
        ];
        if !no_pool {
            specs.push(Spec {
                vtable: layout::RAILROAD_CROSSING_POOL_VTABLE,
                id: 71,
                size: layout::RAILROAD_CROSSING_SIZE,
                elements: crossing_elements,
            });
        }
        build(fake, &specs, 40)
    }

    /// crossings.lua's rows of `world`, in a real Lua, its types named
    /// `type/<index>.rcr`; and lanes.lua's `rowPart`.
    fn lua_crossings(world: &Crossings) -> (Vec<String>, mlua::Lua, mlua::Function) {
        let (lua, row_part, _) = lua_parts();
        let (nodes, crossings) = world;
        let places = lua.create_table().unwrap();
        for (entity, p) in nodes {
            let t = lua.create_table().unwrap();
            t.set("x", f64::from(p[0])).unwrap();
            t.set("y", f64::from(p[1])).unwrap();
            t.set("z", f64::from(p[2])).unwrap();
            places.set(*entity, t).unwrap();
        }
        let list = lua.create_table().unwrap();
        for (entity, ids, type_index) in crossings {
            let c = lua.create_table().unwrap();
            c.set("nodes", ids.clone()).unwrap();
            c.set("typeIndex", *type_index).unwrap();
            list.set(*entity, c).unwrap();
        }
        lua.globals().set("PLACES", places).unwrap();
        lua.globals().set("CROSSINGS", list).unwrap();
        let rows: Vec<String> = lua
            .load(
                "api = { type = { ComponentType = { BASE_NODE = 1, RAILROAD_CROSSING = 2 } },
                   engine = {
                     getComponent = function(e, kind)
                       if kind == 1 then return PLACES[e] and { position = PLACES[e] } end
                       if kind == 2 then return CROSSINGS[e] end
                     end,
                     getEntitiesWithComponent = function(kind)
                       local out = {} for e in pairs(CROSSINGS) do out[#out + 1] = e end return out
                     end,
                     system = { railRoadCrossingSystem = {} } },
                   res = { railroadCrossingTypeRep = { getName = function(i) return 'type/' .. i .. '.rcr' end } } }
                 return (require('tpf3mp.crossings').rows(api))",
            )
            .eval()
            .unwrap();
        (rows, lua, row_part)
    }

    /// The hook reads each level crossing of a part as crossings.lua makes
    /// its row (its nodes' places in whole millimetres rounded half up,
    /// sorted; its type named by the game's Lua), in the part lanes.lua's
    /// `rowPart` puts that row in, every crossing in exactly one part.
    #[test]
    fn crossing_rows_read_as_the_mods_lua_makes_them_in_its_parts() {
        let world = crossing_world();
        let (lua_rows, _lua, row_part) = lua_crossings(&world);
        assert_eq!(lua_rows.len(), 3);
        let names: HashMap<i32, String> = [3, 5]
            .into_iter()
            .map(|i| (i, format!("type/{i}.rcr")))
            .collect();
        let mut fake = Fake::new();
        let engine = crossing_engine(&mut fake, &world, false);
        let junctions = Kinds {
            edges: false,
            junctions: true,
            constructions: false,
        };
        for n in [1u32, 2, 3, 10] {
            let mut all = Vec::new();
            for k in 0..n {
                let part = read_part(&fake, engine, IMAGE, n, k, junctions).unwrap();
                for c in &part.crossings {
                    let row = format!("{}|{}", c.head, names[&c.type_index]);
                    let placed: u32 = row_part.call((0, row.clone(), n)).unwrap();
                    assert_eq!(placed, k, "{row} in part {k} of {n}");
                    all.push(row);
                }
            }
            all.sort();
            assert_eq!(
                all, lua_rows,
                "every crossing once, as the mod's Lua makes it ({n} parts)"
            );
        }
        // The same world numbered otherwise, its nodes stored the other way
        // round, a type of the same file under another index: the same rows.
        let (places, crossings) = world.clone();
        let shift = |e: usize| e + 5;
        let renumbered: Crossings = (
            places.iter().map(|(e, p)| (shift(*e), *p)).collect(),
            crossings
                .iter()
                .map(|(e, ids, t)| {
                    let mut ids: Vec<i32> = ids.iter().map(|n| *n + 5).collect();
                    ids.reverse();
                    (shift(*e), ids, if *t == 5 { 8 } else { *t })
                })
                .collect(),
        );
        let mut names = names.clone();
        names.insert(8, "type/5.rcr".into());
        let mut other = Fake::new();
        let engine2 = crossing_engine(&mut other, &renumbered, false);
        let mut again = Vec::new();
        for c in read_part(&other, engine2, IMAGE, 1, 0, junctions)
            .unwrap()
            .crossings
        {
            again.push(format!("{}|{}", c.head, names[&c.type_index]));
        }
        again.sort();
        assert_eq!(again, lua_rows, "no entity id and no index is in a row");
        // An edges turn reads no crossing pool, and keeps none.
        let part = read_part(
            &fake,
            engine,
            IMAGE,
            1,
            0,
            Kinds {
                edges: true,
                junctions: false,
                constructions: false,
            },
        )
        .unwrap();
        assert!(part.crossings.is_empty());
    }

    /// A world that never had a crossing has no pool of them: none, not a
    /// failure. A crossing that does not read fails the part.
    #[test]
    fn crossings_fail_closed_but_a_world_without_them_has_none() {
        let junctions = Kinds {
            edges: false,
            junctions: true,
            constructions: false,
        };
        let mut fake = Fake::new();
        let engine = crossing_engine(&mut fake, &crossing_world(), true);
        assert!(
            read_part(&fake, engine, IMAGE, 1, 0, junctions)
                .unwrap()
                .crossings
                .is_empty()
        );
        // A pool whose head does not read is no absence of crossings: here
        // the pool table's first slot, empty in this world, names one.
        let mut fake = Fake::new();
        let engine = crossing_engine(&mut fake, &crossing_world(), true);
        let pools = u64::from_le_bytes(
            fake.read(engine + layout::POOLS, 8)
                .unwrap()
                .try_into()
                .unwrap(),
        ) as usize;
        let table = fake.blocks.get_mut(&pools).unwrap();
        table[0..8].copy_from_slice(&8u64.to_le_bytes());
        let Err(err) = read_part(&fake, engine, IMAGE, 1, 0, junctions) else {
            panic!("an unreadable pool passed for no crossings");
        };
        assert!(err.contains("does not read"), "{err}");
        for (nodes, why) in [
            (vec![], "has no node"),
            (vec![10, 10], "two nodes in one place"),
            (vec![10, 39], "node 39 has no position"),
            (vec![10, -1], "names node -1"),
            (vec![10; 9], "a level crossing's nodes"),
        ] {
            let (places, _) = crossing_world();
            let world = (places, vec![(30, nodes, 3)]);
            let mut fake = Fake::new();
            let engine = crossing_engine(&mut fake, &world, false);
            let Err(err) = read_part(&fake, engine, IMAGE, 1, 0, junctions) else {
                panic!("{why}: the part read");
            };
            assert!(err.contains(why), "{why}: {err}");
        }
    }

    /// Every row of the two static lanes is in exactly one of `n` parts,
    /// the parts together are the whole read, and each row's part is the
    /// one lanes.lua's `rowPart` finds from the row alone.
    #[test]
    fn parts_hold_every_row_once_where_the_mods_lua_places_it() {
        let mut fake = Fake::new();
        let engine = junction_engine(&mut fake);
        let full = network(&fake, engine, IMAGE).unwrap();
        let mut full_edges = full.edges.clone();
        full_edges.sort();
        let mut full_heads: Vec<String> = full
            .junctions
            .as_ref()
            .unwrap()
            .iter()
            .map(|j| j.head.clone())
            .collect();
        full_heads.sort();
        let mut full_cons = construction_rows(&fake, engine, IMAGE).unwrap();
        full_cons.sort();
        assert_eq!(full_cons.len(), 5);
        let (_lua, row_part, _) = lua_parts();
        let place =
            |lane: u32, row: &str, n: u32| -> u32 { row_part.call((lane, row, n)).unwrap() };
        let mut split = false;
        for n in [1u32, 2, 3, 10] {
            let (mut edges, mut heads, mut deferred, mut cons) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for k in 0..n {
                let part = read_part(&fake, engine, IMAGE, n, k, Kinds::ALL).unwrap();
                assert_eq!((part.n, part.k), (n, k));
                // Each kind read alone is that kind of the whole part, and
                // nothing of the others.
                let alone = |name| {
                    read_part(&fake, engine, IMAGE, n, k, Kinds::from_name(name).unwrap()).unwrap()
                };
                let (e, j, c) = (alone("edges"), alone("junctions"), alone("constructions"));
                assert_eq!(e.edges, part.edges);
                assert!(
                    e.junctions.is_empty() && e.deferred.is_empty() && e.constructions.is_empty()
                );
                assert_eq!(j.junctions, part.junctions);
                assert_eq!(j.deferred, part.deferred);
                assert!(j.edges.is_empty() && j.constructions.is_empty());
                assert_eq!(c.constructions, part.constructions);
                assert!(c.edges.is_empty() && c.junctions.is_empty());
                for row in &part.edges {
                    assert_eq!(place(0, row, n), k, "{row}");
                }
                for j in &part.junctions {
                    let row = format!("junction:{}|Auto|default|{}", j.head, j.tail);
                    assert_eq!(place(0, &row, n), k, "{row}");
                }
                for row in &part.constructions {
                    assert_eq!(place(1, row, n), k, "{row}");
                }
                split |= n > 1 && !part.edges.is_empty() && part.edges.len() < full_edges.len();
                edges.extend(part.edges);
                heads.extend(part.junctions.into_iter().map(|j| j.head));
                deferred.extend(part.deferred);
                cons.extend(part.constructions);
            }
            edges.sort();
            heads.sort();
            deferred.sort();
            cons.sort();
            assert_eq!(edges, full_edges, "{n} parts");
            assert_eq!(heads, full_heads, "{n} parts");
            assert_eq!(deferred, full.deferred, "{n} parts");
            assert_eq!(cons, full_cons, "{n} parts");
        }
        assert!(split, "the world spreads over several parts");
        assert!(read_part(&fake, engine, IMAGE, 0, 0, Kinds::ALL).is_err());
        assert!(read_part(&fake, engine, IMAGE, 3, 3, Kinds::ALL).is_err());
        assert!(read_part(&fake, engine, IMAGE, MAX_PARTS + 1, 0, Kinds::ALL).is_err());
    }

    /// A full spatial cell remains a full-table component-bit scan, and even
    /// an empty part visits the same entity slots. This count is the work
    /// shape, not a wall-clock bound.
    #[test]
    fn every_part_scans_the_full_entity_table_for_dense_and_empty_cells() {
        const ENTITY_SLOTS: usize = 4096;
        let mut fake = Fake::new();
        let engine = junction_engine_in(&mut fake, ENTITY_SLOTS, true, None);

        // Every edge, junction and construction is in cell zero, so this is
        // the skewed case: one part owns all rows while its neighbors are empty.
        let dense = read_part(&fake, engine, IMAGE, 10, 0, Kinds::ALL).unwrap();
        assert_eq!(dense.edges.len(), 4);
        assert!(!dense.junctions.is_empty());
        assert_eq!(dense.constructions.len(), 5);
        assert_eq!(dense.component_bit_entity_slots_scanned, ENTITY_SLOTS);
        assert!(
            dense
                .timing
                .contains("component bits 4096/4096 entity slots")
        );

        let empty = read_part(&fake, engine, IMAGE, 10, 1, Kinds::ALL).unwrap();
        assert!(empty.edges.is_empty());
        assert!(empty.junctions.is_empty());
        assert!(empty.constructions.is_empty());
        assert_eq!(empty.component_bit_entity_slots_scanned, ENTITY_SLOTS);
    }

    /// Sparse component bits above 115k remain discoverable: the scanner
    /// visits through the final entity slot rather than relying on a low cap.
    #[test]
    fn part_scans_past_115k_slots_to_find_a_sparse_high_entity() {
        const ENTITY_SLOTS: usize = 131_073;
        const HIGH_ID: usize = ENTITY_SLOTS - 1;
        let mut fake = Fake::new();
        let engine = junction_engine_in(&mut fake, ENTITY_SLOTS, false, Some(HIGH_ID));

        let high = read_part(
            &fake,
            engine,
            IMAGE,
            10,
            0,
            Kinds {
                edges: false,
                junctions: false,
                constructions: true,
            },
        )
        .unwrap();
        assert!(
            high.constructions
                .iter()
                .any(|row| row.contains("/station/high_id.con@")),
            "the construction at entity {HIGH_ID} was not found: {:?}",
            high.constructions
        );
        assert_eq!(high.component_bit_entity_slots_scanned, ENTITY_SLOTS);

        let empty = read_part(
            &fake,
            engine,
            IMAGE,
            10,
            2,
            Kinds {
                edges: false,
                junctions: false,
                constructions: true,
            },
        )
        .unwrap();
        assert!(empty.constructions.is_empty());
        assert_eq!(empty.component_bit_entity_slots_scanned, ENTITY_SLOTS);
    }

    /// A junction is placed by its row's millimetres, an edge starting at
    /// the same node by its row's tenths: 255.95001 m is cell 0 for one and
    /// cell 1 for the other, in Rust and in lanes.lua alike.
    #[test]
    fn a_junction_is_placed_by_its_millimetres_and_an_edge_by_its_tenths() {
        let x = 255.950_01_f32;
        assert_eq!(millimetres(x).unwrap(), 255_950);
        assert_eq!(tenths(x).unwrap(), 2_560);
        assert_eq!(part_of(millimetres(x).unwrap(), 0, CELL_MM, 10), 0);
        assert_eq!(part_of(tenths(x).unwrap(), 0, CELL_TENTHS, 10), 1);
        let (_lua, row_part, _) = lua_parts();
        let junction: u32 = row_part
            .call((0, "junction:Street:255950,0,0|x", 10))
            .unwrap();
        let edge: u32 = row_part.call((0, "256,0,0>300,0,0:t|lanes:", 10)).unwrap();
        assert_eq!((junction, edge), (0, 1));
    }

    /// `part_of` is lanes.lua's `partOf` on both sides of 0 and at the
    /// cells' edges.
    #[test]
    fn part_of_is_the_mods_part_of() {
        let (_lua, _, lua_part_of) = lua_parts();
        let values = [
            -123_456_789i64,
            -5_121,
            -5_120,
            -2_561,
            -2_560,
            -2_559,
            -1,
            0,
            1,
            2_559,
            2_560,
            5_119,
            987_654_321,
        ];
        for x in values {
            for y in values {
                for n in [1u32, 3, 10, 1024] {
                    let theirs: f64 = lua_part_of
                        .call((x as f64, y as f64, CELL_TENTHS as f64, f64::from(n)))
                        .unwrap();
                    assert_eq!(
                        f64::from(part_of(x, y, CELL_TENTHS, n)),
                        theirs,
                        "{x},{y} of {n}"
                    );
                }
            }
        }
    }

    /// junctions.lua's own `rows` over a fake `api` holding the same world,
    /// and its `rowsFromParts` over the hook's parts.
    /// junctions.lua's own `rows`, then `rowsFromParts` over the hook's
    /// parts with `rowsOf` the nodes it deferred, then the deferred rows.
    fn lua_junctions(
        parts: &[Junction],
        deferred: &[usize],
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let (nodes, edges, configs) = junction_world();
        let lua = mlua::Lua::new();
        let scripts = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mod/tpf3mp_1/content/scripts/"
        );
        lua.load(format!(
            "package.path = {:?} .. '?.lua;' .. package.path",
            scripts
        ))
        .exec()
        .unwrap();
        let mut world = String::from("local nodes, edges, configs = {}, {}, {}\n");
        for (e, p) in &nodes {
            world += &format!(
                "nodes[{e}] = {{ position = {{ x = {}, y = {}, z = {} }} }}\n",
                f64::from(p[0]),
                f64::from(p[1]),
                f64::from(p[2])
            );
        }
        for (e, n0, n1, street) in &edges {
            world += &format!("edges[{e}] = {{ node0 = {n0}, node1 = {n1}, street = {street} }}\n");
        }
        for c in &configs {
            let turns: Vec<String> = c
                .turns
                .iter()
                .map(|t| {
                    format!(
                        "{{ segment0 = {}, lane0 = {}, segment1 = {}, lane1 = {}, withRoad = {}, withTram = {} }}",
                        t.from, t.lane_in, t.to, t.lane_out, t.road, t.tram
                    )
                })
                .collect();
            let walks: Vec<String> = c.crosswalks.iter().map(ToString::to_string).collect();
            let phases: Vec<String> = c
                .phases
                .iter()
                .map(|p| {
                    let locked: Vec<String> = p.locked.iter().map(ToString::to_string).collect();
                    format!(
                        "{{ lockedLanes = {{ {} }}, duration = {}, minDuration = {}, canSkip = {} }}",
                        locked.join(", "),
                        f64::from(p.duration),
                        f64::from(p.minimum),
                        p.skip
                    )
                })
                .collect();
            world += &format!(
                "configs[{}] = {{ laneConnections = {{ {} }}, crosswalks = {{ {} }}, trafficLightPreference = {}, \
                 doubleSlipSwitch = {}, userModifiedTrafficLightStates = {}, \
                 trafficLightConfig = {{ trafficLightType = {}, states = {{ {} }} }} }}\n",
                c.node,
                turns.join(", "),
                walks.join(", "),
                c.preference,
                c.double_slip,
                c.custom,
                c.light,
                phases.join(", ")
            );
        }
        world += r#"
local function segments(street)
  return function(node)
    local out = {}
    for e, edge in pairs(edges) do
      if edge.street == street and (edge.node0 == node or edge.node1 == node) then out[#out + 1] = e end
    end
    table.sort(out)
    return out
  end
end
local function nodeMap(street)
  return function()
    local out = {}
    for e, edge in pairs(edges) do
      if edge.street == street then
        for _, n in ipairs({ edge.node0, edge.node1 }) do
          out[n] = out[n] or {}
          table.insert(out[n], e)
        end
      end
    end
    return out
  end
end
api = {
  type = {
    ComponentType = { BASE_NODE_CONFIG = "config", BASE_NODE = "node", BASE_EDGE = "edge" },
    enum = { TrafficLightPreference = { AUTO = 0, YES = 1, NO = 2 } },
  },
  res = { trafficLightTypeRep = { getName = function(i) return "lights/type" .. i .. ".lua" end } },
  engine = {
    getComponent = function(id, kind)
      if kind == "config" then return configs[id] end
      if kind == "node" then return nodes[id] end
      if kind == "edge" then return edges[id] end
    end,
    system = { streetSystem = {
      getNode2StreetEdgeMap = nodeMap(true), getNode2TrackEdgeMap = nodeMap(false),
      getNodeStreetSegments = segments(true), getNodeTrackSegments = segments(false),
    } },
  },
}
"#;
        lua.load(&world).exec().unwrap();
        let junctions: mlua::Table = lua
            .load("return require('tpf3mp.junctions')")
            .eval()
            .unwrap();
        let api: mlua::Table = lua.globals().get("api").unwrap();
        let rows: Vec<String> = junctions
            .get::<mlua::Function>("rows")
            .unwrap()
            .call(api.clone())
            .unwrap();
        let table = lua.create_table().unwrap();
        let list = |items: Vec<mlua::Value>| lua.create_sequence_from(items).unwrap();
        table
            .set(
                "heads",
                list(
                    parts
                        .iter()
                        .map(|j| mlua::Value::String(lua.create_string(&j.head).unwrap()))
                        .collect(),
                ),
            )
            .unwrap();
        table
            .set(
                "tails",
                list(
                    parts
                        .iter()
                        .map(|j| mlua::Value::String(lua.create_string(&j.tail).unwrap()))
                        .collect(),
                ),
            )
            .unwrap();
        table
            .set(
                "preferences",
                list(
                    parts
                        .iter()
                        .map(|j| mlua::Value::Number(f64::from(j.preference)))
                        .collect(),
                ),
            )
            .unwrap();
        table
            .set(
                "lights",
                list(
                    parts
                        .iter()
                        .map(|j| mlua::Value::Number(f64::from(j.light)))
                        .collect(),
                ),
            )
            .unwrap();
        let mut made: Vec<String> = junctions
            .get::<mlua::Function>("rowsFromParts")
            .unwrap()
            .call((api.clone(), table))
            .unwrap();
        let left: Vec<String> = junctions
            .get::<mlua::Function>("rowsOf")
            .unwrap()
            .call((api, deferred.to_vec()))
            .unwrap();
        made.extend(left.iter().cloned());
        made.sort();
        (rows, made, left)
    }

    #[test]
    fn junction_rows_read_as_the_mods_lua_makes_them() {
        let mut fake = Fake::new();
        let engine = junction_engine(&mut fake);
        let network = network(&fake, engine, IMAGE).unwrap();
        let parts = network.junctions.unwrap();
        // Node 11 has two crosswalks and a phase locking one of them, node
        // 10 one locking one of its two twice: their rows are left to the
        // game's Lua. Node 12 locks both together.
        assert_eq!(network.deferred, vec![10, 11]);
        assert_eq!(parts.len(), 2, "the node at no edge is left out");
        let (rows, made, left) = lua_junctions(&parts, &network.deferred);
        assert_eq!(rows.len(), 4);
        assert_eq!(left.len(), 2);
        assert_eq!(made, rows);
    }

    #[test]
    fn the_lanes_text_is_the_mods_summary() {
        let mut fake = Fake::new();
        let engine = junction_engine(&mut fake);
        let network = network(&fake, engine, IMAGE).unwrap();
        let (junction_rows, _, left) =
            lua_junctions(network.junctions.as_ref().unwrap(), &network.deferred);
        // lanes.lua's summary over the same rows, in a real Lua.
        let lua = mlua::Lua::new();
        let scripts = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mod/tpf3mp_1/content/scripts/"
        );
        lua.load(format!(
            "package.path = {:?} .. '?.lua;' .. package.path",
            scripts
        ))
        .exec()
        .unwrap();
        let rows = lua.create_table().unwrap();
        for row in &network.edges {
            rows.raw_push(row.as_str()).unwrap();
        }
        for row in &junction_rows {
            rows.raw_push(format!("junction:{row}")).unwrap();
        }
        let theirs: String = lua
            .load(
                r#"local rows = ... local lanes = require("tpf3mp.lanes") table.sort(rows) return #rows .. ":" .. lanes.hash(table.concat(rows, "\30"))"#,
            )
            .call(rows)
            .unwrap();
        let preferences =
            HashMap::from([(0, "Auto".to_string()), (1, "Yes".into()), (2, "No".into())]);
        let lights = HashMap::from([(3, "lights/type3.lua".to_string())]);
        assert_eq!(
            summary(&network, &preferences, &lights, &left).unwrap(),
            theirs
        );
        assert!(
            summary(&network, &preferences, &lights, &[])
                .unwrap_err()
                .contains("left to the mod")
        );
        // Kept, it is summed up once.
        keep(network);
        assert_eq!(
            summary_of_last(&preferences, &lights, &left).unwrap(),
            theirs
        );
        assert!(summary_of_last(&preferences, &lights, &left).is_err());
        // A light no name is given for.
        let mut fake = Fake::new();
        let engine = junction_engine(&mut fake);
        let network = super::network(&fake, engine, IMAGE).unwrap();
        assert!(
            summary(&network, &preferences, &HashMap::new(), &left)
                .unwrap_err()
                .contains("light")
        );
    }

    #[test]
    fn the_constructions_lane_is_the_mods() {
        let constructions: Vec<(usize, (&str, &str), [f32; 2])> = vec![
            (
                3,
                ("", "station/rail/modular_station.con"),
                [1234.56, -98.04],
            ),
            (5, ("mymod_1", "building/x.con"), [0.04, -0.05]),
            (
                6,
                ("", "station/rail/modular_station.con"),
                [-20000.15, 7.0],
            ),
            (9, ("", "industry/farm.con"), [100.0, 100.0]),
        ];
        let mut fake = Fake::new();
        let mut elements = Vec::new();
        for (entity, (first, second), [x, y]) in &constructions {
            let mut b = vec![0u8; layout::CONSTRUCTION_SIZE];
            let a = string(&mut fake, first);
            let c = string(&mut fake, second);
            b[0..0x20].copy_from_slice(&a);
            b[0x20..0x40].copy_from_slice(&c);
            put_f32(&mut b, layout::CONSTRUCTION_X, *x);
            put_f32(&mut b, layout::CONSTRUCTION_Y, *y);
            elements.push((*entity, b));
        }
        let specs = [Spec {
            vtable: layout::CONSTRUCTION_POOL_VTABLE,
            id: 66,
            size: layout::CONSTRUCTION_SIZE,
            elements,
        }];
        let engine = build(&mut fake, &specs, 12);
        let ours = rows_summary(construction_rows(&fake, engine, IMAGE).unwrap());
        // lanes.lua's own read of the lane, over a fake api.
        let lua = mlua::Lua::new();
        let scripts = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mod/tpf3mp_1/content/scripts/"
        );
        lua.load(format!(
            "package.path = {:?} .. '?.lua;' .. package.path",
            scripts
        ))
        .exec()
        .unwrap();
        let mut world = String::from(
            "local cons = {}
",
        );
        for (entity, (first, second), [x, y]) in &constructions {
            world += &format!(
                "cons[{entity}] = {{ fileName = {:?}, transf = {{ 1,0,0,0, 0,1,0,0, 0,0,1,0, {}, {}, 0, 1 }} }}
",
                format!("{first}::/{second}"),
                f64::from(*x),
                f64::from(*y)
            );
        }
        world += r#"
local api = { type = { ComponentType = { CONSTRUCTION = "con" } }, engine = {
  getEntitiesWithComponent = function(kind) local out = {} for e in pairs(cons) do out[#out + 1] = e end return out end,
  getComponent = function(e, kind) return cons[e] end,
} }
local lanes = require("tpf3mp.lanes")
local out = lanes.read(api)
return out[lanes.CONSTRUCTIONS]
"#;
        let theirs: String = lua.load(&world).eval().unwrap();
        assert_eq!(ours, theirs);
        assert!(ours.starts_with("4:"));
    }

    #[test]
    fn a_junction_that_does_not_read_fails_the_junctions_alone() {
        let mut fake = Fake::new();
        let engine = junction_engine(&mut fake);
        // Edge 2 names a road type that is neither.
        let pools = u64_at(&fake.read(engine + layout::POOLS, 8).unwrap(), 0) as usize;
        let pool = u64_at(&fake.read(pools + 3 * 8, 8).unwrap(), 0) as usize;
        let dense = u64_at(&fake.read(pool + layout::POOL_DENSE, 8).unwrap(), 0) as usize;
        fake.blocks.get_mut(&dense).unwrap()[layout::BASE_EDGE_SIZE + layout::EDGE_ROAD_TYPE] = 7;
        assert!(
            network(&fake, engine, IMAGE)
                .err()
                .unwrap()
                .contains("road type 7")
        );
        // A phase locking a lane its junction does not have.
        let mut fake = Fake::new();
        let engine = junction_engine(&mut fake);
        let pools = u64_at(&fake.read(engine + layout::POOLS, 8).unwrap(), 0) as usize;
        let pool = u64_at(&fake.read(pools + 70 * 8, 8).unwrap(), 0) as usize;
        let dense = u64_at(&fake.read(pool + layout::POOL_DENSE, 8).unwrap(), 0) as usize;
        let raw = fake.read(dense, layout::BASE_NODE_CONFIG_SIZE).unwrap();
        let phases = u64_at(&raw, layout::CONFIG_PHASES) as usize;
        let locked = u64_at(&fake.read(phases, 8).unwrap(), 0) as usize;
        fake.blocks.get_mut(&locked).unwrap()[0] = 9;
        let network = network(&fake, engine, IMAGE).unwrap();
        assert_eq!(network.edges.len(), 4);
        assert!(
            network
                .junctions
                .unwrap_err()
                .contains("references no lane")
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_process_reads_its_own_memory_and_not_beyond() {
        let process = Process::new();
        let blocks: Vec<Vec<u8>> = (0..64).map(|i| vec![i as u8; 4096 + i]).collect();
        for _ in 0..2 {
            for (i, block) in blocks.iter().enumerate() {
                let at = block.as_ptr() as usize;
                assert_eq!(process.read(at, block.len()).unwrap(), *block);
                assert_eq!(process.read(at + 7, 9).unwrap(), vec![i as u8; 9]);
            }
        }
        assert!(process.read(0, 8).is_none());
        assert!(
            process.read(16, 8).is_none(),
            "the first page is never readable"
        );
        assert!(process.read(usize::MAX - 4, 8).is_none());
        let regions = process.regions.borrow();
        assert!(
            regions.windows(2).all(|w| w[0].1 < w[1].0),
            "sorted and apart"
        );
    }

    #[test]
    fn the_mode_is_off_unless_asked() {
        assert_eq!(Mode::from_env(None).0, Mode::Off);
        assert_eq!(Mode::from_env(Some(" compare ")).0, Mode::Compare);
        assert_eq!(Mode::from_env(Some("ON")).0, Mode::On);
        let (mode, why) = Mode::from_env(Some("yes"));
        assert_eq!(mode, Mode::Off);
        assert!(why.unwrap().contains(ENV));
    }
}
