//! Big Maps as a native mod: bigger Transport Fever 3 worlds, as a
//! package whose plugin the hook loads (proposed D29 amendment,
//! docs/NATIVE_MODS.md, "Plugins"; docs/BIGMAPS.md).
//!
//! This is the code of Big Maps' own branch (`feat/bigmaps-40420`, the
//! hook's `bigmap` module), moved out of the hook: each patch asks the hook
//! for the hooks its package declares (`manifest`) instead of patching the
//! game itself, and reads its switches from the package's settings instead
//! of `TPF3MP_BIGMAP_*` variables.
//!
//! - [`octree`] and [`depth12`]: the octree root at depth 11 for worlds
//!   with an axis over 256 tiles and depth 12 over 512; always on, and
//!   derived from the world, so a stock-sized world runs the game's code.
//! - [`raster`] (`street_raster`) and [`placement`] (`placement`): the
//!   street raster's cell and the placement spacing, where stock code
//!   would abort or wrap.
//! - [`emission`] (`emission_every`) and [`emission_cell`]
//!   (`emission_cell`): proposals that change simulation results.
//! - [`lua`]: the New Game page's size rows and the added density levels,
//!   from the package's game mod `tpf3mp_bigmap_1` (`memory_gate`).
//!
//! The package's settings travel in a room's terms (D29), so every game of
//! a room runs the same switches. Each patch fails closed: a site that is
//! not the code it was read on leaves that part of the game stock, and
//! `hook.log` says why.

pub mod build40420;
pub mod depth12;
pub mod emission;
pub mod emission_cell;
pub mod host;
pub mod lua;
pub mod manifest;
pub mod octree;
pub mod placement;
pub mod raster;

use tpf3mp_plugin_abi as abi;

/// The hook's log, as the ported modules call it.
pub(crate) mod log {
    /// Appends one line to the hook's log.
    pub fn line(message: &str) {
        crate::host::log(message);
    }
}

/// Whether a switch's value turns it on: `1`, `on`, `true` or `yes`. Unset
/// or empty is off, as is anything else, and the caller says so.
pub fn switch(value: Option<&str>) -> Result<bool, String> {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "0" | "off" | "false" | "no") => Ok(false),
        Some("1" | "on" | "true" | "yes") => Ok(true),
        Some(other) => Err(format!("{other:?} is not 1 or 0")),
    }
}

/// The package's settings (`manifest::settings` has their defaults).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub street_raster: bool,
    pub placement: bool,
    /// `N` for the emission throttle; `None` is the game's own.
    pub emission_every: Option<u32>,
    /// 8 cells per tile instead of 16.
    pub emission_cell: bool,
    pub memory_gate: bool,
}

impl Settings {
    /// The settings in `json`, the hook's: each setting the package names,
    /// its default where it is missing.
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| format!("the settings: {error}"))?;
        let flag =
            |key: &str, default: bool| value.get(key).and_then(|v| v.as_bool()).unwrap_or(default);
        let int =
            |key: &str, default: i64| value.get(key).and_then(|v| v.as_i64()).unwrap_or(default);
        let every = int("emission_every", 1);
        let emission_every = match every {
            0 | 1 => None,
            2..=16 => Some(every as u32),
            other => return Err(format!("emission_every {other} is not 2 to 16, or 1")),
        };
        let emission_cell = match int("emission_cell", 16) {
            16 => false,
            32 => true,
            other => return Err(format!("emission_cell {other} is not 16 or 32")),
        };
        Ok(Self {
            street_raster: flag("street_raster", false),
            placement: flag("placement", false),
            emission_every,
            emission_cell,
            memory_gate: flag("memory_gate", true),
        })
    }
}

/// Asks the hook for every patch the settings want, each checked first.
/// Returns the lines for `hook.log`.
pub fn start(host: &host::Host, settings: &Settings) -> Result<Vec<String>, String> {
    let mut lines = vec![
        octree::attach(host),
        raster::attach(host, settings.street_raster),
        placement::attach(host, settings.placement),
        emission::attach(host, settings.emission_every),
        emission_cell::attach(host, settings.emission_cell),
    ];
    host.lua(&lua::chunk(lua::physical_memory_mb(), settings.memory_gate))?;
    lines.push("big maps: the New Game page and density levels come from tpf3mp_bigmap_1".into());
    Ok(lines)
}

/// The plugin's entry: the hook calls it once, while the game is still
/// suspended.
///
/// # Safety
///
/// `host` is the hook's, valid during the call.
#[unsafe(no_mangle)]
#[allow(unsafe_code)]
pub unsafe extern "C" fn tpf3mp_plugin_init(host: *const abi::Host) -> i32 {
    // SAFETY: the caller's promise.
    let Some(host) = (unsafe { host::Host::new(host) }) else {
        return abi::error::BAD_ARGUMENT;
    };
    let settings = match Settings::parse(host.settings()) {
        Ok(settings) => settings,
        Err(why) => {
            host::log(&format!("big maps: {why}; nothing is patched"));
            return abi::error::BAD_ARGUMENT;
        }
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| start(&host, &settings))) {
        Ok(Ok(lines)) => {
            for line in lines {
                host::log(&line);
            }
            abi::OK
        }
        Ok(Err(why)) => {
            host::log(&format!("big maps: {why}"));
            abi::error::BAD_ARGUMENT
        }
        Err(_) => {
            host::log("big maps: panicked while starting; nothing is patched");
            abi::error::BAD_ARGUMENT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_the_safe_ones() {
        assert_eq!(
            Settings::parse("{}").unwrap(),
            Settings {
                street_raster: false,
                placement: false,
                emission_every: None,
                emission_cell: false,
                memory_gate: true,
            }
        );
        let on = Settings::parse(
            r#"{"street_raster": true, "placement": true, "emission_every": 4, "emission_cell": 32, "memory_gate": false}"#,
        )
        .unwrap();
        assert_eq!(on.emission_every, Some(4));
        assert!(on.emission_cell && on.street_raster && on.placement && !on.memory_gate);
        assert!(Settings::parse(r#"{"emission_every": 40}"#).is_err());
        assert!(Settings::parse(r#"{"emission_cell": 24}"#).is_err());
    }
}
