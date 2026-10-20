//! The build tools in the room's game (docs/HOOKS.md, "The build tools").
//!
//! The street, track and construction tools of Transport Fever 3 are
//! native: they queue a `WorldBuildProposal` command, which the simulation
//! applies at its next step, in this game alone. Nothing on the Lua side can
//! stop one (the tools send no `builder.proposalPrepareForApply`, the game
//! ignores an error raised in `onPreBuildProposal`, and emptying the
//! proposal there crashes it), so the hook does, in two places:
//!
//! - **At the click**, `CommandList::Add` on the main thread: a player's
//!   build queued in the room's game is counted ([`clicks`]). The mod's GUI
//!   keeps the proposal each preview showed, marked with the count it saw,
//!   so the one it saw last before the count went up is the one clicked, and
//!   hands that to the room.
//! - **At the apply**, the simulation's `WorldBuildProposal` apply: a
//!   player-initiated build in the room's game answers false, as a build the
//!   game refused, unless it is the room's own, which the mod's game script
//!   applies with the flag [`set_replaying`] up. The game then tells the
//!   tool it failed, through its own path; the room orders the build for
//!   every game, this one included.
//!
//! The command's layout is the build's own (build 40408's): a `Command`'s
//! payload pointer at +0, the payload's variant index at +0x9b8 (the
//! dispatcher's case minus one), and a `WorldBuildProposal` payload's
//! `playerInitiated` at +0x3d2. A build whose profile has not these targets
//! installs nothing here, and the GUI keeps refusing the tools.
//!
//! The module editor tells game scripts nothing of its proposals, so the GUI
//! has no preview to pair its click with. The add's detour is entered
//! through a thunk that notes where `Add` returns to; a click whose call
//! returns into the module editor's `MousePressed` has its proposal read
//! natively and kept for that click ([`crate::modules`]). A click whose call
//! returns into `ProposalAction::DoApply`, the terrain tools', the painter's
//! and the asset brush's, has its terraform read there ([`crate::terrain`]).
//! While the room's actions are applied, a build is filled with the
//! terraform the game script armed, if it armed one.
//!
//! Only Windows x64 installs the detours ([`install`]); elsewhere they are
//! built for the tests alone.

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::build_data::native::builds::COMMAND_PAYLOAD;
use crate::build_data::native::builds::PAYLOAD_INDEX;
use crate::build_data::native::builds::PLAYER_INITIATED;
use crate::build_data::native::builds::WORLD_BUILD_PROPOSAL;

/// The game's own add and apply, reached through their detours'
/// trampolines.
static ADD_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static APPLY_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// Both detours are in: the tools may build through the room.
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// The player's builds queued in the room's game so far.
static CLICKS: AtomicU64 = AtomicU64::new(0);
/// The player's builds the apply answered false for.
static STOPPED: AtomicU64 = AtomicU64::new(0);
/// The mod's game script is applying the room's actions.
static REPLAYING: AtomicBool = AtomicBool::new(false);

/// Whether the tools build through the room: both detours are in.
pub fn installed() -> bool {
    INSTALLED.load(Ordering::Acquire)
}

/// The player's builds queued in the room's game so far.
pub fn clicks() -> u64 {
    CLICKS.load(Ordering::Acquire)
}

/// The player's builds stopped at the apply so far.
pub fn stopped() -> u64 {
    STOPPED.load(Ordering::Acquire)
}

/// The mod's game script begins or ends applying the room's actions.
pub fn set_replaying(replaying: bool) {
    REPLAYING.store(replaying, Ordering::Release);
    crate::toolchain::replaying(replaying);
    // A crossing armed for a build of the room's never outlives its actions.
    if !replaying {
        crate::crossings::disarm();
    }
}

/// Whether `payload` is a `WorldBuildProposal`'s the player made.
///
/// # Safety
///
/// `payload` is a command payload of the game's, at least `PAYLOAD_INDEX + 1`
/// bytes.
unsafe fn player_build(payload: *const u8) -> bool {
    if payload.is_null() {
        return false;
    }
    // SAFETY: the caller's.
    unsafe {
        payload.add(PAYLOAD_INDEX).cast::<i8>().read_unaligned() == WORLD_BUILD_PROPOSAL
            && payload.add(PLAYER_INITIATED).read() == 1
    }
}

thread_local! {
    /// Where the `Add` call being handled on this thread returns to, as the
    /// entry thunk found it at `[rsp]`; 0 when unknown.
    static RETURN_ADDRESS: Cell<usize> = const { Cell::new(0) };
}

/// Notes where this thread's `Add` call returns to. Called by the thunk
/// with the argument registers saved.
extern "system" fn note_return_address(address: usize) {
    RETURN_ADDRESS.with(|slot| slot.set(address));
}

/// Takes the return address the thunk noted for this call.
fn take_return_address() -> usize {
    RETURN_ADDRESS.with(|slot| slot.replace(0))
}

/// The add's detour entry: saves the four argument registers, passes
/// `[rsp]` (where `Add` returns to) to [`note_return_address`], restores
/// them and jumps to [`add_detour`] with the stack as the caller left it,
/// so its stack argument is the caller's. (The thunk of `feat/capture-all`'s
/// `detours.rs`, by Juliansgith.)
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
unsafe extern "C" fn add_entry() {
    core::arch::naked_asm!(
        // Entry rsp is 8 mod 16: 0x48 makes it 16-aligned for the call,
        // with 0x20 of shadow space under the four saved registers.
        "sub rsp, 0x48",
        "mov [rsp + 0x20], rcx",
        "mov [rsp + 0x28], rdx",
        "mov [rsp + 0x30], r8",
        "mov [rsp + 0x38], r9",
        "mov rcx, [rsp + 0x48]",
        "call {note}",
        "mov r9, [rsp + 0x38]",
        "mov r8, [rsp + 0x30]",
        "mov rdx, [rsp + 0x28]",
        "mov rcx, [rsp + 0x20]",
        "add rsp, 0x48",
        "jmp {detour}",
        note = sym note_return_address,
        detour = sym add_detour,
    )
}

/// `CommandList::Add`'s signature: the list, the connection returned, the
/// command, the callback and the progress; returns the connection.
type AddFn = unsafe extern "C" fn(usize, usize, usize, usize, usize) -> usize;

/// The add's detour: counts the player's builds in the room's game, and
/// adds every command as the game would.
unsafe extern "C" fn add_detour(
    list: usize,
    connection: usize,
    command: usize,
    callback: usize,
    progress: usize,
) -> usize {
    let original = ADD_ORIGINAL.load(Ordering::Acquire);
    let return_address = take_return_address();
    // The stop tool waiting on this click, freed once it is queued
    // ([`crate::stoptool`]).
    let mut stop_tool = None;
    if command != 0 && crate::lua::in_room() {
        // SAFETY: the game passes the command it adds, whose first field is
        // its payload.
        let payload = unsafe { (command as *const usize).add(COMMAND_PAYLOAD).read() };
        // SAFETY: a command's payload, the size the dispatcher reads.
        if unsafe { player_build(payload as *const u8) } {
            stop_tool = crate::stoptool::before_add(return_address, callback);
            let click = CLICKS.fetch_add(1, Ordering::AcqRel);
            // The street or track tool's click, kept to continue from once
            // the room built it ([`crate::toolchain`]); any other drops it.
            // Read before Add, which takes the callback.
            crate::toolchain::before_add(return_address, callback, click);
            if crate::modules::is_module_editor(return_address) {
                // Read before the game takes it: the command is the
                // caller's until Add returns.
                crate::modules::record(&crate::modules::Process, click, payload);
            } else if crate::terrain::is_terrain_tool(return_address) {
                crate::terrain::record(&crate::modules::Process, click, payload);
            } else if crate::crossings::is_crossing_window(return_address) {
                crate::crossings::record(&crate::modules::Process, click, payload);
            } else {
                crate::junctions::record(&crate::modules::Process, click, payload);
                // The street and track tools' toolbar crossing type.
                crate::crossings::record_tool(&crate::modules::Process, click, payload);
            }
        }
    }
    // SAFETY: the trampoline of the add, called with the arguments the game
    // passed.
    let original: AddFn = unsafe { std::mem::transmute::<usize, AddFn>(original) };
    let added = unsafe { original(list, connection, command, callback, progress) };
    if let Some(tool) = stop_tool {
        crate::stoptool::after_add(tool);
    }
    added
}

/// The build apply's signature: the dispatcher's context and the payload;
/// returns whether it built.
type ApplyFn = unsafe extern "C" fn(usize, usize, usize, usize) -> u64;

/// The apply's detour: in the room's game, the player's own builds answer
/// false; the room's, and everyone else's (towns, the game's scripts), apply.
/// A build of the room's is first filled with the terraform the game script
/// armed for it, if any ([`crate::terrain::inject`]); one that cannot be
/// answers false.
unsafe extern "C" fn apply_detour(context: usize, payload: usize, r8: usize, r9: usize) -> u64 {
    let original = APPLY_ORIGINAL.load(Ordering::Acquire);
    if crate::lua::in_room() && !REPLAYING.load(Ordering::Acquire) {
        // SAFETY: the dispatcher passes the payload it dispatched on.
        if unsafe { player_build(payload as *const u8) } {
            STOPPED.fetch_add(1, Ordering::AcqRel);
            return 0;
        }
    }
    if REPLAYING.load(Ordering::Acquire)
        && let Err(why) = crate::terrain::inject(&mut crate::terrain::GameHeap, payload)
    {
        crate::log::line(&format!("terraform: the room's carrier was refused: {why}"));
        return 0;
    }
    #[cfg(all(windows, target_arch = "x86_64"))]
    if REPLAYING.load(Ordering::Acquire)
        && let Err(why) = crate::crossings::inject_game(payload)
    {
        crate::log::line(&format!("crossing: the room's carrier was refused: {why}"));
        return 0;
    }
    // SAFETY: the trampoline of the apply, called as the dispatcher called it.
    let original: ApplyFn = unsafe { std::mem::transmute::<usize, ApplyFn>(original) };
    let built = unsafe { original(context, payload, r8, r9) };
    // What the room's build made, for the street and track tools to
    // continue from ([`crate::toolchain`]).
    if built != 0 && REPLAYING.load(Ordering::Acquire) {
        crate::toolchain::collect(payload);
    }
    built
}

/// Detours the add and the apply, at the addresses the profile resolved.
///
/// # Safety
///
/// Both are the functions the profile names, in this process, which no
/// thread runs yet (the hook installs while the game starts).
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn install(
    add: usize,
    apply: usize,
    module_call: Option<usize>,
    terrain_call: Option<usize>,
    stop_call: Option<usize>,
    crossing_call: Option<(usize, bool)>,
    detour: unsafe fn(*mut u8, *const u8) -> Result<usize, String>,
) -> Result<(), String> {
    if let Some(call) = module_call {
        crate::modules::set_call(call);
    }
    if let Some(call) = terrain_call {
        crate::terrain::set_call(call);
    }
    if let Some(call) = stop_call {
        crate::stoptool::set_call(call);
    }
    if let Some((call, layout)) = crossing_call {
        crate::crossings::set_call(call, layout);
    }
    // SAFETY: the caller's; the entry thunk has the add's ABI and jumps to
    // add_detour, which has it too.
    let add_original = unsafe { detour(add as *mut u8, add_entry as *const u8) }?;
    ADD_ORIGINAL.store(add_original, Ordering::Release);
    // SAFETY: as above.
    let apply_original = unsafe { detour(apply as *mut u8, apply_detour as *const u8) }?;
    APPLY_ORIGINAL.store(apply_original, Ordering::Release);
    INSTALLED.store(true, Ordering::Release);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload of the game's shape: `index` at the variant index and
    /// `player` at playerInitiated.
    fn payload(index: i8, player: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; PAYLOAD_INDEX + 8];
        bytes[PAYLOAD_INDEX] = index as u8;
        bytes[PLAYER_INITIATED] = player;
        bytes
    }

    /// The entry thunk hands the add's detour every argument, the one on
    /// the stack included, and the detour reaches the original with them.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn the_entry_thunk_passes_every_argument_through() {
        unsafe extern "C" fn original(a: usize, b: usize, c: usize, d: usize, e: usize) -> usize {
            a + 2 * b + 3 * c + 4 * d + 5 * e
        }
        ADD_ORIGINAL.store(original as *const () as usize, Ordering::Release);
        // SAFETY: the thunk has the add's ABI; with no command (0) the
        // detour reads nothing of the arguments and calls `original`.
        let entry: AddFn =
            unsafe { std::mem::transmute::<*const (), AddFn>(add_entry as *const ()) };
        assert_eq!(unsafe { entry(1, 2, 0, 4, 5) }, 1 + 4 + 16 + 25);
        assert_eq!(
            take_return_address(),
            0,
            "the detour took what the thunk noted"
        );
        ADD_ORIGINAL.store(0, Ordering::Release);
    }

    #[test]
    fn only_the_players_own_world_builds_count() {
        let build = payload(WORLD_BUILD_PROPOSAL, 1);
        let script = payload(WORLD_BUILD_PROPOSAL, 0);
        let other = payload(WORLD_BUILD_PROPOSAL + 1, 1);
        // SAFETY: each is a buffer of the size player_build reads.
        unsafe {
            assert!(player_build(build.as_ptr()));
            assert!(!player_build(script.as_ptr()), "a town's or a script's");
            assert!(!player_build(other.as_ptr()), "another command");
            assert!(!player_build(std::ptr::null()));
        }
    }
}
