//! Native mods: mods that are more than Lua, managed the way CKAN manages
//! Kerbal mods, but only from the project's signed index. D29 in
//! `docs/DECISIONS.md` approves this policy; [docs/NATIVE_MODS.md] describes
//! the design and the parts still to be built.
//!
//! A native mod (Big Maps is the first) needs code in the game: hook
//! patches behind switches, targets in the build's hook profile, a served
//! page, beside data and a Lua mod folder. Mod Hub cannot carry that (D28
//! covers Mod Hub mods alone), so native mods come through a channel of
//! their own:
//!
//! - [`index`]: the index of native mods, a JSON file signed with the
//!   project's native-mods key (Ed25519, as releases are signed, D7/D19,
//!   with the same primitives: [`signed`]). Each package pins the game
//!   builds it runs on by the executable's SHA-256, as hook profiles do,
//!   names its files with size and SHA-256, its dependencies and conflicts,
//!   the hook features it enables and whether it changes the simulation.
//! - [`features`]: the features built into this hook, by id. In this first
//!   version a package only *enables* code already in the hook and ships
//!   data, Lua and settings; signed plugin libraries are a later step the
//!   index leaves room for ([`index::Plugin`]), refused for now.
//! - [`resolve`]: which packages an install needs, in order, refusing
//!   unknown packages, builds they are not pinned to, unknown features,
//!   conflicts and dependency cycles.
//! - [`store`]: downloads (over HTTPS with the `install` feature) into the launcher's data
//!   folder, `native-mods/<id>/<version>/`, verifying every file's size and
//!   hash, and keeps a registry of every installed file for upgrades,
//!   uninstalls and rollbacks that touch nothing else.
//! - [`enabled`]: what the launcher hands the game (`TPF3MP_NATIVE_MODS`
//!   names the file) and what the hook makes of it: only what the launcher
//!   enabled, only on the build it was enabled for, only features this hook
//!   has; a simulation-changing package that cannot run refuses
//!   multiplayer (fail closed).
//! - [`terms`]: the simulation-changing packages and their settings as the
//!   room's terms: what every member's game must run alike.
//!
//! [docs/NATIVE_MODS.md]: ../../docs/NATIVE_MODS.md

pub mod enabled;
pub mod features;
pub mod fetch;
pub mod index;
pub mod registry;
pub mod resolve;
pub mod signed;
pub mod store;
pub mod terms;

/// The folder in the launcher's data folder that holds native mods.
pub const FOLDER: &str = "native-mods";
