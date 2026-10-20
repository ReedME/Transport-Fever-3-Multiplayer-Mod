//! The street and obstacle raster's cell, grown where 1 m overflows.
//!
//! `Obstacle::Obstacle` (0x8cea50) rasterises a box at a cell size its
//! caller passes, 1 m for the town connections and the street initialiser:
//! `n = floor(extent / cell) + 1` cells a side, sized through a 32-bit
//! multiply. Past 2³¹ − 1 cells (180 tiles square, or 1000 x 32 long) the
//! product wraps, `vector<bool>::resize` throws `length_error` and the game
//! aborts with no message (TPF2's 180-tile wall, which TF3 kept).
//!
//! With [`RASTER_ENV`] on, the constructor is detoured: where the cell
//! passed would overflow, it is doubled until the count fits, as
//! tpf2-bigmap's `street_raster` grows it (silver2127, MIT; no code taken).
//! Two differences, both for TF3 and a room:
//!
//! - **Only where stock code would abort.** tpf2-bigmap grew the cell past
//!   a 1.5-billion budget; here a count that fits int32 is never touched,
//!   so every world a stock game can generate keeps its cells, and a game
//!   with the setting agrees with one without on all of them.
//! - **Powers of two.** The raster's fill scales by `extent / (n − 1)`,
//!   lookups divide by the cell; they agree exactly only when the cell
//!   divides the extent. Tile edges are 256 m, so 2, 4 and 8 m always do,
//!   and the divisions stay exact in f32
//!   (investigation/TF3_BIGMAPS_256KM_2026-10-05.md §2).
//!
//! The multiply is never widened: every index downstream is int32, and
//! the coarser cell keeps them all in range.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::host::Host;

pub use crate::build40420::{OBSTACLE, OBSTACLE_BODY, OBSTACLE_BODY_OFFSET};
use crate::log;

/// The patch's name in `hook.log`.
pub const FIX: &str = "big maps: street raster";

/// The package's setting that grows the raster's cell where the count
/// would overflow; off leaves the game's own.
pub const SETTING: &str = "street_raster";

/// The largest factor the cell is grown by: 64 m cells cover far more
/// than the heightmap allows.
pub const MAX_GROWTH: f32 = 64.0;

/// The constructor's count a side, as its code computes it: the extent
/// over the cell in f32, truncated, one less below zero, plus one. `None`
/// where `vcvttss2si` would not give a number (a NaN or past int32).
pub fn side(extent: f32, cell: f32) -> Option<i64> {
    let q = extent / cell;
    if !q.is_finite() || q.abs() >= 2_147_483_648.0 {
        return None;
    }
    // In range: truncation toward zero, as `vcvttss2si`.
    let t = q as i32;
    let t = if q < 0.0 { t.wrapping_sub(1) } else { t };
    Some(i64::from(t) + 1)
}

/// The constructor's cell count for `bbox` (`{minX, minY, maxX, maxY}`) at
/// `cell`.
pub fn cells(bbox: [f32; 4], cell: f32) -> Option<i64> {
    let x = side(bbox[2] - bbox[0], cell)?;
    let y = side(bbox[3] - bbox[1], cell)?;
    Some(x * y)
}

/// The cell to build `bbox` with: `None` keeps the caller's (it fits, or
/// nothing this patch can do helps), `Some` the smallest power-of-two
/// multiple of it whose count fits int32.
pub fn grown_cell(bbox: [f32; 4], cell: f32) -> Option<f32> {
    if !(cell.is_finite() && cell > 0.0) {
        return None;
    }
    let fits = |c: f32| cells(bbox, c).is_some_and(|n| (0..=i64::from(i32::MAX)).contains(&n));
    if cells(bbox, cell).is_none() || fits(cell) {
        return None;
    }
    let mut factor = 2.0f32;
    while factor <= MAX_GROWTH {
        let grown = cell * factor;
        if fits(grown) {
            return Some(grown);
        }
        factor *= 2.0;
    }
    None
}

/// `Obstacle::Obstacle(this, box, cell) -> this`.
type CtorFn = unsafe extern "system" fn(usize, *const [f32; 4], f32) -> usize;

/// What the constructor's start held (the game's constructor, or the next
/// plugin's detour); 0 until attached.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static GROWN: AtomicU64 = AtomicU64::new(0);
static BROKEN: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn ctor(this: usize, bbox: *const [f32; 4], cell: f32) -> usize {
    let original = ORIGINAL.load(Ordering::Acquire);
    let mut use_cell = cell;
    if !BROKEN.load(Ordering::Acquire) && !bbox.is_null() {
        // SAFETY: the box the game passes, 16 readable bytes (the
        // constructor reads them itself).
        let corners = unsafe { std::ptr::read_unaligned(bbox) };
        let decided = std::panic::catch_unwind(|| grown_cell(corners, cell));
        match decided {
            Ok(Some(grown)) => {
                use_cell = grown;
                let n = GROWN.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 8 || n.is_multiple_of(256) {
                    log::line(&format!(
                        "{FIX}: {:.0} x {:.0} m at {cell} m overflows 2^31 cells; built at {grown} m ({} cells; grown {n} times)",
                        corners[2] - corners[0],
                        corners[3] - corners[1],
                        cells(corners, grown).unwrap_or_default()
                    ));
                }
            }
            Ok(None) => {}
            Err(_) => BROKEN.store(true, Ordering::Release),
        }
    }
    // SAFETY: what the hook put in the slot, called with the
    // constructor's own arguments and the cell decided.
    unsafe { std::mem::transmute::<usize, CtorFn>(original)(this, bbox, use_cell) }
}

/// What installing came to, for `hook.log`.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: installed ({reason})")
    } else {
        format!("{FIX}: off, {reason}")
    }
}

/// The constructor's body must be the one the cell rule models.
pub fn check_body(bytes: &[u8]) -> Result<(), String> {
    let body = bytes
        .get(OBSTACLE_BODY_OFFSET..OBSTACLE_BODY_OFFSET + OBSTACLE_BODY.len())
        .ok_or_else(|| "the constructor is too short".to_owned())?;
    if body != OBSTACLE_BODY {
        return Err(format!(
            "{OBSTACLE}'s body is not the one the cell rule was read on"
        ));
    }
    Ok(())
}

/// Attaches the patch if the package's setting asks for it. Returns the
/// line for `hook.log`.
pub fn attach(host: &Host, wanted: bool) -> String {
    if !wanted {
        return outcome_line(false, &format!("{SETTING} is off; the game's own cells"));
    }
    let checked = host.address(OBSTACLE).and_then(|at| {
        let bytes = crate::host::read_bytes(at, OBSTACLE_BODY_OFFSET + OBSTACLE_BODY.len())
            .ok_or_else(|| format!("{OBSTACLE} at {at:#x} is unreadable"))?;
        check_body(&bytes)?;
        host.attach(OBSTACLE, ctor as *const (), &ORIGINAL)?;
        Ok(at)
    });
    match checked {
        Ok(at) => outcome_line(
            true,
            &format!(
                "at {at:#x}, a cell that would overflow 2^31 cells is doubled until it fits; every other raster is the game's"
            ),
        ),
        Err(why) => outcome_line(false, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_raster_that_would_overflow_is_grown() {
        let km = |x: f32, y: f32| [0.0, 0.0, x * 1000.0, y * 1000.0];
        assert_eq!(grown_cell(km(40.0, 40.0), 1.0), None, "fits at 1 m");
        let wide = km(46.08, 46.08); // 180 tiles square
        assert_eq!(grown_cell(wide, 1.0), None, "the stock limit still fits");
        let past = km(256.0, 25.6); // 1000 x 100 tiles
        let grown = grown_cell(past, 1.0).unwrap();
        assert!(cells(past, grown).unwrap() <= i64::from(i32::MAX));
        assert_eq!(grown, 2.0, "the smallest power of two that fits");
    }

    #[test]
    fn the_body_is_checked() {
        let mut bytes = vec![0; OBSTACLE_BODY_OFFSET];
        bytes.extend_from_slice(&OBSTACLE_BODY);
        assert_eq!(check_body(&bytes), Ok(()));
        bytes[OBSTACLE_BODY_OFFSET] ^= 1;
        assert!(check_body(&bytes).is_err());
    }
}
