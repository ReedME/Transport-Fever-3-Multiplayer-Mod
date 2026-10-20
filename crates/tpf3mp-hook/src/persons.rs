//! The person-order fixes: the batches the person simulation consumes in
//! an order that is the engine's history, not the room's state, sorted by
//! entity id right before the engine reads them (docs/HOOKS.md, "The
//! person-order fixes").
//!
//! Ported from silver2127's TPF2 Multiplayer (`tpf2-multiplayer`,
//! docs/re/HOTJOIN_ORDER.md "Mechanism" and "The fix",
//! native/src/slice/hotjoin_order.inl and the Linux reference
//! native/linux/src/order_canon_linux.cpp): there the person simulation read
//! its batches in ECS node-list order, which is add/swap-remove history in a
//! running world and registration order in a loaded one, and drew one random
//! stream across each batch in that order. The same draw then lands on
//! another person, or another building. TF3 build 40408 has the same shape,
//! read statically with `tools/tpfre`:
//!
//! | TF2 batch | TF3 site | what reads it in order |
//! |---|---|---|
//! | candidates | `GetTargetsByLandUse` `0x8e3d65` | `BinarySearchIndex` `0x8e4400`: cumulative free capacity, one draw |
//! | departures | `SimEntityAtBuildingSystem::Update2` `0xb05f92` | `NoteAtBuildingPersonsLeave` `0xb2e950`: one generator, batch order |
//! | arrivals | `PersonMoveSystem::Update2` `0xaecfcd` | `NoteWalkPersonsArrived` `0xb33a30`: stay draws, batch order |
//! | idle | `SimEntityNeedsPathSystem::Update` `0xb18213` | `PathFactory::Compute` `0x8d11e0`: a generator per chunk start |
//! | freed ids | `Engine::EndModification` `0x2bb4fd1` | the FIFO free-id deque `AddEntity` pops |
//!
//! Each batch is a `std::vector<Entity>` (4-byte ids) the engine is about
//! to read and that nothing indexes by position across the sort, so the
//! hook sorts it ascending in place. TF2's capacity-map relink is not
//! ported: TF3's `SimEntityUpdateHelper` keeps phmap flat maps, not linked
//! lists (the doc says why).
//!
//! The sorts change what the simulation decides, so **every game of a room
//! must run the same set**: one game with a batch sorted and another with
//! it in the engine's order decide apart. They are on by default;
//! [`MASTER_ENV`] `=0` turns all of them off and each batch's own switch
//! turns it alone off, and either must then be set alike on every machine
//! of the room.
//!
//! Each fix installs on its own and fails closed on its own, as the order
//! fixes do (`order.rs`): its profile target must resolve, the bytes at the
//! site (and the instructions around it that name the vector) must be the
//! measured ones, every read on the game's thread is checked through
//! [`crate::image`], a vector of another shape is refused for that call and
//! said once in hook.log, and a panic switches the fix off for the game.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use tpf3mp_hookcore::detour::{SavedRegs, Splice, SpliceHook};
use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::image::Readable as Probe;
use crate::log;
use crate::order::Sorted;
use crate::perf::{self, Piece};

/// Set to `0` (or `off`, `false`, `no`), none of the person-order fixes
/// installs: every batch keeps the engine's order. Must be the same on
/// every machine of a room.
pub const MASTER_ENV: &str = "TPF3MP_HOOK_PERSON_ORDER";

/// A sanity bound on one batch.
pub const MAX_IDS: u64 = 1 << 24;

/// One batch's fix: where it splices in, what it expects there, and where
/// the vectors are at the site.
pub struct Batch {
    /// The fix's name in hook.log.
    pub fix: &'static str,
    /// Its own kill switch.
    pub toggle_env: &'static str,
    /// The profile target of the site.
    pub target: &'static str,
    /// The bytes at the site: the stolen instructions and those after them
    /// up to the instruction that hands the vector on, so the slots the
    /// hook sorts are the ones the engine reads.
    pub expected: &'static [u8],
    pub steal: usize,
    /// More bytes the function must hold, at offsets from the site: the
    /// instructions that fill or read the vector elsewhere.
    pub context: &'static [(i64, &'static [u8])],
    /// What the installed line says the fix does.
    pub does: &'static str,
    /// The alive line's interval, in calls.
    pub alive_every: u64,
    /// The vectors at the site: the address of each `std::vector<Entity>`
    /// (begin at +0, end at +8, capacity at +0x10).
    vectors: fn(&SavedRegs, &mut Probe) -> Result<Vectors, &'static str>,
    hook: SpliceHook,
    state: &'static State,
}

/// Up to two vectors at one site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vectors {
    at: [u64; 2],
    len: usize,
}

impl Vectors {
    pub fn one(at: u64) -> Self {
        Self {
            at: [at, 0],
            len: 1,
        }
    }

    pub fn two(first: u64, second: u64) -> Self {
        Self {
            at: [first, second],
            len: 2,
        }
    }

    pub fn as_slice(&self) -> &[u64] {
        &self.at[..self.len]
    }
}

/// A fix's counters, from any thread.
pub struct State {
    broken: AtomicBool,
    /// Set once the fix's splice is in the game's code.
    installed: AtomicBool,
    calls: AtomicU64,
    reordered: AtomicU64,
    refused: AtomicU64,
    in_step_calls: AtomicU64,
    in_step_reordered: AtomicU64,
    last_refusal: Mutex<Option<&'static str>>,
}

impl State {
    pub const fn new() -> Self {
        Self {
            broken: AtomicBool::new(false),
            installed: AtomicBool::new(false),
            calls: AtomicU64::new(0),
            reordered: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            in_step_calls: AtomicU64::new(0),
            in_step_reordered: AtomicU64::new(0),
            last_refusal: Mutex::new(None),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a fix is wanted: the master switch and its own both on.
pub fn wanted(master: Option<&str>, own: Option<&str>) -> bool {
    crate::ticks::wanted(master) && crate::ticks::wanted(own)
}

/// Sorts entity ids ascending in place; [`Sorted::Unchanged`] (nothing
/// written) when they already are.
pub fn sort_ids(ids: &mut [i32]) -> Sorted {
    if ids.windows(2).all(|pair| pair[0] <= pair[1]) {
        return Sorted::Unchanged;
    }
    ids.sort_unstable();
    Sorted::Reordered
}

/// The number of ids in a `std::vector<Entity>` with these three pointers,
/// or why it is not one.
pub fn vector_len(begin: u64, end: u64, capacity: u64) -> Result<u64, &'static str> {
    if begin == 0 {
        return if end == 0 && capacity == 0 {
            Ok(0)
        } else {
            Err("a vector with no storage has an end")
        };
    }
    if end < begin || capacity < end {
        return Err("the vector's pointers are out of order");
    }
    if !begin.is_multiple_of(4) || !(end - begin).is_multiple_of(4) {
        return Err("the vector's bounds are not whole ids");
    }
    let count = (end - begin) / 4;
    if count > MAX_IDS {
        return Err("more ids than any world holds");
    }
    Ok(count)
}

/// Sorts the `std::vector<Entity>` at `vector` in place, every read and
/// the span checked first.
fn sort_vector(vector: u64, probe: &mut Probe) -> Result<Sorted, &'static str> {
    let begin: u64 = probe.read(vector).ok_or("the vector is unreadable")?;
    let end: u64 = probe
        .read(vector.wrapping_add(8))
        .ok_or("the vector's end is unreadable")?;
    let capacity: u64 = probe
        .read(vector.wrapping_add(16))
        .ok_or("the vector's capacity is unreadable")?;
    let count = vector_len(begin, end, capacity)?;
    if count < 2 {
        return Ok(Sorted::Unchanged);
    }
    let count = usize::try_from(count).map_err(|_| "the vector is too long")?;
    if !probe.readable(begin as usize, count * 4) {
        return Err("the vector's ids are unreadable");
    }
    // SAFETY: `count` readable, 4-aligned ids at `begin`, the engine's own
    // vector on the thread about to read it; nothing else holds a view of
    // it across the site (the module's table), and the sort only permutes.
    let ids = unsafe { std::slice::from_raw_parts_mut(begin as usize as *mut i32, count) };
    Ok(sort_ids(ids))
}

/// The hook body every batch shares.
fn run(batch: &Batch, regs: *mut SavedRegs) {
    let state = batch.state;
    if state.broken.load(Ordering::Acquire) {
        return;
    }
    let _timer = perf::time(Piece::PersonOrder);
    let body = || {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        let in_step = crate::order::in_step();
        let n = state.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let in_step_n = if in_step {
            state.in_step_calls.fetch_add(1, Ordering::Relaxed) + 1
        } else {
            0
        };
        let mut probe = Probe::new();
        let outcome = (batch.vectors)(regs, &mut probe).and_then(|vectors| {
            let mut sorted = Sorted::Unchanged;
            for vector in vectors.as_slice() {
                if sort_vector(*vector, &mut probe)? == Sorted::Reordered {
                    sorted = Sorted::Reordered;
                }
            }
            Ok(sorted)
        });
        match outcome {
            Ok(Sorted::Reordered) => {
                let reorders = state.reordered.fetch_add(1, Ordering::Relaxed) + 1;
                if in_step {
                    state.in_step_reordered.fetch_add(1, Ordering::Relaxed);
                }
                if reorders <= 3 {
                    log::line(&format!(
                        "order fix {}: a batch put in entity order (call #{n})",
                        batch.fix
                    ));
                }
            }
            Ok(Sorted::Unchanged) => {}
            Err(why) => {
                state.refused.fetch_add(1, Ordering::Relaxed);
                let mut last = state.last_refusal.lock().unwrap_or_else(|p| p.into_inner());
                if *last != Some(why) {
                    *last = Some(why);
                    log::line(&format!(
                        "order fix {}: refused a batch, {why}; the engine's own order stands",
                        batch.fix
                    ));
                }
            }
        }
        if n == 1 || n.is_multiple_of(batch.alive_every) {
            log::line(&alive_line(batch.fix, state));
        }
        if in_step_n != 0 && in_step_n.is_multiple_of(batch.alive_every) {
            log::line(&format!(
                "order fix {}: in-step calls={in_step_n} reordered={}",
                batch.fix,
                state.in_step_reordered.load(Ordering::Relaxed)
            ));
        }
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        state.broken.store(true, Ordering::Release);
        log::line(&format!(
            "order fix {}: panicked on the game's thread; switched off for this game",
            batch.fix
        ));
    }
}

/// The `alive` line: every call and those inside the game's step (the
/// ones two games of a room must agree on).
fn alive_line(fix: &str, state: &State) -> String {
    format!(
        "order fix {fix}: alive, calls={} reordered={} refused={} (in the step {}/{})",
        state.calls.load(Ordering::Relaxed),
        state.reordered.load(Ordering::Relaxed),
        state.refused.load(Ordering::Relaxed),
        state.in_step_calls.load(Ordering::Relaxed),
        state.in_step_reordered.load(Ordering::Relaxed),
    )
}

/// The target of the `call rel32` whose opcode is at `at`, read with `read`.
pub fn call_target(at: u64, read: &dyn Fn(u64, usize) -> Option<Vec<u8>>) -> Option<u64> {
    let bytes = read(at, 5)?;
    if bytes[0] != 0xE8 {
        return None;
    }
    let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    Some(at.wrapping_add(5).wrapping_add_signed(i64::from(rel)))
}

/// The function's bytes around the site: each context entry must read back
/// exactly.
pub fn check_context(
    site: u64,
    context: &[(i64, &[u8])],
    read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
) -> Result<(), String> {
    for (offset, bytes) in context {
        let at = site.wrapping_add_signed(*offset);
        match read(at, bytes.len()) {
            Some(found) if found == *bytes => {}
            Some(_) => return Err(format!("the bytes at {at:#x} are not the measured ones")),
            None => return Err(format!("the bytes at {at:#x} are unreadable")),
        }
    }
    Ok(())
}

/// Reads `len` bytes of the game's code, if readable.
fn read_code(at: u64, len: usize) -> Option<Vec<u8>> {
    let address = usize::try_from(at).ok()?;
    crate::image::readable(address, len).then(|| {
        // SAFETY: `len` readable bytes of the game's image.
        unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
    })
}

/// What installing one batch came to, for hook.log.
pub fn outcome_line(fix: &str, installed: bool, reason: &str) -> String {
    if installed {
        format!("order fix {fix}: installed ({reason})")
    } else {
        format!("order fix {fix}: off, {reason}")
    }
}

/// Installs one batch's fix, unless switched off, when its site and every
/// check of its shape hold.
fn install_batch(
    batch: &Batch,
    resolved: &ResolvedProfile,
    wanted: bool,
    read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    extra: &dyn Fn(u64) -> Result<(), String>,
) -> String {
    if !wanted {
        return outcome_line(
            batch.fix,
            false,
            &format!(
                "{MASTER_ENV} or {} says so; the engine's own order stands. Every game of the room must run with the same setting",
                batch.toggle_env
            ),
        );
    }
    let Some(site) = resolved.get(batch.target) else {
        return outcome_line(
            batch.fix,
            false,
            &format!("the profile has no {:?}", batch.target),
        );
    };
    let site = site.address;
    if let Err(why) = check_context(site, batch.context, read).and_then(|()| extra(site)) {
        return outcome_line(batch.fix, false, &why);
    }
    // SAFETY: a resolved site whose bytes and surroundings were checked,
    // installed before any world exists; only the branches the profile
    // notes reach the site, at its first byte; the hook never unwinds and
    // only permutes the ids of the vectors the engine reads next.
    match unsafe {
        Splice::install(
            site as usize as *mut u8,
            batch.expected,
            batch.steal,
            batch.hook,
        )
    } {
        Ok(splice) => {
            let _kept = std::mem::ManuallyDrop::new(splice);
            batch.state.installed.store(true, Ordering::Release);
            outcome_line(batch.fix, true, &format!("at {site:#x}, {}", batch.does))
        }
        Err(error) => outcome_line(batch.fix, false, &format!("the site at {site:#x}: {error}")),
    }
}

/// Installs every person-order fix that resolves on this build. Returns the
/// lines for hook.log, one per fix.
pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
    let master = std::env::var(MASTER_ENV).ok();
    let wanted_batch = |batch: &Batch| {
        wanted(
            master.as_deref(),
            std::env::var(batch.toggle_env).ok().as_deref(),
        )
    };
    let none = |_site: u64| Ok(());
    vec![
        master_line(crate::ticks::wanted(master.as_deref())),
        install_batch(
            &candidates::BATCH,
            resolved,
            wanted_batch(&candidates::BATCH),
            &read_code,
            &none,
        ),
        install_batch(
            &departures::BATCH,
            resolved,
            wanted_batch(&departures::BATCH),
            &read_code,
            &|site| departures::check_emits(site, &read_code).map(|_| ()),
        ),
        install_batch(
            &arrivals::BATCH,
            resolved,
            wanted_batch(&arrivals::BATCH),
            &read_code,
            &|site| arrivals::check_emit(site, resolved, &read_code),
        ),
        install_batch(
            &needs_path::BATCH,
            resolved,
            wanted_batch(&needs_path::BATCH),
            &read_code,
            &|_site| needs_path::resolve_getter(resolved, &read_code),
        ),
        install_batch(
            &freed_ids::BATCH,
            resolved,
            wanted_batch(&freed_ids::BATCH),
            &read_code,
            &none,
        ),
        freed_ids::trace::install(
            resolved,
            crate::ticks::wanted(std::env::var(freed_ids::trace::TOGGLE_ENV).ok().as_deref()),
            &read_code,
            image_base(),
        ),
    ]
}

/// The executable's base, for return addresses as RVAs; 0 where unknown.
#[cfg(all(windows, target_arch = "x86_64"))]
fn image_base() -> u64 {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    // SAFETY: a null name asks for the executable's own module handle, its
    // base address, which stays mapped for the life of the process.
    unsafe { GetModuleHandleW(std::ptr::null()) as u64 }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn image_base() -> u64 {
    0
}

/// The first line: the master switch's answer, and the rule that goes with
/// it.
pub fn master_line(on: bool) -> String {
    if on {
        format!(
            "person-order: on ({MASTER_ENV}=0 turns every person-order fix off); every game of a room must run the same person-order settings"
        )
    } else {
        format!(
            "person-order: off ({MASTER_ENV} says so); every game of a room must run the same person-order settings"
        )
    }
}

/// The candidate buildings of a person's destination draw (TF2's
/// `candidates`). `destination_util::GetTargetsByLandUse` (`0x8e3ac0`, our
/// name) copies the `PersonCapacity` family's node list (8-byte `{entity,
/// index}` records) into a local `vector<Entity>` (`[rsp+0x70]`,
/// `0x8e3d10..0x8e3d3f`), builds one 0x24-byte `TargetDataEntry` per
/// entity in that order on the thread pool (chunks joined in index order),
/// and `BinarySearchIndex` (`0x8e4400`) later sums their free capacity in
/// entry order and binary-searches one draw. The entries also index a
/// sibling vector, so the entity list is sorted before the build, not the
/// entries after it.
pub mod candidates {
    use super::*;

    pub const FIX: &str = "person-candidates-order";
    /// Set to `0` (or `off`), this batch keeps the engine's order.
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_CANDIDATES_ORDER";
    pub use crate::build_data::native::persons::candidates::CONTEXT;
    pub use crate::build_data::native::persons::candidates::EXPECTED;
    pub use crate::build_data::native::persons::candidates::SITE;
    pub use crate::build_data::native::persons::candidates::VECTOR_AT_RSP;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: CONTEXT,
        does: "the PersonCapacity candidates are put in entity order before their capacities are summed for the destination draw",
        alive_every: 1 << 14,
        vectors,
        hook,
        state: &STATE,
    };

    /// The vector, from the site's stack pointer; `rbx` must name it too,
    /// as the copy loop left it.
    pub fn vectors_at(rsp: u64, rbx: u64) -> Result<Vectors, &'static str> {
        let vector = rsp.wrapping_add(VECTOR_AT_RSP);
        if rbx != vector {
            return Err("rbx is not the candidate vector");
        }
        Ok(Vectors::one(vector))
    }

    fn vectors(regs: &SavedRegs, _probe: &mut Probe) -> Result<Vectors, &'static str> {
        vectors_at(SavedRegs::rsp(regs), regs.rbx)
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The persons (and cargo) leaving a building (TF2's `departures`).
/// `ecs::SimEntityAtBuildingSystem::Update2` (vf12, `0xb05ea0`) gathers,
/// on the thread pool in node order (chunks of 256 joined in index order,
/// `0xb052c0`), the entities whose stay ran out into two `vector<Entity>`
/// at `[rbp+7]` and `[rbp+0x1f]`, and emits each to its signal (`0xaeaeb0`,
/// a signal of `(Engine&, vector<Entity> const&)`).
/// `SimPersonSystem::NoteAtBuildingPersonsLeave` (`0xb2e950`) seeds one
/// generator from `updateCount` and draws, person by person in batch order,
/// whether the person recomputes its destination. Both vectors are sorted.
pub mod departures {
    use super::*;

    pub const FIX: &str = "person-departures-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_DEPARTURES_ORDER";
    pub use crate::build_data::native::persons::departures::CONTEXT;
    pub use crate::build_data::native::persons::departures::EMITS;
    pub use crate::build_data::native::persons::departures::EXPECTED;
    pub use crate::build_data::native::persons::departures::FIRST;
    pub use crate::build_data::native::persons::departures::SECOND;
    pub use crate::build_data::native::persons::departures::SITE;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: CONTEXT,
        does: "the persons and cargo leaving buildings are put in entity order before they are signalled",
        alive_every: 1 << 12,
        vectors,
        hook,
        state: &STATE,
    };

    /// Both emits must call one function, the entity-batch signal; it is
    /// what the arrivals fix checks its own emit against.
    pub fn check_emits(
        site: u64,
        read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    ) -> Result<u64, String> {
        let first = call_target(site + EMITS[0], read);
        let second = call_target(site + EMITS[1], read);
        match (first, second) {
            (Some(a), Some(b)) if a == b => Ok(a),
            _ => Err(format!(
                "the two emits after {SITE:?} at {site:#x} do not call one signal"
            )),
        }
    }

    pub fn vectors_at(rbp: u64) -> Vectors {
        Vectors::two(
            rbp.wrapping_add_signed(FIRST),
            rbp.wrapping_add_signed(SECOND),
        )
    }

    fn vectors(regs: &SavedRegs, _probe: &mut Probe) -> Result<Vectors, &'static str> {
        Ok(vectors_at(regs.rbp))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The persons arriving on foot (TF2's `arrivals`).
/// `ecs::PersonMoveSystem::Update2` (vf12, `0xaecaa0`) gathers the walk
/// arrivals on the thread pool in node order (chunks of 128 joined in index
/// order) into a `vector<Entity>` at `[rbp-0x78]` and emits it (`0xaecfe8`)
/// to `SimPersonSystem::NoteWalkPersonsArrived` (`0xb33a30`), which seeds a
/// generator from `updateCount` and draws stay durations in batch order.
pub mod arrivals {
    use super::*;

    pub const FIX: &str = "person-arrivals-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_ARRIVALS_ORDER";
    pub use crate::build_data::native::persons::arrivals::EMIT;
    pub use crate::build_data::native::persons::arrivals::EXPECTED;
    pub use crate::build_data::native::persons::arrivals::SITE;
    pub use crate::build_data::native::persons::arrivals::VECTOR;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: &[],
        does: "the persons arriving on foot are put in entity order before they are signalled",
        alive_every: 1 << 12,
        vectors,
        hook,
        state: &STATE,
    };

    /// The emit must call the signal the departures' two emits call.
    pub fn check_emit(
        site: u64,
        resolved: &ResolvedProfile,
        read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let Some(departures) = resolved.get(departures::SITE) else {
            return Err(format!(
                "the profile has no {:?} to check the emit against",
                departures::SITE
            ));
        };
        let signal = departures::check_emits(departures.address, read)?;
        match call_target(site + EMIT, read) {
            Some(target) if target == signal => Ok(()),
            _ => Err(format!(
                "the emit after {SITE:?} at {site:#x} does not call the signal at {signal:#x}"
            )),
        }
    }

    pub fn vectors_at(rbp: u64) -> Vectors {
        Vectors::one(rbp.wrapping_add_signed(VECTOR))
    }

    fn vectors(regs: &SavedRegs, _probe: &mut Probe) -> Result<Vectors, &'static str> {
        Ok(vectors_at(regs.rbp))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The persons waiting for a path (TF2's `idle`). TF3's
/// `ecs::SimEntityNeedsPathSystem::Update` (vf11, `0xb181b0`; every odd
/// `updateCount`) builds one `PathFactoryInput` per entry of its list
/// `m_systemData->add` (`[[this+0x10]]`, a `vector<Entity>` its
/// `EntityAdded` appends to and its `EntityToBeRemoved` erases from in
/// place: insertion history) and calls `PathFactory::Compute` (`0x8d11e0`),
/// which runs three items per person in chunks and seeds one generator per
/// chunk from `updateCount` and the chunk's start index: each trip's path
/// and mode follow the person's place in the list. The outputs are matched
/// back to `add` by position in the same call (`0xb183c0`), so sorting the
/// list in place before the first read keeps them paired.
///
/// The data is copy-on-write, shared with the replicated engine's copy;
/// the system's own writers take a private copy through the getter first
/// (`0xb186c0`), and so does this fix when the data is shared.
pub mod needs_path {
    use super::*;

    pub const FIX: &str = "person-needs-path-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_PERSON_NEEDS_PATH_ORDER";
    pub use crate::build_data::native::persons::needs_path::CONTEXT;
    pub use crate::build_data::native::persons::needs_path::CONTROL;
    pub use crate::build_data::native::persons::needs_path::DATA;
    pub use crate::build_data::native::persons::needs_path::EXPECTED;
    pub use crate::build_data::native::persons::needs_path::GETTER;
    pub use crate::build_data::native::persons::needs_path::GETTER_CALL;
    pub use crate::build_data::native::persons::needs_path::SITE;
    pub use crate::build_data::native::persons::needs_path::USES;

    type Getter = unsafe extern "system" fn(shared: u64) -> u64;
    static GETTER_AT: AtomicU64 = AtomicU64::new(0);
    static UNSHARED: AtomicU64 = AtomicU64::new(0);
    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 8,
        context: CONTEXT,
        does: "the persons waiting for a path are put in entity order before PathFactory::Compute seeds by their place",
        alive_every: 1 << 12,
        vectors,
        hook,
        state: &STATE,
    };

    /// Finds the copy-on-write getter through the call the profile names
    /// and checks its head.
    pub fn resolve_getter(
        resolved: &ResolvedProfile,
        read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let Some(call) = resolved.get(GETTER_CALL) else {
            return Err(format!("the profile has no {GETTER_CALL:?}"));
        };
        let Some(getter) = call_target(call.address, read) else {
            return Err(format!("no call at {:#x}", call.address));
        };
        if read(getter, GETTER.len()).as_deref() != Some(&GETTER[..]) {
            return Err(format!(
                "the data getter at {getter:#x} is not the measured copy-on-write getter"
            ));
        }
        GETTER_AT.store(getter, Ordering::Release);
        Ok(())
    }

    /// Whether the data must be copied first: a control block whose use
    /// count is above 1.
    pub fn shared(uses: Option<i32>) -> Result<bool, &'static str> {
        match uses {
            Some(n) if n >= 1 => Ok(n > 1),
            Some(_) => Err("the list's data has no owner"),
            None => Err("the list's control block is unreadable"),
        }
    }

    fn vectors(regs: &SavedRegs, probe: &mut Probe) -> Result<Vectors, &'static str> {
        let system = regs.r13;
        let control: u64 = probe
            .read(system.wrapping_add(CONTROL))
            .ok_or("the system's data is unreadable")?;
        if control == 0 {
            return Err("the list's data has no control block");
        }
        let uses: Option<i32> = probe.read(control.wrapping_add(USES));
        if shared(uses)? {
            let getter = GETTER_AT.load(Ordering::Acquire);
            if getter == 0 {
                return Err("the data getter is not resolved");
            }
            // SAFETY: the engine's own copy-on-write getter (head checked at
            // install), called as its writers call it, on the thread that
            // runs the system's update, with the system's shared pointer.
            let fun: Getter = unsafe { std::mem::transmute::<usize, Getter>(getter as usize) };
            unsafe { fun(system.wrapping_add(DATA)) };
            let n = UNSHARED.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 {
                log::line(&format!(
                    "order fix {FIX}: the list's data was shared; took a private copy as the system's writers do"
                ));
            }
        }
        let data: u64 = probe
            .read(system.wrapping_add(DATA))
            .ok_or("the system's data pointer is unreadable")?;
        if data == 0 {
            return Err("the system has no data");
        }
        Ok(Vectors::one(data))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
    }
}

/// The ids freed by one modification (TF2's `freed ids`).
/// `ecs::Engine::EndModification` (`0x2bb4d90`) appends the modification's
/// removed ids (`m_betweenChanges`, `[[engine+0x1f0]]`, a `vector<Entity>`
/// in `RemoveEntity` order) to the FIFO free-id deque at `engine+0xd8`
/// (`0x2bb1110`, insert at its end), and `AddEntity` (`0x2bb37b0`) pops its
/// front. Sorted before the append, the deque depends only on which ids
/// each modification removed. The replicated engine replays the same
/// removals through the same function (`Replicator::Apply`, `0x2bb4489`),
/// so it sorts alike.
pub mod freed_ids {
    use super::*;

    pub const FIX: &str = "freed-id-order";
    pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_FREED_ID_ORDER";
    pub use crate::build_data::native::persons::freed_ids::EXPECTED;
    pub use crate::build_data::native::persons::freed_ids::SITE;

    static STATE: State = State::new();

    pub static BATCH: Batch = Batch {
        fix: FIX,
        toggle_env: TOGGLE_ENV,
        target: SITE,
        expected: &EXPECTED,
        steal: 7,
        context: &[],
        does: "each modification's freed ids join the free-id queue in entity order",
        alive_every: 1 << 16,
        vectors,
        hook,
        state: &STATE,
    };

    fn vectors(regs: &SavedRegs, probe: &mut Probe) -> Result<Vectors, &'static str> {
        let vector: u64 = probe
            .read(regs.r12)
            .ok_or("the removed-ids slot is unreadable")?;
        if vector == 0 {
            return Err("the modification has no removed ids");
        }
        Ok(Vectors::one(vector))
    }

    unsafe extern "system" fn hook(regs: *mut SavedRegs) {
        run(&BATCH, regs);
        // SAFETY: the stub's block, held until the hook returns.
        trace::observe(unsafe { &*regs });
    }

    /// The free-id trace (logging only; docs/HOOKS.md, "The free-id
    /// trace"). Which id a new entity gets is a function of the order of
    /// every modification of the simulation's engine that took or freed an
    /// id, so two games of a room hand out the same ids only while the
    /// histories of their simulations' engines agree. The sort above makes
    /// each modification's batch canonical; it cannot help when the
    /// modifications themselves come at other moments in two games. This
    /// trace says when that happened:
    ///
    /// - at every checkpoint, a fingerprint of the free-id queue (its size,
    ///   its front and a hash of every id in order) and what the
    ///   simulation's modifications took and freed since the last one: two
    ///   games must log equal lines at equal steps, so the first line that
    ///   differs bounds where their ids parted to one checkpoint interval;
    /// - each modification of the simulation's engine that took or freed
    ///   ids outside a room step's update (from another thread, on the
    ///   paused path, or in a batch's first update before the engine's own
    ///   update began), with the return address it came from: such a
    ///   modification lands at a moment that depends on the frames, not on
    ///   the room's steps.
    ///
    /// The simulation's engine is the one `GameSim::Step` hands
    /// `ecs::Engine::Update` (`[[this+8]+0x18]`), noted at every call of the
    /// room's game's step ([`note_step`]); the replicated engine (the
    /// GUI's) is left out. Every read goes through [`crate::image`]; a
    /// refused read loses a count or a line and never touches the game.
    /// `TPF3MP_HOOK_FREED_ID_TRACE=0` turns it off.
    pub mod trace {
        use super::*;
        use crate::build_data::native::persons::freed_ids as layout;
        use crate::order::Fnv1a;
        use std::time::Instant;

        /// `0` (or `off`, `false`, `no`) turns the trace off.
        pub const TOGGLE_ENV: &str = "TPF3MP_HOOK_FREED_ID_TRACE";
        /// The most ids of the queue the trace reads; a longer queue is
        /// not hashed.
        pub const MAX_QUEUE: u64 = 1 << 22;
        /// Lines for modifications outside an update: the first this many,
        /// then every [`OUTSIDE_EVERY`]th.
        pub const OUTSIDE_FIRST: u64 = 16;
        pub const OUTSIDE_EVERY: u64 = 1024;

        static ON: AtomicBool = AtomicBool::new(false);
        static BROKEN: AtomicBool = AtomicBool::new(false);
        /// The simulation's engine of the room's game; 0 when unknown or
        /// outside a room.
        static SIM_ENGINE: AtomicU64 = AtomicU64::new(0);
        /// The executable's base, for return addresses as RVAs.
        static BASE: AtomicU64 = AtomicU64::new(0);
        static TRACE: Mutex<Trace> = Mutex::new(Trace::new());
        /// Enough for the 53 total modifications seen in the first
        /// divergent 50-step interval, with a visible overflow count.
        pub const MAX_OPERATIONS: usize = 96;
        /// The first allocator split was 53 checkpoints before the lane
        /// mismatch. Keep a wider window without writing it every time.
        pub const KEPT_CHECKPOINTS: usize = 128;
        /// Dump only the 64 checkpoints leading to the reported mismatch.
        /// The incident's allocator split led its lane symptom by 53.
        pub const INCIDENT_CHECKPOINTS: usize = 64;
        /// Cap one minute of native-operation trace, including logger prefixes
        /// and a rate-limit notice, below the 192 KiB/minute upload source cap.
        /// Other hook lines can still consume that upload's remaining space.
        pub const MAX_FLUSH_BYTES: usize = 128 << 10;
        // Logger::lines writes "[<u64 seconds>] " and a newline for each
        // message. Reserve the maximum 20-digit timestamp, not today's size.
        const LOG_LINE_OVERHEAD: usize = 24;
        // One short rate-limit notice may follow the full burst in the same
        // minute; both together must stay inside MAX_FLUSH_BYTES.
        const SUPPRESSION_NOTICE_RESERVE: usize = 256;
        const INCIDENT_HEADER_RESERVE: usize = 1024;
        /// Keep each line below DiagnosticText's 1024-byte limit after the
        /// agent tails hook.log. No record may be silently cut there.
        const OPERATIONS_PER_LINE: usize = 4;

        /// Where a modification of the simulation's engine ran.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Place {
            /// In the game's step, inside an update the room released (its
            /// step is current) or a room's action applied for its step.
            Update,
            /// In the game's step, but outside the room's updates: the
            /// paused path, or a batch's first update before
            /// `ecs::Engine::Update` began.
            StepOutsideUpdate,
            /// Outside the game's step: another frame or another thread.
            OutsideStep,
        }

        impl Place {
            /// `in_step` is the calling thread's (the step detour's flag);
            /// `current_step` the room's step of the update the simulation
            /// thread runs, so it only counts on that thread.
            pub fn of(in_step: bool, current_step: Option<u64>) -> Self {
                match (in_step, current_step) {
                    (false, _) => Self::OutsideStep,
                    (true, Some(_)) => Self::Update,
                    (true, None) => Self::StepOutsideUpdate,
                }
            }

            fn says(self) -> &'static str {
                match self {
                    Self::Update => "in an update",
                    Self::StepOutsideUpdate => "in the game's step outside the room's updates",
                    Self::OutsideStep => "outside the game's step",
                }
            }
        }

        /// The engine's counts read at one modification's end, before its
        /// freed ids join the queue.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Seen {
            /// Rows in the entity table: the next new id.
            pub table: u64,
            /// Ids in the free-id queue.
            pub queued: u64,
            /// Ids this modification freed.
            pub freed: u64,
        }

        /// The ids taken since the modification `before` (new ids and ids
        /// popped from the queue), or `None` when the counts do not fit one
        /// history (the queue grew by more than the ids freed, or the table
        /// shrank): only `EndModification` appends to the queue, and only
        /// `AddEntity` pops it or grows the table.
        pub fn taken_since(before: Seen, now: Seen) -> Option<u64> {
            let grown = now.table.checked_sub(before.table)?;
            let popped = before
                .queued
                .checked_add(before.freed)?
                .checked_sub(now.queued)?;
            grown.checked_add(popped)
        }

        /// What the simulation's modifications did between two checkpoints.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Tally {
            last: Option<Seen>,
            pub modifications: u64,
            pub taken: u64,
            pub freed: u64,
            /// The ids the modifications in updates freed, in order.
            pub freed_hash: u64,
            pub outside: u64,
            pub outside_taken: u64,
            pub outside_freed: u64,
            /// Modifications whose counts did not fit (see [`taken_since`]).
            pub unfit: u64,
            /// Every modification outside an update since the engine was
            /// noted.
            pub outside_total: u64,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        struct Operation {
            step: Option<u64>,
            place: Place,
            taken: Option<u64>,
            seen: Seen,
            first_freed: Option<i32>,
            last_freed: Option<i32>,
            freed_hash: u64,
            caller: Option<u64>,
        }

        /// A single lock keeps the aggregate and callback observations in
        /// the same checkpoint interval. Concurrent native modifications may
        /// be observed in callback-lock order rather than causal order.
        struct Trace {
            tally: Tally,
            operations: Vec<Operation>,
            omitted: u64,
            unread: u64,
            history: std::collections::VecDeque<Window>,
            last_flushed: Option<u64>,
            last_flushed_at: Option<Instant>,
            suppressed: u64,
            suppressed_through: Option<u64>,
            last_suppressed_step: Option<u64>,
            suppression_reported: bool,
            expired: u64,
        }

        struct Window {
            step: u64,
            operations: Vec<Operation>,
            omitted: u64,
            unfit: u64,
            unread: u64,
        }

        impl Trace {
            const fn new() -> Self {
                Self {
                    tally: Tally::new(),
                    operations: Vec::new(),
                    omitted: 0,
                    unread: 0,
                    history: std::collections::VecDeque::new(),
                    last_flushed: None,
                    last_flushed_at: None,
                    suppressed: 0,
                    suppressed_through: None,
                    last_suppressed_step: None,
                    suppression_reported: false,
                    expired: 0,
                }
            }

            fn count(
                &mut self,
                seen: Seen,
                ids: &[i32],
                place: Place,
                step: Option<u64>,
                caller: Option<u64>,
            ) -> (Option<u64>, u64) {
                let taken = self.tally.last.and_then(|before| taken_since(before, seen));
                let outside = self.tally.count(seen, ids, place);
                if self.operations.len() < MAX_OPERATIONS {
                    let mut hash = Fnv1a::new();
                    for id in ids {
                        hash.write(&id.to_le_bytes());
                    }
                    self.operations.push(Operation {
                        step,
                        place,
                        taken,
                        seen,
                        first_freed: ids.first().copied(),
                        last_freed: ids.last().copied(),
                        freed_hash: hash.0,
                        caller,
                    });
                } else {
                    self.omitted += 1;
                }
                (outside, self.tally.outside_total)
            }

            fn checkpoint(&mut self, step: u64, queue: Result<Queue, &str>) -> String {
                let tally = self.tally.take();
                let summary = checkpoint_text(step, queue, &tally);
                self.history.push_back(Window {
                    step,
                    operations: std::mem::take(&mut self.operations),
                    omitted: std::mem::take(&mut self.omitted),
                    unfit: tally.unfit,
                    unread: std::mem::take(&mut self.unread),
                });
                if self.history.len() > KEPT_CHECKPOINTS {
                    self.history.pop_front();
                    self.expired += 1;
                }
                summary
            }

            fn incident(&mut self, diverged: u64) -> Vec<String> {
                self.incident_at(diverged, Instant::now())
            }

            fn incident_at(&mut self, diverged: u64, now: Instant) -> Vec<String> {
                if self.last_flushed == Some(diverged) {
                    return Vec::new();
                }
                if self
                    .last_flushed_at
                    .is_some_and(|last| now.saturating_duration_since(last) < crate::lanedump::GAP)
                {
                    if self.last_suppressed_step != Some(diverged) {
                        self.suppressed += 1;
                        self.suppressed_through = Some(diverged);
                        self.last_suppressed_step = Some(diverged);
                    }
                    if self.suppression_reported {
                        return Vec::new();
                    }
                    self.suppression_reported = true;
                    let notice = format!(
                        "free operations: detail burst rate-limited; suppressed={} through={diverged}",
                        self.suppressed
                    );
                    debug_assert!(
                        notice.len() + LOG_LINE_OVERHEAD <= SUPPRESSION_NOTICE_RESERVE,
                        "native-operation suppression notice exceeded its byte reserve"
                    );
                    return vec![notice];
                }
                self.last_flushed = Some(diverged);
                self.last_flushed_at = Some(now);
                let suppressed = std::mem::take(&mut self.suppressed);
                let suppressed_through = self.suppressed_through.take();
                self.last_suppressed_step = None;
                self.suppression_reported = false;
                let mut relevant = self
                    .history
                    .iter()
                    .rev()
                    .filter(|w| w.step <= diverged)
                    .take(INCIDENT_CHECKPOINTS)
                    .collect::<Vec<_>>();
                relevant.reverse();
                let first = relevant.first().map(|w| w.step);
                let last = relevant.last().map(|w| w.step);
                let mut lines = Vec::new();
                // Reserve space for the summary line, including its rate-limit
                // metadata, so the complete incident batch stays inside the
                // same byte ceiling as its operation-detail rows.
                let mut bytes = INCIDENT_HEADER_RESERVE;
                let mut printed = 0usize;
                let omitted = relevant.iter().map(|w| w.omitted).sum::<u64>();
                let unfit = relevant.iter().map(|w| w.unfit).sum::<u64>();
                let unread = relevant.iter().map(|w| w.unread).sum::<u64>();
                let mut cut = false;
                for window in &relevant {
                    for (part, chunk) in window.operations.chunks(OPERATIONS_PER_LINE).enumerate() {
                        let entries = chunk
                            .iter()
                            .map(Operation::compact)
                            .collect::<Vec<_>>()
                            .join(" ");
                        let line = format!(
                            "free operations: step {} part {}: {entries}",
                            window.step,
                            part + 1
                        );
                        if bytes + line.len() + LOG_LINE_OVERHEAD
                            > MAX_FLUSH_BYTES - SUPPRESSION_NOTICE_RESERVE
                        {
                            cut = true;
                            break;
                        }
                        bytes += line.len() + LOG_LINE_OVERHEAD;
                        printed += chunk.len();
                        lines.push(line);
                    }
                    if cut {
                        break;
                    }
                }
                let retained = relevant.iter().map(|w| w.operations.len()).sum::<usize>();
                // Expiry before the selected window is historical context,
                // not a missing record in these 64 checkpoints. A delayed
                // notice with fewer than 64 retained windows is incomplete.
                lines.insert(0, format!(
                    "free operations: diverged={diverged} retained_checkpoints={} expired_checkpoints={} oldest={} newest={} recorded={printed}/{retained} omitted={omitted} unfit={unfit} unread={unread} suppressed={suppressed} suppressed_through={} byte_limit={MAX_FLUSH_BYTES} complete={}",
                    relevant.len(), self.expired, first.map_or_else(|| "-".into(), |n| n.to_string()),
                    last.map_or_else(|| "-".into(), |n| n.to_string()),
                    suppressed_through.map_or_else(|| "-".into(), |n| n.to_string()),
                    !relevant.is_empty() && !cut && omitted == 0 && unfit == 0 && unread == 0
                        && (self.expired == 0 || relevant.len() == INCIDENT_CHECKPOINTS)
                ));
                debug_assert!(
                    lines[0].len() + LOG_LINE_OVERHEAD <= INCIDENT_HEADER_RESERVE,
                    "native-operation incident header exceeded its byte reserve"
                );
                lines
            }
        }

        impl Operation {
            fn compact(&self) -> String {
                let place = match self.place {
                    Place::Update => "U",
                    Place::StepOutsideUpdate => "S",
                    Place::OutsideStep => "X",
                };
                format!(
                    "{}:{place}:{}/{}:{}/{}:{}/{}/{:016x}:{}",
                    self.step.map_or_else(|| "-".into(), |n| n.to_string()),
                    self.taken.map_or_else(|| "?".into(), |n| n.to_string()),
                    self.seen.freed,
                    self.seen.table,
                    self.seen.queued,
                    self.first_freed
                        .map_or_else(|| "-".into(), |n| n.to_string()),
                    self.last_freed
                        .map_or_else(|| "-".into(), |n| n.to_string()),
                    self.freed_hash,
                    self.caller.map_or_else(|| "-".into(), |n| format!("{n:x}"))
                )
            }
        }

        impl Default for Tally {
            fn default() -> Self {
                Self::new()
            }
        }

        impl Tally {
            pub const fn new() -> Self {
                Self {
                    last: None,
                    modifications: 0,
                    taken: 0,
                    freed: 0,
                    freed_hash: Fnv1a::new().0,
                    outside: 0,
                    outside_taken: 0,
                    outside_freed: 0,
                    unfit: 0,
                    outside_total: 0,
                }
            }

            /// Counts one modification. Returns what it took when it took
            /// or freed ids outside an update, for its own line.
            pub fn count(&mut self, seen: Seen, ids: &[i32], place: Place) -> Option<u64> {
                let taken = match self.last {
                    Some(before) => taken_since(before, seen),
                    // The first one since the engine was noted: nothing to
                    // compare with.
                    None => Some(0),
                };
                self.last = Some(seen);
                let Some(taken) = taken else {
                    self.unfit += 1;
                    return None;
                };
                if place == Place::Update {
                    self.modifications += 1;
                    self.taken += taken;
                    self.freed += seen.freed;
                    if !ids.is_empty() {
                        let mut hash = Fnv1a(self.freed_hash);
                        for id in ids {
                            hash.write(&id.to_le_bytes());
                        }
                        hash.write_u32(u32::MAX);
                        self.freed_hash = hash.0;
                    }
                    return None;
                }
                if taken == 0 && seen.freed == 0 {
                    return None;
                }
                self.outside += 1;
                self.outside_total += 1;
                self.outside_taken += taken;
                self.outside_freed += seen.freed;
                Some(taken)
            }

            /// The counts for a checkpoint line; they start again from 0,
            /// the last modification and the total kept.
            pub fn take(&mut self) -> Self {
                let last = self.last;
                let outside_total = self.outside_total;
                let taken = std::mem::take(self);
                self.last = last;
                self.outside_total = outside_total;
                taken
            }
        }

        /// A fingerprint of the free-id queue, front to back.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Queue {
            pub len: u64,
            pub front: Option<i32>,
            pub hash: u64,
        }

        /// The header of the engine's `std::deque<Entity>` (MSVC): the
        /// block map, its size (a power of two), the front's offset and the
        /// number of ids.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Header {
            pub map: u64,
            pub map_size: u64,
            pub offset: u64,
            pub len: u64,
        }

        /// Reads the queue `header` describes, front to back as `AddEntity`
        /// pops it, through `read_u64` (a block pointer of the map) and
        /// `read_i32` (an id): id `i` is in block `((offset + i) / 4) &
        /// (map_size - 1)`, at slot `(offset + i) % 4`.
        pub fn read_queue(
            header: Header,
            read_u64: &mut dyn FnMut(u64) -> Option<u64>,
            read_i32: &mut dyn FnMut(u64) -> Option<i32>,
        ) -> Result<Queue, &'static str> {
            let mut hash = Fnv1a::new();
            hash.write(&header.len.to_le_bytes());
            if header.len == 0 {
                return Ok(Queue {
                    len: 0,
                    front: None,
                    hash: hash.0,
                });
            }
            if header.len > MAX_QUEUE {
                return Err("the free-id queue is longer than the trace reads");
            }
            if header.map == 0 || !header.map_size.is_power_of_two() {
                return Err("the free-id queue's block map is not one");
            }
            let block_ids = layout::QUEUE_BLOCK_IDS;
            let mut front = None;
            let mut block_at = u64::MAX;
            let mut block = 0u64;
            for i in 0..header.len {
                let at = header
                    .offset
                    .checked_add(i)
                    .ok_or("the queue's offset overflows")?;
                let index = (at / block_ids) & (header.map_size - 1);
                if index != block_at {
                    block = read_u64(header.map.wrapping_add(index * 8))
                        .ok_or("a block of the free-id queue is unreadable")?;
                    if block == 0 {
                        return Err("the free-id queue has a block missing");
                    }
                    block_at = index;
                }
                let id = read_i32(block.wrapping_add((at % block_ids) * 4))
                    .ok_or("an id of the free-id queue is unreadable")?;
                if i == 0 {
                    front = Some(id);
                }
                hash.write(&id.to_le_bytes());
            }
            Ok(Queue {
                len: header.len,
                front,
                hash: hash.0,
            })
        }

        /// The checkpoint line: two games of a room log equal ones at equal
        /// steps, the counts outside updates (0 in a sound game) included.
        pub fn checkpoint_text(step: u64, queue: Result<Queue, &str>, tally: &Tally) -> String {
            let queue = match queue {
                Ok(q) => format!(
                    "queued={} front={} hash={:016x}",
                    q.len,
                    q.front.map_or_else(|| "-".to_owned(), |f| f.to_string()),
                    q.hash
                ),
                Err(why) => format!("queue unread ({why})"),
            };
            let mut line = format!(
                "free ids: step {step}: {queue}; updates: {} modification(s) took {} and freed {} ({:016x}); outside updates: {} took {} and freed {}",
                tally.modifications,
                tally.taken,
                tally.freed,
                tally.freed_hash,
                tally.outside,
                tally.outside_taken,
                tally.outside_freed
            );
            if tally.unfit > 0 {
                line.push_str(&format!("; {} unfit", tally.unfit));
            }
            line
        }

        /// The line for one modification that took or freed ids outside an
        /// update.
        pub fn outside_text(
            place: Place,
            taken: u64,
            freed: u64,
            caller: Option<u64>,
            total: u64,
        ) -> String {
            let to = caller.map_or_else(
                || "an unread return address".to_owned(),
                |rva| format!("{rva:#x}"),
            );
            format!(
                "freed-id trace: the simulation's engine took {taken} and freed {freed} id(s) {} (returning to {to}); {total} such modification(s) since the world loaded",
                place.says()
            )
        }

        /// Whether the line for the `n`th modification outside an update is
        /// written.
        pub fn outside_logged(n: u64) -> bool {
            n <= OUTSIDE_FIRST || n.is_multiple_of(OUTSIDE_EVERY)
        }

        /// The simulation's engine as the step names it: `[[this +
        /// STEP_DATA] + DATA_ENGINE]`.
        pub fn engine_of(this: u64, read_u64: &mut dyn FnMut(u64) -> Option<u64>) -> Option<u64> {
            let data = read_u64(this.checked_add(layout::STEP_DATA)?)?;
            if data == 0 {
                return None;
            }
            let engine = read_u64(data.checked_add(layout::DATA_ENGINE)?)?;
            (engine != 0).then_some(engine)
        }

        /// Whether the trace runs.
        pub fn on() -> bool {
            ON.load(Ordering::Acquire) && !BROKEN.load(Ordering::Acquire)
        }

        /// Turns the trace on once its checks passed.
        pub(crate) fn arm(base: u64) {
            BASE.store(base, Ordering::Release);
            ON.store(true, Ordering::Release);
        }

        fn trace() -> std::sync::MutexGuard<'static, Trace> {
            TRACE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn note_unread() {
            trace().unread += 1;
        }

        /// From the step detour, on the simulation thread, before each call
        /// of the game's step: the room's game names its simulation's
        /// engine, any other game none. Another engine (a world loaded)
        /// starts the counts again.
        pub fn note_step(this: u64, room: bool) {
            if !on() {
                return;
            }
            let engine = if room {
                let mut probe = Probe::new();
                engine_of(this, &mut |at| probe.read::<u64>(at)).unwrap_or(0)
            } else {
                0
            };
            if SIM_ENGINE.swap(engine, Ordering::AcqRel) != engine {
                *trace() = Trace::new();
            }
        }

        /// A loaded world starts a fresh trace even if the allocator reuses
        /// the old engine's address.
        pub fn reset_world() {
            SIM_ENGINE.store(0, Ordering::Release);
            *trace() = Trace::new();
        }

        fn read_header(engine: u64, probe: &mut Probe) -> Option<Header> {
            let at = engine.checked_add(layout::FREE_IDS)?;
            Some(Header {
                map: probe.read(at.checked_add(layout::QUEUE_MAP)?)?,
                map_size: probe.read(at.checked_add(layout::QUEUE_MAP_SIZE)?)?,
                offset: probe.read(at.checked_add(layout::QUEUE_OFFSET)?)?,
                len: probe.read(at.checked_add(layout::QUEUE_SIZE)?)?,
            })
        }

        fn table_rows(engine: u64, probe: &mut Probe) -> Option<u64> {
            let at = engine.checked_add(layout::ENTITY_TABLE)?;
            let begin: u64 = probe.read(at)?;
            let end: u64 = probe.read(at.checked_add(8)?)?;
            let bytes = end.checked_sub(begin)?;
            bytes
                .is_multiple_of(layout::ENTITY_ROW)
                .then_some(bytes / layout::ENTITY_ROW)
        }

        /// At the free-id site, after the sort: counts the modification
        /// when it is the simulation's engine's, and says one that took or
        /// freed ids outside an update.
        /// `saved` is the stub's block: the trace reads the return address
        /// from the stack right above it.
        pub fn observe(saved: &SavedRegs) {
            if !on() {
                return;
            }
            let body = || {
                let engine = SIM_ENGINE.load(Ordering::Acquire);
                if engine == 0
                    || saved.r13 != engine
                    || Some(saved.r12) != engine.checked_add(layout::BETWEEN_CHANGES)
                {
                    return;
                }
                let _timer = perf::time(Piece::PersonOrder);
                let mut probe = Probe::new();
                let Some(vector) = probe.read::<u64>(saved.r12) else {
                    note_unread();
                    return;
                };
                let (Some(begin), Some(end)) = (
                    probe.read::<u64>(vector),
                    probe.read::<u64>(vector.wrapping_add(8)),
                ) else {
                    note_unread();
                    return;
                };
                let Ok(count) = vector_len(begin, end, end) else {
                    note_unread();
                    return;
                };
                let Ok(count) = usize::try_from(count) else {
                    note_unread();
                    return;
                };
                let ids: Vec<i32> = (0..count as u64)
                    .map_while(|i| probe.read::<i32>(begin.wrapping_add(4 * i)))
                    .collect();
                if ids.len() != count {
                    note_unread();
                    return;
                }
                let (Some(table), Some(header)) = (
                    table_rows(engine, &mut probe),
                    read_header(engine, &mut probe),
                ) else {
                    note_unread();
                    return;
                };
                let seen = Seen {
                    table,
                    queued: header.len,
                    freed: count as u64,
                };
                let current_step = crate::seeds::current_step();
                let place = Place::of(crate::order::in_step(), current_step);
                let base = BASE.load(Ordering::Acquire);
                let slot = SavedRegs::rsp(std::ptr::from_ref(saved))
                    .wrapping_add(layout::RETURN_FROM_SITE);
                let caller = probe
                    .read::<u64>(slot)
                    .filter(|_| base != 0)
                    .and_then(|address| address.checked_sub(base));
                let (taken, total) = trace().count(seen, &ids, place, current_step, caller);
                let Some(taken) = taken else {
                    return;
                };
                if !outside_logged(total) {
                    return;
                }
                log::line(&outside_text(place, taken, seen.freed, caller, total));
            };
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
                BROKEN.store(true, Ordering::Release);
                log::line(
                    "freed-id trace: panicked on the game's thread; switched off for this game",
                );
            }
        }

        /// At a checkpoint (and the first batch after a load): the line two
        /// games must agree on; nothing when the trace is off or knows no
        /// engine.
        pub fn checkpoint_line(step: u64) -> Option<String> {
            if !on() {
                return None;
            }
            let engine = SIM_ENGINE.load(Ordering::Acquire);
            if engine == 0 {
                return None;
            }
            let body = || {
                let mut probe = Probe::new();
                let queue = match read_header(engine, &mut probe) {
                    Some(header) => {
                        read_queue(header, &mut |at| Probe::new().read::<u64>(at), &mut |at| {
                            Probe::new().read::<i32>(at)
                        })
                    }
                    None => Err("the free-id queue's header is unreadable"),
                };
                trace().checkpoint(step, queue)
            };
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
                Ok(line) => Some(line),
                Err(_) => {
                    BROKEN.store(true, Ordering::Release);
                    Some(
                        "freed-id trace: panicked at a checkpoint; switched off for this game"
                            .into(),
                    )
                }
            }
        }

        /// Flushes retained simulation-engine modifications after a divergent
        /// checkpoint. The same request may reach us as both a notice and a
        /// room-chat lane-dump ask; only the first writes the detail.
        pub fn incident_lines(diverged: u64) -> Vec<String> {
            if !on() {
                return Vec::new();
            }
            trace().incident(diverged)
        }

        /// Checks the code the trace relies on: `EndModification`'s head
        /// leaves the return address where the trace reads it, and the step
        /// names the engine it updates where the layout says.
        pub fn check(
            site: u64,
            step: Option<u64>,
            engine_update: Option<u64>,
            read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
        ) -> Result<(), String> {
            check_context(site, &[(layout::HEAD_FROM_SITE, &layout::HEAD[..])], read)
                .map_err(|why| format!("EndModification's head: {why}"))?;
            let step = step.ok_or("the profile has no GameSim::Step")?;
            let update = engine_update
                .ok_or_else(|| format!("the profile has no {}", crate::seeds::UPDATE_TARGET))?;
            let call = step.wrapping_add(layout::STEP_ENGINE_CALL_AT);
            check_context(call, &[(0, &layout::STEP_ENGINE_CALL[..])], read)
                .map_err(|why| format!("GameSim::Step's engine update: {why}"))?;
            let at = call.wrapping_add(layout::STEP_ENGINE_CALL.len() as u64 - 1);
            match call_target(at, read) {
                Some(target) if target == update => Ok(()),
                Some(target) => Err(format!(
                    "GameSim::Step's engine update calls {target:#x}, not {}",
                    crate::seeds::UPDATE_TARGET
                )),
                None => Err("GameSim::Step's engine update call is unreadable".into()),
            }
        }

        /// Turns the trace on when the freed-id fix is in and the code the
        /// trace relies on is the measured code; returns hook.log's line.
        pub fn install(
            resolved: &ResolvedProfile,
            wanted: bool,
            read: &dyn Fn(u64, usize) -> Option<Vec<u8>>,
            base: u64,
        ) -> String {
            if !wanted {
                return format!("freed-id trace: off ({TOGGLE_ENV} says so)");
            }
            if !BATCH.state.installed.load(Ordering::Acquire) {
                return "freed-id trace: off (the freed-id-order fix is not installed)".into();
            }
            let Some(site) = resolved.get(SITE) else {
                return format!("freed-id trace: off (the profile has no {SITE:?})");
            };
            let step = resolved.get("GameSim::Step").map(|t| t.address);
            let update = resolved.get(crate::seeds::UPDATE_TARGET).map(|t| t.address);
            match check(site.address, step, update, read) {
                Ok(()) => {
                    arm(base);
                    format!(
                        "freed-id trace: on (a fingerprint of the free-id queue at every checkpoint, which two games must log alike, and each modification of the simulation's engine that takes or frees ids outside the room's updates; {TOGGLE_ENV}=0 turns it off)"
                    )
                }
                Err(why) => format!("freed-id trace: off ({why})"),
            }
        }
        #[cfg(test)]
        mod operation_tests {
            use super::*;

            fn queue() -> Result<Queue, &'static str> {
                Ok(Queue {
                    len: 0,
                    front: None,
                    hash: 0,
                })
            }

            #[test]
            fn an_early_allocator_split_survives_until_the_later_lane_mismatch() {
                let mut trace = Trace::new();
                for i in 0..53 {
                    trace.count(
                        Seen {
                            table: 100,
                            queued: i,
                            freed: 1,
                        },
                        &[i as i32],
                        Place::Update,
                        Some(18_601 + i),
                        Some(0x2bb4fd1),
                    );
                }
                trace.checkpoint(18_650, queue());
                for step in (18_700..=21_300).step_by(50) {
                    trace.checkpoint(step, queue());
                }
                let lines = trace.incident(21_300);
                assert!(lines[0].contains("recorded=53/53"), "{}", lines[0]);
                assert!(lines[0].contains("complete=true"), "{}", lines[0]);
                assert!(lines.iter().any(|line| line.contains("step 18650 part 1")));
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains(":U:0/1:100/52:52/52/"))
                );
                assert!(
                    trace.incident(21_300).is_empty(),
                    "notice and room chat must not double the burst"
                );
            }

            #[test]
            fn incident_detail_bursts_are_rate_limited_and_later_report_suppressed_steps() {
                let mut trace = Trace::new();
                let start = Instant::now();
                for step in (50..=3_200).step_by(50) {
                    trace.checkpoint(step, queue());
                }

                let first = trace.incident_at(3_200, start);
                assert!(first[0].contains("complete=true"), "{}", first[0]);
                assert!(first[0].contains("suppressed=0 suppressed_through=-"));

                trace.checkpoint(3_250, queue());
                let limited = trace.incident_at(3_250, start + std::time::Duration::from_secs(1));
                assert_eq!(
                    limited.len(),
                    1,
                    "one compact notice per suppression window"
                );
                assert!(
                    limited[0].contains("detail burst rate-limited"),
                    "{limited:?}"
                );
                assert!(
                    limited[0].contains("suppressed=1 through=3250"),
                    "{limited:?}"
                );

                trace.checkpoint(3_300, queue());
                assert!(
                    trace
                        .incident_at(3_300, start + std::time::Duration::from_secs(2))
                        .is_empty()
                );
                for step in (3_350..=6_450).step_by(50) {
                    trace.checkpoint(step, queue());
                }
                trace.count(
                    Seen {
                        table: 100,
                        queued: 1,
                        freed: 1,
                    },
                    &[7],
                    Place::Update,
                    Some(6_449),
                    Some(0x123),
                );
                trace.checkpoint(6_500, queue());
                let later = trace.incident_at(6_500, start + std::time::Duration::from_secs(61));
                assert!(
                    later[0].contains("suppressed=2 suppressed_through=3300"),
                    "{}",
                    later[0]
                );
                assert!(later[0].contains("complete=true"), "{}", later[0]);
                assert!(later.iter().any(|line| line.contains("step 6500 part 1")));
                assert!(
                    later
                        .iter()
                        .map(|line| line.len() + LOG_LINE_OVERHEAD)
                        .sum::<usize>()
                        + limited[0].len()
                        + LOG_LINE_OVERHEAD
                        <= MAX_FLUSH_BYTES
                );
            }

            #[test]
            fn incident_selects_the_recent_sixty_four_checkpoints() {
                let mut trace = Trace::new();
                for checkpoint in 1..=KEPT_CHECKPOINTS {
                    if checkpoint == 1 || checkpoint == 75 {
                        trace.count(
                            Seen {
                                table: 100,
                                queued: checkpoint as u64,
                                freed: 0,
                            },
                            &[],
                            Place::Update,
                            Some(checkpoint as u64),
                            Some(0x123),
                        );
                    }
                    trace.checkpoint(checkpoint as u64, queue());
                }
                let lines = trace.incident(KEPT_CHECKPOINTS as u64);
                assert!(lines[0].contains("retained_checkpoints=64"), "{}", lines[0]);
                assert!(lines[0].contains("oldest=65 newest=128"), "{}", lines[0]);
                assert!(lines.iter().any(|line| line.contains("step 75 part 1")));
                assert!(!lines.iter().any(|line| line.contains("step 1 part 1")));
            }

            #[test]
            fn old_expired_checkpoints_do_not_mark_a_complete_incident_window_incomplete() {
                let mut trace = Trace::new();
                for checkpoint in 1..=(KEPT_CHECKPOINTS + 1) {
                    trace.count(
                        Seen {
                            table: 100,
                            queued: checkpoint as u64,
                            freed: 1,
                        },
                        &[checkpoint as i32],
                        Place::Update,
                        Some(checkpoint as u64),
                        Some(0x123),
                    );
                    trace.checkpoint(checkpoint as u64, queue());
                }
                let lines = trace.incident((KEPT_CHECKPOINTS + 1) as u64);
                assert!(lines[0].contains("expired_checkpoints=1"), "{}", lines[0]);
                assert!(lines[0].contains("oldest=66 newest=129"), "{}", lines[0]);
                assert!(lines[0].contains("complete=true"), "{}", lines[0]);
            }

            #[test]
            fn delayed_incident_without_its_full_retained_window_is_incomplete() {
                let mut trace = Trace::new();
                for checkpoint in 1..=(KEPT_CHECKPOINTS + 1) {
                    trace.checkpoint(checkpoint as u64, queue());
                }
                let lines = trace.incident(2);
                assert!(lines[0].contains("retained_checkpoints=1"), "{}", lines[0]);
                assert!(lines[0].contains("complete=false"), "{}", lines[0]);
            }

            #[test]
            fn caps_and_expiry_are_visible_in_the_incident_header() {
                let mut trace = Trace::new();
                for i in 0..=MAX_OPERATIONS {
                    trace.count(
                        Seen {
                            table: 100,
                            queued: i as u64,
                            freed: 1,
                        },
                        &[i as i32],
                        Place::Update,
                        Some(50),
                        Some(0x123),
                    );
                }
                trace.checkpoint(50, queue());
                let first_at = Instant::now();
                let lines = trace.incident_at(50, first_at);
                assert!(
                    lines[0].contains("recorded=96/96 omitted=1"),
                    "{}",
                    lines[0]
                );
                assert!(lines[0].contains("complete=false"), "{}", lines[0]);
                assert!(lines.iter().all(|line| line.len() < 1024));
                for step in (100..=6_450).step_by(50) {
                    trace.checkpoint(step, queue());
                }
                let lines = trace.incident_at(6_450, first_at + crate::lanedump::GAP);
                assert!(lines[0].contains("expired_checkpoints=1"), "{}", lines[0]);
                assert!(lines[0].contains("oldest=3300"), "{}", lines[0]);
                assert!(lines[0].contains("complete=true"), "{}", lines[0]);
            }

            #[test]
            fn unread_native_state_marks_operation_detail_incomplete() {
                let mut trace = Trace::new();
                trace.unread = 1;
                trace.checkpoint(50, queue());
                let lines = trace.incident(50);
                assert!(lines[0].contains("unread=1"), "{}", lines[0]);
                assert!(lines[0].contains("complete=false"), "{}", lines[0]);
            }

            #[test]
            fn byte_limit_reports_partial_detail() {
                let mut trace = Trace::new();
                for checkpoint in 1..=KEPT_CHECKPOINTS {
                    for id in 0..MAX_OPERATIONS {
                        trace.count(
                            Seen {
                                table: 100,
                                queued: (checkpoint * MAX_OPERATIONS + id) as u64,
                                freed: 1,
                            },
                            &[id as i32],
                            Place::Update,
                            Some(checkpoint as u64),
                            Some(0x123456),
                        );
                    }
                    trace.checkpoint(checkpoint as u64, queue());
                }
                let lines = trace.incident(KEPT_CHECKPOINTS as u64);
                assert!(lines[0].contains("complete=false"), "{}", lines[0]);
                assert!(
                    lines
                        .iter()
                        .map(|line| line.len() + LOG_LINE_OVERHEAD)
                        .sum::<usize>()
                        <= MAX_FLUSH_BYTES - SUPPRESSION_NOTICE_RESERVE
                );
                assert!(lines.iter().all(|line| line.len() < 1024));
            }

            #[test]
            fn even_widest_numeric_records_fit_telemetry_lines() {
                let mut trace = Trace::new();
                trace.operations = vec![
                    Operation {
                        step: Some(u64::MAX),
                        place: Place::OutsideStep,
                        taken: Some(u64::MAX),
                        seen: Seen {
                            table: u64::MAX,
                            queued: u64::MAX,
                            freed: u64::MAX
                        },
                        first_freed: Some(i32::MIN),
                        last_freed: Some(i32::MAX),
                        freed_hash: u64::MAX,
                        caller: Some(u64::MAX),
                    };
                    OPERATIONS_PER_LINE
                ];
                trace.checkpoint(u64::MAX, queue());
                let lines = trace.incident(u64::MAX);
                assert!(lines.iter().all(|line| line.len() < 1024));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> ResolvedProfile {
        ResolvedProfile {
            name: "empty".into(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        }
    }

    const ALL: [&Batch; 5] = [
        &candidates::BATCH,
        &departures::BATCH,
        &arrivals::BATCH,
        &needs_path::BATCH,
        &freed_ids::BATCH,
    ];

    #[test]
    fn ids_are_sorted_ascending_and_a_sorted_batch_is_left_alone() {
        let mut ids = [30, 10, 20, 10];
        assert_eq!(sort_ids(&mut ids), Sorted::Reordered);
        assert_eq!(ids, [10, 10, 20, 30]);
        assert_eq!(sort_ids(&mut ids), Sorted::Unchanged);
        let mut one = [5];
        assert_eq!(sort_ids(&mut one), Sorted::Unchanged);
        assert_eq!(sort_ids(&mut []), Sorted::Unchanged);
    }

    #[test]
    fn two_histories_of_one_batch_sort_alike() {
        let mut a = [7, 3, 9, 1, 4];
        let mut b = [4, 1, 9, 7, 3];
        sort_ids(&mut a);
        sort_ids(&mut b);
        assert_eq!(a, b);
    }

    #[test]
    fn only_a_whole_vector_of_ids_is_sorted() {
        assert_eq!(vector_len(0, 0, 0), Ok(0));
        assert!(vector_len(0, 8, 8).is_err());
        assert_eq!(vector_len(0x1000, 0x100c, 0x1010), Ok(3));
        assert!(vector_len(0x1000, 0x100a, 0x1010).is_err(), "not whole ids");
        assert!(vector_len(0x1002, 0x100a, 0x1010).is_err(), "misaligned");
        assert!(
            vector_len(0x1010, 0x1000, 0x1010).is_err(),
            "end before begin"
        );
        assert!(vector_len(0x1000, 0x1010, 0x100c).is_err(), "past capacity");
        assert!(vector_len(0x1000, 0x1000 + 4 * (MAX_IDS + 1), u64::MAX).is_err());
    }

    /// A `std::vector<Entity>` header and its ids in this test's memory.
    #[cfg(windows)]
    struct Vector {
        header: Box<[u64; 3]>,
        _ids: Vec<i32>,
    }

    #[cfg(windows)]
    impl Vector {
        fn new(ids: &[i32]) -> Self {
            let mut ids = ids.to_vec();
            ids.reserve(4);
            let begin = ids.as_mut_ptr() as u64;
            let header = Box::new([
                begin,
                begin + 4 * ids.len() as u64,
                begin + 4 * ids.capacity() as u64,
            ]);
            Self { header, _ids: ids }
        }

        fn at(&self) -> u64 {
            self.header.as_ptr() as u64
        }

        fn ids(&self) -> Vec<i32> {
            let n = ((self.header[1] - self.header[0]) / 4) as usize;
            // SAFETY: the ids this vector owns.
            unsafe { std::slice::from_raw_parts(self.header[0] as *const i32, n) }.to_vec()
        }
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn a_vector_in_memory_is_sorted_in_place() {
        let vector = Vector::new(&[9, 2, 5]);
        let mut probe = Probe::new();
        assert_eq!(sort_vector(vector.at(), &mut probe), Ok(Sorted::Reordered));
        assert_eq!(vector.ids(), vec![2, 5, 9]);
        assert_eq!(sort_vector(vector.at(), &mut probe), Ok(Sorted::Unchanged));
        assert!(sort_vector(0, &mut probe).is_err(), "unreadable header");
    }

    fn regs() -> SavedRegs {
        SavedRegs {
            rflags: 0,
            r15: 0,
            r14: 0,
            r13: 0,
            r12: 0,
            r11: 0,
            r10: 0,
            r9: 0,
            r8: 0,
            rdi: 0,
            rsi: 0,
            rbp: 0,
            rbx: 0,
            rdx: 0,
            rcx: 0,
            rax: 0,
        }
    }

    #[test]
    fn each_site_finds_its_vectors_where_the_engine_keeps_them() {
        // departures: [rbp+7] and [rbp+0x1f]; arrivals: [rbp-0x78].
        assert_eq!(departures::vectors_at(0x1000).as_slice(), &[0x1007, 0x101f]);
        assert_eq!(arrivals::vectors_at(0x1000).as_slice(), &[0xf88]);
        // candidates: [rsp+0x70], which rbx must name.
        assert_eq!(
            candidates::vectors_at(0x2000, 0x2070).map(|v| v.as_slice().to_vec()),
            Ok(vec![0x2070])
        );
        assert!(candidates::vectors_at(0x2000, 0x2078).is_err());
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_freed_ids_hook_sorts_the_modifications_removed_ids() {
        let vector = Vector::new(&[44, 12, 30]);
        let slot = Box::new(vector.at());
        let mut regs = regs();
        regs.r12 = &*slot as *const u64 as u64;
        run(&freed_ids::BATCH, &mut regs);
        assert_eq!(vector.ids(), vec![12, 30, 44]);
        // An empty slot is a refusal, nothing written.
        let none = Box::new(0u64);
        regs.r12 = &*none as *const u64 as u64;
        run(&freed_ids::BATCH, &mut regs);
        assert!(freed_ids::BATCH.state.refused.load(Ordering::Relaxed) >= 1);
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_departures_hook_sorts_both_batches() {
        let persons = Vector::new(&[8, 3]);
        let cargo = Vector::new(&[7, 1, 4]);
        // A frame whose [rbp+7] and [rbp+0x1f] hold the two headers.
        let mut frame = vec![0u8; 0x80];
        let rbp = frame.as_mut_ptr() as u64 + 0x20;
        for (offset, vector) in [(7i64, &persons), (0x1f, &cargo)] {
            let at = (rbp.wrapping_add_signed(offset) - frame.as_ptr() as u64) as usize;
            for (i, word) in vector.header.iter().enumerate() {
                frame[at + 8 * i..at + 8 * i + 8].copy_from_slice(&word.to_le_bytes());
            }
        }
        let mut regs = regs();
        regs.rbp = rbp;
        run(&departures::BATCH, &mut regs);
        assert_eq!(persons.ids(), vec![3, 8]);
        assert_eq!(cargo.ids(), vec![1, 4, 7]);
    }

    #[test]
    fn a_shared_list_is_copied_first() {
        assert_eq!(needs_path::shared(Some(1)), Ok(false));
        assert_eq!(needs_path::shared(Some(2)), Ok(true));
        assert!(needs_path::shared(Some(0)).is_err());
        assert!(needs_path::shared(None).is_err());
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_needs_path_hook_sorts_an_unshared_list() {
        // The system: [+0x10] the data (the vector itself), [+0x18] the
        // control block, whose use count at +8 is 1.
        let list = Vector::new(&[600, 100, 300]);
        let control = Box::new([0u32; 4]);
        let mut control = control;
        control[2] = 1;
        let mut system = Box::new([0u64; 4]);
        system[2] = list.at();
        system[3] = control.as_ptr() as u64;
        let mut regs = regs();
        regs.r13 = system.as_mut_ptr() as u64;
        run(&needs_path::BATCH, &mut regs);
        assert_eq!(list.ids(), vec![100, 300, 600]);
    }

    #[test]
    fn calls_and_context_are_read_from_the_code() {
        // call rel32 at 0x100 to 0x200: rel = 0x200 - 0x105.
        let code = |at: u64, len: usize| -> Option<Vec<u8>> {
            let mut image = vec![0u8; 0x300];
            image[0x100] = 0xE8;
            image[0x101..0x105].copy_from_slice(&(0xfb_i32).to_le_bytes());
            image[0x120..0x124].copy_from_slice(&[1, 2, 3, 4]);
            let at = usize::try_from(at).ok()?;
            image.get(at..at + len).map(<[u8]>::to_vec)
        };
        assert_eq!(call_target(0x100, &code), Some(0x200));
        assert_eq!(call_target(0x101, &code), None);
        assert_eq!(
            check_context(0x110, &[(0x10, &[1, 2, 3, 4])], &code),
            Ok(())
        );
        assert!(check_context(0x110, &[(0x10, &[1, 2, 3, 5])], &code).is_err());
        assert!(check_context(0x110, &[(0x1000, &[1])], &code).is_err());
    }

    #[test]
    fn every_fix_is_on_unless_its_switch_or_the_master_says_off() {
        assert!(wanted(None, None));
        assert!(!wanted(Some("0"), None));
        assert!(!wanted(None, Some("off")));
        assert!(!wanted(Some("1"), Some("0")));
        let resolved = empty();
        let none = |_: u64| Ok(());
        for batch in ALL {
            let line = install_batch(batch, &resolved, false, &read_code, &none);
            assert!(
                line.starts_with(&format!("order fix {}: off", batch.fix))
                    && line.contains(MASTER_ENV)
                    && line.contains(batch.toggle_env)
                    && line.contains("same setting"),
                "{line}"
            );
            let line = install_batch(batch, &resolved, true, &read_code, &none);
            assert!(line.contains("the profile has no"), "{line}");
        }
    }

    #[test]
    fn the_master_line_states_the_rule() {
        assert!(master_line(true).starts_with("person-order: on"));
        assert!(master_line(false).starts_with("person-order: off"));
        for on in [true, false] {
            assert!(master_line(on).contains("every game of a room must run the same"));
        }
    }

    #[test]
    fn every_switch_has_its_own_name() {
        let mut names: Vec<&str> = ALL.iter().map(|b| b.toggle_env).collect();
        names.push(MASTER_ENV);
        let mut fixes: Vec<&str> = ALL.iter().map(|b| b.fix).collect();
        names.sort_unstable();
        names.dedup();
        fixes.sort_unstable();
        fixes.dedup();
        assert_eq!(names.len(), ALL.len() + 1);
        assert_eq!(fixes.len(), ALL.len());
    }

    #[test]
    fn the_profile_states_each_sites_bytes() {
        let profile =
            tpf3mp_hookcore::profile::Profile::from_toml(crate::BUILT_IN_PROFILES[0].1).unwrap();
        let target = |name: &str| {
            profile
                .targets
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} in the profile"))
        };
        for batch in ALL {
            let site = target(batch.target);
            assert!(!site.required, "{} is optional", batch.target);
            assert!(
                batch.expected.starts_with(&site.prologue),
                "{}: the profile's prologue is the fix's expected bytes",
                batch.target
            );
            assert!(site.prologue.len() >= batch.steal);
            assert!(batch.steal >= 5 && batch.steal <= batch.expected.len());
        }
        assert_eq!(target(needs_path::GETTER_CALL).prologue, vec![0xE8]);
    }
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod splice_tests {
    use super::*;

    fn fixture(code: &[u8]) -> usize {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc,
        };
        assert!(code.len() <= 0x1000);
        // SAFETY: a fresh read-write-execute page for the test's own code.
        let page = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                0x1000,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        assert!(!page.is_null());
        // SAFETY: `code.len()` bytes into a 4 KiB writable page.
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), page.cast::<u8>(), code.len()) };
        page as usize
    }

    /// The freed-id site's real bytes in a hand-written function: `push r12;
    /// push r13; push rbp; mov r12, imm64; mov r13, imm64; mov rbp, rsp;
    /// sub rsp, 0x80; <site>; ret-target: ...` where the call reaches a
    /// `ret` placed right after it, and then the function returns the first
    /// id of the vector `[r12]` names.
    #[test]
    fn the_freed_id_hook_sorts_through_its_real_site() {
        let ids = vec![50i32, 20, 40, 10];
        let mut ids = ids;
        let begin = ids.as_mut_ptr() as u64;
        let header = Box::new([begin, begin + 16, begin + 16]);
        let slot = Box::new(header.as_ptr() as u64);
        // engine+0xf8 is read by the site: a fake engine big enough.
        let engine = vec![0u8; 0x200];

        let mut code = vec![0x41, 0x54, 0x41, 0x55, 0x55]; // push r12; push r13; push rbp
        code.extend_from_slice(&[0x49, 0xBC]); // mov r12, imm64
        code.extend_from_slice(&(&*slot as *const u64 as u64).to_le_bytes());
        code.extend_from_slice(&[0x49, 0xBD]); // mov r13, imm64
        code.extend_from_slice(&(engine.as_ptr() as u64).to_le_bytes());
        code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        code.extend_from_slice(&[0x48, 0x81, 0xEC, 0x80, 0x00, 0x00, 0x00]); // sub rsp, 0x80
        let site_at = code.len();
        code.extend_from_slice(&freed_ids::EXPECTED);
        // The call's rel32: to a `ret` placed after the epilogue below.
        let rel_at = code.len();
        code.extend_from_slice(&[0, 0, 0, 0]);
        // mov rax, [r12]; mov rax, [rax]; mov eax, [rax]; mov rsp, rbp;
        // pop rbp; pop r13; pop r12; ret
        code.extend_from_slice(&[
            0x49, 0x8B, 0x04, 0x24, 0x48, 0x8B, 0x00, 0x8B, 0x00, 0x48, 0x89, 0xEC, 0x5D, 0x41,
            0x5D, 0x41, 0x5C, 0xC3,
        ]);
        let callee_at = code.len();
        code.push(0xC3);
        let rel = (callee_at - (rel_at + 4)) as i32;
        code[rel_at..rel_at + 4].copy_from_slice(&rel.to_le_bytes());
        let page = fixture(&code);
        // SAFETY: the page holds our hand-written function of no arguments.
        let fun: extern "C" fn() -> u32 =
            unsafe { std::mem::transmute::<usize, extern "C" fn() -> u32>(page) };
        assert_eq!(fun(), 50, "without the hook, the removal order stands");

        // SAFETY: the fixture is this test's own code, not running now; the
        // hook only sorts the vector `[r12]` names.
        let splice = unsafe {
            Splice::install(
                (page + site_at) as *mut u8,
                &freed_ids::EXPECTED,
                freed_ids::BATCH.steal,
                freed_ids::BATCH.hook,
            )
        }
        .unwrap();
        assert_eq!(fun(), 10, "with the hook, the lowest id is first");
        assert_eq!(ids, vec![10, 20, 40, 50]);
        drop(splice);
        drop(header);
    }
}

#[cfg(test)]
mod trace_tests {
    use std::collections::HashMap;

    use super::freed_ids::trace::*;
    use crate::build_data::native::persons::freed_ids as layout;

    /// A fake memory: block pointers and ids by address.
    #[derive(Default)]
    struct Memory {
        words: HashMap<u64, u64>,
        ids: HashMap<u64, i32>,
    }

    impl Memory {
        fn read(&self, header: Header) -> Result<Queue, &'static str> {
            read_queue(header, &mut |at| self.words.get(&at).copied(), &mut |at| {
                self.ids.get(&at).copied()
            })
        }
    }

    /// A queue of `ids` in a map of `map_size` blocks of four, its front at
    /// `offset`: the blocks are at 0x1000 * (block + 1), the map at 0x100.
    fn queue(ids: &[i32], map_size: u64, offset: u64) -> (Memory, Header) {
        let mut memory = Memory::default();
        for block in 0..map_size {
            memory.words.insert(0x100 + 8 * block, 0x1000 * (block + 1));
        }
        for (i, id) in ids.iter().enumerate() {
            let at = offset + i as u64;
            let block = (at / 4) & (map_size - 1);
            memory.ids.insert(0x1000 * (block + 1) + 4 * (at % 4), *id);
        }
        let header = Header {
            map: 0x100,
            map_size,
            offset,
            len: ids.len() as u64,
        };
        (memory, header)
    }

    #[test]
    fn the_queue_is_read_front_to_back_across_blocks_and_around_the_map() {
        // Front at 13: slots 1..3 of block 3, then the map wraps to block 0.
        let ids = [70, 10, 50, 20, 60, 30];
        let (memory, header) = queue(&ids, 4, 13);
        let read = memory.read(header).unwrap();
        assert_eq!(read.len, 6);
        assert_eq!(read.front, Some(70));
        // The same ids in the same order hash alike wherever they sit.
        let (other, at) = queue(&ids, 8, 2);
        assert_eq!(other.read(at).unwrap(), read);
        // Another order of the same ids is another queue.
        let (swapped, at) = queue(&[10, 70, 50, 20, 60, 30], 4, 13);
        assert_ne!(swapped.read(at).unwrap().hash, read.hash);
        // So is one id fewer.
        let (shorter, at) = queue(&ids[..5], 4, 13);
        assert_ne!(shorter.read(at).unwrap().hash, read.hash);
        // An empty queue needs no map.
        let empty = Memory::default()
            .read(Header {
                map: 0,
                map_size: 0,
                offset: 0,
                len: 0,
            })
            .unwrap();
        assert_eq!((empty.len, empty.front), (0, None));
    }

    #[test]
    fn a_queue_that_is_not_one_is_refused() {
        let (memory, header) = queue(&[1, 2, 3], 4, 0);
        assert!(
            memory
                .read(Header {
                    map_size: 3,
                    ..header
                })
                .is_err()
        );
        assert!(memory.read(Header { map: 0, ..header }).is_err());
        assert!(
            memory
                .read(Header {
                    len: MAX_QUEUE + 1,
                    ..header
                })
                .is_err()
        );
        // A block missing, an id unreadable.
        let missing = Memory {
            words: HashMap::new(),
            ids: memory.ids.clone(),
        };
        assert!(missing.read(header).is_err());
        let hole = Memory {
            words: memory.words.clone(),
            ids: HashMap::new(),
        };
        assert!(hole.read(header).is_err());
    }

    fn seen(table: u64, queued: u64, freed: u64) -> Seen {
        Seen {
            table,
            queued,
            freed,
        }
    }

    #[test]
    fn ids_taken_are_new_rows_and_ids_popped_from_the_queue() {
        // 10 queued, 2 freed by the last modification: 12 after it. Now 9
        // queued and 3 more rows: 3 popped and 3 new.
        assert_eq!(taken_since(seen(100, 10, 2), seen(103, 9, 0)), Some(6));
        assert_eq!(taken_since(seen(100, 10, 2), seen(100, 12, 5)), Some(0));
        // The queue grew by more than was freed: another writer.
        assert_eq!(taken_since(seen(100, 10, 2), seen(100, 13, 0)), None);
        // The table shrank: another engine, or a load.
        assert_eq!(taken_since(seen(100, 10, 0), seen(99, 10, 0)), None);
    }

    #[test]
    fn a_place_is_named_by_the_threads_step_and_the_rooms_update() {
        assert_eq!(Place::of(true, Some(5)), Place::Update);
        assert_eq!(Place::of(true, None), Place::StepOutsideUpdate);
        // Another thread, whatever update the simulation runs meanwhile.
        assert_eq!(Place::of(false, Some(5)), Place::OutsideStep);
        assert_eq!(Place::of(false, None), Place::OutsideStep);
    }

    #[test]
    fn the_tally_keeps_updates_and_what_came_outside_them_apart() {
        let mut tally = Tally::new();
        assert_eq!(tally.count(seen(100, 10, 2), &[4, 9], Place::Update), None);
        assert_eq!(tally.count(seen(101, 11, 0), &[], Place::Update), None);
        // Outside an update and taking an id: said.
        assert_eq!(
            tally.count(seen(101, 10, 1), &[3], Place::StepOutsideUpdate),
            Some(1)
        );
        // Outside an update and touching no id: not counted.
        assert_eq!(tally.count(seen(101, 11, 0), &[], Place::OutsideStep), None);
        assert_eq!(
            (tally.modifications, tally.taken, tally.freed),
            (2, 2, 2),
            "the first one has nothing to compare with; then 1 new row and 1 popped"
        );
        assert_eq!(
            (tally.outside, tally.outside_taken, tally.outside_freed),
            (1, 1, 1)
        );
        let taken = tally.take();
        assert_eq!(taken.modifications, 2);
        assert_eq!((tally.modifications, tally.outside), (0, 0));
        assert_eq!(tally.outside_total, 1, "the total stays");
        // The last modification stays too: the next one is compared with it.
        assert_eq!(tally.count(seen(102, 11, 0), &[], Place::Update), None);
        assert_eq!(tally.taken, 1);
        // Counts that do not fit one history are counted apart.
        assert_eq!(tally.count(seen(90, 11, 0), &[], Place::OutsideStep), None);
        assert_eq!(tally.unfit, 1);
    }

    #[test]
    fn the_freed_ids_hash_follows_the_order_of_the_modifications() {
        let mut a = Tally::new();
        let mut b = Tally::new();
        a.count(seen(10, 0, 2), &[1, 2], Place::Update);
        a.count(seen(10, 2, 1), &[3], Place::Update);
        b.count(seen(10, 0, 1), &[3], Place::Update);
        b.count(seen(10, 1, 2), &[1, 2], Place::Update);
        assert_eq!(a.freed, b.freed);
        assert_ne!(a.freed_hash, b.freed_hash);
        // The same ids split into other modifications differ too.
        let mut c = Tally::new();
        c.count(seen(10, 0, 3), &[1, 2, 3], Place::Update);
        assert_ne!(a.freed_hash, c.freed_hash);
    }

    #[test]
    fn the_lines_say_what_two_games_compare() {
        let mut tally = Tally::new();
        tally.count(seen(100, 10, 2), &[4, 9], Place::Update);
        tally.count(seen(101, 11, 1), &[5], Place::OutsideStep);
        let queue = Queue {
            len: 12,
            front: Some(4),
            hash: 0xabc,
        };
        let line = checkpoint_text(350, Ok(queue), &tally);
        assert!(
            line.starts_with(
                "free ids: step 350: queued=12 front=4 hash=0000000000000abc; updates: 1 modification(s) took 0 and freed 2 ("
            ),
            "{line}"
        );
        assert!(
            line.ends_with("); outside updates: 1 took 2 and freed 1"),
            "{line}"
        );
        assert_eq!(
            checkpoint_text(1, Err("why"), &Tally::new()),
            format!(
                "free ids: step 1: queue unread (why); updates: 0 modification(s) took 0 and freed 0 ({:016x}); outside updates: 0 took 0 and freed 0",
                Tally::new().freed_hash
            )
        );
        assert_eq!(
            outside_text(Place::StepOutsideUpdate, 2, 1, Some(0xbace10), 3),
            "freed-id trace: the simulation's engine took 2 and freed 1 id(s) in the game's step outside the room's updates (returning to 0xbace10); 3 such modification(s) since the world loaded"
        );
        assert!(
            outside_text(Place::OutsideStep, 0, 1, None, 1)
                .contains("outside the game's step (returning to an unread return address)")
        );
        assert!(outside_logged(1) && outside_logged(OUTSIDE_FIRST));
        assert!(!outside_logged(OUTSIDE_FIRST + 1));
        assert!(outside_logged(OUTSIDE_EVERY * 3));
    }

    #[test]
    fn the_engine_is_the_one_the_step_updates() {
        let memory: HashMap<u64, u64> = [
            (0x500 + layout::STEP_DATA, 0x900),
            (0x900 + layout::DATA_ENGINE, 0x7000),
        ]
        .into_iter()
        .collect();
        let mut read = |at: u64| memory.get(&at).copied();
        assert_eq!(engine_of(0x500, &mut read), Some(0x7000));
        assert_eq!(engine_of(0x600, &mut read), None);
        let empty: HashMap<u64, u64> = [(0x500 + layout::STEP_DATA, 0)].into_iter().collect();
        assert_eq!(engine_of(0x500, &mut |at| empty.get(&at).copied()), None);
    }

    /// Code with `EndModification`'s head before its site, and the step's
    /// engine update calling `update_from_call` bytes past the call.
    fn code(update_from_call: i32) -> (HashMap<u64, u8>, u64, u64, u64) {
        let site = 0x10_0000u64;
        let step = 0x20_0000u64;
        let mut bytes = HashMap::new();
        let head = site.wrapping_add_signed(layout::HEAD_FROM_SITE);
        for (i, b) in layout::HEAD.iter().enumerate() {
            bytes.insert(head + i as u64, *b);
        }
        let call = step + layout::STEP_ENGINE_CALL_AT;
        for (i, b) in layout::STEP_ENGINE_CALL.iter().enumerate() {
            bytes.insert(call + i as u64, *b);
        }
        let e8 = call + layout::STEP_ENGINE_CALL.len() as u64 - 1;
        for (i, b) in update_from_call.to_le_bytes().iter().enumerate() {
            bytes.insert(e8 + 1 + i as u64, *b);
        }
        let update = (e8 + 5).wrapping_add_signed(i64::from(update_from_call));
        (bytes, site, step, update)
    }

    #[test]
    fn the_trace_checks_the_code_it_relies_on() {
        let (bytes, site, step, update) = code(0x1234);
        let read = |at: u64, len: usize| -> Option<Vec<u8>> {
            (0..len as u64)
                .map(|i| bytes.get(&(at + i)).copied())
                .collect()
        };
        assert_eq!(check(site, Some(step), Some(update), &read), Ok(()));
        assert!(check(site, Some(step), Some(update + 1), &read).is_err());
        assert!(check(site, None, Some(update), &read).is_err());
        assert!(check(site, Some(step), None, &read).is_err());
        assert!(check(site + 1, Some(step), Some(update), &read).is_err());
        assert!(check(site, Some(step + 1), Some(update), &read).is_err());
    }

    #[test]
    fn the_frame_puts_the_return_address_where_the_trace_reads_it() {
        // Three stores into the caller's home slots, then five pushes and
        // `sub rsp, 0x100`: the site's rsp is 0x128 below the return address.
        assert_eq!(
            &layout::HEAD[15..24],
            &[0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57]
        );
        assert_eq!(
            &layout::HEAD[29..36],
            &[0x48, 0x81, 0xEC, 0x00, 0x01, 0x00, 0x00]
        );
        assert_eq!(layout::RETURN_FROM_SITE, 5 * 8 + 0x100);
    }

    /// The engine's fields the trace reads, in this test's memory.
    #[cfg(windows)]
    struct Engine {
        words: Vec<u64>,
        table: Vec<u8>,
        map: Vec<u64>,
        block: Vec<i32>,
        removed: Vec<i32>,
        vector: Box<[u64; 3]>,
    }

    #[cfg(windows)]
    impl Engine {
        fn new() -> Self {
            Self {
                words: vec![0; 0x300 / 8],
                table: vec![0; 0x18 * 64],
                map: vec![0; 4],
                block: vec![0; 16],
                removed: Vec::with_capacity(8),
                vector: Box::new([0; 3]),
            }
        }

        fn at(&self) -> u64 {
            self.words.as_ptr() as u64
        }

        fn put(&mut self, offset: u64, value: u64) {
            self.words[(offset / 8) as usize] = value;
        }

        /// The engine as one modification ends: `rows` entities, the queue
        /// `queued` (one block), `removed` freed by the modification.
        fn set(&mut self, rows: u64, queued: &[i32], removed: &[i32]) {
            let table = self.table.as_ptr() as u64;
            self.put(layout::ENTITY_TABLE, table);
            self.put(layout::ENTITY_TABLE + 8, table + rows * layout::ENTITY_ROW);
            self.block[..queued.len()].copy_from_slice(queued);
            self.map[0] = self.block.as_ptr() as u64;
            self.put(
                layout::FREE_IDS + layout::QUEUE_MAP,
                self.map.as_ptr() as u64,
            );
            self.put(layout::FREE_IDS + layout::QUEUE_MAP_SIZE, 1);
            self.put(layout::FREE_IDS + layout::QUEUE_OFFSET, 0);
            self.put(layout::FREE_IDS + layout::QUEUE_SIZE, queued.len() as u64);
            self.removed.clear();
            self.removed.extend_from_slice(removed);
            let begin = self.removed.as_ptr() as u64;
            *self.vector = [
                begin,
                begin + 4 * removed.len() as u64,
                begin + 4 * self.removed.capacity() as u64,
            ];
            let vector = self.vector.as_ptr() as u64;
            self.put(layout::BETWEEN_CHANGES, vector);
        }
    }

    // Reads real memory through the hook's readable check, which only
    // answers on Windows.
    #[cfg(windows)]
    #[test]
    fn the_trace_counts_the_simulations_modifications_and_fingerprints_its_queue() {
        use tpf3mp_hookcore::detour::SavedRegs;

        let mut engine = Engine::new();
        // A GameSim whose data names the engine.
        let data: Box<[u64; 4]> = Box::new([0, 0, 0, engine.at()]);
        let sim: Box<[u64; 2]> = Box::new([0, data.as_ptr() as u64]);
        assert_eq!(layout::STEP_DATA, 8);
        assert_eq!(layout::DATA_ENGINE, 0x18);

        super::freed_ids::trace::arm(0);
        note_step(sim.as_ptr() as u64, true);
        crate::order::set_in_step(true);
        let mut frame = vec![0u8; std::mem::size_of::<SavedRegs>() + 0x200];
        let regs = frame.as_mut_ptr().cast::<SavedRegs>();
        let observe_at = |engine: &Engine| {
            // SAFETY: the frame is this test's, as big as the block and the
            // stack the trace reads above it.
            unsafe {
                (*regs).r13 = engine.at();
                (*regs).r12 = engine.at() + layout::BETWEEN_CHANGES;
            }
            // SAFETY: as above.
            observe(unsafe { &*regs });
        };
        // Modifications inside the step while no room's update is current
        // (the paused path's place): the first frees two ids and is the
        // baseline the next one's taken ids are counted from.
        engine.set(10, &[3, 5], &[8, 9]);
        observe_at(&engine);
        // The next one took a new id and popped one: said and counted.
        engine.set(11, &[5, 8, 9], &[]);
        observe_at(&engine);
        // Another engine's modification (the GUI's) is left alone.
        let other = Engine::new();
        observe_at(&other);
        crate::order::set_in_step(false);

        let line = checkpoint_line(50).expect("a line while the trace knows the engine");
        assert!(
            line.starts_with("free ids: step 50: queued=3 front=5 hash="),
            "{line}"
        );
        assert!(
            line.ends_with("; outside updates: 2 took 2 and freed 2"),
            "{line}"
        );
        // Outside a room the trace knows no engine and says nothing.
        note_step(sim.as_ptr() as u64, false);
        assert_eq!(checkpoint_line(100), None);
        drop((data, sim));
    }
}
