//! The emission throttle: a PROPOSAL that CHANGES SIMULATION RESULTS.
//!
//! On a 100 x 1000 tile world the noise and pollution grids hold 25.6 M
//! cells each, and `ecs::EmissionGridSystem::Update` sweeps both every
//! update: about 1.75 GB of memory traffic, 30 to 60 ms of a ~260 ms update
//! (investigation/TF3_BIGMAP_SIM_COST_2026-10-05.md §1). With
//! [`EVERY_ENV`] set to `N` (2 to [`MAX_EVERY`]), the grid's update and the
//! emitters' splat (`ecs::EmissionEmitterSystem::Update2`) run only on the
//! updates whose `GameTime.updateCount` is a multiple of `N`, each with the
//! `dt` the game passes. On the other updates both are skipped, so the
//! grids stand still.
//!
//! **Why the normal `dt`.** The grid's `Update` scales its weights by
//! `5·dt` and also runs `round(dt / 0.2)` steps: with `dt·N` the weights
//! sum to `N·(1 − decay) > 1` and the field grows without bound (the tests
//! run the game's own `Diffuse` to show it). With the normal `dt` and both
//! systems skipped together, the grids follow exactly the stock trajectory,
//! `N` times slower: the state after `k·N` throttled updates is the stock
//! state after `k` updates, bit for bit, for emitters that do not change.
//! The steady field is the same; the response to a change is `N` times
//! slower. Town noise and pollution ratings, the eco mechanics and the
//! script getters read the grids, so their results change.
//!
//! **Why `updateCount`.** It is `GameTime`'s update counter (+0x40),
//! advanced once before each `ecs::Engine::Update` on the running path and
//! never on the paused one (crate::ticks); it is in the save, and two games
//! of a room log it equal at every checkpoint. `ecs::Engine::Update` calls
//! every system's update serially on the step's thread, after that advance,
//! so both systems see the same count in an update and every game of a room
//! skips the same updates. The hook reads it through the game's own getter
//! from the step's `CGameTime` ([`crate::install::update_count_now`]); a
//! count it cannot read runs the stock update (and says so once).
//!
//! **Saves.** Nothing new is stored: the grids keep their size, the skipped
//! updates leave them as they were, and the phase comes from the saved
//! counter, so a save and reload, or a room's rebase, skips on the same
//! updates as before.
//!
//! **How.** Two vtable slots, never code: `EmissionGridSystem`'s slot 11
//! and `EmissionEmitterSystem`'s slot 12 point at gates that call the
//! function the slot held. Each slot is checked to hold the function the
//! profile resolved before it is rewritten, both are rewritten or neither,
//! and a target the profile lacks leaves the game stock with the reason in
//! `hook.log`. Patching the slots composes with any inline detour of the
//! functions themselves.
//!
//! **Every game of a room** must run the same `N`. The room does not
//! compare it yet (docs/BIGMAPS.md, "Simulation switches (proposal)"); the
//! hook says the setting in `hook.log` at install and at the first skipped
//! update.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::host::Host;

pub use crate::build40420::{
    EMITTER_SLOT, EMITTER_UPDATE2, EMITTER_VTABLE, GRID_SLOT, GRID_UPDATE, GRID_VTABLE, LEA_RAX_RIP,
};
use crate::log;

/// The patch's name in `hook.log`.
pub const FIX: &str = "big maps: emission throttle (PROPOSAL, changes simulation results)";

/// The package's setting `N` (2 to [`MAX_EVERY`]) runs the emission
/// systems on every `N`-th update; 0 or 1 leaves them as the game has them.
pub const EVERY_ENV: &str = "emission_every";

/// The largest `N`: past it a town's ratings would answer changes minutes
/// late at 1x.
pub const MAX_EVERY: u32 = 16;

/// The setting from [`EVERY_ENV`]'s value: `None` is the game's own.
pub fn every_setting(value: Option<&str>) -> Result<Option<u32>, String> {
    let value = value.map(|v| v.trim().to_ascii_lowercase());
    match value.as_deref() {
        None | Some("" | "0" | "1" | "off" | "false" | "no") => Ok(None),
        Some(text) => match text.parse::<u32>() {
            Ok(n) if (2..=MAX_EVERY).contains(&n) => Ok(Some(n)),
            _ => Err(format!(
                "{EVERY_ENV}={text:?} is not 2 to {MAX_EVERY} (or 0 for the game's own)"
            )),
        },
    }
}

/// Whether the emission systems run in the update numbered `update_count`:
/// on every `every`-th, always when the throttle is off (`every` below 2),
/// and always when the count is unread.
pub fn runs(update_count: Option<u32>, every: u32) -> bool {
    match update_count {
        _ if every < 2 => true,
        None => true,
        Some(count) => count.is_multiple_of(every),
    }
}

/// `N`; 0 while not installed.
static EVERY: AtomicU32 = AtomicU32::new(0);
/// What each slot held: the game's functions (or whatever detours them).
static GRID_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static EMITTER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static RAN: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static UNREAD_SAID: AtomicBool = AtomicBool::new(false);

/// The count the gates read: the game's, or a test's.
fn update_count() -> Option<u32> {
    #[cfg(test)]
    {
        let forced = tests::FORCED_COUNT.load(Ordering::Acquire);
        if forced >= 0 {
            return u32::try_from(forced).ok();
        }
    }
    crate::host::update_count()
}

/// The decision for one call of a gated system, with its log lines. Only
/// the grid's gate counts and logs, so each update is counted once.
fn gate(counting: bool) -> bool {
    let every = EVERY.load(Ordering::Acquire);
    let count = update_count();
    if count.is_none() && every >= 2 && !UNREAD_SAID.swap(true, Ordering::AcqRel) {
        log::line(&format!(
            "{FIX}: updateCount unread outside the game's step; those updates run the stock emission systems"
        ));
    }
    let run = runs(count, every);
    if counting {
        if run {
            RAN.fetch_add(1, Ordering::Relaxed);
        } else {
            let skipped = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
            if skipped == 1 || skipped.is_multiple_of(1 << 14) {
                log::line(&format!(
                    "{FIX}: {EVERY_ENV}={every}: skipping the emission systems at updateCount {} (ran {}, skipped {skipped}); every game of the room must run {EVERY_ENV}={every}",
                    count.unwrap_or_default(),
                    RAN.load(Ordering::Relaxed),
                ));
            }
        }
    }
    run
}

/// `EmissionGridSystem::Update(this, Engine*, INodeList*, float dt)`.
type GridUpdate = unsafe extern "system" fn(usize, usize, usize, f32);
/// `EmissionEmitterSystem::Update2(this, Engine*, int, float dt)`.
type EmitterUpdate = unsafe extern "system" fn(usize, usize, usize, f32);

/// Slot 11 of `EmissionGridSystem`'s vtable.
/// The grid gate's address, as a slot would hold it.
pub fn grid_gate_address() -> u64 {
    grid_gate as *const () as usize as u64
}

/// Whether `address` is one of the throttle's gates, which another slot
/// hook (the `perf: sim` timers) may find in a slot and wrap.
pub fn is_gate(address: u64) -> bool {
    address == grid_gate as *const () as usize as u64
        || address == emitter_gate as *const () as usize as u64
}

unsafe extern "C" fn grid_gate(this: usize, engine: usize, nodes: usize, dt: f32) {
    let original = GRID_ORIGINAL.load(Ordering::Acquire);
    if original == 0 || !gate(true) {
        return;
    }
    // SAFETY: what the slot held, called with the arguments the engine
    // passed (the function's own ABI: dt in xmm3).
    unsafe { std::mem::transmute::<usize, GridUpdate>(original)(this, engine, nodes, dt) }
}

/// Slot 12 of `EmissionEmitterSystem`'s vtable.
unsafe extern "C" fn emitter_gate(this: usize, engine: usize, count: usize, dt: f32) {
    let original = EMITTER_ORIGINAL.load(Ordering::Acquire);
    if original == 0 || !gate(false) {
        return;
    }
    // SAFETY: as for `grid_gate`.
    unsafe { std::mem::transmute::<usize, EmitterUpdate>(original)(this, engine, count, dt) }
}

/// What installing came to, for `hook.log`.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: installed ({reason})")
    } else {
        format!("{FIX}: off, {reason}")
    }
}

/// The slot `hook` names, checked to hold the function `function` names:
/// the slot's address and what it holds.
fn slot_of(host: &Host, hook: &str, function: &str) -> Result<(usize, usize), String> {
    let at = host.address(hook)?;
    let function_at = host.address(function)?;
    let held = crate::host::read::<usize>(at)
        .ok_or_else(|| format!("{hook}'s slot at {at:#x} is unreadable"))?;
    if held != function_at {
        return Err(format!(
            "{hook}'s slot at {at:#x} holds {held:#x}, not {function} at {function_at:#x}"
        ));
    }
    Ok((at, held))
}

/// Attaches the throttle if the package's setting asks for it: both slots,
/// each checked first. Returns the line for `hook.log`.
pub fn attach(host: &Host, every: Option<u32>) -> String {
    let Some(every) = every else {
        return outcome_line(
            false,
            &format!("{EVERY_ENV} is not set; the game's own emission updates"),
        );
    };
    let checked = slot_of(host, GRID_VTABLE, GRID_UPDATE)
        .and_then(|grid| Ok((grid, slot_of(host, EMITTER_VTABLE, EMITTER_UPDATE2)?)));
    let (grid, emitter) = match checked {
        Ok(slots) => slots,
        Err(why) => return outcome_line(false, &why),
    };
    EVERY.store(every, Ordering::Release);
    let asked = host
        .attach(GRID_VTABLE, grid_gate as *const (), &GRID_ORIGINAL)
        .and_then(|()| host.attach(EMITTER_VTABLE, emitter_gate as *const (), &EMITTER_ORIGINAL));
    match asked {
        Ok(()) => outcome_line(
            true,
            &format!(
                "{EVERY_ENV}={every}: the emission grid and emitters run only when updateCount % {every} == 0, with the game's dt; vtable slots at {:#x}, {:#x}; every game of a room runs the same setting, as the room's terms carry it",
                grid.0, emitter.0
            ),
        ),
        Err(why) => {
            EVERY.store(0, Ordering::Release);
            outcome_line(false, &why)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A count the gates read instead of the game's; -1 for none.
    pub(crate) static FORCED_COUNT: std::sync::atomic::AtomicI64 =
        std::sync::atomic::AtomicI64::new(-1);

    #[test]
    fn the_setting_is_two_to_sixteen_or_the_games_own() {
        assert_eq!(every_setting(None), Ok(None));
        assert_eq!(every_setting(Some("1")), Ok(None));
        assert_eq!(every_setting(Some("4")), Ok(Some(4)));
        assert!(every_setting(Some("17")).is_err());
    }

    #[test]
    fn the_systems_run_on_every_nth_update() {
        assert!(runs(Some(8), 4));
        assert!(!runs(Some(9), 4));
        assert!(
            runs(None, 4),
            "a count it cannot read runs the stock update"
        );
        assert!(runs(Some(9), 1), "off");
    }
}
