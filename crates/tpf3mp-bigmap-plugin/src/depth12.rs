//! The octree at depth 12: ids for the level stock 32-bit ids cannot
//! number, and the renderer's level decoder taught to read them.
//!
//! A tree of depth `d` has levels `0..d`, the root counting as one; the
//! descent (0xae33c0) numbers a child `8·id + 1 + octant` in 32 bits. At
//! depth 11 the deepest level, 10, ends at id 1,227,133,512; depth 12's
//! level 11 would run from 1,227,133,513 to 9,817,068,104, past 32 bits,
//! and the renderer's `CalcOctreeLevel` (0x818b30), whose level thresholds
//! wrap to a step of 0 after level 10, would loop forever on any of them
//! (investigation/TF3_BIGMAPS_256KM_2026-10-05.md, "Depth 12").
//!
//! tpf2-bigmap (silver2127, MIT; no code taken) gave levels 11 and 12
//! ranges of ids drawn from counters. A counter's ids depend on the
//! process's history (which world it loaded before, how often), so two
//! games of a room would number their deep nodes differently. Here a
//! level-11 node's id is a pure function of its box instead, as a stock
//! id is a pure function of its path: `0x50000000 + zslot·2²² + y·2¹¹ +
//! x`, the node's 128 m cell in the root's 2,048 by 2,048 grid and one of
//! 128 height slots, z from −8,192 to +8,192 m (TF3's terrain spans −100 to
//! about 3,200 m). Every game of a room with the same tree has the same ids,
//! whoever reads them. A node outside the height band is clamped into it
//! (and counted): its id may then repeat another's, which only the
//! renderer's per-node caches key by.
//!
//! What it touches, each only on a tree of depth 12 (the descent reads the
//! depth from its own `EcsOctree`), so any shallower world runs the game's
//! code unchanged:
//!
//! - a detour of the descent that replaces the incoming id of a level-11
//!   node, before the game stores it;
//! - a splice before the decoder's loop: an id at or past `0x50000000`
//!   (no stock id reaches it) answers level 11 and skips the loop.
//!
//! Level 11 is the deepest level of a depth-12 tree, so its nodes are
//! leaves: no id is ever derived from theirs.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use std::ffi::c_void;

use tpf3mp_plugin_abi::Registers as SavedRegs;

use crate::host::Host;

pub use crate::build40420::{
    DECODER, DECODER_STOLEN, DESCENT, DESCENT_CHECKS, DESCENT_START, DESCENT_START_CALL,
    DESCENT_START_CHECKS, ECS_OCTREE_BOX, ECS_OCTREE_DEPTH,
};
use crate::log;

/// The depth this module numbers.
pub const DEPTH: i32 = 12;
/// Its deepest level, the one stock ids cannot number.
pub const DEEP_LEVEL: i32 = DEPTH - 1;
/// The first id of the deep level: past every stock id (`0x49249248`).
pub const LEVEL11_BASE: i32 = 0x5000_0000;
/// The deep level's cells a side: 2¹¹.
pub const CELLS: i64 = 1 << 11;
/// Height slots, centred on z = 0.
pub const Z_SLOTS: i64 = 128;
/// The last stock id at depth 11 (level 10's last).
pub const STOCK_LAST_ID: i64 = 0x4924_9248;

/// A level-11 node's id from the root's box (min x, y, z, max x, y, z)
/// and the node's minimum corner, and whether its height was clamped.
pub fn level11_id(root: [f32; 6], node_min: [f32; 3]) -> (i32, bool) {
    let mut clamped = false;
    let mut cell = |axis: usize| -> i64 {
        let size = (f64::from(root[axis + 3]) - f64::from(root[axis])) / CELLS as f64;
        let index = ((f64::from(node_min[axis]) - f64::from(root[axis])) / size).floor();
        if !index.is_finite() {
            clamped = true;
            return 0;
        }
        index as i64
    };
    let (x, y, z) = (cell(0), cell(1), cell(2));
    let fit = |v: i64, n: i64, clamped: &mut bool| {
        if !(0..n).contains(&v) {
            *clamped = true;
        }
        v.clamp(0, n - 1)
    };
    let x = fit(x, CELLS, &mut clamped);
    let y = fit(y, CELLS, &mut clamped);
    let z = fit(z - (CELLS / 2 - Z_SLOTS / 2), Z_SLOTS, &mut clamped);
    let id = i64::from(LEVEL11_BASE) + (z << 22) + (y << 11) + x;
    (i32::try_from(id).unwrap_or(i32::MAX), clamped)
}

/// The level the patched decoder gives an id: 11 at or past
/// [`LEVEL11_BASE`]; otherwise the game's own loop's answer.
pub fn decoded_level(id: i32) -> Option<i32> {
    if id >= LEVEL11_BASE {
        Some(DEEP_LEVEL)
    } else {
        stock_level(id, 64)
    }
}

/// The game's decoder loop, as its code runs it (32-bit wrapping), with a
/// bound on the iterations: `None` where it would not have ended.
pub fn stock_level(id: i32, bound: u32) -> Option<i32> {
    if id < 0 {
        return None; // the game asserts `nodeIndex >= 0`
    }
    let (mut sum, mut step, mut level) = (0i32, 1i32, 0i32);
    if id < step {
        return Some(level);
    }
    for _ in 0..bound {
        sum = sum.wrapping_add(step);
        level += 1;
        step = step.wrapping_mul(8);
        if id < sum.wrapping_add(step) {
            return Some(level);
        }
    }
    None
}

/// The stock id a node gets from its path of octants, in 64 bits.
pub fn stock_id(path: &[i64]) -> i64 {
    path.iter().fold(0, |id, octant| 8 * id + 1 + octant)
}

/// `detail::EcsOctreeIterator`'s descent: ten arguments, returns `rdx`.
type DescentFn = unsafe extern "system" fn(
    usize,
    usize,
    i32,
    i32,
    i32,
    usize,
    usize,
    i32,
    *const f32,
    i32,
) -> usize;

static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static NUMBERED: AtomicU64 = AtomicU64::new(0);
static CLAMPED: AtomicU64 = AtomicU64::new(0);

/// The id a node at `remaining` depth left gets in the tree `octree`
/// points at, where depth 12 needs one; `None` keeps the game's.
///
/// # Safety
///
/// `octree` is the descent's `EcsOctree` and `node_box` its node's box,
/// as the game passes them.
unsafe fn deep_id(octree: usize, remaining: i32, node_box: *const f32) -> Option<i32> {
    // SAFETY: the EcsOctree the game passed, which the descent reads too.
    let depth = unsafe { std::ptr::read_unaligned((octree + ECS_OCTREE_DEPTH) as *const i32) };
    if depth != DEPTH || depth - remaining != DEEP_LEVEL || node_box.is_null() {
        return None;
    }
    // SAFETY: as above; the node's box is six floats the descent reads.
    let root = unsafe { std::ptr::read_unaligned((octree + ECS_OCTREE_BOX) as *const [f32; 6]) };
    let min = unsafe { std::ptr::read_unaligned(node_box.cast::<[f32; 3]>()) };
    let (id, clamped) = level11_id(root, min);
    let n = NUMBERED.fetch_add(1, Ordering::Relaxed);
    if clamped && CLAMPED.fetch_add(1, Ordering::Relaxed) == 0 {
        log::line(&format!(
            "big maps: octree depth 12: a node at {min:?} is outside the height band (±8,192 m); its id {id:#x} is clamped (renderer caches only)"
        ));
    }
    if n == 0 {
        log::line(&format!(
            "big maps: octree depth 12: first level-11 node numbered {id:#x}"
        ));
    }
    Some(id)
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn descent(
    octree: usize,
    out: usize,
    parent: i32,
    id: i32,
    octant: i32,
    position: usize,
    extent: usize,
    remaining: i32,
    node_box: *const f32,
    node: i32,
) -> usize {
    // SAFETY: the game's own arguments.
    let id = unsafe { deep_id(octree, remaining, node_box) }.unwrap_or(id);
    let original = ORIGINAL.load(Ordering::Acquire);
    // SAFETY: what the hook put in the slot (the game's descent, or the
    // next plugin's), with its arguments.
    unsafe {
        std::mem::transmute::<usize, DescentFn>(original)(
            octree, out, parent, id, octant, position, extent, remaining, node_box, node,
        )
    }
}

/// Before the decoder's loop: a deep id gets level 11 and skips it
/// (`r9d` becomes the level the stolen `mov r12d,r9d` copies, and `r8d`
/// the step the stolen `cmp edx,r8d` makes `jl` take past the loop).
unsafe extern "C" fn decoder_hook(regs: *mut SavedRegs, _: *mut c_void) {
    // SAFETY: the stub's block, held until the hook returns.
    let regs = unsafe { &mut *regs };
    let id = regs.rdx as u32 as i32;
    if id >= LEVEL11_BASE {
        regs.r9 = DEEP_LEVEL as u64;
        regs.r8 = 0x7fff_ffff;
    }
}

/// What a check of the game's bytes found.
fn check_bytes(at: usize, checks: &[(usize, &[u8])], what: &str) -> Result<(), String> {
    for (offset, bytes) in checks {
        let found = crate::host::read_bytes(at + offset, bytes.len())
            .ok_or_else(|| format!("{what}+{offset:#x} is unreadable"))?;
        if found != *bytes {
            return Err(format!(
                "{what}+{offset:#x} is not the code depth 12 was read on"
            ));
        }
    }
    Ok(())
}

/// Depth 12's pieces, checked and ready to be asked for.
#[derive(Debug, Clone, Copy)]
pub struct Checked;

/// Checks the descent, its starter's call of it, and its body where the
/// deep ids rely on it.
pub fn check(host: &Host) -> Result<Checked, String> {
    let (descent_at, start_at) = (host.address(DESCENT)?, host.address(DESCENT_START)?);
    host.address(DECODER)?;
    check_bytes(descent_at, &DESCENT_CHECKS, DESCENT)?;
    check_bytes(start_at, &DESCENT_START_CHECKS, DESCENT_START)?;
    let call = start_at + DESCENT_START_CALL;
    let bytes = crate::host::read_bytes(call, 5)
        .ok_or_else(|| format!("{DESCENT_START}'s call is unreadable"))?;
    let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    if bytes[0] != 0xE8 || (call + 5).wrapping_add_signed(rel as isize) != descent_at {
        return Err(format!("{DESCENT_START} does not call {DESCENT}"));
    }
    Ok(Checked)
}

/// Asks for the decoder's splice and the descent's detour.
pub fn attach(host: &Host, _checked: Checked) -> Result<(), String> {
    host.splice(DECODER, decoder_hook)?;
    host.attach(DESCENT, descent as *const (), &ORIGINAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stock_decoder_ends_up_to_depth_11_and_loops_past_it() {
        assert_eq!(stock_level(0, 64), Some(0));
        assert_eq!(stock_level(1, 64), Some(1));
        assert_eq!(stock_level(8, 64), Some(1));
        assert_eq!(stock_level(9, 64), Some(2));
        let deepest = stock_id(&[7; 10]);
        assert_eq!(stock_level(deepest as i32, 64), Some(10));
        assert_eq!(stock_level(LEVEL11_BASE, 64), None, "the loop never ends");
    }

    #[test]
    fn a_deep_node_is_numbered_by_its_cell() {
        let root = [
            -131_072.0, -131_072.0, -8_192.0, 131_072.0, 131_072.0, 8_192.0,
        ];
        let (first, clamped) = level11_id(root, [-131_072.0, -131_072.0, 0.0]);
        assert!(!clamped);
        assert!(first >= LEVEL11_BASE);
        let (next, _) = level11_id(root, [-131_072.0 + 128.0, -131_072.0, 0.0]);
        assert_eq!(next, first + 1, "the next cell east");
        let (_, clamped) = level11_id(root, [0.0, 0.0, 20_000.0]);
        assert!(clamped, "outside the height band");
    }
}
