//! One compiled native ABI bundle, paired with hooks.toml.

pub use crate::build_data::{
    COMPILED_PROFILE_NAME as PROFILE_NAME, COMPILED_PROFILE_TOML as PROFILE_TOML,
};
pub const STEAM_BUILD_ID: u64 = 25533170;
pub const GAME_BUILD: u32 = 40408;

pub mod builds;
pub mod drawing;
pub mod edgewatch;
pub mod emission;
pub mod guiplayer;
pub mod industries;
pub mod junctions;
pub mod menu;
pub mod modules;
pub mod netread;
pub mod network;
pub mod order;
pub mod persons;
pub mod previewcancel;
pub mod savefast;
pub mod probe;
pub mod seeds;
pub mod simperf;
pub mod stoptool;
pub mod toolchain;
pub mod streettrace;
pub mod terrain;
pub mod ticks;
pub mod toolplayer;
pub mod townfield;
pub mod towntrace;
