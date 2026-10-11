//! Seeds: the randomness in Transport Fever 3 that is not a function of the
//! room's state, made one (docs/HOOKS.md, "Seeds, as built";
//! investigation/TPF3_RNG_2026-09-29.md, items 5 to 7).
//!
//! Lockstep needs every replica to draw the same random numbers at the same
//! step. TF3's simulation seeds its generators from game state, with three
//! exceptions the survey found, each a piece here, each independent of the
//! others, each failing closed on its own with its reason in `hook.log`:
//!
//! 1. **The game scripts' `math.random`** is the engine's own `mt19937`,
//!    one per `lua::State`, seeded with the constant 5489 when the state is
//!    created and never saved. The game runs its game scripts on a pool of
//!    such states, one per worker thread of its job pool (with eight or more
//!    hardware threads the engine's shared pool; its own "GameScriptSystem
//!    Pool", of half the threads, only below eight), so which state, and so
//!    which stream, a script's `update` draws from depends on how that
//!    update's jobs were scheduled, which no two machines share. Measured in
//!    a room of three games that loaded one save: `reforestation.script.tl`
//!    planted its first trees at steps 360, 360 and 364, and every build
//!    after that at other steps in each game. So the hook reseeds the state
//!    about to run a script, on the worker thread about to run it, right
//!    before each call: the engine hands that state to a functor first (the
//!    ones `game_script_util::Update`, `PostUpdate` and `HandleEvent` make,
//!    [`UPDATE_CALL_TARGET`], [`POST_UPDATE_CALL_TARGET`] and
//!    [`EVENT_CALL_TARGET`]), and the detour there calls
//!    `math.randomseed(script_seed(step, call, entity))` in it: the room's
//!    step of the update running, which callback, and the game script's
//!    entity. Every script then draws the same numbers at the same step on
//!    every game, whichever state runs it and whatever ran in that state
//!    before. The engine does the same itself for an event that carries a
//!    seed (`HandleEvent`'s `int const*`, through its `lua::State::
//!    RandomSeed`), and such an event keeps the engine's seed. The step
//!    comes from a detour on `ecs::Engine::Update` (the per-update advance,
//!    the one call `GameSim::Step` makes per update, before the systems and
//!    so before `ecs::GameScriptSystem::Update` runs the scripts): per
//!    update, not per call of the game's step, since a call runs a batch of
//!    updates and batches differ per machine. Outside the room's released
//!    updates no step is current and nothing is reseeded: the game's own
//!    randomness, as before.
//! 2. **`TownDevelopAt`** seeds its town developer from the CRT `rand()`,
//!    which the game seeds from the wall clock. The player's
//!    `makeTownDevelopAtCmd` is refused in a room already (the mod's
//!    `guard.lua`, docs/HOOKS.md "The player's commands"); the
//!    alternative for later is here behind [`TOWN_DEVELOP_RESEED`]: a
//!    detour on `TownDevelopAt::Apply` that calls the CRT's `srand` on the
//!    applying thread with a seed from the step and the command's number
//!    within the step, right before the original runs.
//! 3. **The CRT's transcendental dispatch** (`sinf`, `cosf`, ... pick an
//!    FMA3 or an SSE2 body at run time by CPU) is a measurement, not a fix:
//!    [`cpu_report`] goes to `hook.log` at bootstrap so two replicas' logs
//!    say whether their CRTs took the same path.
//!
//! The seed is [`seed_for`]: a `splitmix64` mix of the step and a salt,
//! folded to `1..=0x7fff_ffff` (an `mt19937` takes a 32-bit seed, the
//! engine's `math.randomseed` reads an integer, and `minstd_rand` treats 0
//! as 1, so the range suits every generator here). A script call's salt is
//! [`script_salt`]: the call's kind and the script's entity, never the state
//! or the thread, which differ per machine.
//!
//! The detours that hand control to this module are assembly thunks: they
//! save the four argument registers and `xmm0`-`xmm3` (the targets take
//! floats in them: `ecs::Engine::Update(engine, float dt)`), call the
//! Rust side, restore, and jump to the original's trampoline. So nothing
//! here assumes a target's signature, and the original sees its stack and
//! registers exactly as its caller left them.

#![allow(unsafe_code)]
// Elsewhere the detours are not installed, so their code is unused there.
#![cfg_attr(not(all(windows, target_arch = "x86_64")), allow(dead_code))]

use std::{
    ffi::{c_char, c_int, c_void},
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use tpf3mp_hookcore::profile::ResolvedProfile;

use crate::{
    log,
    lua::{self, State},
    step::Updates,
};

/// The profile target where a game script's `update` gets its state: the
/// call of the functor `game_script_util::Update` makes (`_Do_call(this,
/// lua::State*& rdx, GameScriptData& r8)`), on the thread and with the state
/// that then run the script's `update`. The script's entity is at
/// [`UPDATE_ENTITY`] in the functor.
pub const UPDATE_CALL_TARGET: &str = "game_script_util::Update/lambda_1::_Do_call";
/// As [`UPDATE_CALL_TARGET`], for `postUpdate`: the functor
/// `game_script_util::PostUpdate` makes (the same code serves
/// `HandleApplyCommandBuildProposal`'s), the entity at [`POST_UPDATE_ENTITY`].
pub const POST_UPDATE_CALL_TARGET: &str = "game_script_util::PostUpdate/lambda_1::_Do_call";
/// As [`UPDATE_CALL_TARGET`], for `handleEvent`: the operator that
/// `game_script_util::HandleEvent`'s inner functor calls (`(captures,
/// lua::State* rdx, GameScriptData& r8)`), the entity at [`EVENT_ENTITY`]
/// in the captures and the event's own seed, when it has one, behind
/// [`EVENT_SEED`].
pub const EVENT_CALL_TARGET: &str = "game_script_util::HandleEvent/lambda_1/lambda_1::operator()";
/// The profile target for the per-update advance: `ecs::Engine::Update
/// (engine, float dt)`, the one call `GameSim::Step` makes for each update
/// after applying that update's commands, before any system runs.
pub const UPDATE_TARGET: &str = "ecs::Engine::Update";
/// The profile target for the `TownDevelopAt` command's applier.
pub const TOWN_DEVELOP_TARGET: &str = "TownDevelopAt::Apply";

/// Whether the `TownDevelopAt::Apply` detour is installed. `false` until the
/// reseed is measured in a room: the command is refused at
/// `CommandList::Add` meanwhile, so a player's cannot diverge the worlds,
/// and a game script's (none in the base game sends one) would diverge on
/// its `rand()` seed exactly as the survey says. Flip it once a room has
/// shown, in the `t` lane, that a reseeded `TownDevelopAt` develops the
/// same town everywhere; the seed under batching is documented at
/// [`Batch::town_apply`].
pub const TOWN_DEVELOP_RESEED: bool = false;

/// Set to `0` (or `off`) in the game's environment, the game scripts'
/// calls (update, postUpdate, handleEvent) are not detoured and not
/// reseeded: the kill switch, for timing the reseed against the game
/// (docs/HOOKS.md, "What the hook costs"). The per-update detour stays, so
/// the mod's own `tpf3mp_native.seed` still knows the step.
pub const SCRIPT_RESEED_ENV: &str = "TPF3MP_HOOK_SCRIPT_RESEED";

/// The salt of the game scripts' seeds, before the call's kind and the
/// script's entity ([`script_salt`]).
pub const GAME_SCRIPT_SALT: u32 = 0;
/// The salt of the `TownDevelopAt` seed (`"town"`), plus the command's
/// number within its step.
pub const TOWN_DEVELOP_SALT: u32 = 0x746f_776e;

pub use crate::build_data::native::seeds::EVENT_ENTITY;
pub use crate::build_data::native::seeds::EVENT_SEED;
pub use crate::build_data::native::seeds::POST_UPDATE_ENTITY;
pub use crate::build_data::native::seeds::UPDATE_ENTITY;

/// After the first few calls of each kind, the reseed goes to the log once
/// per this many steps.
const LOG_FIRST: u64 = 3;
const LOG_EVERY: u64 = 1_000;

/// The seed every replica uses at `step` for the generator `salt` names:
/// `splitmix64(step ^ (salt << 32))`, folded to `1..=0x7fff_ffff`.
pub fn seed_for(step: u64, salt: u32) -> u32 {
    let mut z = step ^ (u64::from(salt) << 32);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // The high bits, into 1..=2^31-1.
    ((z >> 33) as u32) % 0x7fff_fffe + 1
}

/// Which of a game script's callbacks is about to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptCall {
    Update,
    PostUpdate,
    Event,
}

impl ScriptCall {
    /// Its part of the salt (`"updt"`, `"post"`, `"evnt"`).
    pub const fn salt(self) -> u32 {
        match self {
            Self::Update => 0x7570_6474,
            Self::PostUpdate => 0x706f_7374,
            Self::Event => 0x6576_6e74,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::PostUpdate => "postUpdate",
            Self::Event => "handleEvent",
        }
    }
}

/// A script call's salt: the game scripts' salt, the call's kind and the
/// script's entity, which every replica of the room shares (one save, one
/// numbering).
pub fn script_salt(call: ScriptCall, entity: u32) -> u32 {
    GAME_SCRIPT_SALT ^ call.salt() ^ entity.wrapping_mul(0x9e37_79b1)
}

/// The seed a script call gets at `step`.
pub fn script_seed(step: u64, call: ScriptCall, entity: u32) -> u32 {
    seed_for(step, script_salt(call, entity))
}

/// What the per-call reseed does with a call: the seed for the update
/// running now (`current`), or nothing outside the room's released updates
/// or for an event the engine seeds itself.
pub fn call_seed(
    current: Option<u64>,
    call: ScriptCall,
    entity: u32,
    engine_seeded: bool,
) -> Option<u32> {
    let step = current?;
    if engine_seeded {
        return None;
    }
    Some(script_seed(step, call, entity))
}

// ---------- the batch of updates one call of the game's step runs ----------

/// The updates one call of the game's step runs, as the step driver armed
/// them: the room's step of the first, how many, and how many have begun.
/// Pure; the statics below wrap it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Batch {
    base: u64,
    count: u32,
    index: u32,
    /// `TownDevelopAt` commands applied in the update about to run.
    town_applies: u32,
}

impl Batch {
    /// Arms the batch: `next_step` and `updates` as the driver has them. No
    /// step known, the game's own speed, or no update disarms it, and
    /// nothing is reseeded until the driver arms again. Answers how many
    /// calls of `ecs::Engine::Update` the previous batch saw against how
    /// many it expected, when they differ.
    pub fn arm(&mut self, next_step: Option<u64>, updates: Updates) -> Option<(u32, u32)> {
        let mismatch =
            (self.count > 0 && self.index != self.count).then_some((self.index, self.count));
        *self = match (next_step, updates) {
            (Some(step), Updates::Exactly(count)) if count > 0 => Self {
                base: step,
                count,
                index: 0,
                town_applies: 0,
            },
            _ => Self::default(),
        };
        mismatch
    }

    /// Whether the driver armed a batch that has updates left.
    pub fn armed(&self) -> bool {
        self.count > 0 && self.index < self.count
    }

    /// An update begins: its step, or `None` when the batch is not armed
    /// or the game runs more updates than the driver released (counted,
    /// so the next `arm` can report it; never reseeded, the room did not
    /// release the step).
    pub fn next_update(&mut self) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let index = self.index;
        self.index = self.index.saturating_add(1);
        if index >= self.count {
            return None;
        }
        self.town_applies = 0;
        Some(self.base + u64::from(index))
    }

    /// A `TownDevelopAt` is applied: the step it belongs to and its number
    /// within that step, or `None` outside an armed batch. Commands for an
    /// update are applied just before that update's `ecs::Engine::Update`
    /// (the game's step, `GameSim.cpp`), so the step is the one the next
    /// update runs.
    pub fn town_apply(&mut self) -> Option<(u64, u32)> {
        if !self.armed() {
            return None;
        }
        let number = self.town_applies;
        self.town_applies = self.town_applies.saturating_add(1);
        Some((self.base + u64::from(self.index), number))
    }
}

static BATCH: Mutex<Batch> = Mutex::new(Batch {
    base: 0,
    count: 0,
    index: 0,
    town_applies: 0,
});

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Logs `message` once per process for `flag`.
fn once(flag: &AtomicBool, message: &str) {
    if !flag.swap(true, Ordering::Relaxed) {
        log::line(message);
    }
}

static UPDATE_HOOKED: AtomicBool = AtomicBool::new(false);
/// Every `ecs::Engine::Update` call seen, for checking what a call of the
/// game's step really ran ([`updates_seen`]).
static UPDATES_SEEN: AtomicU64 = AtomicU64::new(0);

/// The simulation updates the game ran so far, counted by the per-update
/// detour; `None` without it. Its difference across a call of the game's
/// step is what the call ran, whatever the step was asked: the game's
/// pending debug steps (`makeGamePerformSimulationStepsCmd`, the native
/// simulation panel) can add updates the room never released.
pub fn updates_seen() -> Option<u64> {
    update_hooked().then(|| UPDATES_SEEN.load(Ordering::Acquire))
}

/// Whether the shared simulation-update detour is already installed.
pub(crate) fn update_hooked() -> bool {
    UPDATE_HOOKED.load(Ordering::Acquire)
}
static MISMATCH_LOGGED: AtomicBool = AtomicBool::new(false);
static NO_API_LOGGED: AtomicBool = AtomicBool::new(false);
static EXTRA_UPDATE_LOGGED: AtomicBool = AtomicBool::new(false);
static TOWN_OUTSIDE_LOGGED: AtomicBool = AtomicBool::new(false);
static UNREADABLE_LOGGED: AtomicBool = AtomicBool::new(false);
static FAILED_LOGGED: AtomicBool = AtomicBool::new(false);

/// The room's step of the update running now, plus one; 0 when none is
/// (between the driver's batches, outside the room, an update the room did
/// not release). Written on the simulation thread before each update, read
/// by the worker threads that run that update's scripts.
static CURRENT_STEP: AtomicU64 = AtomicU64::new(0);
/// Script calls reseeded, per kind (update, postUpdate, handleEvent).
static RESEEDS: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
/// The last step a periodic reseed line was written for.
static LOGGED_STEP: AtomicU64 = AtomicU64::new(0);

/// The room's step of the update running now.
pub fn current_step() -> Option<u64> {
    CURRENT_STEP.load(Ordering::Acquire).checked_sub(1)
}

/// Scope nested script events to the ordered command's step, without
/// starting an Engine::Update or advancing either game-time counter.
pub(crate) fn command_step(step: Option<u64>) -> Option<u64> {
    CURRENT_STEP
        .swap(step.map_or(0, |s| s + 1), Ordering::AcqRel)
        .checked_sub(1)
}

/// From the step driver, on the simulation thread, right before it runs
/// the game's step: the room's next step and the updates this call runs.
/// Arms the per-update reseed for exactly those updates; anything else
/// (no step known yet, the game's own speed, the paused path) disarms it.
pub fn before_updates(next_step: Option<u64>, updates: Updates) {
    CURRENT_STEP.store(0, Ordering::Release);
    let mismatch = lock(&BATCH).arm(next_step, updates);
    if let Some((saw, expected)) = mismatch
        && UPDATE_HOOKED.load(Ordering::Relaxed)
    {
        once(
            &MISMATCH_LOGGED,
            &format!(
                "seeds: a batch of {expected} updates saw {saw} calls of {UPDATE_TARGET}; the per-update reseed does not match the game's updates (measure before trusting it)"
            ),
        );
    }
}

// ---------- the reseed ----------

/// A Lua type's code, as `lua_type` returns it.
const LUA_TTABLE: c_int = 5;
const LUA_TFUNCTION: c_int = 6;

/// `lua_pcall` as the reseed calls it: Lua 5.1's own, or 5.2's `lua_pcallk`
/// with no continuation.
#[derive(Clone, Copy)]
pub enum PCall {
    Lua51(unsafe extern "C-unwind" fn(State, c_int, c_int, c_int) -> c_int),
    Lua52(unsafe extern "C-unwind" fn(State, c_int, c_int, c_int, isize, *const c_void) -> c_int),
}

/// The few functions of Lua's C API the reseed calls. The link to the mod
/// ([`crate::lua`]) calls no Lua code, so its API has no `pcall`; the
/// reseed resolves its own from the profile ([`install`]).
#[derive(Clone, Copy)]
pub struct SeedApi {
    pub gettop: unsafe extern "C-unwind" fn(State) -> c_int,
    pub settop: unsafe extern "C-unwind" fn(State, c_int),
    pub checkstack: unsafe extern "C-unwind" fn(State, c_int) -> c_int,
    pub type_of: unsafe extern "C-unwind" fn(State, c_int) -> c_int,
    pub rawgeti: unsafe extern "C-unwind" fn(State, c_int, c_int),
    pub getfield: unsafe extern "C-unwind" fn(State, c_int, *const c_char),
    pub pushnumber: unsafe extern "C-unwind" fn(State, f64),
    pub pcall: PCall,
    pub globals: lua::Globals,
}

static SEED_API: OnceLock<SeedApi> = OnceLock::new();

/// Makes `api` the one the reseed uses; the first one stays.
pub fn install_seed_api(api: SeedApi) -> bool {
    SEED_API.set(api).is_ok()
}

fn type_name(code: c_int) -> &'static str {
    match code {
        0 => "nil",
        1 => "a boolean",
        3 => "a number",
        4 => "a string",
        LUA_TTABLE => "a table",
        LUA_TFUNCTION => "a function",
        -1 => "no value",
        _ => "another type",
    }
}

/// Calls `math.randomseed(seed)` in `state`, leaving the stack as it was.
/// Errs, with the stack restored, when `math` or `math.randomseed` is not
/// there or the call raises.
///
/// # Safety
/// `state` is a live Lua state that no other thread is running, and `api`
/// is the API of the Lua it belongs to.
pub unsafe fn reseed_state(api: &SeedApi, state: State, seed: u32) -> Result<(), String> {
    // SAFETY: the caller's contract.
    unsafe {
        let base = (api.gettop)(state);
        let outcome = (|| {
            if (api.checkstack)(state, 4) == 0 {
                return Err("the Lua stack has no room".to_owned());
            }
            match api.globals {
                lua::Globals::Registry { index, key } => {
                    (api.rawgeti)(state, index, key);
                    (api.getfield)(state, -1, c"math".as_ptr());
                }
                lua::Globals::Pseudo(index) => (api.getfield)(state, index, c"math".as_ptr()),
            }
            let math = (api.type_of)(state, -1);
            if math != LUA_TTABLE {
                return Err(format!("math is {}, not a table", type_name(math)));
            }
            (api.getfield)(state, -1, c"randomseed".as_ptr());
            let randomseed = (api.type_of)(state, -1);
            if randomseed != LUA_TFUNCTION {
                return Err(format!(
                    "math.randomseed is {}, not a function",
                    type_name(randomseed)
                ));
            }
            (api.pushnumber)(state, f64::from(seed));
            let status = match api.pcall {
                PCall::Lua51(pcall) => pcall(state, 1, 0, 0),
                PCall::Lua52(pcallk) => pcallk(state, 1, 0, 0, 0, std::ptr::null()),
            };
            if status != 0 {
                return Err(format!("math.randomseed raised (status {status})"));
            }
            Ok(())
        })();
        (api.settop)(state, base);
        outcome
    }
}

/// A game script is about to run in the state whose `lua::State` object is
/// at `state_object` (its first word the `lua_State*`), on this thread:
/// reseed that state's `math.random` for the update running now, the call
/// and the script. Nothing outside the room's released updates, nor for an
/// event the engine seeds itself.
fn before_script_call(state_object: usize, call: ScriptCall, entity: u32, engine_seeded: bool) {
    let Some(seed) = call_seed(current_step(), call, entity, engine_seeded) else {
        return;
    };
    let Some(api) = SEED_API.get() else {
        once(
            &NO_API_LOGGED,
            "seeds: the profile lacks the Lua functions the reseed calls, so the game scripts are not reseeded",
        );
        return;
    };
    if !crate::image::readable_cached(state_object, std::mem::size_of::<usize>()) {
        once(
            &UNREADABLE_LOGGED,
            &format!(
                "seeds: a game script's {} ran with an unreadable lua::State {state_object:#x}; that call is not reseeded",
                call.name()
            ),
        );
        return;
    }
    // SAFETY: the word at `state_object` is readable, checked above.
    let state = unsafe { std::ptr::read_unaligned(state_object as *const usize) };
    if state == 0 {
        return;
    }
    let step = current_step().unwrap_or_default();
    // SAFETY: the state the engine is about to run this script in, handed
    // to the call's functor on this thread; nothing else runs it until the
    // call returns (the engine runs one script at a time per worker state).
    match unsafe { reseed_state(api, state as State, seed) } {
        Ok(()) => {
            let n = RESEEDS[call as usize].fetch_add(1, Ordering::Relaxed) + 1;
            let periodic =
                step.is_multiple_of(LOG_EVERY) && LOGGED_STEP.swap(step, Ordering::Relaxed) != step;
            if n <= LOG_FIRST || periodic {
                log::line(&format!(
                    "seeds: step {step}: math.randomseed({seed}) before the {} of script entity {entity} (state {state:#x}; {} update, {} postUpdate, {} handleEvent calls reseeded so far)",
                    call.name(),
                    RESEEDS[0].load(Ordering::Relaxed),
                    RESEEDS[1].load(Ordering::Relaxed),
                    RESEEDS[2].load(Ordering::Relaxed),
                ));
            }
        }
        Err(reason) => once(
            &FAILED_LOGGED,
            &format!(
                "seeds: step {step}: the {} of script entity {entity} was not reseeded: {reason} (said once)",
                call.name()
            ),
        ),
    }
}

/// The seed for the mod's own game script's `math.randomseed` in the
/// running update (`tpf3mp_native.seed`, asked at the start of its
/// `update`): the room step's, or `None` outside the room's steps. The
/// per-call reseed above already seeds every script's call in the state
/// that runs it, the mod's own included, so this adds nothing to it; it
/// stays for the mod's call. Neither calls into a Lua state the hook would
/// have to know is still alive: an earlier cut reseeded a roster of states
/// its registrar detour had seen, and after a rebase that roster held states
/// the world load had freed, so the next reseed crashed the game in
/// `lua_getfield` (2026-09-30, three-player playtest).
pub fn current_seed() -> Option<u32> {
    current_step().map(|step| seed_for(step, GAME_SCRIPT_SALT))
}

/// An update of the simulation begins (the `ecs::Engine::Update` detour,
/// on the simulation thread): its step becomes the current one, for the
/// script calls it runs.
fn before_update() {
    UPDATES_SEEN.fetch_add(1, Ordering::AcqRel);
    if crate::order::measure::enabled() {
        crate::order::measure::update_begins();
    }
    let step = lock(&BATCH).next_update();
    CURRENT_STEP.store(step.map_or(0, |step| step + 1), Ordering::Release);
    if step.is_none() && lock(&BATCH).count > 0 {
        once(
            &EXTRA_UPDATE_LOGGED,
            &format!(
                "seeds: {UPDATE_TARGET} ran more often than the driver released updates; the extra updates are not reseeded"
            ),
        );
    }
}

// ---------- TownDevelopAt ----------

/// `srand` in the CRT the game's `rand()` belongs to; 0 until resolved.
static SRAND: AtomicUsize = AtomicUsize::new(0);
static TOWN_RESEEDS: AtomicU64 = AtomicU64::new(0);

type SrandFn = unsafe extern "C" fn(u32);

/// A `TownDevelopAt` is about to be applied (its detour, on the applying
/// thread): seed the CRT `rand()` this thread reads from the step and the
/// command's number within it.
fn before_town_develop() {
    let Some((step, number)) = lock(&BATCH).town_apply() else {
        once(
            &TOWN_OUTSIDE_LOGGED,
            "seeds: TownDevelopAt applied outside the room's updates; its rand() seed is the game's own",
        );
        return;
    };
    let srand = SRAND.load(Ordering::Acquire);
    if srand == 0 {
        return;
    }
    let seed = seed_for(step, TOWN_DEVELOP_SALT.wrapping_add(number));
    // SAFETY: the address GetProcAddress gave for the CRT's srand, whose
    // signature is void srand(unsigned).
    let srand: SrandFn = unsafe { std::mem::transmute::<usize, SrandFn>(srand) };
    // SAFETY: as above; the CRT keeps rand()'s state per thread, and this
    // is the thread the applier's rand() runs on.
    unsafe { srand(seed) };
    let n = TOWN_RESEEDS.fetch_add(1, Ordering::Relaxed) + 1;
    log::line(&format!(
        "seeds: TownDevelopAt #{number} at step {step}: srand({seed}) before the applier (reseed #{n})"
    ));
}

// ---------- the CPU report ----------

/// What the CPU says of itself, as far as the CRT's dispatch cares.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CpuFeatures {
    pub vendor: String,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub sse42: bool,
    pub fma: bool,
    pub movbe: bool,
    pub osxsave: bool,
    pub avx: bool,
    pub f16c: bool,
    pub bmi1: bool,
    pub avx2: bool,
    pub bmi2: bool,
    pub avx512f: bool,
    pub avx512dq: bool,
    pub avx512cd: bool,
    pub avx512bw: bool,
    pub avx512vl: bool,
    /// XCR0, the OS's enabled state components (bit 1 SSE, bit 2 AVX,
    /// bits 5-7 AVX-512); 0 without `osxsave`.
    pub xcr0: u64,
}

impl CpuFeatures {
    /// The `__isa_available` level the Microsoft C runtime would pick for
    /// this CPU, by the rule its start-up (`__isa_available_init`) is
    /// documented to apply: 0 x86, 1 SSE2, 2 SSE4.2, 3 AVX, 4 AVX2 (with
    /// FMA3 and BMI), 5 AVX-512 (F, CD, BW, DQ and VL). An estimate from
    /// the same `cpuid` bits, not a read of the variable (it is not
    /// exported, and the survey named no profile target for it): what
    /// matters for comparing two replicas is that the bits are logged.
    pub fn isa_level(&self) -> (u8, &'static str) {
        let os_avx = self.osxsave && self.xcr0 & 0x6 == 0x6;
        let os_avx512 = self.osxsave && self.xcr0 & 0xe6 == 0xe6;
        if os_avx512
            && self.avx512f
            && self.avx512cd
            && self.avx512bw
            && self.avx512dq
            && self.avx512vl
        {
            (5, "AVX-512")
        } else if os_avx && self.avx2 && self.fma && self.bmi1 && self.bmi2 {
            (4, "AVX2")
        } else if os_avx && self.avx {
            (3, "AVX")
        } else if self.sse42 {
            (2, "SSE4.2")
        } else {
            (1, "SSE2")
        }
    }

    /// Whether the CRT's `sinf`, `cosf`, `expf`, `logf`, `powf` and the
    /// double versions would run their FMA3 bodies: they do from the AVX2
    /// level up (the UCRT's `_set_FMA3_enable` default).
    pub fn fma3_math(&self) -> bool {
        self.isa_level().0 >= 4
    }

    /// One log line.
    pub fn report(&self) -> String {
        let (level, name) = self.isa_level();
        format!(
            "cpu: {} family {:#x} model {:#x} stepping {}; sse4.2 {} fma {} movbe {} avx {} f16c {} bmi1 {} avx2 {} bmi2 {} avx512 f {} cd {} bw {} dq {} vl {}; xcr0 {:#x}; estimated CRT __isa_available {level} ({name}); CRT math on FMA3 bodies: {}",
            self.vendor,
            self.family,
            self.model,
            self.stepping,
            self.sse42,
            self.fma,
            self.movbe,
            self.avx,
            self.f16c,
            self.bmi1,
            self.avx2,
            self.bmi2,
            self.avx512f,
            self.avx512cd,
            self.avx512bw,
            self.avx512dq,
            self.avx512vl,
            self.xcr0,
            if self.fma3_math() { "yes" } else { "no" }
        )
    }
}

/// The running CPU's features, from `cpuid`.
#[cfg(target_arch = "x86_64")]
pub fn cpu_features() -> CpuFeatures {
    use std::arch::x86_64::{__cpuid, __cpuid_count};
    // cpuid is available on every x86-64 CPU; leaf 1 always exists.
    let leaf0 = __cpuid(0);
    let mut vendor = Vec::with_capacity(12);
    for word in [leaf0.ebx, leaf0.edx, leaf0.ecx] {
        vendor.extend_from_slice(&word.to_le_bytes());
    }
    let vendor = String::from_utf8_lossy(&vendor).trim().to_owned();
    let max_leaf = leaf0.eax;
    let leaf1 = __cpuid(1);
    let leaf7 = if max_leaf >= 7 {
        __cpuid_count(7, 0)
    } else {
        std::arch::x86_64::CpuidResult {
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
        }
    };
    let bit = |word: u32, n: u32| word & (1 << n) != 0;
    let family_id = (leaf1.eax >> 8) & 0xf;
    let model_id = (leaf1.eax >> 4) & 0xf;
    let family = if family_id == 0xf {
        family_id + ((leaf1.eax >> 20) & 0xff)
    } else {
        family_id
    };
    let model = if family_id == 0xf || family_id == 0x6 {
        model_id + (((leaf1.eax >> 16) & 0xf) << 4)
    } else {
        model_id
    };
    let osxsave = bit(leaf1.ecx, 27);
    let xcr0 = if osxsave {
        // SAFETY: OSXSAVE set means the OS enabled XGETBV.
        unsafe { xgetbv0() }
    } else {
        0
    };
    CpuFeatures {
        vendor,
        family,
        model,
        stepping: leaf1.eax & 0xf,
        sse42: bit(leaf1.ecx, 20),
        fma: bit(leaf1.ecx, 12),
        movbe: bit(leaf1.ecx, 22),
        osxsave,
        avx: bit(leaf1.ecx, 28),
        f16c: bit(leaf1.ecx, 29),
        bmi1: bit(leaf7.ebx, 3),
        avx2: bit(leaf7.ebx, 5),
        bmi2: bit(leaf7.ebx, 8),
        avx512f: bit(leaf7.ebx, 16),
        avx512dq: bit(leaf7.ebx, 17),
        avx512cd: bit(leaf7.ebx, 28),
        avx512bw: bit(leaf7.ebx, 30),
        avx512vl: bit(leaf7.ebx, 31),
        xcr0,
    }
}

/// `xgetbv(0)`.
///
/// # Safety
/// The OS must have set `CR4.OSXSAVE` (the `osxsave` cpuid bit).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "xsave")]
unsafe fn xgetbv0() -> u64 {
    // SAFETY: the caller's contract.
    unsafe { std::arch::x86_64::_xgetbv(0) }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn cpu_features() -> CpuFeatures {
    CpuFeatures {
        vendor: format!("not x86-64 ({})", std::env::consts::ARCH),
        ..CpuFeatures::default()
    }
}

/// The CPU line for `hook.log`.
pub fn cpu_report() -> String {
    cpu_features().report()
}

// ---------- installing ----------

/// Installs the pieces, each on its own, logging each outcome. Windows x64
/// only, as the step gate is.
pub fn install(resolved: &ResolvedProfile) {
    log::line(&cpu_report());
    match seed_api(resolved) {
        Ok(api) => {
            install_seed_api(api);
        }
        Err(missing) => log::line(&format!(
            "seeds: the profile lacks {missing}; the game-script states are not reseeded"
        )),
    }
    native::install(resolved);
}

/// The reseed's Lua 5.2 API from the profile's targets, or the first name
/// missing.
#[allow(clippy::missing_transmute_annotations)]
fn seed_api(resolved: &ResolvedProfile) -> Result<SeedApi, &'static str> {
    macro_rules! function {
        ($name:literal) => {{
            let address = resolved
                .get($name)
                .map(|target| target.address as usize)
                .filter(|address| *address != 0)
                .ok_or($name)?;
            // SAFETY: the profile resolved this function of Lua 5.2's C API
            // by its signature and prologue in the running build; the type
            // is the field's, that function's signature.
            unsafe { std::mem::transmute::<usize, _>(address) }
        }};
    }
    Ok(SeedApi {
        gettop: function!("lua_gettop"),
        settop: function!("lua_settop"),
        checkstack: function!("lua_checkstack"),
        type_of: function!("lua_type"),
        rawgeti: function!("lua_rawgeti"),
        getfield: function!("lua_getfield"),
        pushnumber: function!("lua_pushnumber"),
        pcall: PCall::Lua52(function!("lua_pcallk")),
        globals: lua::LUA52_GLOBALS,
    })
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod native {
    use std::panic::catch_unwind;

    use super::*;

    /// The trampolines to the originals; 0 until installed. The thunks
    /// read them.
    static UPDATE_CALL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static POST_UPDATE_CALL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static EVENT_CALL_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static UPDATE_ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static TOWN_DEVELOP_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

    /// A detour that keeps the target's ABI whatever it is: saves the four
    /// integer and the four floating-point argument registers, calls
    /// `$before(rcx, rdx, r8, r9)` on the Rust side, restores them and
    /// jumps to the original's trampoline with the stack exactly as the
    /// caller left it, so stack arguments and the return address are the
    /// original's. Before the trampoline is published it returns to the
    /// caller without running the original (the window inside install,
    /// while no world exists).
    macro_rules! thunk {
        ($name:ident, $before:path, $original:ident) => {
            #[unsafe(naked)]
            unsafe extern "C" fn $name() {
                core::arch::naked_asm!(
                    "cmp qword ptr [rip + {orig}], 0",
                    "je 2f",
                    // 0x20 shadow space for the call, 4 register slots,
                    // 4 xmm slots; entry rsp is 8 mod 16, so this makes it
                    // 16-aligned for the call.
                    "sub rsp, 0x88",
                    "mov [rsp + 0x20], rcx",
                    "mov [rsp + 0x28], rdx",
                    "mov [rsp + 0x30], r8",
                    "mov [rsp + 0x38], r9",
                    "movdqu [rsp + 0x40], xmm0",
                    "movdqu [rsp + 0x50], xmm1",
                    "movdqu [rsp + 0x60], xmm2",
                    "movdqu [rsp + 0x70], xmm3",
                    "call {before}",
                    "movdqu xmm3, [rsp + 0x70]",
                    "movdqu xmm2, [rsp + 0x60]",
                    "movdqu xmm1, [rsp + 0x50]",
                    "movdqu xmm0, [rsp + 0x40]",
                    "mov r9, [rsp + 0x38]",
                    "mov r8, [rsp + 0x30]",
                    "mov rdx, [rsp + 0x28]",
                    "mov rcx, [rsp + 0x20]",
                    "add rsp, 0x88",
                    "jmp qword ptr [rip + {orig}]",
                    "2:",
                    "ret",
                    before = sym $before,
                    orig = sym $original,
                )
            }
        };
    }

    thunk!(
        update_call_thunk,
        before_update_call_c,
        UPDATE_CALL_ORIGINAL
    );
    thunk!(
        post_update_call_thunk,
        before_post_update_call_c,
        POST_UPDATE_CALL_ORIGINAL
    );
    thunk!(event_call_thunk, before_event_call_c, EVENT_CALL_ORIGINAL);
    thunk!(update_thunk, before_update_c, UPDATE_ORIGINAL);
    thunk!(
        town_develop_thunk,
        before_town_develop_c,
        TOWN_DEVELOP_ORIGINAL
    );

    /// A word of the game's memory, if it is readable.
    fn word(address: usize) -> Option<usize> {
        if address == 0 || !crate::image::readable_cached(address, std::mem::size_of::<usize>()) {
            return None;
        }
        // SAFETY: readable, checked just above; read by value, unaligned.
        Some(unsafe { std::ptr::read_unaligned(address as *const usize) })
    }

    /// The script's entity, an `int` at `offset` in the call's functor.
    fn entity(functor: usize, offset: usize) -> Option<u32> {
        let address = functor.checked_add(offset)?;
        if !crate::image::readable_cached(address, std::mem::size_of::<u32>()) {
            return None;
        }
        // SAFETY: as in `word`.
        Some(unsafe { std::ptr::read_unaligned(address as *const u32) })
    }

    /// The Rust side of each thunk: never panics out (a panic would abort
    /// the game at the `extern "C"` boundary).
    ///
    /// `update`: `_Do_call(functor, lua::State*& rdx, GameScriptData& r8)`.
    extern "C" fn before_update_call_c(functor: usize, state_ref: usize, _data: usize, _d: usize) {
        let _timer = crate::perf::time(crate::perf::Piece::Reseed);
        let _ = catch_unwind(|| {
            if let (Some(state), Some(entity)) = (word(state_ref), entity(functor, UPDATE_ENTITY)) {
                before_script_call(state, ScriptCall::Update, entity, false);
            }
        });
    }

    /// `postUpdate`: as `update`.
    extern "C" fn before_post_update_call_c(
        functor: usize,
        state_ref: usize,
        _data: usize,
        _d: usize,
    ) {
        let _timer = crate::perf::time(crate::perf::Piece::Reseed);
        let _ = catch_unwind(|| {
            if let (Some(state), Some(entity)) =
                (word(state_ref), entity(functor, POST_UPDATE_ENTITY))
            {
                before_script_call(state, ScriptCall::PostUpdate, entity, false);
            }
        });
    }

    /// `handleEvent`: `operator()(captures, lua::State* rdx, GameScriptData&
    /// r8)`; an event with its own seed is the engine's to seed.
    extern "C" fn before_event_call_c(captures: usize, state: usize, _data: usize, _d: usize) {
        let _timer = crate::perf::time(crate::perf::Piece::Reseed);
        let _ = catch_unwind(|| {
            let Some(own_seed) = captures.checked_add(EVENT_SEED).and_then(word) else {
                return;
            };
            if let Some(entity) = entity(captures, EVENT_ENTITY) {
                before_script_call(state, ScriptCall::Event, entity, own_seed != 0);
            }
        });
    }

    extern "C" fn before_update_c(_engine: usize, _b: usize, _c: usize, _d: usize) {
        crate::perf::update();
        // The engine frees between updates: the readable regions are asked
        // for again.
        crate::image::invalidate();
        let _ = catch_unwind(before_update);
    }

    extern "C" fn before_town_develop_c(_a: usize, _b: usize, _c: usize, _d: usize) {
        let _ = catch_unwind(before_town_develop);
    }

    /// Detours `target` to `thunk` for the life of the game and publishes
    /// the trampoline in `original`.
    fn detour(
        resolved: &ResolvedProfile,
        target: &str,
        thunk: unsafe extern "C" fn(),
        original: &AtomicUsize,
    ) -> Result<(), String> {
        let address = resolved
            .get(target)
            .map(|target| target.address as usize as *mut u8)
            .ok_or_else(|| format!("the profile has no target {target:?}"))?;
        // SAFETY: a function of the running game the profile resolved and
        // prologue-checked, detoured while the game starts, before a world
        // exists (the quiescence rule in docs/HOOKS.md); the thunk keeps
        // any ABI (it saves and restores every argument register and
        // leaves the stack as it found it).
        let installed = unsafe {
            tpf3mp_hookcore::detour::InlineDetour::install(address, thunk as *const () as *const u8)
        }
        .map_err(|error| format!("{target}: {error}"))?;
        original.store(installed.trampoline() as usize, Ordering::SeqCst);
        std::mem::forget(installed);
        Ok(())
    }

    /// The CRT's `srand`, from the UCRT the game imports `rand` from
    /// (`api-ms-win-crt-utility-l1-1-0.dll`, which forwards to
    /// `ucrtbase.dll`), so the state seeded is the one the applier reads.
    fn resolve_srand() -> Result<usize, String> {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        for module in ["api-ms-win-crt-utility-l1-1-0.dll", "ucrtbase.dll"] {
            let wide: Vec<u16> = module.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a NUL-terminated wide string; the handle of a loaded
            // module is not owned by us.
            let handle = unsafe { GetModuleHandleW(wide.as_ptr()) };
            if handle.is_null() {
                continue;
            }
            // SAFETY: a valid module handle and a NUL-terminated name.
            let address = unsafe { GetProcAddress(handle, c"srand".as_ptr().cast::<u8>()) };
            if let Some(function) = address {
                return Ok(function as usize);
            }
        }
        Err("no loaded CRT module exports srand".to_owned())
    }

    pub(super) fn install(resolved: &ResolvedProfile) {
        install_script_reseed(resolved);
        install_srand(resolved);
    }

    fn install_script_reseed(resolved: &ResolvedProfile) {
        let wanted = crate::ticks::wanted(std::env::var(SCRIPT_RESEED_ENV).ok().as_deref());
        let calls = [
            (
                UPDATE_CALL_TARGET,
                update_call_thunk as unsafe extern "C" fn(),
                &UPDATE_CALL_ORIGINAL,
            ),
            (
                POST_UPDATE_CALL_TARGET,
                post_update_call_thunk,
                &POST_UPDATE_CALL_ORIGINAL,
            ),
            (EVENT_CALL_TARGET, event_call_thunk, &EVENT_CALL_ORIGINAL),
        ];
        if !wanted {
            log::line(&format!(
                "seeds: {SCRIPT_RESEED_ENV} says so; the game scripts' calls are not reseeded, their math.random is the game's own"
            ));
        }
        for (target, thunk, original) in calls.into_iter().filter(|_| wanted) {
            match detour(resolved, target, thunk, original) {
                Ok(()) => log::line(&format!(
                    "seeds: detour installed on {target}; each game script's call there is reseeded from the room's step in the state that runs it"
                )),
                Err(error) => log::line(&format!(
                    "seeds: no reseed at {target} ({error}); that callback's math.random is the game's own"
                )),
            }
        }
        match detour(resolved, UPDATE_TARGET, update_thunk, &UPDATE_ORIGINAL) {
            Ok(()) => {
                UPDATE_HOOKED.store(true, Ordering::SeqCst);
                log::line(&format!(
                    "seeds: detour installed on {UPDATE_TARGET}; each update's step is known to the game scripts' calls"
                ));
            }
            Err(error) => log::line(&format!(
                "seeds: no per-update hook ({error}); no step is known, so the game scripts' math.random is not reseeded"
            )),
        }
    }

    fn install_srand(resolved: &ResolvedProfile) {
        let srand = match resolve_srand() {
            Ok(address) => {
                SRAND.store(address, Ordering::SeqCst);
                format!("srand at {address:#x}")
            }
            Err(error) => format!("srand not found: {error}"),
        };
        if !TOWN_DEVELOP_RESEED {
            log::line(&format!(
                "seeds: {TOWN_DEVELOP_TARGET} reseed off (TOWN_DEVELOP_RESEED is false; the command is refused in a room); {srand}"
            ));
        } else if SRAND.load(Ordering::SeqCst) == 0 {
            log::line(&format!("seeds: {TOWN_DEVELOP_TARGET} reseed off: {srand}"));
        } else {
            match detour(
                resolved,
                TOWN_DEVELOP_TARGET,
                town_develop_thunk,
                &TOWN_DEVELOP_ORIGINAL,
            ) {
                Ok(()) => log::line(&format!(
                    "seeds: detour installed on {TOWN_DEVELOP_TARGET}; its rand() is seeded from the step ({srand})"
                )),
                Err(error) => log::line(&format!(
                    "seeds: {TOWN_DEVELOP_TARGET} reseed off ({error})"
                )),
            }
        }
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
mod native {
    use super::*;

    pub(super) fn install(_resolved: &ResolvedProfile) {
        log::line(
            "seeds: the detours are installed on Windows x64 only so far; nothing is reseeded",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Seeds, the step driver and the Lua link share CURRENT_STEP: use one
    // guard for every test that changes the process-wide step or batch.
    use crate::lua::tests::SERIAL;

    #[test]
    fn the_seed_is_a_pure_function_of_step_and_salt_in_the_mt_and_minstd_range() {
        assert_eq!(seed_for(0, 0), seed_for(0, 0));
        assert_eq!(seed_for(12_345, 7), seed_for(12_345, 7));
        assert_ne!(seed_for(1, 0), seed_for(2, 0));
        assert_ne!(seed_for(1, 0), seed_for(1, 1));
        assert_ne!(seed_for(0, 0), seed_for(u64::MAX, 0));
        for step in (0..10_000).chain([u64::MAX, u64::MAX - 1, 1 << 40]) {
            for salt in [GAME_SCRIPT_SALT, TOWN_DEVELOP_SALT, u32::MAX] {
                let seed = seed_for(step, salt);
                assert!((1..=0x7fff_ffff).contains(&seed), "{step} {salt} -> {seed}");
            }
        }
        // Pinned: every replica derives exactly this, whatever its build.
        assert_eq!(seed_for(1, GAME_SCRIPT_SALT), 1_216_681_719);
        assert_eq!(seed_for(50, GAME_SCRIPT_SALT), 1_568_932_195);
    }

    #[test]
    fn a_batch_hands_one_step_per_update_and_disarms_otherwise() {
        let mut batch = Batch::default();
        assert!(!batch.armed());
        assert_eq!(batch.next_update(), None);
        assert_eq!(batch.town_apply(), None);

        assert_eq!(batch.arm(Some(10), Updates::Exactly(3)), None);
        assert!(batch.armed());
        assert_eq!(batch.town_apply(), Some((10, 0)));
        assert_eq!(batch.town_apply(), Some((10, 1)));
        assert_eq!(batch.next_update(), Some(10));
        assert_eq!(
            batch.town_apply(),
            Some((11, 0)),
            "the count restarts per step"
        );
        assert_eq!(batch.next_update(), Some(11));
        assert_eq!(batch.next_update(), Some(12));
        assert!(!batch.armed());
        assert_eq!(
            batch.next_update(),
            None,
            "an update the room did not release gets no step"
        );
        assert_eq!(batch.town_apply(), None);
        // Four calls for three updates: the next arm says so.
        assert_eq!(batch.arm(Some(13), Updates::Exactly(1)), Some((4, 3)));
        assert_eq!(batch.next_update(), Some(13));
        // A matching batch reports nothing.
        assert_eq!(batch.arm(Some(14), Updates::Exactly(2)), None);
        assert_eq!(batch.next_update(), Some(14));
        // Fewer calls than updates is a mismatch too.
        assert_eq!(batch.arm(Some(16), Updates::Exactly(1)), Some((1, 2)));

        // Disarmed: no step known, the game's own speed, or paused.
        for (step, updates) in [
            (None, Updates::Exactly(2)),
            (Some(20), Updates::Own),
            (Some(20), Updates::Exactly(0)),
        ] {
            batch.arm(Some(1), Updates::Exactly(1));
            batch.next_update();
            batch.arm(step, updates);
            assert!(!batch.armed(), "{step:?} {updates:?}");
            assert_eq!(batch.next_update(), None);
            assert_eq!(batch.town_apply(), None);
            assert_eq!(
                batch.arm(Some(1), Updates::Exactly(1)),
                None,
                "a disarmed batch reports no mismatch"
            );
        }
    }

    #[test]
    fn a_script_calls_seed_is_its_step_call_and_entity_and_nothing_else() {
        let seed = script_seed(360, ScriptCall::Update, 5_023);
        assert_eq!(seed, script_seed(360, ScriptCall::Update, 5_023));
        assert_ne!(seed, script_seed(361, ScriptCall::Update, 5_023));
        assert_ne!(seed, script_seed(360, ScriptCall::PostUpdate, 5_023));
        assert_ne!(seed, script_seed(360, ScriptCall::Event, 5_023));
        assert_ne!(seed, script_seed(360, ScriptCall::Update, 5_024));
        assert_eq!(
            seed,
            seed_for(360, script_salt(ScriptCall::Update, 5_023)),
            "the step's seed, salted by the call"
        );
        assert!((1..=0x7fff_ffff).contains(&seed));
        // Pinned: every replica derives exactly these, whatever its build.
        assert_eq!(script_seed(1, ScriptCall::Update, 0), 883_849_443);
        assert_eq!(script_seed(360, ScriptCall::Update, 5_023), 1_108_749_812);
        assert_eq!(script_seed(360, ScriptCall::Event, 5_023), 471_669_604);
    }

    #[test]
    fn a_call_is_reseeded_only_in_a_released_update_and_not_over_the_engines_own_seed() {
        assert_eq!(call_seed(None, ScriptCall::Update, 7, false), None);
        assert_eq!(
            call_seed(Some(12), ScriptCall::Update, 7, false),
            Some(script_seed(12, ScriptCall::Update, 7))
        );
        assert_eq!(
            call_seed(Some(12), ScriptCall::Event, 7, true),
            None,
            "an event with its own seed keeps the engine's"
        );
        assert_eq!(
            call_seed(Some(12), ScriptCall::Event, 7, false),
            Some(script_seed(12, ScriptCall::Event, 7))
        );
        assert_eq!(
            call_seed(Some(0), ScriptCall::PostUpdate, 7, false),
            Some(script_seed(0, ScriptCall::PostUpdate, 7))
        );
    }

    #[test]
    fn each_released_update_offers_its_steps_seed_and_nothing_else_does() {
        let _serial = lock(&SERIAL);
        before_updates(Some(5), Updates::Exactly(2));
        before_update();
        assert_eq!(current_seed(), Some(seed_for(5, GAME_SCRIPT_SALT)));
        before_update();
        assert_eq!(current_seed(), Some(seed_for(6, GAME_SCRIPT_SALT)));
        // An update the room did not release, and the game's own speed.
        before_update();
        assert_eq!(current_seed(), None);
        before_updates(None, Updates::Own);
        before_update();
        assert_eq!(current_seed(), None);
    }

    #[test]
    fn the_current_step_is_the_released_update_running_and_none_between_batches() {
        let _serial = lock(&SERIAL);
        before_updates(None, Updates::Own);
        assert_eq!(current_step(), None);
        before_update();
        assert_eq!(current_step(), None, "no batch armed");

        before_updates(Some(5), Updates::Exactly(2));
        assert!(lock(&BATCH).armed());
        assert_eq!(current_step(), None, "the batch's updates have not begun");
        before_update();
        assert_eq!(current_step(), Some(5));
        before_update();
        assert_eq!(current_step(), Some(6));
        before_update();
        assert_eq!(current_step(), None, "an update the room did not release");

        before_updates(Some(0), Updates::Exactly(1));
        before_update();
        assert_eq!(current_step(), Some(0), "step 0 is a step");
        before_updates(Some(7), Updates::Exactly(0));
        assert!(!lock(&BATCH).armed());
        assert_eq!(current_step(), None, "the paused path");
    }

    fn avx2_cpu() -> CpuFeatures {
        CpuFeatures {
            vendor: "GenuineIntel".into(),
            sse42: true,
            fma: true,
            movbe: true,
            osxsave: true,
            avx: true,
            f16c: true,
            bmi1: true,
            avx2: true,
            bmi2: true,
            xcr0: 0x7,
            ..CpuFeatures::default()
        }
    }

    #[test]
    fn the_crt_isa_level_is_estimated_by_the_published_rule() {
        let avx2 = avx2_cpu();
        assert_eq!(avx2.isa_level(), (4, "AVX2"));
        assert!(avx2.fma3_math());

        let no_fma = CpuFeatures {
            fma: false,
            ..avx2_cpu()
        };
        assert_eq!(no_fma.isa_level(), (3, "AVX"), "AVX2 needs FMA3 and BMI");
        assert!(!no_fma.fma3_math());

        let os_without_avx = CpuFeatures {
            xcr0: 0x3,
            ..avx2_cpu()
        };
        assert_eq!(os_without_avx.isa_level(), (2, "SSE4.2"));

        let avx512 = CpuFeatures {
            avx512f: true,
            avx512cd: true,
            avx512bw: true,
            avx512dq: true,
            avx512vl: true,
            xcr0: 0xe7,
            ..avx2_cpu()
        };
        assert_eq!(avx512.isa_level(), (5, "AVX-512"));
        let avx512_os_off = CpuFeatures {
            xcr0: 0x7,
            ..avx512.clone()
        };
        assert_eq!(avx512_os_off.isa_level(), (4, "AVX2"));

        let old = CpuFeatures::default();
        assert_eq!(old.isa_level(), (1, "SSE2"));
        let sse42 = CpuFeatures {
            sse42: true,
            ..CpuFeatures::default()
        };
        assert_eq!(sse42.isa_level(), (2, "SSE4.2"));

        let report = avx2.report();
        assert!(report.contains("GenuineIntel"), "{report}");
        assert!(report.contains("__isa_available 4 (AVX2)"), "{report}");
        assert!(report.contains("FMA3 bodies: yes"), "{report}");
    }

    #[test]
    fn the_running_cpu_reports_itself() {
        let features = cpu_features();
        #[cfg(target_arch = "x86_64")]
        {
            assert!(!features.vendor.is_empty());
            assert!(features.osxsave || features.xcr0 == 0);
        }
        let report = features.report();
        assert!(report.starts_with("cpu: "), "{report}");
        assert_eq!(cpu_report(), report);
    }

    mod lua51 {
        use std::ffi::{c_char, c_int};

        use mlua::ffi;

        use crate::lua::State;

        pub unsafe extern "C-unwind" fn gettop(l: State) -> c_int {
            unsafe { ffi::lua_gettop(l.cast()) }
        }
        pub unsafe extern "C-unwind" fn settop(l: State, index: c_int) {
            unsafe { ffi::lua_settop(l.cast(), index) }
        }
        pub unsafe extern "C-unwind" fn checkstack(l: State, n: c_int) -> c_int {
            unsafe { ffi::lua_checkstack(l.cast(), n) }
        }
        pub unsafe extern "C-unwind" fn type_of(l: State, index: c_int) -> c_int {
            unsafe { ffi::lua_type(l.cast(), index) }
        }
        pub unsafe extern "C-unwind" fn rawgeti(l: State, index: c_int, n: c_int) {
            unsafe { ffi::lua_rawgeti_(l.cast(), index, n) }
        }
        pub unsafe extern "C-unwind" fn getfield(l: State, index: c_int, k: *const c_char) {
            unsafe { ffi::lua_getfield_(l.cast(), index, k) }
        }
        pub unsafe extern "C-unwind" fn pushnumber(l: State, n: f64) {
            unsafe { ffi::lua_pushnumber(l.cast(), n) }
        }
        pub unsafe extern "C-unwind" fn pcall(l: State, n: c_int, r: c_int, f: c_int) -> c_int {
            unsafe { ffi::lua_pcall(l.cast(), n, r, f) }
        }
    }

    fn seed_api51() -> SeedApi {
        SeedApi {
            gettop: lua51::gettop,
            settop: lua51::settop,
            checkstack: lua51::checkstack,
            type_of: lua51::type_of,
            rawgeti: lua51::rawgeti,
            getfield: lua51::getfield,
            pushnumber: lua51::pushnumber,
            pcall: PCall::Lua51(lua51::pcall),
            globals: lua::Globals::Pseudo(mlua::ffi::LUA_GLOBALSINDEX),
        }
    }

    /// The reseed calls the state's own `math.randomseed` with the step's
    /// seed and leaves the stack as it was; a state without it is refused
    /// with the reason, the stack restored too.
    #[test]
    fn the_reseed_calls_math_randomseed_with_the_steps_seed() {
        let api = seed_api51();
        let lua = crate::lua::tests::Lua::new();
        lua.run("SEEN = nil; math.randomseed = function(n) SEEN = n end")
            .unwrap();
        let seed = seed_for(42, GAME_SCRIPT_SALT);
        let top = unsafe { (api.gettop)(lua.state()) };
        unsafe { reseed_state(&api, lua.state(), seed) }.unwrap();
        assert_eq!(unsafe { (api.gettop)(lua.state()) }, top);
        assert_eq!(lua.run("return SEEN").unwrap(), seed.to_string());

        lua.run("math = nil").unwrap();
        let refused = unsafe { reseed_state(&api, lua.state(), seed) }.unwrap_err();
        assert!(refused.contains("math is nil"), "{refused}");
        assert_eq!(unsafe { (api.gettop)(lua.state()) }, top);

        lua.run("math = { randomseed = function() error('no') end }")
            .unwrap();
        let raised = unsafe { reseed_state(&api, lua.state(), seed) }.unwrap_err();
        assert!(raised.contains("raised"), "{raised}");
        assert_eq!(unsafe { (api.gettop)(lua.state()) }, top);
    }
}
