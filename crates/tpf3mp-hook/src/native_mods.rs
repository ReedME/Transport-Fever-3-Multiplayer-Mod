//! Native mods in the game (D29, docs/NATIVE_MODS.md): the hook
//! switches on only the built-in features the launcher enabled, for the
//! build it enabled them for, where the matched profiles have every target
//! they patch.
//!
//! The launcher names the enabled packages' file in the game's environment
//! (`TPF3MP_NATIVE_MODS`, [`tpf3mp_nativemods::enabled::ENV`]); without it
//! nothing is enabled, and no other switch (such as the `TPF3MP_BIGMAP_*`
//! variables of Big Maps' own branches) turns a feature on. The plan is made once,
//! at bootstrap, before any hook is installed; a feature's code asks
//! [`feature`] for its settings. A simulation-changing package that cannot
//! run refuses multiplayer: [`decide`] returns why, and the hook then
//! installs nothing, as for a build it has no profile for.
//!
//! No feature is built in yet ([`tpf3mp_nativemods::features::BUILT_IN`]).

use std::sync::OnceLock;

use tpf3mp_hookcore::profile::Profile;
use tpf3mp_nativemods::{
    enabled::{self, Active, Plan},
    features::BUILT_IN,
};

use crate::Logger;

static PLAN: OnceLock<Plan> = OnceLock::new();

/// Makes the plan for the game running `build` (the executable's SHA-256)
/// with `profiles`, the profiles matching it, logs it, and returns why the
/// game must not play in a room, if it must not.
pub(crate) fn decide(profiles: &[Profile], build: &str, log: &mut Logger) -> Option<String> {
    let plan = enabled::plan_from_env(
        |key| std::env::var(key).ok(),
        build,
        BUILT_IN,
        |target| has_target(profiles, target),
    );
    for active in &plan.active {
        log.line(&format!(
            "native mod {} {}: feature {} on",
            active.package, active.version, active.feature
        ));
    }
    for why in &plan.left_off {
        log.line(&format!("native mod left off: {why}"));
    }
    let refusal = plan.refusal.clone();
    let _ = PLAN.set(plan);
    refusal
}

/// Whether one of `profiles` has the target `name`.
fn has_target(profiles: &[Profile], name: &str) -> bool {
    profiles
        .iter()
        .any(|profile| profile.targets.iter().any(|target| target.name == name))
}

/// The feature `id`, with its package's settings and folder, if the
/// launcher enabled it and it runs here.
pub fn feature(id: &str) -> Option<&'static Active> {
    PLAN.get()?.feature(id)
}

/// Losing a required core target must never turn its bytes into an
/// unprotected plugin patch site.
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn core_sites(
    profiles: &[Profile],
    code: &[u8],
    text_rva: u64,
    base: usize,
) -> Result<Vec<usize>, String> {
    let mut sites = Vec::new();
    for profile in profiles {
        let resolved = tpf3mp_hookcore::profile::resolve(profile, code, text_rva)
            .map_err(|error| format!("cannot protect core profile {}: {error:?}", profile.name))?;
        for target in resolved.targets {
            let address = usize::try_from(target.address)
                .ok()
                .and_then(|rva| base.checked_add(rva))
                .ok_or_else(|| {
                    format!("core target {} is outside the address space", target.name)
                })?;
            sites.push(address);
        }
    }
    Ok(sites)
}

/// Loads the plan's plugins and patches the functions they attach to
/// ([`crate::plugins`]), before any of the hook's own patches, whose
/// functions (`profiles`' targets) no plugin may patch. Any failure loads
/// what it loaded so far but patches nothing, and keeps the game out of
/// rooms.
#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)]
pub(crate) fn load_plugins(profiles: &[Profile], log: &mut Logger) -> Result<(), String> {
    use tpf3mp_hookcore::pe::PeHeaders;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    use crate::plugins;

    let Some(plan) = PLAN.get().filter(|plan| !plan.plugins.is_empty()) else {
        return Ok(());
    };
    // SAFETY: a null name asks for the executable's own module, mapped for
    // the life of the process.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if base == 0 {
        return Err("cannot find the game's module".into());
    }
    // SAFETY: the first page of a mapped module holds its headers.
    let head = unsafe { std::slice::from_raw_parts(base as *const u8, 0x1000) };
    let pe = PeHeaders::parse(head).map_err(|error| format!("the game's headers: {error:?}"))?;
    let text = pe.section(".text").ok_or("the game has no .text section")?;
    let start = base + text.virtual_address as usize;
    // SAFETY: .text is mapped there for its virtual size, and only read.
    let code =
        unsafe { std::slice::from_raw_parts(start as *const u8, text.virtual_size as usize) };
    let core = core_sites(profiles, code, u64::from(text.virtual_address), base)?;
    // A slot's table lies in the game's data: read only where readable.
    let pointer = |at: usize| {
        crate::image::readable(at, 8).then(|| {
            // SAFETY: eight readable bytes, checked just above.
            unsafe { std::ptr::read_unaligned(at as *const usize) }
        })
    };
    let mut found = Vec::new();
    for package in &plan.plugins {
        found.extend(plugins::locate(package, code, start, &pointer)?);
    }
    let sites = plugins::arrange(found.clone(), &core)?;
    // Verify the whole plan before loading any library: loading invokes
    // native startup code even before its explicit plugin entry is called.
    for package in &plan.plugins {
        for plugin in &package.plugins {
            plugins::verify(&package.root.join(&plugin.path), &plugin.sha256)?;
        }
    }
    let mut started = plugins::Started::default();
    for package in &plan.plugins {
        let own: Vec<plugins::Found> = found
            .iter()
            .filter(|f| f.package == package.id)
            .cloned()
            .collect();
        for plugin in &package.plugins {
            let path = package.root.join(&plugin.path);
            plugins::verify(&path, &plugin.sha256)?;
            let init = open(&path)?;
            let mut line = |text: &str| log.line(text);
            // SAFETY: the plugin is the one its package pins, which the
            // launcher installed from the server's index (proposed D29
            // amendment): its code runs here by the player's choice.
            unsafe { plugins::start(package, &own, init, &mut started, &mut line) }?;
            log.line(&format!(
                "native mod {} {}: plugin {} started",
                package.id, package.version, plugin.path
            ));
        }
    }
    let lua = std::mem::take(&mut started.lua);
    // SAFETY: the game is suspended; each site was found by its signature
    // and checked, and each function, callback and slot is its plugin's,
    // loaded for good.
    let patches = unsafe { plugins::patch(&sites, started) }?;
    log.line(&format!(
        "native mods' plugins: {} sites patched, {} Lua chunks for the game's Lua states",
        patches.len(),
        lua.len()
    ));
    plugins::keep_lua(lua);
    // The patches stay for the life of the game.
    std::mem::forget(patches);
    Ok(())
}

/// Plugins are loaded only into the game on Windows x86-64 yet: a plan
/// with any keeps the game out of rooms elsewhere.
#[cfg(not(all(windows, target_arch = "x86_64")))]
pub(crate) fn load_plugins(_profiles: &[Profile], _log: &mut Logger) -> Result<(), String> {
    match PLAN.get() {
        Some(plan) if !plan.plugins.is_empty() => {
            Err("native mods' plugins are loaded only on Windows yet".into())
        }
        _ => Ok(()),
    }
}

/// The plugin library at `path`, loaded for good, and its entry.
#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)]
fn open(path: &std::path::Path) -> Result<tpf3mp_plugin_abi::Init, String> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; loading runs the library's own
    // start-up code, the plugin's.
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!(
            "{} cannot be loaded: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: a loaded module and a NUL-terminated name.
    let entry = unsafe { GetProcAddress(module, tpf3mp_plugin_abi::ENTRY.as_ptr()) }
        .ok_or_else(|| format!("{} has no tpf3mp_plugin_init", path.display()))?;
    // SAFETY: the interface says the entry is an `Init`.
    Ok(unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, tpf3mp_plugin_abi::Init>(entry)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_core_target_refuses_plugin_preflight() {
        let profile = |name: &str, signature: &str| {
            Profile::from_toml(&format!(
                "name = '{name}'\n[build]\nsha256 = '00'\n[[target]]\nname = 'step'\nsignature = '{signature}'\nprologue = '{signature}'\n"
            ))
            .unwrap()
        };
        let good = profile("good", "40 53");
        let missing = profile("missing", "AA BB");
        assert_eq!(
            core_sites(std::slice::from_ref(&good), &[0x40, 0x53], 0x1000, 0x2000),
            Ok(vec![0x3000])
        );
        let error = core_sites(&[good, missing], &[0x40, 0x53], 0x1000, 0x2000).unwrap_err();
        assert!(error.contains("cannot protect core profile missing"));
    }

    #[test]
    fn a_target_counts_from_any_matching_profile() {
        let profile = |target: &str| {
            Profile::from_toml(&format!(
                r#"
name = "p"
[build]
sha256 = "00"
[[target]]
name = "{target}"
signature = "40 53"
prologue = "40 53"
"#
            ))
            .unwrap()
        };
        let profiles = [profile("step"), profile("bigmap::octree_root")];
        assert!(has_target(&profiles, "bigmap::octree_root"));
        assert!(has_target(&profiles, "step"));
        assert!(!has_target(&profiles, "bigmap::street_raster"));
        assert!(!has_target(&[], "step"));
        // Nothing enabled until the launcher's plan is made.
        assert!(feature("bigmap.octree").is_none());
    }
}
