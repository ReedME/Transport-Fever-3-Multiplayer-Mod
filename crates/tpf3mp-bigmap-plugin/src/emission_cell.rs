//! Coarser emission cells for new worlds: a PROPOSAL that CHANGES
//! SIMULATION RESULTS.
//!
//! `ecs::component::CreateEmissionGrid` gives a new world's noise and
//! pollution grids 16 cells per tile a side: 16 m cells on 256 m tiles,
//! 25.6 M cells per grid on 100 x 1000 tiles, five buffers of 102 MB
//! (investigation/TF3_BIGMAP_SIM_COST_2026-10-05.md §1.3). With
//! [`CELL_ENV`] set to `32`, its four `shl reg,4` become `shl reg,3` and its
//! cell factor load reads the game's own 0.125f instead of 0.0625f: 8 cells
//! per tile, 32 m cells, a quarter of the cells, memory and sweep time.
//!
//! **Every consumer reads the geometry from the grid** (CONFIRMED-static,
//! the investigation's "Switches built"): the emitters' splat and its
//! sub-grid split, `Diffuse`, `Wind` (its wind speed cap is 4 m either
//! way), `Average`, `GetSumPolygonEmission`, `GetInterpolatedEmission` (the
//! script getters, animals' scores), the visualisation grids and the save:
//! the loader reads `X0`, `Y0`, size and `gridPointSize` from the save and
//! never compares them with the map. So nothing asserts or misreads, and
//! a world's cell size travels with its save: a coarse world loads coarse
//! in any game, a stock save loads stock with the switch on. There is no
//! conflict for a load to refuse; the switch acts only when a world is
//! generated (`CreateEmissionGrid` runs only on the new-game path).
//!
//! **What changes**: the diffusion, decay and averaging weights are per
//! cell and step, so emissions spread twice as far in metres; a point
//! emitter deposits the same amount per (4x larger) cell, a radius emitter
//! a quarter; a town's sum covers a quarter of the cells. Town noise and
//! pollution, ratings, eco levels and animal scores shift.
//!
//! **Rooms**: a room's world reaches every game as the owner's save, so
//! only the game that generates it needs the switch, and every game then
//! runs the same grid. The hook says the setting in `hook.log`.
//!
//! **Fails closed**: the body between the first shift and the factor's
//! multiplies is checked byte for byte, the factor load must read 0.0625f
//! and the coarse constant must hold 0.125f; all five sites are rewritten
//! or none.

#![allow(unsafe_code)]

use crate::host::Host;

pub use crate::build40420::{
    CREATE_GRID, CREATE_GRID_BODY, CREATE_GRID_BODY_OFFSET, CREATE_GRID_COARSE_FACTOR_DELTA,
    CREATE_GRID_FACTOR_HOOK, CREATE_GRID_FACTOR_LOAD, CREATE_GRID_SHIFT_HOOKS, CREATE_GRID_SHIFTS,
    VMOVSS_XMM2_RIP,
};

/// The patch's name in `hook.log`.
pub const FIX: &str = "big maps: emission cells (PROPOSAL, changes simulation results)";

/// `32` in the game's environment gives new worlds 8 emission cells per
/// tile (32 m on 256 m tiles); unset, `0` or `16` keeps the game's 16.
pub const CELL_ENV: &str = "emission_cell";

/// The setting from [`CELL_ENV`]'s value: whether to coarsen.
pub fn cell_setting(value: Option<&str>) -> Result<bool, String> {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "0" | "16" | "off" | "no") => Ok(false),
        Some("32") => Ok(true),
        Some(other) => Err(format!(
            "{CELL_ENV}={other:?} is not 32 (or 16 for the game's own)"
        )),
    }
}

/// One rewrite, at an offset from the function: the bytes found and the
/// bytes written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub offset: usize,
    pub expected: Vec<u8>,
    pub replacement: Vec<u8>,
}

/// The rewrites for the function whose bytes from its start are `code`
/// (at least to the end of the checked body), at `function`; `read_f32`
/// reads a constant at an address. Checks everything first.
pub fn edits(
    code: &[u8],
    function: usize,
    read_f32: &dyn Fn(usize) -> Option<f32>,
) -> Result<Vec<Edit>, String> {
    let body = code
        .get(CREATE_GRID_BODY_OFFSET..CREATE_GRID_BODY_OFFSET + CREATE_GRID_BODY.len())
        .ok_or_else(|| format!("{CREATE_GRID}'s body is short"))?;
    let disp_at = CREATE_GRID_FACTOR_LOAD + VMOVSS_XMM2_RIP.len() - CREATE_GRID_BODY_OFFSET;
    for (i, (found, expected)) in body.iter().zip(CREATE_GRID_BODY).enumerate() {
        if (disp_at..disp_at + 4).contains(&i) {
            continue;
        }
        if *found != expected {
            return Err(format!(
                "{CREATE_GRID} differs at +{:#x}; not the code this patch was read on",
                CREATE_GRID_BODY_OFFSET + i
            ));
        }
    }
    let load = &code[CREATE_GRID_FACTOR_LOAD..CREATE_GRID_FACTOR_LOAD + 8];
    let disp = i32::from_le_bytes([load[4], load[5], load[6], load[7]]);
    let next = function + CREATE_GRID_FACTOR_LOAD + 8;
    let stock = next.wrapping_add_signed(disp as isize);
    if read_f32(stock) != Some(0.0625) {
        return Err(format!(
            "{CREATE_GRID}'s cell factor at {stock:#x} is not 0.0625"
        ));
    }
    let coarse = stock + CREATE_GRID_COARSE_FACTOR_DELTA;
    if read_f32(coarse) != Some(0.125) {
        return Err(format!("the constant at {coarse:#x} is not 0.125"));
    }
    let new_disp = i32::try_from(coarse as i64 - next as i64)
        .map_err(|_| "the coarse factor is out of reach".to_owned())?;
    let mut edits: Vec<Edit> = CREATE_GRID_SHIFTS
        .iter()
        .map(|&offset| {
            let expected = code[offset..offset + 3].to_vec();
            let mut replacement = expected.clone();
            replacement[2] = 3;
            Edit {
                offset,
                expected,
                replacement,
            }
        })
        .collect();
    let mut replacement = load.to_vec();
    replacement[4..].copy_from_slice(&new_disp.to_le_bytes());
    edits.push(Edit {
        offset: CREATE_GRID_FACTOR_LOAD,
        expected: load.to_vec(),
        replacement,
    });
    Ok(edits)
}

/// What installing came to, for `hook.log`.
pub fn outcome_line(installed: bool, reason: &str) -> String {
    if installed {
        format!("{FIX}: installed ({reason})")
    } else {
        format!("{FIX}: off, {reason}")
    }
}

/// Reads a float the game holds, if readable.
fn read_f32(address: usize) -> Option<f32> {
    crate::host::read::<f32>(address)
}

/// Attaches the patch if the package's setting asks for it: the five
/// rewrites, each computed and checked first. Returns the line for
/// `hook.log`.
pub fn attach(host: &Host, wanted: bool) -> String {
    if !wanted {
        return outcome_line(
            false,
            &format!("{CELL_ENV} is not 32; the game's 16 cells per tile"),
        );
    }
    let asked = host.address(CREATE_GRID).and_then(|function| {
        let len = CREATE_GRID_BODY_OFFSET + CREATE_GRID_BODY.len();
        let code = crate::host::read_bytes(function, len)
            .ok_or_else(|| format!("{CREATE_GRID} at {function:#x} is unreadable"))?;
        let edits = edits(&code, function, &read_f32)?;
        let hooks = CREATE_GRID_SHIFT_HOOKS
            .iter()
            .chain([&CREATE_GRID_FACTOR_HOOK]);
        for (edit, hook) in edits.iter().zip(hooks) {
            host.rewrite(hook, &edit.replacement)?;
        }
        Ok(function)
    });
    match asked {
        Ok(at) => outcome_line(
            true,
            &format!(
                "{CELL_ENV}=32 at {at:#x}: worlds generated from now get 8 emission cells per tile (32 m on 256 m tiles) instead of 16; a loaded save keeps the cells it was made with; noise and pollution results differ from the game's"
            ),
        ),
        Err(why) => outcome_line(false, &why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The function's code on build 40420, from its start through the body.
    fn code() -> Vec<u8> {
        let mut code = vec![0; CREATE_GRID_BODY_OFFSET];
        code.extend_from_slice(&CREATE_GRID_BODY);
        code
    }

    #[test]
    fn the_shifts_and_the_factor_are_rewritten() {
        let function = 0x10_0000;
        let next = function + CREATE_GRID_FACTOR_LOAD + 8;
        let code = code();
        let disp = i32::from_le_bytes(
            code[CREATE_GRID_FACTOR_LOAD + 4..CREATE_GRID_FACTOR_LOAD + 8]
                .try_into()
                .unwrap(),
        );
        let stock = next.wrapping_add_signed(disp as isize);
        let floats = |at: usize| {
            if at == stock {
                Some(0.0625)
            } else if at == stock + CREATE_GRID_COARSE_FACTOR_DELTA {
                Some(0.125)
            } else {
                None
            }
        };
        let made = edits(&code, function, &floats).unwrap();
        assert_eq!(made.len(), 5);
        for (edit, offset) in made.iter().zip(CREATE_GRID_SHIFTS) {
            assert_eq!(edit.offset, offset);
            assert_eq!(edit.replacement[2], 3, "shl reg,3");
        }
        assert!(
            edits(&code, function, &|_| None).is_err(),
            "the factor must read 0.0625"
        );
    }
}
