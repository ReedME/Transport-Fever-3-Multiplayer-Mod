//! Installs the step gate in the running game: finds `GameSim::Step` with
//! the matched profile in the game's own mapped image, attaches the session
//! to the agent, and detours the step to [`crate::step::StepDriver`]. It
//! also detours Lua's `print`, which gives each of the game's Lua states the
//! mod's link to the hook ([`crate::lua`]), and, where the profile has them,
//! the main menu's frame and Lua registration, so a game at its main menu
//! can load the room's world ([`crate::menu`]).
//!
//! Windows only for now: the one profile so far is Steam build 40408 on
//! Windows, and on other systems the hook installs nothing (fail closed).

#![allow(unsafe_code)]
// Elsewhere install_inner installs nothing, so the detours are unused there.
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::{
    ffi::c_int,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

use tpf3mp_hookcore::profile::Profile;

use crate::{
    at_menu::Seen,
    lua,
    step::{StepHandler, Updates},
};

/// The profile's name for the simulation step.
pub const STEP_TARGET: &str = "GameSim::Step";
/// The profile's name for the speed the step reads.
pub const SPEED_TARGET: &str = "CGameTime::GetSpeed";
/// The profile's name for the step's own call of the speed getter.
pub const SPEED_CALL_TARGET: &str = "GameSim::Step/GetSpeed call";
/// `CGame::Sync`, which the batch interval's wrapper calls
/// ([`crate::interval`]).
pub const SYNC_TARGET: &str = "CGame::Sync";
/// `CGame::Step`'s call of it, which the wrapper takes over.
pub const SYNC_CALL_TARGET: &str = "CGame::Step/Sync call";

/// `CGame::Sync`'s address, for [`sync_wrap`].
static SYNC: AtomicUsize = AtomicUsize::new(0);

/// `bool CGame::Sync(CGame*, std::function<...>&)`: the bool in `al`.
/// `C-unwind`: a C++ exception Sync throws passes through the wrapper to
/// the game's own handlers, as it would without it.
type SyncFn = unsafe extern "C-unwind" fn(usize, usize) -> u8;

/// Where `CGame::Step`'s call of `CGame::Sync` goes: the real Sync, then,
/// after a successful one, the batch interval for the batch it exposed
/// (crate::interval). The main thread's; it never waits for the step
/// driver's lock.
unsafe extern "C-unwind" fn sync_wrap(cgame: usize, function: usize) -> u8 {
    let sync = SYNC.load(Ordering::Acquire);
    // SAFETY: SYNC holds CGame::Sync's address, which the profile resolved
    // and the redirect checked the call targets; the arguments are the
    // ones CGame::Step passed.
    let sync: SyncFn = unsafe { std::mem::transmute::<usize, SyncFn>(sync) };
    let ok = unsafe { sync(cgame, function) };
    let _ = std::panic::catch_unwind(|| crate::interval::after_sync(cgame, ok != 0));
    ok
}
/// The profile's name for Lua's `print`.
pub const PRINT_TARGET: &str = "luaB_print";
/// The command queue's add and the simulation's apply of a build: without
/// them the tools stay refused in the room's game (crate::builds).
pub const ADD_TARGET: &str = "CommandList::Add";
pub const BUILD_APPLY_TARGET: &str = "WorldBuildProposal apply";

/// Lua's `print`, reached through its detour's trampoline.
static PRINT_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// Lua's `print`, in any of the game's Lua states: prints as the game's
/// does, then gives the state `tpf3mp_native` (the mod prints before it
/// looks for the table). `C-unwind`: a Lua error in `print` itself passes
/// through as the game raised it.
unsafe extern "C-unwind" fn print_detour(l: lua::State) -> c_int {
    let original = PRINT_ORIGINAL.load(Ordering::Acquire);
    let printed = if original == 0 {
        0
    } else {
        // SAFETY: the trampoline of Lua's print, a C function of the game's
        // Lua, called with the state the game called it with.
        unsafe { std::mem::transmute::<usize, lua::CFunction>(original)(l) }
    };
    if let Some(api) = lua::api() {
        // SAFETY: the state `print` was called in, on its own thread, inside
        // that call.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            lua::register(api, l);
        }));
    }
    printed
}

/// The game's own step, reached through the detour's trampoline.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The driver the detour hands each call to.
static DRIVER: Mutex<Option<Box<dyn StepHandler>>> = Mutex::new(None);
/// Set when the detour itself failed (a panic): from then on it runs
/// nothing, holding the world, as the driver does on an error.
static BROKEN: AtomicBool = AtomicBool::new(false);
/// Where the driver's log lines go.
static LOG: Mutex<Option<crate::Logger>> = Mutex::new(None);
/// The game's own speed getter, reached through its detour's trampoline.
static SPEED_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// In the room's game: the speed row's value is recorded.
static IN_ROOM: AtomicBool = AtomicBool::new(false);
/// While the game's step runs, the updates it must run (`OWN_SPEED` for the
/// game's own speed): what the step's call of the speed getter answers.
static UPDATES: AtomicU64 = AtomicU64::new(OWN_SPEED);
const OWN_SPEED: u64 = u64::MAX;
/// The game's own speed (the speed row) as the getter last read it in the
/// room's game; `NO_SPEED` until then.
static CHOSEN: AtomicU64 = AtomicU64::new(NO_SPEED);
const NO_SPEED: u64 = u64::MAX;
/// While the game's step runs: the `CGameTime` its speed call was made on,
/// which the checkpoint line reads the counters through ([`crate::ticks`]);
/// 0 otherwise.
static GAME_TIME: AtomicUsize = AtomicUsize::new(0);
/// The room's step the last batch ended at: a batch that does not start
/// right after it (a world loaded) logs its counters too.
static LAST_STEP_RUN: AtomicU64 = AtomicU64::new(u64::MAX);
/// The time of the game's own step inside the step detour's call running
/// now (`crate::perf`): the detour's time less this is the gate's.
static STEP_GAME_NANOS: AtomicU64 = AtomicU64::new(0);
/// When the game's step last ran, in milliseconds since [`EPOCH`]; 0 never.
static LAST_STEP: AtomicU64 = AtomicU64::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    let epoch = EPOCH.get_or_init(Instant::now);
    u64::try_from(epoch.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

/// The room's step the last batch ended at, if one ran
/// ([`crate::edgewatch`]).
pub(crate) fn last_step_run() -> Option<u64> {
    let last = LAST_STEP_RUN.load(Ordering::Acquire);
    (last != u64::MAX).then_some(last)
}

/// The game's `updateCount` while its step runs, else `None`
/// ([`crate::edgewatch`]).
pub(crate) fn update_count_now() -> Option<u32> {
    crate::ticks::read_counters(GAME_TIME.load(Ordering::Acquire))
        .ok()
        .map(|c| c.update_count)
}

/// The `CGameTime` the game's step running now called its speed getter on,
/// 0 outside the step ([`crate::netread`]).
pub(crate) fn game_time_now() -> usize {
    GAME_TIME.load(Ordering::Acquire)
}

/// The thread running the game's step now, if it runs.
static STEP_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

/// Whether this thread is the one running the game's step, inside it: the
/// only place the engine may be read natively ([`crate::netread`]), as
/// nothing changes it beside the step's own work on its own thread then.
/// The mod's game script's `postUpdate` runs there (build 40408: its
/// `update` runs on the game's pool of threads).
pub(crate) fn on_step_thread() -> bool {
    *STEP_THREAD.lock().unwrap_or_else(PoisonError::into_inner) == Some(std::thread::current().id())
}

/// Writes `line` to the hook's log, if it has one.
pub(crate) fn log_line(line: &str) {
    if let Some(log) = LOG.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        log.line(line);
    }
}

/// Writes a bounded incident batch with one file write.
pub(crate) fn log_lines(lines: &[String]) {
    if let Some(log) = LOG.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
        log.lines(lines);
    }
}

/// Where the main menu's frames last saw the game (`crate::at_menu`), and
/// the rule that decides it.
struct MenuSight {
    gate: crate::at_menu::MenuGate,
    /// What the frames saw last, and what was logged last.
    seen: Option<Seen>,
    logged: Option<Seen>,
}

static MENU_SIGHT: Mutex<MenuSight> = Mutex::new(MenuSight {
    gate: crate::at_menu::MenuGate::new(),
    seen: None,
    logged: None,
});

fn menu_sight() -> std::sync::MutexGuard<'static, MenuSight> {
    MENU_SIGHT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Added to the menu's clock by the tests, which cannot wait out the quiet
/// stretch.
#[cfg(test)]
static MENU_CLOCK_SKEW: AtomicU64 = AtomicU64::new(0);

fn menu_now_ms() -> u64 {
    #[cfg(test)]
    return now_ms().saturating_add(MENU_CLOCK_SKEW.load(Ordering::Acquire));
    #[cfg(not(test))]
    now_ms()
}

/// Where the game is at this menu frame (`crate::at_menu`), and the lines
/// that say where it went. `menu` is the `UI::CMenuUI` whose frame this is.
fn menu_seen(menu: usize) -> (Seen, Vec<String>) {
    // SAFETY: DoStep's `this`, live, on its thread, after its frame.
    let world_loaded = unsafe { crate::menu::world_loaded(menu) };
    let last_step_ms = LAST_STEP.load(Ordering::Acquire);
    let world_gui_started = lua::any_world_started();
    // Read the loader's future on its own thread, without Lua or its locks.
    // SAFETY: DoStep's live receiver, on its thread.
    let loading = unsafe { crate::menu::load_in_progress(menu) };
    let frame = crate::at_menu::Frame {
        now_ms: menu_now_ms(),
        last_step_ms,
        world_gui_started,
        world_loaded,
        loading,
    };
    let mut sight = menu_sight();
    let seen = sight.gate.frame(&frame);
    let before = sight.seen.replace(seen);
    let mut lines = Vec::new();
    if before == Some(Seen::WorldUp) && seen != Seen::WorldUp {
        let forgotten = crate::menu::forget_world_states();
        lines.push(format!(
            "menu: the world closed (CMenuUI::m_game cleared); the {forgotten} Lua state(s) its GUI was given are never used by the main menu"
        ));
        // Closed by the player, not by a load of the room's the hook
        // started: in a room's game, any world up after is none the room
        // loaded, and the step gate holds it (crate::step, WorldMark).
        if !lua::load_started() {
            crate::menu::note_world_closed();
        }
        // Its company's entity is no entity of the next world: forgotten
        // before the views' refresh below, so no GUI hands it on.
        let notes = lua::forget_world_notes();
        if notes > 0 {
            lines.push(format!(
                "menu: the closed world's company note(s) forgotten ({notes}): the next world's views follow its own room's roster"
            ));
        }
    }
    crate::guiplayer::refresh();
    lines.extend(crate::probe::frame(menu, frame.now_ms));
    // Once per change; a load's task coming and going is one wait.
    let waiting = |seen: Option<Seen>| matches!(seen, Some(Seen::Loading | Seen::Closing));
    if sight.logged != Some(seen) && !(waiting(sight.logged) && waiting(Some(seen))) {
        sight.logged = Some(seen);
        if seen != Seen::Fresh {
            lines.push(format!("menu: {}", seen.describe()));
        }
    }
    (seen, lines)
}

/// One of the main menu's frames (`crate::menu`), after the game's own,
/// `menu` the `UI::CMenuUI`: while the game is at its main menu with no
/// world (`crate::at_menu`) and has a menu Lua state to load from on this
/// thread, the driver follows the room from here
/// ([`StepHandler::on_menu`]), and a load of the room's save it asks for is
/// started in the menu. Otherwise nothing: the step's detour drives the room
/// while a world is up.
pub(crate) fn menu_frame(menu: usize) {
    // The GUI thread and its menu, for the others' previews drawn.
    crate::drawing::note_menu(menu);
    if BROKEN.load(Ordering::Acquire) {
        return;
    }
    // Never while a world is loaded, stepping or not: a world that stops
    // stepping (saving the room's world, held while another player loads)
    // is not at the menu, and taking the room's session from its menu
    // frames there hung the owner's game (measured, 2026-09-30); a world
    // loaded before its first step neither (the owner's save for the room
    // comes then). After a world, only once it closed and nothing loads.
    let (seen, mut lines) = menu_seen(menu);
    // Fresh menus also run during loads. Unknown fields fail closed; a
    // hook-started load remains busy until its GUI arrives or it fails.
    if !crate::menu::outermost_frame() || lua::load_started()
        // SAFETY: the game's live menu, on its thread.
        || unsafe { crate::menu::load_in_progress(menu) } != Some(false)
    {
        for line in lines {
            log_line(&line);
        }
        return;
    }
    // Room control (including Leave) must progress even if the menu's
    // Lua registration was missed. Only serving a load requires that state;
    // `serve` reports its absence instead of freezing the room's message queue.
    if !seen.allows() {
        for line in lines {
            log_line(&line);
        }
        return;
    }
    // Never waits: were the step's detour to hold the driver on this
    // thread, the menu skips a frame.
    let mut guard = match DRIVER.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            for line in lines {
                log_line(&line);
            }
            return;
        }
    };
    let Some(driver) = guard.as_mut() else {
        drop(guard);
        for line in lines {
            log_line(&line);
        }
        return;
    };
    let was_in_room = driver.in_room();
    driver.on_menu();
    let in_room = driver.in_room();
    IN_ROOM.store(in_room, Ordering::Release);
    lua::set_in_room(in_room);
    lines.extend(driver.take_log());
    drop(guard);
    if in_room && !was_in_room {
        lines.push(format!(
            "the room began at the main menu; the menu sees: {}",
            seen.describe()
        ));
    }
    let room_load = lua::take_menu_load();
    lines.extend(auto_load_frame(room_load.is_some() || in_room));
    if let Some(name) = room_load {
        // SAFETY: the menu's frame, on the thread that runs its Lua, after
        // the game's own frame: no Lua runs on it now.
        match unsafe { crate::menu::serve(&name) } {
            Some(crate::menu::Served::Started) => {
                lua::menu_load_started();
                lines.push(format!(
                    "the main menu is loading the room's world ({name}); the game starts it by itself"
                ));
            }
            Some(crate::menu::Served::Busy) => lua::menu_load_later(&name),
            Some(crate::menu::Served::Failed(why)) => {
                lines.push(format!(
                    "the main menu could not load the room's world: {why}"
                ));
                lua::menu_load_failed(why);
            }
            None => lua::menu_load_failed("no Lua state of the menu's on this thread".into()),
        }
    }
    lines.extend(lua::take_log());
    for line in lines {
        log_line(&line);
    }
}

/// The launcher's `--auto-load` save, loaded from the menu's frames.
static AUTO_LOAD: Mutex<Option<crate::autoload::AutoLoad>> = Mutex::new(None);

/// One of the menu's frames for `--auto-load` (`crate::autoload`): loads
/// the save the launcher named, unless the room's own world comes instead.
/// Returns the lines to log.
fn auto_load_frame(room_world: bool) -> Option<String> {
    let mut slot = AUTO_LOAD.lock().unwrap_or_else(PoisonError::into_inner);
    let auto = slot.get_or_insert_with(crate::autoload::AutoLoad::from_env);
    if room_world {
        return auto.cancel();
    }
    let now = Instant::now();
    let save = auto.due(now)?.to_owned();
    // The lock is let go while the menu's Lua runs.
    drop(slot);
    // SAFETY: the menu's frame, on the thread that runs its Lua, after the
    // game's own frame: no Lua runs on it now.
    let served = unsafe { crate::menu::serve(&save) };
    AUTO_LOAD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
        .and_then(|auto| auto.answered(served, now))
}

/// The speed getter's signature, passed through as the step's is.
type SpeedFn = unsafe extern "C" fn(usize, usize, usize, usize) -> u64;

/// The speed getter's detour: it only reads. In the room's game the speed
/// row's value is the player's request to the room (the step detour passes
/// it on). Every caller gets the game's own answer: the UI, the camera and
/// the particles read it too, and the game asserts when they are told a
/// speed it is not running at.
unsafe extern "C" fn speed_detour(this: usize, a: usize, b: usize, c: usize) -> u64 {
    let original = SPEED_ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return 1;
    }
    // SAFETY: the trampoline InlineDetour::install returned for the getter,
    // which keeps the game's own ABI.
    let original: SpeedFn = unsafe { std::mem::transmute::<usize, SpeedFn>(original) };
    // SAFETY: the game's own getter, called as the game called it.
    let own = unsafe { original(this, a, b, c) };
    if IN_ROOM.load(Ordering::Acquire) {
        // The getter returns an int: its low 32 bits.
        CHOSEN.store(u64::from(own as u32), Ordering::Release);
    }
    own
}

/// Where the step's own call of the speed getter goes: the number of
/// updates the step driver chose for this call of the step. In the room's
/// game the room sets the pace (the step gate releases steps at its speed),
/// whatever the speed row or a key says, paused included: the room's pause
/// is the only pause. Otherwise the game's own answer.
unsafe extern "C" fn step_speed(this: usize, a: usize, b: usize, c: usize) -> u64 {
    GAME_TIME.store(this, Ordering::Release);
    // SAFETY: the getter's detour, called with the arguments the step passed.
    let own = unsafe { speed_detour(this, a, b, c) };
    match UPDATES.load(Ordering::Acquire) {
        OWN_SPEED => own,
        updates => updates,
    }
}

/// The step's signature: a member function, `this` and its arguments in
/// the first registers. All four are passed on unchanged, so the detour is
/// transparent whatever the game's step takes in them.
type StepFn = unsafe extern "C" fn(usize, usize, usize, usize);

/// Runs the game's own step once, with `updates` answered to its call of
/// the speed getter; `room` says the call is the room's game's, so its
/// paused path leaves the game's tickCount alone ([`crate::ticks`]).
///
/// # Safety
///
/// `original` is the step's trampoline; the arguments are the game's.
unsafe fn run_step(
    original: StepFn,
    updates: Updates,
    room: bool,
    this: usize,
    a: usize,
    b: usize,
    c: usize,
) {
    let answer = match updates {
        Updates::Own => OWN_SPEED,
        Updates::Exactly(updates) => u64::from(updates),
    };
    UPDATES.store(answer, Ordering::Release);
    crate::ticks::set_room(room);
    // Whatever the engine freed since the last step, no cached region
    // answers for it.
    crate::image::invalidate();
    let perf = crate::perf::start();
    let started = crate::steptrace::step_timer(perf, crate::steptrace::enabled());
    // The free-id trace learns which engine this game simulates.
    crate::persons::freed_ids::trace::note_step(this as u64, room);
    crate::order::set_in_step(true);
    *STEP_THREAD.lock().unwrap_or_else(PoisonError::into_inner) = Some(std::thread::current().id());
    // SAFETY: the caller's.
    unsafe { original(this, a, b, c) };
    *STEP_THREAD.lock().unwrap_or_else(PoisonError::into_inner) = None;
    crate::order::set_in_step(false);
    if let Some(started) = started {
        let nanos = crate::perf::nanos_since(started);
        if perf.is_some() {
            crate::perf::game_step(nanos);
        }
        STEP_GAME_NANOS.fetch_add(nanos, Ordering::Relaxed);
    }
    crate::ticks::set_room(false);
    UPDATES.store(OWN_SPEED, Ordering::Release);
}

/// Closes the per-call timing window after the detour has done its work.
fn finish_perf(started: Option<Instant>) {
    if let Some(started) = started {
        let total = crate::perf::nanos_since(started);
        let game = STEP_GAME_NANOS.swap(0, Ordering::Relaxed);
        crate::perf::add(crate::perf::Piece::Gate, total.saturating_sub(game));
        // Once a window: the timing's established pair and the step line.
        if let Some(lines) = crate::perf::tick(Instant::now()) {
            for line in lines {
                log_line(&line);
            }
        }
    }
}

/// After a batch of the room's steps `first..first + updates`: at a
/// checkpoint, and at the first batch after a world was loaded, the game's
/// two counters go to the log with the room's step, for two games' logs to
/// be compared ([`crate::ticks::checkpoint_line`]).
fn log_counters(first: u64, updates: u32, checkpoint: bool) {
    let last = first.saturating_add(u64::from(updates)).saturating_sub(1);
    let before = LAST_STEP_RUN.swap(last, Ordering::AcqRel);
    if checkpoint || before.saturating_add(1) != first {
        let counters = crate::ticks::read_counters(GAME_TIME.load(Ordering::Acquire));
        log_line(&crate::ticks::checkpoint_line(last, counters));
        // The free-id queue's fingerprint (docs/HOOKS.md, "The free-id
        // trace").
        if let Some(line) = crate::persons::freed_ids::trace::checkpoint_line(last) {
            log_line(&line);
        }
        // The road entry trace's digest of the in-step appends since the
        // last checkpoint (docs/HOOKS.md, "The road entry trace").
        if let Some(line) = crate::roadtrace::take_checkpoint(last) {
            log_line(&line);
        }
    }
}

/// The detour: every call of the game's step comes here, and runs the
/// game's step exactly once (a call skipped turns the game's clock back).
unsafe extern "C" fn step_detour(this: usize, a: usize, b: usize, c: usize) {
    let original = ORIGINAL.load(Ordering::Acquire);
    if original == 0 {
        return;
    }
    // SAFETY: ORIGINAL holds the trampoline InlineDetour::install returned
    // for this function, which keeps the game's own ABI.
    let original: StepFn = unsafe { std::mem::transmute::<usize, StepFn>(original) };
    let started = crate::perf::start();
    let traced_at = crate::steptrace::enabled().then(Instant::now);
    STEP_GAME_NANOS.store(0, Ordering::Relaxed);
    LAST_STEP.store(now_ms(), Ordering::Release);
    // The step's speed call sets it again for this call.
    GAME_TIME.store(0, Ordering::Release);
    if BROKEN.load(Ordering::Acquire) {
        // SAFETY: the game's step on its paused path: the world stands still.
        unsafe { run_step(original, Updates::Exactly(0), false, this, a, b, c) };
        crate::perf::step_call(false, Some(0), started);
        finish_perf(started);
        return;
    }
    let mut ran = false;
    let mut selected: Option<(Updates, bool)> = None;
    // What this call answered, and why, for the step trace.
    let mut answered: Option<(Updates, bool)> = None;
    let mut why: &'static str = "own";
    let mut note: Option<String> = None;
    let mut info = crate::interval::BatchInfo::default();
    let seen_before = crate::seeds::updates_seen();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut driver = DRIVER.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(driver) = driver.as_mut() else {
            ran = true;
            answered = Some((Updates::Own, false));
            selected = Some((Updates::Own, false));
            // SAFETY: the game's own step, called as the game called it.
            unsafe { run_step(original, Updates::Own, false, this, a, b, c) };
            return;
        };
        // SAFETY: as above, once per call, with the updates the driver chose;
        // the batch's first update hands the mod the room's actions for it.
        driver.on_step(lua::take_commands(), &mut |batch| {
            ran = true;
            answered = Some((batch.updates, batch.lanes));
            selected = Some((batch.updates, batch.room));
            let updates = match batch.updates {
                Updates::Exactly(updates) => updates,
                Updates::Own => 0,
            };
            match lua::begin_batch(&[], updates, batch.lanes, batch.dump) {
                Ok(()) => {
                    unsafe { run_step(original, batch.updates, batch.room, this, a, b, c) };
                    if let Some(first) = batch.first_step {
                        log_counters(first, updates, batch.lanes);
                    }
                    lua::end_batch()
                }
                Err(reason) => {
                    selected = Some((Updates::Exactly(0), batch.room));
                    unsafe { run_step(original, Updates::Exactly(0), batch.room, this, a, b, c) };
                    Err(reason)
                }
            }
        });
        why = driver.why();
        if traced_at.is_some() {
            note = driver.cadence_note();
        }
        info = driver.batch_info();
        for (ticket, why) in driver.take_refused() {
            lua::refused(ticket, &why);
        }
        for text in lua::take_said() {
            driver.say(text);
        }
        if let Some(preview) = crate::previews::take_out(Instant::now()) {
            driver.preview(preview);
        }
        // The main menu's Multiplayer window, whose lobby the step just read.
        crate::lobby::exchange(driver.as_mut());
        IN_ROOM.store(driver.in_room(), Ordering::Release);
        lua::set_in_room(driver.in_room());
        let chosen = CHOSEN.load(Ordering::Acquire);
        if chosen != NO_SPEED {
            driver.chosen_speed(chosen);
        }
        let mut lines = driver.take_log();
        lines.extend(lua::take_log());
        lines.extend(crate::interval::take_lines());
        if !lines.is_empty()
            && let Some(log) = LOG.lock().unwrap_or_else(|p| p.into_inner()).as_mut()
        {
            // One write: a lane dump is thousands of lines.
            log.lines(&lines);
        }
    }));
    if result.is_err() {
        BROKEN.store(true, Ordering::Release);
        UPDATES.store(OWN_SPEED, Ordering::Release);
        if !ran {
            selected = Some((Updates::Exactly(0), false));
            // SAFETY: as above.
            unsafe { run_step(original, Updates::Exactly(0), false, this, a, b, c) };
        }
    }
    if let Some((updates, room)) = selected {
        let updates = match updates {
            Updates::Own => None,
            Updates::Exactly(updates) => Some(updates),
        };
        crate::perf::step_call(room, updates, started);
    }
    // The batch interval (main thread, after the Sync that exposes this
    // batch) times it by the updates it really ran.
    if crate::interval::armed() {
        let (count, room) = match selected {
            Some((Updates::Exactly(count), room)) => (count, room),
            _ => (0, false),
        };
        // What the step really ran: the game adds pending debug steps to
        // the answer. A batch that ran other than answered is not timed.
        let ran = match (seen_before, crate::seeds::updates_seen()) {
            (Some(before), Some(after)) => u32::try_from(after.wrapping_sub(before)).ok(),
            _ => None,
        };
        let mut record = info.record(this, count);
        record.eligible &= room && result.is_ok() && ran == Some(count);
        if room && ran.is_some_and(|ran| ran != count) {
            crate::interval::note_mismatch(count, ran.unwrap_or(0));
        }
        crate::interval::SLOTS.publish(&record);
    }
    if let (Some(at), Some((updates, lanes))) = (traced_at, answered) {
        let updates = match updates {
            Updates::Exactly(updates) => Some(updates),
            Updates::Own => None,
        };
        let game = STEP_GAME_NANOS.load(Ordering::Relaxed);
        let call = u64::try_from(at.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let lines = crate::steptrace::call(at, updates, why, lanes, game, call, note.as_deref());
        if !lines.is_empty()
            && let Some(log) = LOG.lock().unwrap_or_else(|p| p.into_inner()).as_mut()
        {
            log.lines(&lines);
        }
    }
    finish_perf(started);
}

/// The main menu's Multiplayer window asks (crate::menu_entry): its actions
/// go to the launcher and its lobby comes back through the step driver,
/// which at the menu is the only reader of the link. Never waits for the
/// driver: the step's detour holds it only while a step runs, and exchanges
/// the lobby itself after it.
pub(crate) fn lobby_pump() {
    let Ok(mut guard) = DRIVER.try_lock() else {
        return;
    };
    let Some(driver) = guard.as_mut() else {
        drop(guard);
        crate::lobby::unlinked();
        return;
    };
    let lines = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::lobby::exchange(driver.as_mut());
        driver.take_log()
    }))
    .unwrap_or_default();
    drop(guard);
    if !lines.is_empty()
        && let Some(log) = LOG.lock().unwrap_or_else(|p| p.into_inner()).as_mut()
    {
        for line in lines {
            log.line(&line);
        }
    }
}

/// Installs a detour for the life of the process and returns its trampoline.
///
/// # Safety
///
/// As [`tpf3mp_hookcore::detour::InlineDetour::install`]: `target` is a
/// function in this process that no thread is running, and `detour` has its
/// ABI.
#[cfg(target_arch = "x86_64")]
unsafe fn detour_forever(target: *mut u8, detour: *const u8) -> Result<usize, String> {
    // SAFETY: the caller's.
    let installed = unsafe { tpf3mp_hookcore::detour::InlineDetour::install(target, detour) }
        .map_err(|error| format!("{error:?}"))?;
    let trampoline = installed.trampoline() as usize;
    std::mem::forget(installed);
    Ok(trampoline)
}

/// What installing came to.
pub enum Installed {
    Yes { step_rva: u64 },
    No(String),
}

/// Resolves the profile in the running image, attaches the session and
/// detours the step. Any failure installs nothing.
pub fn install(profile: &Profile, link_name: &str, log: crate::Logger) -> Installed {
    *LOG.lock().unwrap_or_else(|p| p.into_inner()) = Some(log);
    match install_inner(profile, link_name) {
        Ok(step_rva) => Installed::Yes { step_rva },
        Err(reason) => Installed::No(reason),
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn install_inner(profile: &Profile, link_name: &str) -> Result<u64, String> {
    use std::time::Duration;

    use tpf3mp_hookcore::{pe::PeHeaders, profile};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    // SAFETY: a null name asks for the executable's own module handle, its
    // base address, which stays mapped for the life of the process.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if base == 0 {
        return Err("cannot find the game's module".into());
    }
    // SAFETY: the first page of a mapped module holds its headers.
    let head = unsafe { std::slice::from_raw_parts(base as *const u8, 0x1000) };
    let pe = PeHeaders::parse(head).map_err(|error| format!("the game's headers: {error:?}"))?;
    let text = pe.section(".text").ok_or("the game has no .text section")?;
    // SAFETY: .text is mapped at base + its virtual address for its virtual
    // size, and is only read here.
    let code = unsafe {
        std::slice::from_raw_parts(
            (base + text.virtual_address as usize) as *const u8,
            text.virtual_size as usize,
        )
    };
    let resolved = profile::resolve(profile, code, u64::from(text.virtual_address))
        .map_err(|refusal| format!("the profile does not resolve here: {refusal:?}"))?;
    // Scan the opt-in diagnostic sites before installing any hook patches.
    // They are pinned to 40408 and do not extend the release profile shared
    // with the independently verified Preview build.
    let industry_probe =
        crate::industries::resolve_probe(profile, code, u64::from(text.virtual_address));
    let step_rva = resolved
        .get(STEP_TARGET)
        .ok_or_else(|| format!("the profile has no {STEP_TARGET}"))?
        .address;
    // Without the speed held, one call of the step could run several
    // updates: no room's game without it (fail closed).
    let speed_rva = resolved
        .get(SPEED_TARGET)
        .ok_or_else(|| format!("the profile has no {SPEED_TARGET}"))?
        .address;
    let call_rva = resolved
        .get(SPEED_CALL_TARGET)
        .ok_or_else(|| format!("the profile has no {SPEED_CALL_TARGET}"))?
        .address;
    // Without the link to the mod, the room's actions could not be applied
    // and the player's not handed over: no room's game without it.
    let at = |name: &str| -> Result<usize, String> {
        resolved
            .get(name)
            .map(|target| base + target.address as usize)
            .ok_or_else(|| format!("the profile has no {name}"))
    };
    let print_at = at(PRINT_TARGET)?;
    // SAFETY: each address is the function of Lua 5.2's C API the profile
    // names, found by its signature and checked by its prologue in this very
    // build; the types are that API's.
    let api = unsafe { lua_api(&at)? };

    let session = tpf3mp_bridge::Session::attach(link_name, &profile.name, Duration::from_secs(30))
        .map_err(|error| format!("the agent's link: {error}"))?;
    lua::install_api(api);
    crate::network::install(base);
    let mut driver = crate::step::StepDriver::new(
        session,
        Box::new(crate::worlds::GuiWorlds::in_steam_folder()),
    );
    let env = std::env::var(crate::lanedump::ENV).ok();
    let (setting, refused) = crate::lanedump::Setting::from_env(env.as_deref());
    // The town trace adds the towns lane, its size factors, experience and
    // level, at every checkpoint (crate::towntrace).
    let setting = setting.with_town_trace(crate::towntrace::wanted(
        std::env::var(crate::towntrace::ENV).ok().as_deref(),
    ));
    // The network lane cut to a box at the checkpoints of a step range
    // (crate::lanedump::BOX_ENV), even with dumps off.
    let boxed = crate::lanedump::BoxDump::from_env(
        std::env::var(crate::lanedump::BOX_ENV).ok().as_deref(),
        std::env::var(crate::lanedump::BOX_STEPS_ENV)
            .ok()
            .as_deref(),
    )
    .unwrap_or_else(|why| {
        log_line(&why);
        None
    });
    if let Some(boxed) = &boxed {
        log_line(&boxed.describe());
    }
    let setting = setting.with_box(boxed);
    if let Some(why) = refused {
        log_line(&why);
    } else if setting.off {
        log_line("lane dumps are off, even after a divergence");
    } else if !setting.always.is_empty() {
        log_line(&format!(
            "dumping lanes {:?} at every checkpoint ({} or {})",
            setting.always,
            crate::lanedump::ENV,
            crate::towntrace::ENV
        ));
    }
    driver.set_lane_dumps(crate::lanedump::LaneDumps::new(setting));
    let even = crate::cadence::wanted(std::env::var(crate::cadence::ENV).ok().as_deref());
    driver.set_even_steps(even);
    log_line(&if even {
        format!(
            "even steps: each call of the game's step runs the room's pace over the game's call period, keeping {} released step(s) in reserve ({}=0 runs all released, as before)",
            crate::cadence::RESERVE,
            crate::cadence::ENV
        )
    } else {
        format!(
            "even steps off ({} says so): each call runs every step released",
            crate::cadence::ENV
        )
    });
    // The batch interval (crate::interval): only on the build whose layout
    // was verified, with both of its targets, even steps on and the knob
    // not saying no; otherwise the game paces itself.
    let interval_wanted =
        crate::interval::wanted(std::env::var(crate::interval::ENV).ok().as_deref());
    let layout = crate::interval::layout_for(&profile.name);
    let sync_targets = (
        resolved.get(SYNC_TARGET).map(|t| t.address),
        resolved.get(SYNC_CALL_TARGET).map(|t| t.address),
    );
    let interval_line = match (interval_wanted, even, layout, sync_targets) {
        (false, ..) => format!("batch interval off ({} says so)", crate::interval::ENV),
        (true, false, ..) => "batch interval off: it needs even steps".to_owned(),
        (true, true, None, _) => format!(
            "batch interval off: no verified layout for {}",
            profile.name
        ),
        (true, true, Some(layout), (Some(sync_rva), Some(call_rva))) => {
            SYNC.store(base + sync_rva as usize, Ordering::Release);
            // SAFETY: the call site the profile resolved inside CGame::Step,
            // which no thread runs yet (the game has no world); install
            // checks it calls CGame::Sync, and sync_wrap has Sync's ABI.
            match unsafe {
                tpf3mp_hookcore::detour::CallRedirect::install(
                    (base + call_rva as usize) as *mut u8,
                    base + sync_rva as usize,
                    sync_wrap as *const u8,
                )
            } {
                Ok(redirect) => {
                    std::mem::forget(redirect);
                    crate::interval::arm(layout);
                    driver.set_batch_interval(true);
                    format!(
                        "batch interval: each batch of the room's steps is shown over its updates times the room's time per step, set right after CGame::Sync ({}=0 turns it off)",
                        crate::interval::ENV
                    )
                }
                Err(error) => {
                    format!("batch interval off: redirecting {SYNC_CALL_TARGET}: {error:?}")
                }
            }
        }
        (true, true, Some(_), _) => {
            format!("batch interval off: the profile has no {SYNC_TARGET} or {SYNC_CALL_TARGET}")
        }
    };
    log_line(&interval_line);
    *DRIVER.lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(driver));

    // SAFETY: both targets are functions the profile resolved, exactly once,
    // in this process's code; the game has not run a step yet (the hook
    // installs while the game starts, before any world is loaded); each
    // detour has its target's ABI (four register arguments passed through).
    // The getter and the step's call of it go first, so the step never runs
    // with the room's pace but the game's speed.
    let speed = unsafe {
        detour_forever(
            (base + speed_rva as usize) as *mut u8,
            speed_detour as *const u8,
        )
    }
    .map_err(|error| format!("detouring {SPEED_TARGET}: {error}"))?;
    SPEED_ORIGINAL.store(speed, Ordering::Release);
    // SAFETY: the call site the profile resolved inside the step, which no
    // thread runs yet; install checks it is a call of the getter, and
    // step_speed has the getter's ABI.
    let redirect = unsafe {
        tpf3mp_hookcore::detour::CallRedirect::install(
            (base + call_rva as usize) as *mut u8,
            base + speed_rva as usize,
            step_speed as *const u8,
        )
    }
    .map_err(|error| format!("redirecting {SPEED_CALL_TARGET}: {error:?}"))?;
    std::mem::forget(redirect);
    // SAFETY: Lua's print, which the profile resolved; no Lua state runs
    // before the game's loading screen, long after the hook installs;
    // print_detour has the ABI of a Lua C function.
    let print = unsafe { detour_forever(print_at as *mut u8, print_detour as *const u8) }
        .map_err(|error| format!("detouring {PRINT_TARGET}: {error}"))?;
    PRINT_ORIGINAL.store(print, Ordering::Release);
    // SAFETY: as above.
    let step = unsafe {
        detour_forever(
            (base + step_rva as usize) as *mut u8,
            step_detour as *const u8,
        )
    }
    .map_err(|error| format!("detouring {STEP_TARGET}: {error}"))?;
    ORIGINAL.store(step, Ordering::Release);

    // The build tools through the room, where the profile has what they
    // need; without it they stay refused in the room's game.
    let builds = match (at(ADD_TARGET), at(BUILD_APPLY_TARGET)) {
        // SAFETY: both are the functions the profile resolved, which no
        // thread runs yet; detour_forever installs each for good.
        (Ok(add), Ok(apply)) => {
            let module = at(crate::modules::MODULE_ADD_CALL).ok();
            let terrain = at(crate::terrain::DO_APPLY_ADD_CALL).ok();
            // The stop tool takes its next click at once only where the
            // profile finds both its call and its busy byte.
            let stop = at(crate::stoptool::STOP_ADD_CALL)
                .ok()
                .filter(|_| at(crate::stoptool::STOP_BUSY_SET).is_ok());
            // The crossing window's type change, read only where its maps
            // were found too (crates/tpf3mp-hook/src/crossings.rs).
            let crossing = at(crate::crossings::CROSSING_ADD_CALL)
                .ok()
                .map(|call| (call, at(crate::crossings::MAPS_LAYOUT).is_ok()));
            // The street and track tools' toolbar crossing type, read in
            // the same maps whether or not the window's call is found.
            crate::crossings::set_layout(at(crate::crossings::MAPS_LAYOUT).is_ok());
            // The game's insert into a crossing map, which the room's replay
            // fills its carrier with: through the window's own call of it.
            let read = |at: u64, len: usize| {
                usize::try_from(at)
                    .ok()
                    .and_then(|at| crate::modules::Memory::read(&crate::modules::Process, at, len))
            };
            let insert = at(crate::crossings::MAP_INSERT_CALL)
                .ok()
                .and_then(|call| crate::persons::call_target(call as u64, &read))
                .filter(|insert| {
                    read(*insert, crate::crossings::INSERT_HEAD.len()).as_deref()
                        == Some(crate::crossings::INSERT_HEAD)
                })
                .and_then(|insert| usize::try_from(insert).ok());
            if let Some(insert) = insert {
                crate::crossings::set_insert(insert);
            }
            crate::junctions::enable(
                at(crate::junctions::CONFIG_LAYOUT).is_ok()
                    && at(crate::junctions::PROPOSAL_LAYOUT).is_ok()
                    && at(crate::junctions::CROSSWALK_LAYOUT).is_ok(),
            );
            // SAFETY: as above.
            unsafe {
                crate::builds::install(add, apply, module, terrain, stop, crossing, detour_forever)
            }
                .map(|()| {
                    let module = match module {
                        Some(call) => format!(
                            "the module editor's builds are read where Add returns to {:#x}",
                            call + 5
                        ),
                        None => "the profile has no module editor call, so its builds stay \
                                 refused"
                            .to_owned(),
                    };
                    let terrain = match terrain {
                        Some(call) => {
                            format!("the terrain tools' where it returns to {:#x}", call + 5)
                        }
                        None => "the profile has no terrain tools' call, so terraforming \
                                 stays refused"
                            .to_owned(),
                    };
                    let stop = match stop {
                        Some(call) => format!(
                            "the stop tool takes its next click at once where Add returns to {:#x}",
                            call + 5
                        ),
                        None => "the profile has no stop tool call, so it waits for each \
                                 click's answer"
                            .to_owned(),
                    };
                    let crossing = match crossing {
                        Some((call, true)) => format!(
                            "the level crossing window's where it returns to {:#x}, its \
                             replay {}",
                            call + 5,
                            if insert.is_some() {
                                "through the game's crossing map insert"
                            } else {
                                "refused: the profile does not lead to the crossing map's insert"
                            }
                        ),
                        Some((_, false)) => "the profile does not find the crossing maps, so the crossing window stays refused"
                            .to_owned(),
                        None => "the profile has no crossing window call, so it stays refused"
                            .to_owned(),
                    };
                    format!(
                        "the build tools build through the room; {module}; {terrain}; {stop}; {crossing}"
                    )
                })
                .unwrap_or_else(|error| format!("the build tools stay refused: {error}"))
        }
        _ => "the build tools stay refused: the profile has no build targets".to_owned(),
    };
    log_line(&builds);
    // The street and track tools continue from the room's segment
    // (docs/HOOKS.md, "The street and track tools continue"): without every
    // target, they wait for a start point after each segment.
    if let (Ok(add), Ok(_)) = (at(ADD_TARGET), at(BUILD_APPLY_TARGET)) {
        log_line(&crate::toolchain::install(&at, add));
    }
    // Loading the room's world from the main menu (docs/HOOKS.md, "Loading
    // from the main menu"): without it, a game needs a world up to take the
    // room's, as before.
    // SAFETY: `at` gives the functions the profile resolved in this build,
    // which no thread runs yet; detour_forever installs each for good.
    match unsafe { crate::menu::install(&at, detour_forever) } {
        Ok(line) | Err(line) => log_line(&line),
    }
    // The others' build previews, drawn (docs/HOOKS.md, "Build previews"):
    // without every target, nobody's is.
    // SAFETY: as above.
    log_line(&unsafe { crate::drawing::install(&at, detour_forever) });
    // The street/track tool may abort its proposal while remaining open.
    // SAFETY: the profile checked the reset, and no tool runs yet.
    log_line(&unsafe { crate::previewcancel::install(&at, detour_forever) });
    // Faster saves (docs/BIGMAPS.md, "Saving"): without either site, the
    // game's own level and buffer.
    // SAFETY: `at` gives addresses in this image; nothing saves yet.
    log_line(&unsafe { crate::savefast::install(&at) });
    // The seeds and the order fixes (docs/HOOKS.md, "Seeds, as built" and
    // "The order fixes, as built") take the targets at their addresses in
    // this process; each piece installs, and fails closed, on its own, and
    // logs its own outcome.
    let mut absolute = resolved.clone();
    for target in &mut absolute.targets {
        target.address = target.address.saturating_add(base as u64);
    }
    log_line(&if crate::perf::configure_from_env() {
        format!(
            "perf: timing the hook's work, two lines every {} s ({}=0 turns it off)",
            crate::perf::WINDOW.as_secs(),
            crate::perf::ENV
        )
    } else {
        format!("perf: timing off ({} says so)", crate::perf::ENV)
    });
    // Guarded reads (docs/HOOKS.md, "Reading the game's memory"): the
    // handler goes in before any fix reads.
    log_line(&crate::image::guarded::configure_from_env());
    if let Some(line) = crate::steptrace::configure_from_env() {
        log_line(&line);
    }
    crate::seeds::install(&absolute);
    log_line(&crate::ticks::install(&absolute));
    for line in crate::roadtrace::configure_from_env() {
        log_line(&line);
    }
    for outcome in crate::order::install(&absolute) {
        log_line(&outcome.to_string());
    }
    match industry_probe {
        Ok(Some(mut diagnostic)) => {
            for target in &mut diagnostic.targets {
                target.address = target.address.saturating_add(base as u64);
            }
            for line in crate::industries::install(&diagnostic) {
                log_line(&line);
            }
        }
        Ok(None) => {}
        Err(error) => log_line(&format!("industry-spawn probe: off; {error}")),
    }
    log_line(&crate::townfield::install(&absolute));
    log_line(&crate::probe::install(&absolute));
    for line in crate::toolplayer::install(&absolute) {
        log_line(&line);
    }
    for line in crate::guiplayer::install(&absolute) {
        log_line(&line);
    }
    for line in crate::persons::install(&absolute) {
        log_line(&line);
    }
    for line in crate::towntrace::install(&absolute, base as u64) {
        log_line(&line);
    }
    for line in crate::edgewatch::install(&absolute, base as u64) {
        log_line(&line);
    }
    for line in crate::streettrace::install(&absolute) {
        log_line(&line);
    }
    // The game's own systems timed (crate::simperf) and the faster component
    // lookup (crate::fastindex), each failing closed on its own.
    for line in crate::simperf::install(&absolute, base as u64) {
        log_line(&line);
    }
    log_line(&crate::fastindex::install(&absolute));
    // The fused emission grid (crate::emission): bit-identical, on unless
    // TPF3MP_HOOK_FAST_EMISSION=0.
    log_line(&crate::emission::install(&absolute));
    Ok(step_rva)
}

/// Lua 5.2's C API, from the addresses `at` gives for the profile's names.
///
/// # Safety
///
/// Every address `at` gives is the named function of Lua 5.2's C API.
#[cfg(all(windows, target_arch = "x86_64"))]
// Each transmute's type is the one of the field it fills: the API's
// signature, spelled once, in `lua::LuaApi`.
#[allow(clippy::missing_transmute_annotations)]
unsafe fn lua_api(at: &dyn Fn(&str) -> Result<usize, String>) -> Result<lua::LuaApi, String> {
    macro_rules! function {
        ($name:literal) => {{
            let address = at($name)?;
            // SAFETY: the caller's: `address` is this function of the API,
            // with the type the field it goes to has.
            unsafe { std::mem::transmute::<usize, _>(address) }
        }};
    }
    Ok(lua::LuaApi {
        gettop: function!("lua_gettop"),
        settop: function!("lua_settop"),
        checkstack: function!("lua_checkstack"),
        pushvalue: function!("lua_pushvalue"),
        type_of: function!("lua_type"),
        toboolean: function!("lua_toboolean"),
        tonumberx: function!("lua_tonumberx"),
        tolstring: function!("lua_tolstring"),
        // Optional on older external profiles; Lua keeps its existing reader.
        touserdata: at("lua_touserdata").ok().map(|address| {
            // SAFETY: signature/prologue resolution identifies this Lua API.
            unsafe { std::mem::transmute::<usize, _>(address) }
        }),
        next: function!("lua_next"),
        pushnil: function!("lua_pushnil"),
        pushnumber: function!("lua_pushnumber"),
        pushboolean: function!("lua_pushboolean"),
        pushlstring: function!("lua_pushlstring"),
        pushcclosure: function!("lua_pushcclosure"),
        createtable: function!("lua_createtable"),
        rawget: function!("lua_rawget"),
        rawset: function!("lua_rawset"),
        rawgeti: function!("lua_rawgeti"),
        globals: lua::LUA52_GLOBALS,
    })
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn install_inner(_profile: &Profile, _link_name: &str) -> Result<u64, String> {
    Err("the step gate is installed on Windows x64 only so far".into())
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {

    use tpf3mp_bridge::{Load, StepGate};
    use tpf3mp_hookcore::detour::InlineDetour;

    use super::*;
    use crate::{
        lua::tests::{Lua, SERIAL, depot_build, lua51, run_in},
        step::{
            StepDriver,
            tests::{FakeControl, Script, begin, command_event},
        },
    };

    /// A Lua state the fake step's "game script" runs in, while one is set.
    static SCRIPT_STATE: AtomicUsize = AtomicUsize::new(0);

    /// What each call of the fake step was told to run.
    static CALLS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
    static ARGS_OK: AtomicBool = AtomicBool::new(true);

    /// A stand-in for the game's step, in this test binary: long enough a
    /// prologue for the detour engine to steal. It checks the arguments
    /// arrive unchanged and records what its call of the speed getter would
    /// be answered.
    #[inline(never)]
    extern "C" fn fake_step(this: usize, a: usize, b: usize, c: usize) {
        if (this, a, b, c) != (0x1111, 0x2222, 0x3333, 0x4444) {
            ARGS_OK.store(false, Ordering::SeqCst);
        }
        let updates = std::hint::black_box(UPDATES.load(Ordering::SeqCst));
        CALLS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(updates);
        // The mod's game script, in each update: take the room's actions.
        let state = SCRIPT_STATE.load(Ordering::SeqCst);
        if state != 0 && updates != OWN_SPEED {
            for _ in 0..updates {
                let taken = run_in(
                    state as lua::State,
                    "local t = tpf3mp_native.take() if t then TAKEN = (TAKEN or 0) + #t end",
                );
                assert!(taken.is_ok(), "the game script failed: {taken:?}");
            }
        }
    }

    static SPEED: AtomicU64 = AtomicU64::new(4);

    /// A stand-in for the game's speed getter, with a prologue long enough
    /// to steal.
    #[inline(never)]
    extern "C" fn fake_speed(this: usize, a: usize, b: usize, c: usize) -> u64 {
        let noise = std::hint::black_box(this ^ a ^ b ^ c) as u64;
        let zero = std::hint::black_box(0u64);
        SPEED.load(Ordering::SeqCst) + noise * zero
    }

    #[test]
    fn the_step_reads_the_drivers_updates_and_everyone_else_the_games_speed() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let target = fake_speed as *mut u8;
        // SAFETY: fake_speed is this binary's own function, not running now,
        // and speed_detour has its signature.
        let detour = unsafe { InlineDetour::install(target, speed_detour as *const u8) }.unwrap();
        SPEED_ORIGINAL.store(detour.trampoline() as usize, Ordering::Release);
        let speed: extern "C" fn(usize, usize, usize, usize) -> u64 =
            std::hint::black_box(fake_speed);
        // SAFETY: step_speed is what the step's redirected call reaches.
        let step_reads = |this| unsafe { step_speed(this, 2, 3, 4) };
        IN_ROOM.store(false, Ordering::Release);
        UPDATES.store(OWN_SPEED, Ordering::Release);
        assert_eq!(speed(1, 2, 3, 4), 4, "outside a room, the game's own speed");
        assert_eq!(step_reads(1), 4, "for the step too");
        assert_eq!(CHOSEN.load(Ordering::SeqCst), NO_SPEED);
        IN_ROOM.store(true, Ordering::Release);
        UPDATES.store(1, Ordering::Release);
        assert_eq!(step_reads(1), 1, "in the room's game, the driver's updates");
        assert_eq!(
            speed(1, 2, 3, 4),
            4,
            "every other caller still reads the game's own speed"
        );
        assert_eq!(
            CHOSEN.load(Ordering::SeqCst),
            4,
            "the speed row's value is kept"
        );
        SPEED.store(0, Ordering::SeqCst);
        assert_eq!(
            step_reads(1),
            1,
            "the game's own pause does not stop the room"
        );
        assert_eq!(
            CHOSEN.load(Ordering::SeqCst),
            0,
            "but it asks the room to pause"
        );
        UPDATES.store(0, Ordering::Release);
        SPEED.store(4, Ordering::SeqCst);
        assert_eq!(step_reads(1), 0, "nor does its 4x run a withheld step");
        UPDATES.store(OWN_SPEED, Ordering::Release);
        CHOSEN.store(NO_SPEED, Ordering::SeqCst);
        IN_ROOM.store(false, Ordering::Release);
        SPEED_ORIGINAL.store(0, Ordering::Release);
        // SAFETY: nothing runs fake_speed now.
        unsafe { detour.detach() }.unwrap();
    }

    #[test]
    fn the_detour_runs_the_games_step_once_a_call_with_the_released_steps() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let mut script = Script::default();
        script.begin.push_back(None);
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        *DRIVER.lock().unwrap() = Some(Box::new(StepDriver::new(
            script,
            Box::new(FakeControl::default()),
        )));
        let target = fake_step as *mut u8;
        // SAFETY: fake_step is this binary's own function, not running now,
        // and step_detour has its signature.
        let detour = unsafe { InlineDetour::install(target, step_detour as *const u8) }.unwrap();
        ORIGINAL.store(detour.trampoline() as usize, Ordering::Release);

        let step: extern "C" fn(usize, usize, usize, usize) = std::hint::black_box(fake_step);
        // No room's game yet: the game's own speed.
        step(0x1111, 0x2222, 0x3333, 0x4444);
        // The room released three steps: one call of the game's step runs
        // three updates.
        step(0x1111, 0x2222, 0x3333, 0x4444);
        // Withheld: the game's step still runs, on its paused path.
        step(0x1111, 0x2222, 0x3333, 0x4444);
        assert_eq!(*CALLS.lock().unwrap(), vec![OWN_SPEED, 3, 0]);
        assert_eq!(
            UPDATES.load(Ordering::SeqCst),
            OWN_SPEED,
            "outside the step, the getter is the game's own"
        );
        assert!(
            ARGS_OK.load(Ordering::SeqCst),
            "the arguments reached the step unchanged"
        );
        assert!(!BROKEN.load(Ordering::SeqCst));

        ORIGINAL.store(0, Ordering::Release);
        // SAFETY: nothing runs fake_step now.
        unsafe { detour.detach() }.unwrap();
        *DRIVER.lock().unwrap() = None;
        IN_ROOM.store(false, Ordering::Release);
        step(0x1111, 0x2222, 0x3333, 0x4444);
        assert_eq!(
            CALLS.lock().unwrap().len(),
            4,
            "detached, the step is the game's own again"
        );
    }

    #[test]
    fn the_rooms_actions_hold_updates_until_the_event_replay_finishes() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        lua51();
        lua::take_commands();
        let _ = lua::end_batch();
        let game_script = Lua::new();
        game_script.register();
        SCRIPT_STATE.store(game_script.state() as usize, Ordering::SeqCst);
        CALLS.lock().unwrap_or_else(|p| p.into_inner()).clear();

        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([
            StepGate::Load(Load {
                world_epoch: 1,
                file: None,
                next_step: 1,
            }),
            StepGate::Run,
            StepGate::Run,
            StepGate::Wait,
        ]);
        script
            .events
            .extend([vec![], vec![command_event(1, 1, &depot_build())]]);
        *DRIVER.lock().unwrap() = Some(Box::new(StepDriver::new(
            script,
            Box::new(FakeControl::default()),
        )));
        let target = fake_step as *mut u8;
        // SAFETY: fake_step is this binary's own function, not running now,
        // and step_detour has its signature.
        let detour = unsafe { InlineDetour::install(target, step_detour as *const u8) }.unwrap();
        ORIGINAL.store(detour.trampoline() as usize, Ordering::Release);
        let step: extern "C" fn(usize, usize, usize, usize) = std::hint::black_box(fake_step);

        // The first call queues the replay without running any updates.
        step(0x1111, 0x2222, 0x3333, 0x4444);
        assert_eq!(game_script.run("return TAKEN"), Ok("nil".into()));
        assert!(!BROKEN.load(Ordering::SeqCst));
        // The player's action goes to the room from the step as well.
        game_script
            .run("tpf3mp_native.command({ SellVehicle = { vehicles = { 7 } } })")
            .unwrap();
        step(0x1111, 0x2222, 0x3333, 0x4444);
        assert!(lua::take_commands().is_empty(), "handed to the room");
        assert_eq!(*CALLS.lock().unwrap(), vec![0, 2]);

        ORIGINAL.store(0, Ordering::Release);
        // SAFETY: nothing runs fake_step now.
        unsafe { detour.detach() }.unwrap();
        *DRIVER.lock().unwrap() = None;
        SCRIPT_STATE.store(0, Ordering::SeqCst);
        IN_ROOM.store(false, Ordering::Release);
        CALLS.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    #[test]
    fn a_step_whose_game_script_took_nothing_holds_the_world() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        lua51();
        let _ = lua::end_batch();
        SCRIPT_STATE.store(0, Ordering::SeqCst);
        CALLS.lock().unwrap_or_else(|p| p.into_inner()).clear();
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.extend([StepGate::Run, StepGate::Run]);
        script
            .events
            .push_back(vec![command_event(1, 1, &depot_build())]);
        let control = FakeControl::default();
        control.state.lock().unwrap().replay_wait = true;
        *DRIVER.lock().unwrap() = Some(Box::new(StepDriver::new(script, Box::new(control))));
        let target = fake_step as *mut u8;
        // SAFETY: as above.
        let detour = unsafe { InlineDetour::install(target, step_detour as *const u8) }.unwrap();
        ORIGINAL.store(detour.trampoline() as usize, Ordering::Release);
        let step: extern "C" fn(usize, usize, usize, usize) = std::hint::black_box(fake_step);
        step(0x1111, 0x2222, 0x3333, 0x4444);
        step(0x1111, 0x2222, 0x3333, 0x4444);
        assert_eq!(
            *CALLS.lock().unwrap(),
            vec![0, 0],
            "no update runs while the ordered replay is unfinished"
        );
        let driver = DRIVER.lock().unwrap().take().unwrap();
        assert!(driver.in_room(), "held, not left");

        ORIGINAL.store(0, Ordering::Release);
        // SAFETY: nothing runs fake_step now.
        unsafe { detour.detach() }.unwrap();
        IN_ROOM.store(false, Ordering::Release);
        CALLS.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    #[test]
    fn a_menu_without_its_lua_state_still_processes_start_and_leave() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        lua::forget_worlds();
        forget_menu_sight();
        crate::menu::tests::menu51();
        assert!(!crate::menu::available());
        // Other serialized tests have stepped a world. This scenario models
        // a fresh process, so its last-step clock must start fresh as well.
        LAST_STEP.store(0, Ordering::Release);
        let mut script = Script::default();
        script.begin.push_back(Some(begin()));
        script.gates.push_back(StepGate::Wait);
        script.departure_done = true;
        script.reset_ended = true;
        *DRIVER.lock().unwrap() = Some(Box::new(StepDriver::new(
            script,
            Box::new(FakeControl::default()),
        )));
        // No step ran in this game: a test that ran the step's detour
        // before this one leaves its time behind, and the menu then waits
        // for the world it thinks is closing.
        LAST_STEP.store(0, Ordering::Release);
        let mut cmenu = [0usize; 3];
        crate::menu::set_load_field(16);
        let at = cmenu.as_mut_ptr() as usize;
        menu_frame(at);
        assert!(
            DRIVER.lock().unwrap().as_ref().unwrap().in_room(),
            "Begin must not block the lobby queue when menu Lua is missing"
        );
        DRIVER
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .lobby(vec![tpf3mp_bridge::LobbyAction::Leave]);
        menu_frame(at);
        assert!(
            !DRIVER.lock().unwrap().as_ref().unwrap().in_room(),
            "Leave must drain End and return to the lobby without menu Lua"
        );
        *DRIVER.lock().unwrap() = None;
        IN_ROOM.store(false, Ordering::Release);
        forget_menu_sight();
    }

    /// A game at its main menu, no world up: the menu's frame follows the
    /// room into its game and has the menu's Lua load the room's save.
    #[test]
    fn at_the_main_menu_the_rooms_save_is_loaded_by_the_menus_lua() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        // No world has started in this game (SERIAL is the Lua link's).
        lua::forget_worlds();
        forget_menu_sight();
        crate::menu::tests::menu51();
        let dir = std::env::temp_dir().join(format!("tpf3mp-menu-frame-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let room = dir.join("room.sav");
        std::fs::write(&room, b"the room's world").unwrap();
        let menu = Lua::new();
        menu.run(
            "LOADS = {} api = { type = { SavegameId = { new = function() return {} end } } } \
             app = { SaveGameNamespace = { getSavegame = function() return 'savegame' end }, \
                     getProgressMonitor = function() return { getTask = function() return '' end } end, \
                     loadGame = function(id) LOADS[#LOADS + 1] = id.saveGameName end }",
        )
        .unwrap();
        let mut script = Script::default();
        script.begin.extend([None, None, Some(begin())]);
        script.gates.push_back(StepGate::Load(Load {
            world_epoch: 1,
            file: Some(room.clone()),
            next_step: 9,
        }));
        *DRIVER.lock().unwrap() = Some(Box::new(StepDriver::new(
            script,
            Box::new(crate::worlds::GuiWorlds::in_folder(Ok(dir.clone()))),
        )));
        LAST_STEP.store(0, Ordering::Release);

        let mut cmenu = [0usize; 3];
        crate::menu::set_load_field(16);
        let at = cmenu.as_mut_ptr() as usize;
        // No menu state yet: the menu does nothing (fail closed).
        menu_frame(at);
        assert_eq!(menu.run("return #LOADS"), Ok("0".into()));
        assert!(!DRIVER.lock().unwrap().as_ref().unwrap().in_room());

        assert_eq!(unsafe { crate::menu::adopt(menu.state()) }, Ok(true));
        // The lobby: nothing to load.
        let before = crate::menu::tests::PCALLS.load(Ordering::SeqCst);
        crate::menu::set_load_field(0);
        menu_frame(at);
        crate::menu::set_load_field(16);
        unsafe {
            std::ptr::write_volatile((at + 16) as *mut usize, 1);
        }
        menu_frame(at);
        assert_eq!(
            crate::menu::tests::PCALLS.load(Ordering::SeqCst),
            before,
            "unknown or active loads must never enter menu Lua"
        );
        assert!(!DRIVER.lock().unwrap().as_ref().unwrap().in_room());
        unsafe {
            std::ptr::write_volatile((at + 16) as *mut usize, 0);
        }
        menu_frame(at);
        assert_eq!(menu.run("return #LOADS"), Ok("0".into()));
        // The room begins and orders its save: the menu loads it.
        menu_frame(at);
        let name = format!("tpf3mp_room_{}", std::process::id());
        assert_eq!(menu.run("return #LOADS, LOADS[1]"), Ok(format!("1|{name}")));
        assert!(dir.join(format!("{name}.sav")).is_file());
        menu_frame(at);
        assert_eq!(menu.run("return #LOADS"), Ok("1".into()), "loaded once");
        // A world's GUI has started: the menu keeps out, even before its
        // first step (the owner's save for the room comes then).
        lua::menu_load_failed(String::new());
        let _ = lua::take_load_failure();
        lua::request_menu_load("again");
        {
            let world = Lua::new();
            world.register();
            world.run("tpf3mp_native.world()").unwrap();
        }
        menu_frame(at);
        assert_eq!(
            menu.run("return #LOADS"),
            Ok("1".into()),
            "no menu work once a world's GUI started"
        );
        // A world has stepped: the menu keeps out of it for good, also
        // while that world stops stepping (a save, or held for another
        // player), and even with an order waiting.
        lua::forget_worlds();
        LAST_STEP.store(now_ms().max(1), Ordering::Release);
        menu_frame(at);
        assert_eq!(
            menu.run("return #LOADS"),
            Ok("1".into()),
            "no menu work after a world"
        );

        *DRIVER.lock().unwrap() = None;
        LAST_STEP.store(0, Ordering::Release);
        IN_ROOM.store(false, Ordering::Release);
        lua::set_in_room(false);
        crate::menu::tests::forget_all();
        let _ = lua::take_menu_load();
        lua::menu_load_failed(String::new());
        let _ = lua::take_load_failure();
        let _ = std::fs::remove_dir_all(&dir);
        forget_menu_sight();
    }

    /// The menu's frames start over: no world seen, the clock unskewed.
    fn forget_menu_sight() {
        let mut sight = menu_sight();
        sight.gate = crate::at_menu::MenuGate::new();
        sight.seen = None;
        sight.logged = None;
        drop(sight);
        MENU_CLOCK_SKEW.store(0, Ordering::Release);
        crate::menu::set_game_field(0);
        crate::menu::set_load_field(0);
    }

    /// A game that had a world up and closed it: the menu keeps out while
    /// the world is loaded (stepping or not) and while the next one loads,
    /// and follows the room again once the menu is quiet, logging what it
    /// sees when the room begins there.
    #[test]
    fn back_at_the_main_menu_after_a_world_the_room_is_followed_again() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        lua::forget_worlds();
        forget_menu_sight();
        crate::menu::tests::menu51();
        let dir = std::env::temp_dir().join(format!("tpf3mp-menu-back-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        *LOG.lock().unwrap() = Some(crate::Logger::open(Some(&dir)));
        let hook_log = || std::fs::read_to_string(dir.join("hook.log")).unwrap_or_default();
        let room = dir.join("room.sav");
        std::fs::write(&room, b"the room's world").unwrap();
        let menu = Lua::new();
        menu.run(
            "LOADS = {} TASK = '' api = { type = { SavegameId = { new = function() return {} end } } } \
             app = { SaveGameNamespace = { getSavegame = function() return 'savegame' end }, \
                     getProgressMonitor = function() return { getTask = function() return TASK end } end, \
                     loadGame = function(id) LOADS[#LOADS + 1] = id.saveGameName end }",
        )
        .unwrap();
        assert_eq!(unsafe { crate::menu::adopt(menu.state()) }, Ok(true));
        let mut script = Script::default();
        script.begin.extend([None, Some(begin())]);
        script.gates.push_back(StepGate::Load(Load {
            world_epoch: 1,
            file: Some(room.clone()),
            next_step: 9,
        }));
        *DRIVER.lock().unwrap() = Some(Box::new(StepDriver::new(
            script,
            Box::new(crate::worlds::GuiWorlds::in_folder(Ok(dir.clone()))),
        )));
        // The menu: m_game is its second word here.
        let mut cmenu = [0usize; 4];
        crate::menu::set_game_field(8);
        crate::menu::set_load_field(16);
        let at = cmenu.as_mut_ptr() as usize;
        let set_world = |loaded: bool| unsafe {
            std::ptr::write_volatile((at + 8) as *mut usize, usize::from(loaded));
        };
        // A world is loaded, then steps, then stops stepping (a save): the
        // menu keeps out all along.
        set_world(true);
        menu_frame(at);
        LAST_STEP.store(now_ms().max(1), Ordering::Release);
        menu_frame(at);
        MENU_CLOCK_SKEW.store(10 * crate::at_menu::QUIET_MS, Ordering::Release);
        menu_frame(at);
        assert!(!DRIVER.lock().unwrap().as_ref().unwrap().in_room());
        assert!(hook_log().contains("menu: a world is loaded (CMenuUI::m_game set)"));
        // The world's GUI noted its company and the room's companies, and
        // the save's player; the probe's switch is the hook's own.
        lua::set_note("tpf3mp.company", "372610");
        lua::set_note("tpf3mp.companies", "372553,372610");
        lua::set_note("tpf3mp.player", "214443");
        lua::set_note("tpf3mp.probe", "1");
        menu_frame(at);
        assert_eq!(lua::noted("tpf3mp.company").as_deref(), Some("372610"));
        // The world closes, and the next one loads: still out.
        set_world(false);
        unsafe {
            std::ptr::write_volatile((at + 16) as *mut usize, 1);
        }
        menu_frame(at);
        MENU_CLOCK_SKEW.store(20 * crate::at_menu::QUIET_MS, Ordering::Release);
        menu_frame(at);
        assert!(hook_log().contains("menu: the world closed (CMenuUI::m_game cleared)"));
        // Its company is no entity of the next world (a new world's first
        // frame crashed on it, 2026-10-02): forgotten with the world, so
        // the views answer the game's own player until the next world's
        // GUI notes its own. The rest stays.
        assert_eq!(lua::noted("tpf3mp.company"), None);
        assert_eq!(lua::noted("tpf3mp.companies"), None);
        assert_eq!(lua::noted("tpf3mp.player").as_deref(), Some("214443"));
        assert_eq!(lua::noted("tpf3mp.probe").as_deref(), Some("1"));
        assert!(
            hook_log().contains("menu: the closed world's company note(s) forgotten (2)"),
            "{}",
            hook_log()
        );
        lua::set_note("tpf3mp.player", "");
        lua::set_note("tpf3mp.probe", "");
        assert!(hook_log().contains("menu: no world loaded, but the game is loading one"));
        // Nothing loads: quiet for the stretch, then the menu follows the
        // room, which begins, and loads the room's save.
        unsafe {
            std::ptr::write_volatile((at + 16) as *mut usize, 0);
        }
        menu_frame(at);
        assert_eq!(menu.run("return #LOADS"), Ok("0".into()), "not quiet yet");
        MENU_CLOCK_SKEW.store(21 * crate::at_menu::QUIET_MS, Ordering::Release);
        for _ in 0..4 {
            menu_frame(at);
        }
        let log = hook_log();
        assert!(
            log.contains("menu: back at the main menu after a world"),
            "{log}"
        );
        assert!(
            log.contains("the game is at its main menu (arrival 1)"),
            "{log}"
        );
        assert!(
            log.contains(
                "the room began at the main menu; the menu sees: back at the main menu after a world"
            ),
            "{log}"
        );
        let name = format!("tpf3mp_room_{}", std::process::id());
        assert_eq!(menu.run("return #LOADS, LOADS[1]"), Ok(format!("1|{name}")));
        assert!(DRIVER.lock().unwrap().as_ref().unwrap().in_room());
        // The loaded world comes up: out again.
        set_world(true);
        lua::menu_load_failed(String::new());
        let _ = lua::take_load_failure();
        lua::request_menu_load("again");
        menu_frame(at);
        assert_eq!(menu.run("return #LOADS"), Ok("1".into()));

        *LOG.lock().unwrap() = None;
        *DRIVER.lock().unwrap() = None;
        LAST_STEP.store(0, Ordering::Release);
        IN_ROOM.store(false, Ordering::Release);
        lua::set_in_room(false);
        crate::menu::tests::forget_all();
        let _ = lua::take_menu_load();
        lua::menu_load_failed(String::new());
        let _ = lua::take_load_failure();
        forget_menu_sight();
        let _ = std::fs::remove_dir_all(&dir);
    }

    static PRINTED: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C-unwind" fn fake_print(_l: lua::State) -> c_int {
        PRINTED.fetch_add(1, Ordering::SeqCst);
        0
    }

    #[test]
    fn a_state_that_prints_gets_the_link_after_its_print() {
        let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        lua51();
        let state = Lua::new();
        PRINT_ORIGINAL.store(fake_print as *const () as usize, Ordering::Release);
        // SAFETY: a live state, on this thread.
        let results = unsafe { print_detour(state.state()) };
        assert_eq!(results, 0);
        assert_eq!(PRINTED.load(Ordering::SeqCst), 1, "the game's print ran");
        assert_eq!(state.run("return tpf3mp_native.version"), Ok("15".into()));
        PRINT_ORIGINAL.store(0, Ordering::Release);
    }
}
