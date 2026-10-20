//! The GUI's native tools act as the player's company in a room
//! (docs/HOOKS.md, "The tools' player"; investigation/
//! TF3_LOCAL_PLAYER_2026-10-01.md, "Why the street tool will not split a
//! road the room built").
//!
//! `UI::CGameUI`'s constructor reads the save's player once and hands a copy
//! to each tool it builds. The room builds every road, track and
//! construction for the acting company (`PlayerOwned` = the company), so to
//! a player of any company but the room's first, the native tools take the
//! company's own edges for another player's: no split in the middle of a
//! road (`sub_610ea0` from the street builder's snap), no bulldozing, no
//! tram track onto its rail. This writes the company the GUI notes
//! (`note("tpf3mp.company")`, `tpf3mp/follow.lua`) into six tools' own
//! copies, at the start of each tool's frame, on the main thread:
//!
//! - `UI::StreetBuilder` (the street and the track builder, one class built
//!   twice), its player at `+0xc0` (the constructor's store, profile target
//!   [`STREET_STORE`]);
//! - `UI::TrackModifier` (the road and track modifiers: tram track, bus
//!   lane, electrification, ...), its player at `+0xa0` ([`MODIFIER_STORE`]);
//! - `UI::ConstructionBuilder` (stations, depots, every construction the
//!   menu places), `+0xa0` ([`CONSTRUCTION_STORE`]);
//! - `UI::StreetTerminalBuilder` (the stop builder, and the signal and
//!   waypoint builder, a second instance), `+0xa0` ([`TERMINAL_STORE`]);
//! - `UI::ModuleBuilder` (a station's modules), `+0xa8` ([`MODULE_STORE`]);
//! - `UI::Bulldozer`: its own player at `+0x28`, which its proposals are
//!   made for ([`BULLDOZER_STORE`]), and the one entry of its
//!   `BulldozerFilter`'s player list (`[[+0xc0] + 0x10]`,
//!   [`BULLDOZER_FILTER`]), which every bulldozer action asks
//!   (`sub_5f7db0`).
//!
//! Each is a UI object, made by `CGameUI` and read by its own class's code
//! only (the readers are listed in the profile). Nothing the simulation
//! runs reads them: the builds a tool makes are stopped at their apply and
//! built by the room as the acting company's, which every game checks
//! (`companies.mayTouch`). A value is written only where the field holds the
//! save's player (as the mod's game script notes it) or the company this
//! wrote; anything else is left alone and said once. Outside a room, for
//! the room's first company, while either note is missing, or with
//! [`ENV`]`=0`, the game's own value stays (and one this wrote goes back).

#![allow(unsafe_code)]
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use tpf3mp_hookcore::profile::ResolvedProfile;

pub use crate::build_data::native::toolplayer::COMPANY_NOTE;
pub use crate::build_data::native::toolplayer::ENV;
pub use crate::build_data::native::toolplayer::FIX;

pub use crate::build_data::native::toolplayer::BULLDOZER_FILTER;
pub use crate::build_data::native::toolplayer::BULLDOZER_STEP;
pub use crate::build_data::native::toolplayer::MODIFIER_STEP;
pub use crate::build_data::native::toolplayer::MODIFIER_STORE;
pub use crate::build_data::native::toolplayer::STREET_STEP;
pub use crate::build_data::native::toolplayer::STREET_STORE;

pub use crate::build_data::native::toolplayer::FILTER_BYTES;
pub use crate::build_data::native::toolplayer::FILTER_DISP_AT;
pub use crate::build_data::native::toolplayer::FILTER_PLAYERS;
pub use crate::build_data::native::toolplayer::MODIFIER_DISP_AT;
pub use crate::build_data::native::toolplayer::MODIFIER_STORE_BYTES;
pub use crate::build_data::native::toolplayer::STREET_DISP_AT;
pub use crate::build_data::native::toolplayer::STREET_STORE_BYTES;

/// Reads a little-endian disp32 at `at` as an offset below 64 KiB.
fn field_disp(bytes: &[u8], at: usize) -> Option<usize> {
    let disp = i32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?);
    let offset = usize::try_from(disp).ok()?;
    (offset < 0x1_0000 && offset.is_multiple_of(4)).then_some(offset)
}

/// The street builder's or the track modifier's player offset, if `code`
/// is the store expected (`expected`, with the field's disp32 at `at`).
pub fn store_offset(code: &[u8], expected: &[u8], at: usize) -> Option<usize> {
    let code = code.get(..expected.len())?;
    let same = code
        .iter()
        .zip(expected)
        .enumerate()
        .all(|(i, (c, e))| (at..at + 4).contains(&i) || c == e);
    if !same {
        return None;
    }
    field_disp(code, at)
}

/// The bulldozer filter's layout read from its constructor's code at
/// address `site`: the filter's vtable (absolute) and its place in the
/// bulldozer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterLayout {
    pub vtable: usize,
    pub filter: usize,
}

pub fn filter_layout(code: &[u8], site: usize) -> Option<FilterLayout> {
    let code = code.get(..48)?;
    let same = code
        .iter()
        .zip(FILTER_BYTES.iter())
        .all(|(c, e)| e.is_none_or(|e| *c == e));
    if !same {
        return None;
    }
    let rel = i32::from_le_bytes(code.get(3..7)?.try_into().ok()?);
    let vtable = site
        .checked_add(7)?
        .checked_add_signed(isize::try_from(rel).ok()?)?;
    let filter = usize::try_from(i32::from_le_bytes(
        code.get(FILTER_DISP_AT..FILTER_DISP_AT + 4)?
            .try_into()
            .ok()?,
    ))
    .ok()?;
    (filter < 0x1_0000 && filter.is_multiple_of(8)).then_some(FilterLayout { vtable, filter })
}

/// What to do with one tool's player field this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Leave it as it is.
    Leave,
    /// Write this player.
    Write(i32),
    /// It holds a value this does not know: leave it, and say so once.
    Unknown,
}

/// One tool object's field: what the game put there and what this wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Slot {
    pub original: Option<i32>,
    pub wrote: Option<i32>,
}

/// Decides for a field holding `current`. `company` is the player's
/// company, `save` the save's player, each when known; `active` whether
/// this acts at all (in a room, switched on).
pub fn decide(
    current: i32,
    slot: Slot,
    save: Option<i32>,
    company: Option<i32>,
    active: bool,
) -> Decision {
    let want = match (active, save, company) {
        (true, Some(save), Some(company)) if company >= 0 && company != save => Some(company),
        _ => None,
    };
    match want {
        Some(company) => {
            if current == company {
                Decision::Leave
            } else if Some(current) == save || Some(current) == slot.wrote {
                Decision::Write(company)
            } else {
                Decision::Unknown
            }
        }
        None => match (slot.wrote, slot.original) {
            (Some(wrote), Some(original)) if wrote == current && original != current => {
                Decision::Write(original)
            }
            _ => Decision::Leave,
        },
    }
}

pub use crate::build_data::native::toolplayer::BULLDOZER_DISP_AT;
pub use crate::build_data::native::toolplayer::BULLDOZER_LIST;
pub use crate::build_data::native::toolplayer::BULLDOZER_LIST_BYTES;
pub use crate::build_data::native::toolplayer::BULLDOZER_LIST_DISP_AT;
pub use crate::build_data::native::toolplayer::BULLDOZER_SETTER;
pub use crate::build_data::native::toolplayer::BULLDOZER_SETTER_BYTES;
pub use crate::build_data::native::toolplayer::BULLDOZER_SETTER_DISP_AT;
pub use crate::build_data::native::toolplayer::BULLDOZER_STORE;
pub use crate::build_data::native::toolplayer::BULLDOZER_STORE_BYTES;
pub use crate::build_data::native::toolplayer::CONSTRUCTION_DISP_AT;
pub use crate::build_data::native::toolplayer::CONSTRUCTION_STEP;
pub use crate::build_data::native::toolplayer::CONSTRUCTION_STORE;
pub use crate::build_data::native::toolplayer::CONSTRUCTION_STORE_BYTES;
pub use crate::build_data::native::toolplayer::MODULE_DISP_AT;
pub use crate::build_data::native::toolplayer::MODULE_STEP;
pub use crate::build_data::native::toolplayer::MODULE_STORE;
pub use crate::build_data::native::toolplayer::MODULE_STORE_BYTES;
pub use crate::build_data::native::toolplayer::TERMINAL_DISP_AT;
pub use crate::build_data::native::toolplayer::TERMINAL_STEP;
pub use crate::build_data::native::toolplayer::TERMINAL_STORE;
pub use crate::build_data::native::toolplayer::TERMINAL_STORE_BYTES;

/// The bulldozer's own player offset, if `code` is its store.
pub fn bulldozer_player_offset(code: &[u8]) -> Option<usize> {
    let code = code.get(..BULLDOZER_STORE_BYTES.len())?;
    let same = code
        .iter()
        .zip(BULLDOZER_STORE_BYTES)
        .enumerate()
        .all(|(i, (c, e))| (3..7).contains(&i) || i == BULLDOZER_DISP_AT || *c == e);
    let offset = usize::from(*code.get(BULLDOZER_DISP_AT)?);
    (same && offset.is_multiple_of(4) && offset < 0x80).then_some(offset)
}

/// Which tool a field is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Street,
    Modifier,
    Construction,
    Terminal,
    Module,
    Bulldozer,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::Street,
        Tool::Modifier,
        Tool::Construction,
        Tool::Terminal,
        Tool::Module,
        Tool::Bulldozer,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Tool::Street => "the street and track builder",
            Tool::Modifier => "the road and track modifier",
            Tool::Construction => "the construction builder",
            Tool::Terminal => "the stop and signal builder",
            Tool::Module => "the module builder",
            Tool::Bulldozer => "the bulldozer",
        }
    }

    /// Its `Step` and the target that gives its field.
    fn targets(self) -> (&'static str, &'static str) {
        match self {
            Tool::Street => (STREET_STEP, STREET_STORE),
            Tool::Modifier => (MODIFIER_STEP, MODIFIER_STORE),
            Tool::Construction => (CONSTRUCTION_STEP, CONSTRUCTION_STORE),
            Tool::Terminal => (TERMINAL_STEP, TERMINAL_STORE),
            Tool::Module => (MODULE_STEP, MODULE_STORE),
            Tool::Bulldozer => (BULLDOZER_STEP, BULLDOZER_STORE),
        }
    }

    /// For a tool with one stored player: the store's bytes and where its
    /// disp32 is.
    fn store(self) -> Option<(&'static [u8], usize)> {
        match self {
            Tool::Street => Some((&STREET_STORE_BYTES, STREET_DISP_AT)),
            Tool::Modifier => Some((&MODIFIER_STORE_BYTES, MODIFIER_DISP_AT)),
            Tool::Construction => Some((&CONSTRUCTION_STORE_BYTES, CONSTRUCTION_DISP_AT)),
            Tool::Terminal => Some((&TERMINAL_STORE_BYTES, TERMINAL_DISP_AT)),
            Tool::Module => Some((&MODULE_STORE_BYTES, MODULE_DISP_AT)),
            Tool::Bulldozer => None,
        }
    }
}

/// The fields seen, by address: at most this many are kept (CGameUI makes
/// a handful of tools; a new world makes them anew).
const SLOTS: usize = 64;

struct State {
    slots: Vec<(usize, Slot)>,
    said: Vec<String>,
    /// Writes of a company over the save's player the game set back.
    rewrites: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    slots: Vec::new(),
    said: Vec::new(),
    rewrites: 0,
});
static ON: AtomicBool = AtomicBool::new(false);
/// Set when a frame's work panicked: no more writes for this game.
static BROKEN: AtomicBool = AtomicBool::new(false);
/// Each tool's player offset (the bulldozer's own player for the
/// bulldozer), 0 while its tool is not handled.
static FIELDS: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
static FILTER_FIELD: AtomicUsize = AtomicUsize::new(0);
static FILTER_VTABLE: AtomicUsize = AtomicUsize::new(0);
/// The bulldozer's owner list's offset, 0 while unknown.
static LIST_FIELD: AtomicUsize = AtomicUsize::new(0);
static SETTER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static STREET_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static MODIFIER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static CONSTRUCTION_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static TERMINAL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static MODULE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static BULLDOZER_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

fn say_once(state: &mut State, line: String) {
    if state.said.len() < 64 && !state.said.contains(&line) {
        crate::log::line(&line);
        state.said.push(line);
    }
}

fn noted(key: &str) -> Option<i32> {
    crate::lua::noted(key)
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|&v| v >= 0)
}

fn read_usize(address: usize) -> Option<usize> {
    if address == 0 || !crate::image::readable(address, 8) {
        return None;
    }
    // SAFETY: eight readable bytes, checked just above.
    Some(unsafe { std::ptr::read_unaligned(address as *const usize) })
}

/// The one player of the `std::vector<Entity>` at `vector`, if it holds
/// exactly one (as the constructor and the menu's setter make it; an empty
/// list, which lets every owner through, is left alone).
fn one_player(vector: usize) -> Option<usize> {
    let begin = read_usize(vector)?;
    let end = read_usize(vector.checked_add(8)?)?;
    (end.checked_sub(begin) == Some(4)).then_some(begin)
}

/// The bulldozer filter's one player, if the filter has the shape expected.
fn filter_player(this: usize) -> Option<usize> {
    let filter = read_usize(this.checked_add(FILTER_FIELD.load(Ordering::Acquire))?)?;
    if read_usize(filter)? != FILTER_VTABLE.load(Ordering::Acquire) {
        return None;
    }
    one_player(filter + FILTER_PLAYERS)
}

/// The bulldozer's own owner list's one player.
fn list_player(this: usize) -> Option<usize> {
    let list = LIST_FIELD.load(Ordering::Acquire);
    if list == 0 {
        return None;
    }
    one_player(this.checked_add(list)?)
}

/// The addresses of `tool`'s player fields in the object `this`, with what
/// each is, if its shape is the one expected.
fn fields_of(tool: Tool, this: usize) -> Option<Vec<(usize, &'static str)>> {
    let offset = FIELDS[tool.index()].load(Ordering::Acquire);
    if offset == 0 {
        return None;
    }
    let own = this.checked_add(offset)?;
    if tool == Tool::Bulldozer {
        // Its own player always; each owner list while it holds one
        // player (empty, it lets every owner through).
        let mut fields = vec![(own, "its player")];
        if let Some(list) = list_player(this) {
            fields.push((list, "its owner list"));
        }
        if let Some(filter) = filter_player(this) {
            fields.push((filter, "its owner filter"));
        }
        return Some(fields);
    }
    Some(vec![(own, "its player")])
}

/// One field: brings it to what [`decide`] says.
fn field(state: &mut State, tool: Tool, this: usize, field: usize, what: &str) {
    if !crate::image::readable(field, 4) || !field.is_multiple_of(4) {
        return;
    }
    // SAFETY: four readable, aligned bytes of the tool's own object, read
    // on the main thread, which owns the tool.
    let current = unsafe { std::ptr::read_volatile(field as *const i32) };
    let at = state.slots.iter().position(|(f, _)| *f == field);
    let slot = at.map(|i| state.slots[i].1).unwrap_or_default();
    let save = noted("tpf3mp.player");
    let company = noted(COMPANY_NOTE);
    match decide(current, slot, save, company, crate::lua::in_room()) {
        Decision::Leave => {}
        Decision::Unknown => say_once(
            state,
            format!(
                "{FIX}: {} ({what}) holds player {current}, neither the save's player {} nor a company this wrote; left alone",
                tool.name(),
                save.map_or("(unknown)".into(), |s| s.to_string())
            ),
        ),
        Decision::Write(value) => {
            // SAFETY: the tool's own field, checked readable and aligned
            // above; the tool runs on this thread, and its pool tasks read
            // the whole aligned dword, old or new.
            unsafe { std::ptr::write_volatile(field as *mut i32, value) };
            let restoring = slot.original == Some(value);
            let next = Slot {
                original: slot.original.or(Some(current)),
                wrote: (!restoring).then_some(value),
            };
            match at {
                Some(i) => state.slots[i].1 = next,
                None => {
                    if state.slots.len() >= SLOTS {
                        state.slots.remove(0);
                    }
                    state.slots.push((field, next));
                }
            }
            if slot.wrote == Some(value) {
                // The game set the save's player back (the menu's step sets
                // the bulldozer's list): written again, said once per field.
                state.rewrites += 1;
                if state.rewrites == 1 || state.rewrites.is_power_of_two() {
                    crate::log::line(&format!(
                        "{FIX}: {} at {this:#x} ({what}) was set back to player {current}; the company {value} written again ({} time(s) so far, all tools)",
                        tool.name(),
                        state.rewrites
                    ));
                }
                return;
            }
            crate::log::line(&if restoring {
                format!(
                    "{FIX}: {} at {this:#x} ({what}) acts as the save's player {value} again",
                    tool.name()
                )
            } else {
                format!(
                    "{FIX}: {} at {this:#x} ({what}) acts as the player's company {value} (was player {current})",
                    tool.name()
                )
            });
        }
    }
}

/// One tool's frame, before the game's own.
fn frame(tool: Tool, this: usize) {
    if !ON.load(Ordering::Acquire) || BROKEN.load(Ordering::Acquire) || this == 0 {
        return;
    }
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(fields) = fields_of(tool, this) else {
        say_once(
            &mut state,
            format!(
                "{FIX}: {} at {this:#x} is not the shape expected; its player is left as the game made it",
                tool.name()
            ),
        );
        return;
    };
    for (address, what) in fields {
        field(&mut state, tool, this, address, what);
    }
}

/// A tool's frame, guarded: a panic switches this off for the game.
fn before(tool: Tool, this: usize) {
    if std::panic::catch_unwind(|| frame(tool, this)).is_err() {
        BROKEN.store(true, Ordering::Release);
    }
}

extern "system" fn before_street(this: usize) {
    before(Tool::Street, this);
    // The street and track tools continue from the room's segment, from
    // the tool's own frame ([`crate::toolchain`]).
    let _ = std::panic::catch_unwind(|| crate::toolchain::on_step(this));
}
extern "system" fn before_modifier(this: usize) {
    before(Tool::Modifier, this);
}
extern "system" fn before_construction(this: usize) {
    before(Tool::Construction, this);
}
extern "system" fn before_terminal(this: usize) {
    before(Tool::Terminal, this);
}
extern "system" fn before_module(this: usize) {
    before(Tool::Module, this);
}
extern "system" fn before_bulldozer(this: usize) {
    before(Tool::Bulldozer, this);
}

/// An entry for a tool's `Step`: saves the four argument registers and
/// xmm0-3, calls `$before` with `this`, restores them and jumps to the
/// original through `$original` with the stack as the caller left it.
macro_rules! entry {
    ($name:ident, $before:ident, $original:ident) => {
        #[cfg(all(windows, target_arch = "x86_64"))]
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                // Entry rsp is 8 mod 16; four pushes keep it so, and 0x68
                // makes it 16-aligned with 0x20 of shadow space under the
                // four saved xmm registers.
                "push rcx",
                "push rdx",
                "push r8",
                "push r9",
                "sub rsp, 0x68",
                "movaps [rsp + 0x20], xmm0",
                "movaps [rsp + 0x30], xmm1",
                "movaps [rsp + 0x40], xmm2",
                "movaps [rsp + 0x50], xmm3",
                "call {before}",
                "movaps xmm0, [rsp + 0x20]",
                "movaps xmm1, [rsp + 0x30]",
                "movaps xmm2, [rsp + 0x40]",
                "movaps xmm3, [rsp + 0x50]",
                "add rsp, 0x68",
                "pop r9",
                "pop r8",
                "pop rdx",
                "pop rcx",
                "jmp qword ptr [rip + {original}]",
                before = sym $before,
                original = sym $original,
            )
        }
    };
}

entry!(street_entry, before_street, STREET_ORIGINAL);
entry!(modifier_entry, before_modifier, MODIFIER_ORIGINAL);
entry!(
    construction_entry,
    before_construction,
    CONSTRUCTION_ORIGINAL
);
entry!(terminal_entry, before_terminal, TERMINAL_ORIGINAL);
entry!(module_entry, before_module, MODULE_ORIGINAL);
entry!(bulldozer_entry, before_bulldozer, BULLDOZER_ORIGINAL);

/// `len` bytes of the game's code at `address`, if readable.
fn code(address: u64, len: usize) -> Option<&'static [u8]> {
    let address = usize::try_from(address).ok()?;
    if !crate::image::readable(address, len) {
        return None;
    }
    // SAFETY: `len` readable bytes of the game's mapped code, only read.
    Some(unsafe { std::slice::from_raw_parts(address as *const u8, len) })
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn detour(
    target: u64,
    entry: unsafe extern "C" fn(),
    original: &AtomicUsize,
) -> Result<(), String> {
    // SAFETY: a function the profile resolved and prologue-checked in this
    // build, detoured while the game starts, before any tool exists; the
    // entry restores every argument register before it jumps on.
    let installed = unsafe {
        tpf3mp_hookcore::detour::InlineDetour::install(
            target as usize as *mut u8,
            entry as *const u8,
        )
    };
    match installed {
        Ok(detoured) => {
            original.store(detoured.trampoline() as usize, Ordering::Release);
            let _kept = std::mem::ManuallyDrop::new(detoured);
            Ok(())
        }
        Err(error) => Err(format!("{error:?}")),
    }
}

/// Installs the tools' frames unless [`ENV`] says no; the lines for
/// hook.log.
pub fn install(resolved: &ResolvedProfile) -> Vec<String> {
    install_with(
        resolved,
        crate::ticks::wanted(std::env::var(ENV).ok().as_deref()),
    )
}

pub fn install_with(resolved: &ResolvedProfile, wanted: bool) -> Vec<String> {
    ON.store(false, Ordering::Release);
    if !wanted {
        return vec![format!(
            "{FIX}: off, {ENV} says so; the native tools act as the save's player"
        )];
    }
    install_tools(resolved)
}

/// Reads `tool`'s field offsets from its constructor's code (`shape`, and
/// the filter's target for the bulldozer); false when the code is not the
/// code expected.
#[cfg(all(windows, target_arch = "x86_64"))]
fn read_layout(resolved: &ResolvedProfile, tool: Tool, shape: u64) -> bool {
    if let Some((bytes, at)) = tool.store() {
        let Some(offset) = code(shape, bytes.len()).and_then(|c| store_offset(c, bytes, at)) else {
            return false;
        };
        FIELDS[tool.index()].store(offset, Ordering::Release);
        return true;
    }
    let player = code(shape, BULLDOZER_STORE_BYTES.len()).and_then(bulldozer_player_offset);
    let filter = resolved
        .get(BULLDOZER_FILTER)
        .and_then(|f| code(f.address, 48).and_then(|c| filter_layout(c, f.address as usize)));
    let list = resolved.get(BULLDOZER_LIST).and_then(|l| {
        code(l.address, BULLDOZER_LIST_BYTES.len())
            .and_then(|c| store_offset(c, &BULLDOZER_LIST_BYTES, BULLDOZER_LIST_DISP_AT))
    });
    match (player, filter, list) {
        (Some(player), Some(filter), Some(list)) => {
            FIELDS[tool.index()].store(player, Ordering::Release);
            FILTER_FIELD.store(filter.filter, Ordering::Release);
            FILTER_VTABLE.store(filter.vtable, Ordering::Release);
            LIST_FIELD.store(list, Ordering::Release);
            true
        }
        _ => false,
    }
}

/// The bulldozer's list setter's detour: the game's own assignment, then
/// the bulldozer's fields brought to the company at once, so the list the
/// menu's step sets never stands until the tool's next frame.
extern "C" fn set_owner_list(this: usize, list: usize) {
    let original = SETTER_ORIGINAL.load(Ordering::Acquire);
    // SAFETY: the trampoline of the setter, stored before the detour could
    // be reached; called with the arguments the game passed.
    let original: extern "C" fn(usize, usize) =
        unsafe { std::mem::transmute::<usize, extern "C" fn(usize, usize)>(original) };
    original(this, list);
    before(Tool::Bulldozer, this);
}

/// Detours the bulldozer's list setter, once its offset matches the list's.
#[cfg(all(windows, target_arch = "x86_64"))]
fn install_setter(resolved: &ResolvedProfile) -> String {
    let Some(setter) = resolved.get(BULLDOZER_SETTER) else {
        return format!(
            "{FIX}: the bulldozer's owner list is set back each time the menu sets it: the profile lacks {BULLDOZER_SETTER}"
        );
    };
    let offset = code(setter.address, BULLDOZER_SETTER_BYTES.len())
        .and_then(|c| store_offset(c, &BULLDOZER_SETTER_BYTES, BULLDOZER_SETTER_DISP_AT));
    if offset.is_none() || offset != Some(LIST_FIELD.load(Ordering::Acquire)) {
        return format!(
            "{FIX}: the bulldozer's owner list setter at {:#x} is not the code expected; left alone",
            setter.address
        );
    }
    // SAFETY: as [`detour`]; the detour takes the setter's two arguments
    // and calls it with them.
    let installed = unsafe {
        tpf3mp_hookcore::detour::InlineDetour::install(
            setter.address as usize as *mut u8,
            set_owner_list as *const u8,
        )
    };
    match installed {
        Ok(detoured) => {
            SETTER_ORIGINAL.store(detoured.trampoline() as usize, Ordering::Release);
            let _kept = std::mem::ManuallyDrop::new(detoured);
            format!(
                "{FIX}: the bulldozer's owner list is set to the company again each time the menu sets it ({BULLDOZER_SETTER} at {:#x})",
                setter.address
            )
        }
        Err(error) => format!("{FIX}: detouring {BULLDOZER_SETTER} failed: {error:?}"),
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_tools(resolved: &ResolvedProfile) -> Vec<String> {
    let mut lines = Vec::new();
    let mut any = false;
    for tool in Tool::ALL {
        let (step, shape) = tool.targets();
        let (entry, original): (unsafe extern "C" fn(), &AtomicUsize) = match tool {
            Tool::Street => (street_entry, &STREET_ORIGINAL),
            Tool::Modifier => (modifier_entry, &MODIFIER_ORIGINAL),
            Tool::Construction => (construction_entry, &CONSTRUCTION_ORIGINAL),
            Tool::Terminal => (terminal_entry, &TERMINAL_ORIGINAL),
            Tool::Module => (module_entry, &MODULE_ORIGINAL),
            Tool::Bulldozer => (bulldozer_entry, &BULLDOZER_ORIGINAL),
        };
        let (Some(step_at), Some(shape_at)) = (resolved.get(step), resolved.get(shape)) else {
            lines.push(format!(
                "{FIX}: {} stays the save's player's: the profile lacks {step} or {shape}",
                tool.name()
            ));
            continue;
        };
        if !read_layout(resolved, tool, shape_at.address) {
            lines.push(format!(
                "{FIX}: {} stays the save's player's: {shape} is not the code expected",
                tool.name()
            ));
            continue;
        }
        match detour(step_at.address, entry, original) {
            Ok(()) => {
                any = true;
                if tool == Tool::Street {
                    // Its frame also lets the tools continue from the
                    // room's segment ([`crate::toolchain`]).
                    crate::toolchain::set_step_hooked(true);
                }
                if tool == Tool::Bulldozer {
                    lines.push(install_setter(resolved));
                }
                lines.push(format!(
                    "{FIX}: {} acts as the player's company in a room ({step} at {:#x}; {ENV}=0 turns it off)",
                    tool.name(),
                    step_at.address
                ));
            }
            Err(error) => lines.push(format!(
                "{FIX}: {} stays the save's player's: detouring {step} failed: {error}",
                tool.name()
            )),
        }
    }
    ON.store(any, Ordering::Release);
    lines
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_tools(_resolved: &ResolvedProfile) -> Vec<String> {
    vec![format!("{FIX}: off, Windows x86-64 only")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stores_give_their_fields() {
        assert_eq!(
            store_offset(&STREET_STORE_BYTES, &STREET_STORE_BYTES, STREET_DISP_AT),
            Some(0xc0)
        );
        assert_eq!(
            store_offset(
                &MODIFIER_STORE_BYTES,
                &MODIFIER_STORE_BYTES,
                MODIFIER_DISP_AT
            ),
            Some(0xa0)
        );
        let mut other = STREET_STORE_BYTES;
        other[30] = 0x30;
        assert_eq!(
            store_offset(&other, &STREET_STORE_BYTES, STREET_DISP_AT),
            None,
            "another source: refused"
        );
        assert_eq!(
            store_offset(
                &STREET_STORE_BYTES[..20],
                &STREET_STORE_BYTES,
                STREET_DISP_AT
            ),
            None
        );
    }

    #[test]
    fn the_station_tools_stores_give_their_fields() {
        for (bytes, at, want) in [
            (&CONSTRUCTION_STORE_BYTES[..], CONSTRUCTION_DISP_AT, 0xa0),
            (&TERMINAL_STORE_BYTES[..], TERMINAL_DISP_AT, 0xa0),
            (&MODULE_STORE_BYTES[..], MODULE_DISP_AT, 0xa8),
        ] {
            assert_eq!(store_offset(bytes, bytes, at), Some(want));
            let mut other = bytes.to_vec();
            other[at - 6] ^= 0x10;
            assert_eq!(store_offset(&other, bytes, at), None, "another source");
        }
        for tool in Tool::ALL {
            let (step, shape) = tool.targets();
            assert!(!step.is_empty() && !shape.is_empty());
            assert_eq!(tool.store().is_none(), tool == Tool::Bulldozer);
        }
    }

    #[test]
    fn the_bulldozers_own_player_is_read_from_its_store() {
        // Build 40408 at rva 0x4c4a41: lea rax,[rip+0x31ea3f0]; ...; mov
        // [r14+0x28],ebx.
        let mut bytes = BULLDOZER_STORE_BYTES;
        bytes[3..7].copy_from_slice(&0x031e_a3f0_i32.to_le_bytes());
        assert_eq!(bulldozer_player_offset(&bytes), Some(0x28));
        bytes[15] = 0x88;
        assert_eq!(
            bulldozer_player_offset(&bytes),
            None,
            "not a store of ebx: refused"
        );
        assert_eq!(bulldozer_player_offset(&BULLDOZER_STORE_BYTES[..10]), None);
    }

    #[test]
    fn the_bulldozers_owner_list_and_its_setter_agree() {
        assert_eq!(
            store_offset(
                &BULLDOZER_LIST_BYTES,
                &BULLDOZER_LIST_BYTES,
                BULLDOZER_LIST_DISP_AT
            ),
            Some(0xa8)
        );
        assert_eq!(
            store_offset(
                &BULLDOZER_SETTER_BYTES,
                &BULLDOZER_SETTER_BYTES,
                BULLDOZER_SETTER_DISP_AT
            ),
            Some(0xa8),
            "the setter writes the list the constructor made"
        );
        let mut other = BULLDOZER_SETTER_BYTES;
        other[12] = 0x9A;
        assert_eq!(
            store_offset(&other, &BULLDOZER_SETTER_BYTES, BULLDOZER_SETTER_DISP_AT),
            None,
            "another register: refused"
        );
    }

    #[test]
    fn the_filter_gives_its_vtable_and_place() {
        // Build 40408: lea rax,[rip+0x31ea337] at rva 0x4c4c32.
        let mut bytes: Vec<u8> = FILTER_BYTES.iter().map(|b| b.unwrap_or(0)).collect();
        bytes[3..7].copy_from_slice(&0x031e_a337_i32.to_le_bytes());
        assert_eq!(
            filter_layout(&bytes, 0x1_404c_4c32),
            Some(FilterLayout {
                vtable: 0x1_436a_ef70,
                filter: 0xc0
            })
        );
        bytes[17] = 0x18;
        assert_eq!(
            filter_layout(&bytes, 0x1_404c_4c32),
            None,
            "players elsewhere"
        );
    }

    #[test]
    fn a_tool_acts_as_the_company_only_in_a_room_and_goes_back() {
        let save = Some(214_443);
        let company = Some(372_363);
        let fresh = Slot::default();
        // Outside a room, or without a company: the game's own.
        assert_eq!(
            decide(214_443, fresh, save, company, false),
            Decision::Leave
        );
        assert_eq!(decide(214_443, fresh, save, None, true), Decision::Leave);
        assert_eq!(decide(214_443, fresh, None, company, true), Decision::Leave);
        // The room's first company is the save's player.
        assert_eq!(decide(214_443, fresh, save, save, true), Decision::Leave);
        // In a room, for another company: written.
        assert_eq!(
            decide(214_443, fresh, save, company, true),
            Decision::Write(372_363)
        );
        let written = Slot {
            original: Some(214_443),
            wrote: Some(372_363),
        };
        assert_eq!(
            decide(372_363, written, save, company, true),
            Decision::Leave
        );
        // The player switches company: the new one.
        assert_eq!(
            decide(372_363, written, save, Some(380_000), true),
            Decision::Write(380_000)
        );
        // Leaving the room: back to the save's player.
        assert_eq!(
            decide(372_363, written, save, company, false),
            Decision::Write(214_443)
        );
        assert_eq!(
            decide(372_363, written, save, None, true),
            Decision::Write(214_443)
        );
        // A value this does not know is left alone.
        assert_eq!(decide(5, fresh, save, company, true), Decision::Unknown);
        assert_eq!(decide(5, written, save, company, false), Decision::Leave);
    }

    #[test]
    fn off_unless_wanted() {
        let empty = ResolvedProfile {
            name: String::new(),
            targets: Vec::new(),
            absent_optional: Vec::new(),
        };
        assert!(install_with(&empty, false)[0].contains("off"));
        let lines = install_with(&empty, true);
        assert!(
            lines
                .iter()
                .all(|l| l.contains("stays the save's player's") || l.contains("Windows"))
        );
        frame(Tool::Street, 0x1000);
    }
}
