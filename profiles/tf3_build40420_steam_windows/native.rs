//! One compiled native ABI bundle, paired with hooks.toml.

pub use crate::build_data::{
    COMPILED_PROFILE_NAME as PROFILE_NAME, COMPILED_PROFILE_TOML as PROFILE_TOML,
};
pub const STEAM_BUILD_ID: u64 = 25754343;
pub const GAME_BUILD: u32 = 40420;

pub mod builds;
pub mod crossings;
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
pub mod probe;
pub mod seeds;
pub mod simperf;
pub mod stoptool;
pub mod streettrace;
pub mod terrain;
pub mod ticks;
pub mod toolchain;
pub mod toolplayer;
pub mod townfield;
pub mod towntrace;
