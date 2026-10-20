//! The octree root at depth 11 for worlds with an axis over 256 tiles.
//!
//! TF3 sizes its entity octree from a two-tier constant, not from the map:
//! ±16,384 m at depth 9, and, when either axis is over 128 tiles,
//! ±32,768 m at depth 10, set by `OctreeSystem::Resize` at two sites, one
//! on the new-game path and one on the load path. An entity past the root
//! is invisible to lookups, and the street builder stacks duplicate nodes
//! there, so a world with an axis over 256 tiles (65.5 km) damages itself
//! (investigation/TF3_BIGMAPS_PORT_2026-10-01.md §1.3).
//!
//! The instruction after each site's call is spliced, always: there is no
//! setting, and every world gets the deepest root it needs: when an axis of the world being allocated is over 256
//! tiles, the hook calls the game's own `Resize` again, with depth 11 and
//! ±65,536 m. That is tpf2-bigmap's depth-11 root (silver2127, MIT; no code
//! taken), reached without patching the shared 32768.0f constant or any
//! instruction but the splice's jump. Depth 11 keeps the 128 m leaves, and
//! its deepest level's ids end at 1,227,133,512, inside the stock id scheme
//! and the renderer's level decoder (investigation/
//! TF3_BIGMAPS_256KM_2026-10-05.md §1). A world over 512 tiles needs depth
//! 12, which is not built: it is left at the game's depth, with a line in
//! `hook.log`, and the New Game page must not offer it.
//!
//! A world of 256 tiles or less is never touched, so a game with the
//! setting and one without agree on every stock-sized world. On a world
//! over 256 tiles every game of a room needs the setting.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

use std::ffi::c_void;

use tpf3mp_plugin_abi::Registers as SavedRegs;

use crate::host::Host;

pub use crate::build40420::{
    INIT_SITE, LOAD_SITE, OCTREE_FIELD, RESIZE, ROOT_CALL_OFFSET, ROOT_DEPTH_BYTES, ROOT_INIT,
    ROOT_LOAD, ROOT_NEXT_OFFSET, ROOT_OWNER_OFFSET, Reg, RootSite,
};
use crate::log;

/// The patch's name in `hook.log`.
pub const FIX: &str = "big maps: octree";

/// The site's own test: a root of depth 10 above this many tiles.
pub const STOCK_LARGE_TILES: i32 = 128;
/// The longest axis the stock depth-10 root covers: ±32,768 m.
pub const STOCK_ROOT_TILES: i32 = 256;
/// The longest axis depth 11 covers: ±65,536 m.
pub const DEPTH11_TILES: i32 = 512;
/// Depth 11's depth and half extent.
pub const DEPTH11: (i32, f32) = (11, 65_536.0);
/// The longest axis depth 12 covers: ±131,072 m.
pub const DEPTH12_TILES: i32 = 1024;
/// Depth 12's depth and half extent.
pub const DEPTH12: (i32, f32) = (12, 131_072.0);

/// How deep the patch reaches: always [`Setting::Depth12`] in the game
/// ([`install`]); the shallower ones remain for the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// The game's own root.
    Off,
    /// Depth 11 for worlds with an axis over 256 tiles.
    Depth11,
    /// Depth 11 over 256 tiles and depth 12 (crate::bigmap::depth12) over
    /// 512.
    Depth12,
}

/// What a world of `x` by `y` tiles gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    /// The game's own root, as its site set it.
    Stock,
    /// Depth 11, ±65,536 m.
    Depth11,
    /// Depth 12, ±131,072 m.
    Depth12,
    /// Longer than the setting covers: left at the game's own, which damages
    /// the world.
    Beyond { tiles: i32 },
}

/// The root a world of `x` by `y` tiles gets with the patch set to
/// `setting`: the game's own up to 256 tiles, depth 11 up to 512, depth 12
/// up to 1,024 when set to 12, and past that nothing this hook can give.
pub fn root_for(x: i32, y: i32, setting: Setting) -> Root {
    let longest = x.max(y);
    if setting == Setting::Off || longest <= STOCK_ROOT_TILES {
        Root::Stock
    } else if longest <= DEPTH11_TILES {
        Root::Depth11
    } else if longest <= DEPTH12_TILES && setting == Setting::Depth12 {
        Root::Depth12
    } else {
        Root::Beyond { tiles: longest }
    }
}

/// `OctreeSystem::Resize(octree, depth, half)`.
type ResizeFn = unsafe extern "system" fn(usize, i32, f32);

/// `Resize`'s address while the patch is on; 0 otherwise.
static RESIZE_AT: AtomicUsize = AtomicUsize::new(0);
/// The setting the splices act on: 0 off, 11 or 12.
static INSTALLED: AtomicI32 = AtomicI32::new(0);
/// Set when the hook panicked: it does nothing more.
static BROKEN: AtomicBool = AtomicBool::new(false);

/// A register of the splice's block.
fn reg(regs: &SavedRegs, which: Reg) -> u64 {
    match which {
        Reg::Rbp => regs.rbp,
        Reg::Rsi => regs.rsi,
    }
}

fn read<T: Copy>(address: u64) -> Option<T> {
    let address = usize::try_from(address).ok()?;
    if address == 0 || !crate::host::readable(address, std::mem::size_of::<T>()) {
        return None;
    }
    // SAFETY: the bytes are readable, checked just above.
    Some(unsafe { std::ptr::read_unaligned(address as *const T) })
}

/// After a site's `Resize`: the world's tile counts and the octree, from
/// the site's registers; depth 11 where the world needs it.
fn after_resize(regs: &SavedRegs, site: &RootSite) {
    let tiles = reg(regs, site.tiles);
    let (Some(x), Some(y)) = (read::<i32>(tiles), read::<i32>(tiles.wrapping_add(4))) else {
        log::line(&format!(
            "{FIX}: {}: the tile counts at {tiles:#x} are unreadable; the game's root stays",
            site.target
        ));
        return;
    };
    let setting = match INSTALLED.load(Ordering::Acquire) {
        12 => Setting::Depth12,
        11 => Setting::Depth11,
        _ => Setting::Off,
    };
    let root = root_for(x, y, setting);
    let (depth, half) = if root == Root::Depth12 {
        DEPTH12
    } else {
        DEPTH11
    };
    match root {
        Root::Stock => {}
        Root::Beyond { .. } => log::line(&format!(
            "{FIX}: {}: a world of {x} x {y} tiles is past depth 12's ±131,072 m; the game's root (±32,768 m) stays and entities past it will be lost",
            site.target
        )),
        Root::Depth11 | Root::Depth12 => {
            let owner = reg(regs, site.owner);
            let octree = read::<usize>(owner.wrapping_add(OCTREE_FIELD as u64)).unwrap_or(0);
            let resize = RESIZE_AT.load(Ordering::Acquire);
            if octree == 0 || resize == 0 || !crate::host::readable(octree, 0x2c) {
                log::line(&format!(
                    "{FIX}: {}: no octree at {owner:#x}+{OCTREE_FIELD:#x}; the game's root stays",
                    site.target
                ));
                return;
            }
            // SAFETY: the game's own Resize (the profile resolved it, and
            // install checked each site calls it), on the octree the site
            // just resized, on the game's thread, before anything is in it:
            // it only stores the box and the depth.
            unsafe { std::mem::transmute::<usize, ResizeFn>(resize)(octree, depth, half) };
            log::line(&format!(
                "{FIX}: {}: a world of {x} x {y} tiles; root at depth {depth}, ±{half} m",
                site.target
            ));
        }
    }
}

fn guarded(regs: *mut SavedRegs, site: &RootSite) {
    if BROKEN.load(Ordering::Acquire) {
        return;
    }
    let body = || {
        // SAFETY: the stub's block, held until the hook returns.
        let regs = unsafe { &*regs };
        after_resize(regs, site);
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        BROKEN.store(true, Ordering::Release);
        log::line(&format!(
            "{FIX}: panicked on the game's thread; switched off for this game"
        ));
    }
}

unsafe extern "C" fn init_hook(regs: *mut SavedRegs, _: *mut c_void) {
    guarded(regs, &INIT_SITE);
}

unsafe extern "C" fn load_hook(regs: *mut SavedRegs, _: *mut c_void) {
    guarded(regs, &LOAD_SITE);
}

/// The bytes at a site must be the block the patch was read on: the owner
/// load, `mov edx,10`, a `call` of `Resize`, and the instruction to splice.
pub fn check_site(
    bytes: &[u8],
    site_at: u64,
    resize_at: u64,
    site: &RootSite,
) -> Result<(), String> {
    let end = ROOT_NEXT_OFFSET + site.next.len();
    if bytes.len() < end {
        return Err(format!("{}: only {} bytes", site.target, bytes.len()));
    }
    let owner = &bytes[ROOT_OWNER_OFFSET..ROOT_OWNER_OFFSET + 4];
    if owner != site.owner_load {
        return Err(format!(
            "{}: the owner load is not the expected one",
            site.target
        ));
    }
    if bytes[ROOT_OWNER_OFFSET + 4..ROOT_CALL_OFFSET] != ROOT_DEPTH_BYTES {
        return Err(format!("{}: no `mov edx, 10` before the call", site.target));
    }
    if bytes[ROOT_CALL_OFFSET] != 0xE8 {
        return Err(format!("{}: no call at +{ROOT_CALL_OFFSET}", site.target));
    }
    let rel = i32::from_le_bytes(
        bytes[ROOT_CALL_OFFSET + 1..ROOT_NEXT_OFFSET]
            .try_into()
            .map_err(|_| "a short call".to_owned())?,
    );
    let callee = (site_at + ROOT_NEXT_OFFSET as u64).wrapping_add_signed(i64::from(rel));
    if callee != resize_at {
        return Err(format!(
            "{}: the call reaches {callee:#x}, not {RESIZE} at {resize_at:#x}",
            site.target
        ));
    }
    if bytes[ROOT_NEXT_OFFSET..end] != site.next {
        return Err(format!(
            "{}: the instruction after the call is not the expected one",
            site.target
        ));
    }
    Ok(())
}

/// What installing came to, for `hook.log`.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: installed ({reason})")
    } else {
        format!("{FIX}: off, {reason}")
    }
}

/// Attaches the patch at its deepest: depth 11 for worlds with an axis
/// over 256 tiles, depth 12 over 512. There is no setting: a world gets the
/// deepest root it needs in every game, so no two games of a room differ.
/// Returns the line for `hook.log`.
pub fn attach(host: &Host) -> String {
    attach_with(host, Setting::Depth12)
}

/// [`attach`] with the setting given. Every check is made before anything
/// is asked for, so a site that is not as read leaves the game's own root.
pub fn attach_with(host: &Host, wanted: Setting) -> String {
    if wanted == Setting::Off {
        return outcome_line(false, "off; the game's own root");
    }
    let checked = check_sites(host).and_then(|sites| {
        let deep = if wanted == Setting::Depth12 {
            Some(super::depth12::check(host).map_err(|why| format!("depth 12: {why}"))?)
        } else {
            None
        };
        Ok((sites, deep))
    });
    let (resize, deep) = match checked {
        Ok(found) => found,
        Err(why) => return outcome_line(false, &why),
    };
    RESIZE_AT.store(resize, Ordering::Release);
    INSTALLED.store(if deep.is_some() { 12 } else { 11 }, Ordering::Release);
    // Depth 12's ids and decoder go in with the roots that use them.
    let asked = deep
        .map_or(Ok(()), |deep| super::depth12::attach(host, deep))
        .and_then(|()| host.splice(ROOT_INIT, init_hook))
        .and_then(|()| host.splice(ROOT_LOAD, load_hook));
    match asked {
        Ok(()) => outcome_line(
            true,
            &format!(
                "depth 11 (±65,536 m) for worlds with an axis over {STOCK_ROOT_TILES} tiles{}; worlds of 256 tiles or less keep the game's own",
                if deep.is_some() {
                    format!(" and depth 12 (±131,072 m, deep ids by cell) over {DEPTH11_TILES}")
                } else {
                    String::new()
                }
            ),
        ),
        Err(why) => outcome_line(false, &why),
    }
}

/// Checks both root sites, each as found at its splice, and returns
/// `Resize`'s address.
fn check_sites(host: &Host) -> Result<usize, String> {
    let resize = host.address(RESIZE)?;
    for site in [INIT_SITE, LOAD_SITE] {
        let next = host.address(site.target)?;
        let at = next - ROOT_NEXT_OFFSET;
        let len = ROOT_NEXT_OFFSET + site.next.len();
        let bytes = crate::host::read_bytes(at, len)
            .ok_or_else(|| format!("{} at {at:#x} is unreadable", site.target))?;
        check_site(&bytes, at as u64, resize as u64, &site)?;
    }
    Ok(resize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_gets_the_root_it_needs() {
        assert_eq!(root_for(256, 256, Setting::Depth12), Root::Stock);
        assert_eq!(root_for(100, 400, Setting::Depth12), Root::Depth11);
        assert_eq!(root_for(1000, 100, Setting::Depth12), Root::Depth12);
        assert_eq!(
            root_for(1000, 100, Setting::Depth11),
            Root::Beyond { tiles: 1000 }
        );
        assert_eq!(
            root_for(2000, 100, Setting::Depth12),
            Root::Beyond { tiles: 2000 }
        );
        assert_eq!(root_for(400, 400, Setting::Off), Root::Stock);
    }

    /// The init site's bytes on build 40420, from the site.
    fn init_bytes(call_to: i32) -> Vec<u8> {
        let mut bytes = vec![0xC5, 0xFA, 0x10, 0x15, 0x65, 0x74, 0x44, 0x03];
        bytes.extend_from_slice(&INIT_SITE.owner_load);
        bytes.extend_from_slice(&ROOT_DEPTH_BYTES);
        bytes.push(0xE8);
        bytes.extend_from_slice(&call_to.to_le_bytes());
        bytes.extend_from_slice(&INIT_SITE.next);
        bytes
    }

    #[test]
    fn a_site_must_be_the_block_it_was_read_on() {
        let site_at = 0x1000_u64;
        let resize = site_at + ROOT_NEXT_OFFSET as u64 + 0x500;
        assert_eq!(
            check_site(&init_bytes(0x500), site_at, resize, &INIT_SITE),
            Ok(())
        );
        assert!(
            check_site(&init_bytes(0x501), site_at, resize, &INIT_SITE)
                .unwrap_err()
                .contains("reaches")
        );
        let mut other = init_bytes(0x500);
        other[ROOT_NEXT_OFFSET] = 0x90;
        assert!(check_site(&other, site_at, resize, &INIT_SITE).is_err());
        assert!(check_site(&init_bytes(0x500), site_at, resize, &LOAD_SITE).is_err());
    }
}
