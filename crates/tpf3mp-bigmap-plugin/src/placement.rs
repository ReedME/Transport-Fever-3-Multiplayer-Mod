//! Town and industry placement spacing in 64 bits.
//!
//! `RandomLocationFactory` scores each candidate's spacing (0x8d31f0): the
//! nearest other candidate or exclusion, as `dx·dx + dy·dy` in **int32**
//! heightmap pixels (4 m), then `sqrt(float(d) · resolution²)` and
//! `minimum / distance`, or 99,999 when closer than the minimum. Pairs more
//! than 46,340 px (185.4 km) apart wrap: negative squares give NaN scores,
//! wrapped positives false proximity, and towns cluster along a long map's
//! middle (tpf2-bigmap `docs/placement-distance.md`, silver2127, MIT; no
//! code taken). Runtime industry founding scores through the same code.
//!
//! With the `placement` setting on, the score is replaced by [`score`]: the same
//! arithmetic with the squares in 64 bits, saturated at `INT_MAX`, the
//! stock minimum's start. Where no square overflows, every score is the
//! game's to the bit (the tests run the game's own code against it), so a
//! game with the setting agrees with one without on every map whose pairs
//! stay within 185 km: every map up to 724 tiles long.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use crate::host::Host;

pub use crate::build40420::{SPACING, SPACING_SQUARES, SPACING_TOO_CLOSE};
use crate::log;

/// The patch's name in `hook.log`.
pub const FIX: &str = "big maps: placement spacing";

/// The package's setting that replaces the spacing score; off leaves the
/// game's own.
pub const SETTING: &str = "placement";

/// The game's score, which the replacement never calls (the package
/// declares the hook `replace`).
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A candidate: heightmap pixels and an angle.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub x: i32,
    pub y: i32,
    pub angle: f32,
}

/// An exclusion: heightmap pixels.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exclusion {
    pub x: i32,
    pub y: i32,
}

/// MSVC's `std::vector`: three pointers.
#[repr(C)]
#[derive(Debug)]
pub struct RawVec<T> {
    pub begin: *mut T,
    pub end: *mut T,
    pub capacity: *mut T,
}

/// `dx·dx + dy·dy` without wrapping, saturated at `INT_MAX`. A component
/// past 46,340 alone squares past `INT_MAX`, so it saturates before any
/// 64-bit sum could overflow.
pub fn distance_squared(ax: i32, ay: i32, bx: i32, by: i32) -> i32 {
    let dx = i64::from(ax) - i64::from(bx);
    let dy = i64::from(ay) - i64::from(by);
    if dx.abs() > 46_340 || dy.abs() > 46_340 {
        return i32::MAX;
    }
    i32::try_from(dx * dx + dy * dy).unwrap_or(i32::MAX)
}

/// The spacing scores, one per candidate, as the game computes them but
/// with [`distance_squared`]: each candidate's nearest later candidate
/// (folded into that one's nearest too), then the exclusions, then
/// `sqrt(float(nearest) · resolution²)`.
pub fn score(
    candidates: &[Candidate],
    exclusions: &[Exclusion],
    minimum: f32,
    resolution: f32,
    out: &mut [f32],
) {
    let resolution_squared = resolution * resolution;
    let mut nearest = vec![i32::MAX; candidates.len()];
    for (i, a) in candidates.iter().enumerate() {
        let mut best = i32::MAX;
        for (j, b) in candidates.iter().enumerate().skip(i + 1) {
            let d = distance_squared(a.x, a.y, b.x, b.y);
            best = best.min(d);
            nearest[j] = nearest[j].min(d);
        }
        best = best.min(nearest[i]);
        for e in exclusions {
            best = best.min(distance_squared(a.x, a.y, e.x, e.y));
        }
        let distance = (best as f32 * resolution_squared).sqrt();
        out[i] = if minimum > distance {
            SPACING_TOO_CLOSE
        } else {
            minimum / distance
        };
    }
}

static CALLS: AtomicU64 = AtomicU64::new(0);

/// A borrowed game vector as a slice; empty when it is not one.
///
/// # Safety
///
/// `vector` is null or a live `std::vector<T>` of the game's.
unsafe fn slice<'a, T>(vector: *const RawVec<T>) -> &'a [T] {
    if vector.is_null() {
        return &[];
    }
    // SAFETY: the caller's contract: a live vector.
    let vector = unsafe { &*vector };
    if vector.begin.is_null() || vector.end < vector.begin {
        return &[];
    }
    // SAFETY: a vector's elements, begin to end.
    let len = unsafe { vector.end.offset_from(vector.begin) } as usize;
    unsafe { std::slice::from_raw_parts(vector.begin, len) }
}

/// The game's spacing score, replaced: the same arguments (Windows x64:
/// `rcx`, `rdx`, `xmm2`, `r9`, the stack).
unsafe extern "C" fn spacing(
    out: *const RawVec<f32>,
    candidates: *const RawVec<Candidate>,
    minimum: f32,
    exclusions: *const RawVec<Exclusion>,
    resolution: f32,
) {
    // SAFETY: the game's vectors, borrowed for the call, as the game's own
    // score borrows them; the output has a slot per candidate (the game's
    // score writes one per candidate without checking either).
    let (candidates, exclusions) = unsafe { (slice(candidates), slice(exclusions)) };
    if candidates.is_empty() || out.is_null() {
        return;
    }
    // SAFETY: as above.
    let out = unsafe { std::slice::from_raw_parts_mut((*out).begin, candidates.len()) };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        score(candidates, exclusions, minimum, resolution, out);
    }))
    .is_err()
    {
        out.fill(SPACING_TOO_CLOSE);
        log::line(&format!(
            "{FIX}: panicked; these candidates scored as too close"
        ));
    }
    if CALLS.fetch_add(1, Ordering::Relaxed) == 0 {
        log::line(&format!(
            "{FIX}: first scores, {} candidates, {} exclusions",
            candidates.len(),
            exclusions.len()
        ));
    }
}

/// What installing came to, for `hook.log`.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: installed ({reason})")
    } else {
        format!("{FIX}: off, {reason}")
    }
}

/// The score's two squarings must be where they were read.
pub fn check_body(bytes: &[u8]) -> Result<(), String> {
    for (at, expected) in SPACING_SQUARES {
        if bytes.get(at..at + expected.len()) != Some(&expected[..]) {
            return Err(format!(
                "{SPACING}'s int32 squares are not at +{at:#x}; not the score this replaces"
            ));
        }
    }
    Ok(())
}

/// Attaches the patch if the package's setting asks for it. Returns the
/// line for `hook.log`.
pub fn attach(host: &Host, wanted: bool) -> String {
    if !wanted {
        return outcome_line(false, &format!("{SETTING} is off; the game's own score"));
    }
    let len = SPACING_SQUARES
        .iter()
        .map(|(at, bytes)| at + bytes.len())
        .max()
        .unwrap_or_default();
    let checked = host.address(SPACING).and_then(|at| {
        let bytes = crate::host::read_bytes(at, len)
            .ok_or_else(|| format!("{SPACING} at {at:#x} is unreadable"))?;
        check_body(&bytes)?;
        host.attach(SPACING, spacing as *const (), &NEXT)?;
        Ok(at)
    });
    match checked {
        Ok(at) => outcome_line(
            true,
            &format!("at {at:#x}, separations in 64 bits, saturated at INT_MAX"),
        ),
        Err(why) => outcome_line(false, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_squarings_are_checked() {
        let mut bytes = vec![0; 0x1c1];
        for (at, square) in SPACING_SQUARES {
            bytes[at..at + square.len()].copy_from_slice(&square);
        }
        assert_eq!(check_body(&bytes), Ok(()));
        bytes[SPACING_SQUARES[1].0] = 0x90;
        assert!(check_body(&bytes).is_err());
    }
}
