//! Terraforming through the room (docs/HOOKS.md, "Terraforming").
//!
//! The terrain tools (raise, lower, smooth, flatten and the heightmap brush:
//! `UI::TerrainModifier`), the terrain painter and the asset brush tell game
//! scripts nothing on build 40408: the game forwards `builder.proposalCreate`
//! for six other tools only. All three are `UI::ProposalAction`s, and queue
//! their `WorldBuildProposal` from `ProposalAction::DoApply` (0x549ac0), the
//! one call of `CommandList::Add` there (0x549be5, returning to 0x549bea),
//! after the factory 0x9ee860 made the command with `playerInitiated` 1. So
//! the click is counted and its apply stopped as every player's build is
//! ([`crate::builds`]), and at that call the hook reads the proposal here
//! ([`record`]): a terraform is the proposal's height grid and nothing else.
//! The GUI takes it for that click (`tpf3mp_native.built(n)`, as the module
//! editor's) and hands the room `Terraform` actions of it.
//!
//! Every game applies a terraform as the tool would: its mod's game script
//! arms the hook with the grid (`tpf3mp_native.terrain(t)`, [`arm`]) and
//! sends an empty proposal of its own, the carrier, as the player's build;
//! the hook fills the carrier's height grid at the build's apply, which it
//! sees while the room's actions are applied ([`inject`]), and the game
//! applies it as the tool's. A script cannot fill the grid itself: Lua's
//! `GridVec2f` has `width`, `height`, `x0`, `y0` and `at` and no setter
//! (`RegisterUsertypesBase`, 0x12e4580). TPF2's TPF2-MP did the same with a
//! file (`tpf2-multiplayer`, `mod/mp_lockstep_1/res/scripts/mp/terrain.lua`).
//!
//! The layout (build 40408, read with `tools/tpfre`):
//!
//! - `Proposal.terrain` (a `ProposalTerrain`) at + 0x2d8, its
//!   `baseHeightMod` first (the Lua bindings: `RegisterUsertypesTransport`,
//!   0x22c1ab0, registers `Proposal` with `terrain` at 0x2d8 and
//!   `ProposalTerrain` with `baseHeightMod` at 0);
//! - the `Proposal`'s destructor (0x48ee90) frees three vectors there, at
//!   0x2e8 (8-byte elements: the heights' `Vec2f` cells), 0x310 (bytes: the
//!   materials a paint sets) and 0x338 (4-byte words: their mask), each
//!   0x28 past the last: three grids `{ x0, y0, width, height; data }` of
//!   0x28 bytes, as TPF2's `Grid<T>`. The `Proposal` is 0x358 bytes
//!   (`ProposalAction::DoApply` frees it with that size);
//! - its vectors are allocated by the game's `operator new` (0x3184230,
//!   the UCRT's `malloc`) and freed by `operator delete` (0x3184970, `free`),
//!   with MSVC's alignment of 32 for 4 KiB or more, the block's own address
//!   just before the data (the destructor checks it).
//!
//! INFERRED, not yet seen in the game: the order of a grid's four integers
//! (TPF2's; a grid whose cell count is not width times height is refused),
//! that a cell is `{ height, height before }` (TPF2's; both are carried
//! whatever they are), that the cells go row by row, that the tool's
//! proposal has no street part, and that the carrier's grid is empty when
//! the game script sends it.

#![allow(unsafe_code)]

use std::sync::{Mutex, PoisonError};

use tpf3mp_proto::{
    BoundedVec,
    action::{Action, MAX_PAINT_RUNS, PaintGrid, PaintRun, TerrainPaint},
    lua::{LuaValue, action_from_lua, action_to_lua},
};

use crate::modules::{Memory, i32_at, read, u64_at, vector};

/// The profile's name for `ProposalAction::DoApply`'s call of
/// `CommandList::Add`. `Add` returns 5 bytes past it.
pub const DO_APPLY_ADD_CALL: &str = "ProposalAction::DoApply/Add call";

pub use crate::build_data::native::terrain::layout;

/// Most cells one stroke's grid has before it is a misread: a square of 256
/// cells, 1 km on a side at 4 m. The GUI cuts what it hands the room into
/// actions the room carries ([`tpf3mp_proto::action::MAX_TERRAIN_CELLS`]).
pub const MAX_CELLS: usize = 65_536;

/// A terraform's grid: its first cell, its size in cells, and each cell's
/// two values, row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub x0: i32,
    pub y0: i32,
    pub width: u32,
    pub height: u32,
    pub cells: Vec<(f32, f32)>,
}

impl Grid {
    /// The grid as the GUI takes it from `built(n)`: `{ terrain = { x0 =,
    /// y0 =, width =, height =, cells = { v1, w1, v2, w2, ... } } }`.
    pub fn to_lua(&self) -> LuaValue {
        let key = LuaValue::string;
        let cells = self
            .cells
            .iter()
            .flat_map(|(a, b)| [*a, *b])
            .enumerate()
            .map(|(i, v)| {
                (
                    LuaValue::Integer(i as i64 + 1),
                    LuaValue::Number(f64::from(v)),
                )
            })
            .collect();
        let grid = LuaValue::Table(vec![
            (key("x0"), LuaValue::Integer(i64::from(self.x0))),
            (key("y0"), LuaValue::Integer(i64::from(self.y0))),
            (key("width"), LuaValue::Integer(i64::from(self.width))),
            (key("height"), LuaValue::Integer(i64::from(self.height))),
            (key("cells"), LuaValue::Table(cells)),
        ]);
        LuaValue::Table(vec![(key("terrain"), grid)])
    }

    /// The grid `tpf3mp_native.terrain(t)` is given, `t` as [`Grid::to_lua`]'s
    /// `terrain` part: whole numbers in range, a cell count of width times
    /// height, every value finite. Anything else is refused.
    pub fn from_lua(value: &LuaValue) -> Result<Self, String> {
        let LuaValue::Table(entries) = value else {
            return Err("a terrain grid is a table".into());
        };
        let field = |name: &str| {
            entries
                .iter()
                .find(|(k, _)| *k == LuaValue::string(name))
                .map(|(_, v)| v)
                .ok_or_else(|| format!("a terrain grid with no {name}"))
        };
        let whole = |name: &str| -> Result<i64, String> {
            match field(name)? {
                LuaValue::Integer(n) => Ok(*n),
                LuaValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => Ok(*n as i64),
                _ => Err(format!("a terrain grid whose {name} is not a whole number")),
            }
        };
        let x0 = i32::try_from(whole("x0")?).map_err(|_| "x0 out of range".to_string())?;
        let y0 = i32::try_from(whole("y0")?).map_err(|_| "y0 out of range".to_string())?;
        let width =
            u32::try_from(whole("width")?).map_err(|_| "a width out of range".to_string())?;
        let height =
            u32::try_from(whole("height")?).map_err(|_| "a height out of range".to_string())?;
        let count = (width as usize)
            .checked_mul(height as usize)
            .filter(|n| *n > 0 && *n <= MAX_CELLS)
            .ok_or_else(|| format!("a terrain grid of {width} by {height} cells"))?;
        let LuaValue::Table(cells) = field("cells")? else {
            return Err("a terrain grid whose cells are not a list".into());
        };
        if cells.len() != 2 * count {
            return Err(format!(
                "a terrain grid of {width} by {height} cells with {} values",
                cells.len()
            ));
        }
        let mut values = vec![f32::NAN; 2 * count];
        for (k, v) in cells {
            let index = match k {
                LuaValue::Integer(n) => usize::try_from(*n).ok(),
                LuaValue::Number(n) if n.fract() == 0.0 && *n >= 1.0 => Some(*n as usize),
                _ => None,
            }
            .filter(|i| (1..=2 * count).contains(i))
            .ok_or_else(|| "a terrain grid's cells are not a list".to_string())?;
            let number = match v {
                LuaValue::Integer(n) => *n as f64,
                LuaValue::Number(n) => *n,
                _ => return Err("a terrain cell that is not a number".into()),
            };
            #[allow(clippy::cast_possible_truncation)]
            let single = number as f32;
            if !single.is_finite() {
                return Err("a terrain cell that is not finite".into());
            }
            values[index - 1] = single;
        }
        if values.iter().any(|v| v.is_nan()) {
            return Err("a terrain grid's cells are not a list".into());
        }
        Ok(Self {
            x0,
            y0,
            width,
            height,
            cells: values
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| (c[0], c[1]))
                .collect(),
        })
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let (low, high) = self
            .cells
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (v, _)| {
                (lo.min(*v), hi.max(*v))
            });
        let changed = self.cells.iter().filter(|(v, b)| v != b).count();
        format!(
            "{} by {} cells from cell ({}, {}), {} changed, heights {:.2} to {:.2} m",
            self.width, self.height, self.x0, self.y0, changed, low, high
        )
    }
}

/// One of a paint stroke's grids as the game holds it: its first cell, its
/// size in cells, and each cell's value, row by row (a mask's 0 or 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cells {
    pub x0: i32,
    pub y0: i32,
    pub width: u32,
    pub height: u32,
    pub cells: Vec<u8>,
}

impl Cells {
    fn same_place(&self, other: &Cells) -> bool {
        (self.x0, self.y0, self.width, self.height)
            == (other.x0, other.y0, other.width, other.height)
    }

    /// Rows `first..last` as a grid of runs, or why not.
    fn band(&self, first: u32, last: u32) -> Result<PaintGrid, String> {
        let w = self.width as usize;
        let slice = &self.cells[first as usize * w..last as usize * w];
        let mut runs: Vec<PaintRun> = Vec::new();
        for &value in slice {
            match runs.last_mut() {
                Some(run) if run.value == value => run.count += 1,
                _ => runs.push(PaintRun { value, count: 1 }),
            }
        }
        let runs = BoundedVec::new(runs)
            .map_err(|_| format!("a band of paint with more than {MAX_PAINT_RUNS} runs"))?;
        #[allow(clippy::cast_possible_wrap)]
        PaintGrid::new(
            self.x0,
            self.y0 + first as i32,
            self.width,
            last - first,
            runs,
        )
        .map_err(|e| e.to_string())
    }

    /// How many runs row `row` has.
    fn row_runs(&self, row: u32) -> usize {
        let w = self.width as usize;
        let cells = &self.cells[row as usize * w..(row as usize + 1) * w];
        1 + cells.windows(2).filter(|p| p[0] != p[1]).count()
    }

    fn from_grid(grid: &PaintGrid) -> Cells {
        Cells {
            x0: grid.x0,
            y0: grid.y0,
            width: grid.columns,
            height: grid.rows,
            cells: grid.cells(),
        }
    }
}

/// A terrain paint stroke as the game computed it: the materials grid
/// ([`UNCHANGED`] where the stroke leaves a texel as it is) and, where it
/// has one, the mask grid beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paint {
    pub materials: Cells,
    pub mask: Option<Cells>,
}

/// The material a paint grid's texel has where the stroke leaves it.
pub const UNCHANGED: u8 = 255;

impl Paint {
    /// The stroke as the room's actions: bands of whole rows, each within
    /// [`MAX_PAINT_RUNS`] runs, materials and mask together. A mask that
    /// is not the materials' own rectangle goes whole with them, in one
    /// action, or the stroke is refused.
    pub fn actions(&self) -> Result<Vec<TerrainPaint>, String> {
        let m = &self.materials;
        if let Some(mask) = &self.mask
            && !mask.same_place(m)
        {
            return Ok(vec![TerrainPaint {
                materials: m.band(0, m.height)?,
                mask: Some(mask.band(0, mask.height)?),
            }]);
        }
        let mut out = Vec::new();
        let mut first = 0;
        while first < m.height {
            let mut last = first;
            let mut runs = 0;
            while last < m.height {
                let more = m.row_runs(last) + self.mask.as_ref().map_or(0, |k| k.row_runs(last));
                if runs + more > MAX_PAINT_RUNS && last > first {
                    break;
                }
                runs += more;
                last += 1;
            }
            out.push(TerrainPaint {
                materials: m.band(first, last)?,
                mask: self
                    .mask
                    .as_ref()
                    .map(|k| k.band(first, last))
                    .transpose()?,
            });
            first = last;
        }
        Ok(out)
    }

    /// The stroke again from the room's action.
    pub fn from_action(paint: &TerrainPaint) -> Paint {
        Paint {
            materials: Cells::from_grid(&paint.materials),
            mask: paint.mask.as_ref().map(Cells::from_grid),
        }
    }

    /// The stroke as the GUI takes it from `built(n)`: `{ paint = { a1,
    /// a2, ... } }`, each a `Paint` action as the mod's Lua names it.
    pub fn to_lua(&self) -> Result<LuaValue, String> {
        let actions = self
            .actions()?
            .into_iter()
            .enumerate()
            .map(|(i, part)| {
                action_to_lua(&Action::Paint(part))
                    .map(|a| (LuaValue::Integer(i as i64 + 1), a))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(LuaValue::Table(vec![(
            LuaValue::string("paint"),
            LuaValue::Table(actions),
        )]))
    }

    /// The stroke `tpf3mp_native.terrain(t)` is given, `t` a `Paint`
    /// action's body as the mod's Lua names it.
    pub fn from_lua(value: &LuaValue) -> Result<Paint, String> {
        let wrapped = LuaValue::Table(vec![(LuaValue::string("Paint"), value.clone())]);
        let action = action_from_lua(&wrapped).map_err(|e| format!("a paint stroke: {e}"))?;
        action.validate().map_err(|e| e.to_string())?;
        match action {
            Action::Paint(paint) => Ok(Paint::from_action(&paint)),
            _ => Err("not a paint stroke".into()),
        }
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let m = &self.materials;
        let changed = m.cells.iter().filter(|c| **c != UNCHANGED).count();
        let mask = match &self.mask {
            None => "no mask".to_owned(),
            Some(k) => format!(
                "a mask of {} by {} with {} set",
                k.width,
                k.height,
                k.cells.iter().filter(|c| **c != 0).count()
            ),
        };
        format!(
            "{} by {} texels from ({}, {}), {} painted, {}",
            m.width, m.height, m.x0, m.y0, changed, mask
        )
    }
}

/// One asset the asset brush places: its model's file and its world matrix
/// (columns of four).
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub model: String,
    pub matrix: [f32; 16],
}

/// An asset brush stroke as the tool proposed it: the asset groups it
/// replaces, by entity, and the groups it builds.
#[derive(Debug, Clone, PartialEq)]
pub struct Assets {
    pub removed: Vec<i32>,
    pub groups: Vec<Vec<Placed>>,
}

impl Assets {
    /// As the GUI takes it from `built(n)`: `{ assets = { removed = { id,
    /// ... }, groups = { { { model =, m = { 16 numbers } }, ... }, ... } } }`.
    /// The GUI names each removed group as the room does, then makes the
    /// stroke's actions.
    pub fn to_lua(&self) -> LuaValue {
        let key = LuaValue::string;
        let list = |items: Vec<LuaValue>| {
            LuaValue::Table(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (LuaValue::Integer(i as i64 + 1), v))
                    .collect(),
            )
        };
        let removed = list(
            self.removed
                .iter()
                .map(|id| LuaValue::Integer(i64::from(*id)))
                .collect(),
        );
        let groups = list(
            self.groups
                .iter()
                .map(|group| {
                    list(
                        group
                            .iter()
                            .map(|a| {
                                LuaValue::Table(vec![
                                    (key("model"), LuaValue::string(&a.model)),
                                    (
                                        key("m"),
                                        list(
                                            a.matrix
                                                .iter()
                                                .map(|v| LuaValue::Number(f64::from(*v)))
                                                .collect(),
                                        ),
                                    ),
                                ])
                            })
                            .collect(),
                    )
                })
                .collect(),
        );
        LuaValue::Table(vec![(
            key("assets"),
            LuaValue::Table(vec![(key("removed"), removed), (key("groups"), groups)]),
        )])
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let assets: usize = self.groups.iter().map(Vec::len).sum();
        format!(
            "{} group(s) of {} asset(s), replacing {} group(s)",
            self.groups.len(),
            assets,
            self.removed.len()
        )
    }
}

/// What a terrain tool's click queued: a terraform's heights, paint, or the
/// asset brush's groups.
#[derive(Debug, Clone, PartialEq)]
pub enum Stroke {
    Heights(Grid),
    Paint(Paint),
    Assets(Assets),
}

impl Stroke {
    /// One line for the log.
    pub fn summary(&self) -> String {
        match self {
            Stroke::Heights(grid) => grid.summary(),
            Stroke::Paint(paint) => paint.summary(),
            Stroke::Assets(assets) => assets.summary(),
        }
    }

    fn noun(&self) -> &'static str {
        match self {
            Stroke::Heights(_) => "terraform",
            Stroke::Paint(_) => "paint",
            Stroke::Assets(_) => "asset brush",
        }
    }

    /// As the GUI takes it from `built(n)`.
    pub fn to_lua(&self) -> Result<LuaValue, String> {
        match self {
            Stroke::Heights(grid) => Ok(grid.to_lua()),
            Stroke::Paint(paint) => paint.to_lua(),
            Stroke::Assets(assets) => Ok(assets.to_lua()),
        }
    }
}

/// How many elements the vector at `offset` of `head` has.
fn count(head: &[u8], offset: usize, stride: usize, what: &str) -> Result<usize, String> {
    vector(head, offset, stride, usize::MAX / stride, what).map(|(_, n)| n)
}

/// Why the proposal in `head` changes streets, if it does.
fn no_streets(head: &[u8]) -> Result<(), String> {
    for (offset, stride, what) in [
        (layout::ADDED_NODES, layout::NODE_SIZE, "the nodes added"),
        (
            layout::ADDED_SEGMENTS,
            layout::SEGMENT_SIZE,
            "the segments added",
        ),
        (
            layout::REMOVED_NODES,
            layout::NODE_SIZE,
            "the nodes removed",
        ),
        (
            layout::REMOVED_SEGMENTS,
            layout::SEGMENT_SIZE,
            "the segments removed",
        ),
        (
            layout::EDGE_OBJECTS_TO_ADD,
            layout::EDGE_OBJECT_SIZE,
            "the edge objects added",
        ),
        (
            layout::EDGE_OBJECTS_TO_REMOVE,
            4,
            "the edge objects removed",
        ),
    ] {
        if count(head, offset, stride, what)? > 0 {
            return Err(format!("a terrain tool's build with {what}"));
        }
    }
    Ok(())
}

/// Why the proposal in `head` is more than terrain, if it is.
fn only_terrain(head: &[u8]) -> Result<(), String> {
    no_streets(head)?;
    if count(head, layout::TO_REMOVE, 4, "the entities removed")? > 0 {
        return Err("a terrain tool's build with the entities removed".into());
    }
    if count(
        head,
        layout::TO_ADD,
        layout::ENTITY_SIZE,
        "the constructions added",
    )? > 0
    {
        return Err("the asset brush: the room does not carry it yet".into());
    }
    Ok(())
}

/// Reads what the `WorldBuildProposal` payload at `payload` changes: its
/// height grid, or its paint, which must be all it changes.
pub fn decode(memory: &dyn Memory, payload: usize) -> Result<Stroke, String> {
    let head = read(memory, payload, layout::PROPOSAL_LEN, "the proposal")?;
    if count(
        &head,
        layout::TO_ADD,
        layout::ENTITY_SIZE,
        "the constructions added",
    )? > 0
        || count(&head, layout::TO_REMOVE, 4, "the entities removed")? > 0
    {
        return decode_assets(memory, &head).map(Stroke::Assets);
    }
    only_terrain(&head)?;
    let materials = count(&head, layout::MATERIALS + layout::GRID_DATA, 1, "the paint")?;
    let mask = count(
        &head,
        layout::MASK + layout::GRID_DATA,
        4,
        "the paint's mask",
    )?;
    let (begin, cells) = vector(
        &head,
        layout::HEIGHTS + layout::GRID_DATA,
        layout::CELL_SIZE,
        MAX_CELLS,
        "the terrain grid",
    )?;
    if cells == 0 {
        if materials > 0 || mask > 0 {
            return decode_paint(memory, &head).map(Stroke::Paint);
        }
        return Err("a terrain tool's build that changes nothing".into());
    }
    if materials > 0 || mask > 0 {
        return Err("a terraform that paints too: the room does not carry paint yet".into());
    }
    let grid = &head[layout::HEIGHTS..layout::HEIGHTS + layout::GRID_SIZE];
    let x0 = i32_at(grid, layout::GRID_X0);
    let y0 = i32_at(grid, layout::GRID_Y0);
    let (width, height) = (
        i32_at(grid, layout::GRID_WIDTH),
        i32_at(grid, layout::GRID_HEIGHT),
    );
    let fits = u32::try_from(width)
        .ok()
        .zip(u32::try_from(height).ok())
        .filter(|(w, h)| (*w as usize).checked_mul(*h as usize) == Some(cells));
    let Some((width, height)) = fits else {
        return Err(format!(
            "a terrain grid of {width} by {height} with {cells} cells"
        ));
    };
    let raw = read(
        memory,
        begin,
        cells * layout::CELL_SIZE,
        "the terrain cells",
    )?;
    let float = |at: usize| {
        let mut word = [0u8; 4];
        word.copy_from_slice(&raw[at..at + 4]);
        f32::from_le_bytes(word)
    };
    let cells: Vec<(f32, f32)> = (0..cells)
        .map(|i| (float(i * 8), float(i * 8 + 4)))
        .collect();
    if cells.iter().any(|(a, b)| !a.is_finite() || !b.is_finite()) {
        return Err("a terrain cell that is not finite".into());
    }
    Ok(Stroke::Heights(Grid {
        x0,
        y0,
        width,
        height,
        cells,
    }))
}

/// Most asset groups one brush stroke adds or replaces before it is a
/// misread.
pub const MAX_ASSET_GROUPS: usize = 4096;
/// Most assets one group holds before it is a misread.
pub const MAX_GROUP_ASSETS: usize = 65_536;

/// The asset brush's stroke in a proposal whose head is `head`: the groups
/// it removes, and each group it adds, every model's name and matrix.
/// Nothing else may be in it: no street, no terrain, no construction of a
/// file.
fn decode_assets(memory: &dyn Memory, head: &[u8]) -> Result<Assets, String> {
    no_streets(head)?;
    let grids = &head[layout::HEIGHTS..layout::PROPOSAL_LEN];
    if grids.iter().any(|b| *b != 0) {
        return Err("an asset stroke that changes the terrain too".into());
    }
    let (begin, count) = vector(
        head,
        layout::TO_REMOVE,
        4,
        MAX_ASSET_GROUPS,
        "the groups removed",
    )?;
    let removed = crate::modules::ids(memory, begin, count, 4, 0, "the groups removed")?;
    let (begin, count) = vector(
        head,
        layout::TO_ADD,
        layout::ENTITY_SIZE,
        MAX_ASSET_GROUPS,
        "the groups added",
    )?;
    let mut groups = Vec::with_capacity(count);
    for i in 0..count {
        let record = begin + i * layout::ENTITY_SIZE;
        let what = format!("group {}", i + 1);
        let file = crate::modules::res_name(memory, record + layout::ENTITY_FILE, &what)?;
        if !file.is_empty() {
            return Err(format!(
                "the asset brush: {what} is a construction of {file}"
            ));
        }
        let raw = read(memory, record + layout::ASSET_SUBS, 24, &what)?;
        let (subs, bytes) = vector(&raw, 0, 1, usize::MAX, &what)?;
        if bytes == 0 {
            return Err(format!("the asset brush: {what} has no subconstruction"));
        }
        let sub = read(memory, subs + layout::SUB_MODELS, 24, &what)?;
        let (models, n) = vector(&sub, 0, layout::MODEL_SIZE, MAX_GROUP_ASSETS, &what)?;
        if n == 0 {
            return Err(format!("the asset brush: {what} has no models"));
        }
        let mut group = Vec::with_capacity(n);
        for k in 0..n {
            let at = models + k * layout::MODEL_SIZE;
            let model = crate::modules::res_name(memory, at, &what)?;
            if model.is_empty() {
                return Err(format!("the asset brush: {what} has a model of no file"));
            }
            let m = read(memory, at + layout::MODEL_TRANSF, 64, &what)?;
            let mut matrix = [0f32; 16];
            for (j, v) in matrix.iter_mut().enumerate() {
                let mut w = [0u8; 4];
                w.copy_from_slice(&m[j * 4..j * 4 + 4]);
                *v = f32::from_le_bytes(w);
            }
            let affine = matrix.iter().all(|v| v.is_finite())
                && matrix[3] == 0.0
                && matrix[7] == 0.0
                && matrix[11] == 0.0
                && matrix[15] == 1.0;
            if !affine {
                return Err(format!(
                    "the asset brush: {what}'s {model} has a matrix that is not a placement: {matrix:?}"
                ));
            }
            group.push(Placed { model, matrix });
        }
        groups.push(group);
    }
    if removed.is_empty() && groups.is_empty() {
        return Err("an asset stroke that changes nothing".into());
    }
    Ok(Assets { removed, groups })
}

/// A grid header's first cell and size, at `at` of `head`.
fn header(head: &[u8], at: usize) -> (i32, i32, i32, i32) {
    let grid = &head[at..at + layout::GRID_SIZE];
    (
        i32_at(grid, layout::GRID_X0),
        i32_at(grid, layout::GRID_Y0),
        i32_at(grid, layout::GRID_WIDTH),
        i32_at(grid, layout::GRID_HEIGHT),
    )
}

/// The size of a grid of `cells` cells whose header says `width` by
/// `height`, or why not.
fn fits(what: &str, width: i32, height: i32, cells: usize) -> Result<(u32, u32), String> {
    u32::try_from(width)
        .ok()
        .zip(u32::try_from(height).ok())
        .filter(|(w, h)| *w > 0 && *h > 0 && (*w as usize).checked_mul(*h as usize) == Some(cells))
        .ok_or_else(|| format!("{what} of {width} by {height} with {cells} cells"))
}

/// Most texels one paint stroke's grid has before it is a misread: 16 times
/// a terraform's, the painter's texels being a quarter of a height cell's
/// side.
pub const MAX_TEXELS: usize = MAX_CELLS * 16;

/// The paint of a proposal whose head is `head`: its materials grid, a
/// byte a texel, and its mask, a `vector<bool>`, with its count of bits.
fn decode_paint(memory: &dyn Memory, head: &[u8]) -> Result<Paint, String> {
    let (begin, bytes) = vector(
        head,
        layout::MATERIALS + layout::GRID_DATA,
        1,
        MAX_TEXELS,
        "the paint",
    )?;
    let (x0, y0, w, h) = header(head, layout::MATERIALS);
    if bytes == 0 {
        return Err(format!(
            "terrain paint with a mask and no materials (a grid of {w} by {h})"
        ));
    }
    let (width, height) = fits("a paint grid", w, h, bytes)?;
    let materials = Cells {
        x0,
        y0,
        width,
        height,
        cells: read(memory, begin, bytes, "the paint's texels")?,
    };
    let (begin, words) = vector(
        head,
        layout::MASK + layout::GRID_DATA,
        layout::MASK_WORD,
        MAX_TEXELS / 32 + 1,
        "the paint's mask",
    )?;
    let bits = u64_at(head, layout::MASK + layout::MASK_BITS) as usize;
    let (mx0, my0, mw, mh) = header(head, layout::MASK);
    if words == 0 {
        if bits != 0 {
            return Err(format!("a paint mask of {bits} bits and no words"));
        }
        return Ok(Paint {
            materials,
            mask: None,
        });
    }
    if words != bits.div_ceil(32) {
        return Err(format!("a paint mask of {bits} bits in {words} words"));
    }
    let (mwidth, mheight) = fits("a paint mask", mw, mh, bits)?;
    let raw = read(
        memory,
        begin,
        words * layout::MASK_WORD,
        "the paint's mask words",
    )?;
    let cells = (0..bits).map(|i| (raw[i / 8] >> (i % 8)) & 1).collect();
    Ok(Paint {
        materials,
        mask: Some(Cells {
            x0: mx0,
            y0: my0,
            width: mwidth,
            height: mheight,
            cells,
        }),
    })
}

/// One vector a probe found: where it sits (offsets from the record, one per
/// level), its data and its length in bytes.
struct Found {
    path: Vec<usize>,
    begin: usize,
    bytes: usize,
}

/// A vector `{begin, end, capacity}` at `offset` of `raw` that looks like
/// one: in order, not null, at most 4 MiB.
fn plausible_vector(raw: &[u8], offset: usize) -> Option<(usize, usize)> {
    let begin = u64_at(raw, offset);
    let end = u64_at(raw, offset + 8);
    let capacity = u64_at(raw, offset + 16);
    (begin >= 0x10000 && end > begin && capacity >= end && end - begin <= 4 << 20)
        .then(|| (begin as usize, (end - begin) as usize))
}

/// Whether a model's resource name (two MSVC strings, as `res_name` reads
/// one) is at `address`.
fn model_at(memory: &dyn Memory, address: usize) -> Option<String> {
    crate::modules::res_name(memory, address, "a model")
        .ok()
        .filter(|name| name.ends_with(".mdl"))
}

/// The vectors within `len` bytes at `base`, two levels deep, whose first
/// element starts with a model's name.
fn find_models(
    memory: &dyn Memory,
    base: usize,
    len: usize,
    depth: u8,
    path: &[usize],
    out: &mut Vec<Found>,
) {
    let Some(raw) = memory.read(base, len) else {
        return;
    };
    for offset in (0..len.saturating_sub(24)).step_by(8) {
        let Some((begin, bytes)) = plausible_vector(&raw, offset) else {
            continue;
        };
        let mut here = path.to_vec();
        here.push(offset);
        if model_at(memory, begin).is_some() {
            out.push(Found {
                path: here,
                begin,
                bytes,
            });
        } else if depth > 0 && out.len() < 8 {
            find_models(memory, begin, bytes.min(0x400), depth - 1, &here, out);
        }
    }
}

/// What the asset brush's proposal at `payload` holds, for the log: the
/// groups it removes, and in each group it adds (the first three), every
/// vector of models, its stride and its first models' names and matrices.
/// A probe while the room does not carry the brush: it reads, never writes.
pub fn probe_assets(memory: &dyn Memory, payload: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(head) = read(memory, payload, layout::PROPOSAL_LEN, "the proposal") else {
        return vec!["asset probe: the proposal does not read".into()];
    };
    if let Ok((begin, count)) = vector(&head, layout::TO_REMOVE, 4, 4096, "removed")
        && let Ok(ids) = crate::modules::ids(memory, begin, count, 4, 0, "removed")
    {
        lines.push(format!("asset probe: removes {count} entities {ids:?}"));
    }
    let Ok((begin, count)) = vector(&head, layout::TO_ADD, layout::ENTITY_SIZE, 4096, "added")
    else {
        return lines;
    };
    lines.push(format!(
        "asset probe: adds {count} records of {:#x} bytes",
        layout::ENTITY_SIZE
    ));
    for i in 0..count.min(3) {
        let record = begin + i * layout::ENTITY_SIZE;
        if let Some(raw) = memory.read(record, 0x40) {
            let ints: Vec<i32> = (0..16).map(|k| i32_at(&raw, k * 4)).collect();
            lines.push(format!("asset probe: record {i} starts {ints:?}"));
        }
        let mut found = Vec::new();
        find_models(memory, record, layout::ENTITY_SIZE, 2, &[], &mut found);
        if found.is_empty() {
            lines.push(format!(
                "asset probe: record {i}: no vector of models found"
            ));
        }
        for f in found.iter().take(4) {
            let path: Vec<String> = f.path.iter().map(|o| format!("{o:#x}")).collect();
            let mut stride = None;
            for candidate in [0x40, 0x48, 0x50, 0x60, 0x70, 0x80, 0x88, 0x90, 0xa0, 0xc0] {
                if f.bytes % candidate == 0
                    && (f.bytes == candidate || model_at(memory, f.begin + candidate).is_some())
                {
                    stride = Some(candidate);
                    break;
                }
            }
            let first = model_at(memory, f.begin).unwrap_or_default();
            let floats = memory
                .read(f.begin + 0x40, 0x40)
                .map(|raw| {
                    (0..16)
                        .map(|k| {
                            let mut w = [0u8; 4];
                            w.copy_from_slice(&raw[k * 4..k * 4 + 4]);
                            format!("{:.3}", f32::from_le_bytes(w))
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            lines.push(format!(
                "asset probe: record {i}: models at [{}], {} bytes, stride {:?}, first {first}, then {floats}",
                path.join(" > "),
                f.bytes,
                stride.map(|s| format!("{s:#x}"))
            ));
        }
    }
    lines
}

/// Where `Add` returns to from `DoApply`'s call; 0 when the profile does not
/// name it.
static DO_APPLY_RETURN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Notes `DoApply`'s call of `Add`, at `call` (its absolute address).
pub fn set_call(call: usize) {
    DO_APPLY_RETURN.store(call + 5, std::sync::atomic::Ordering::Release);
}

/// Whether a call of `Add` returning to `return_address` is a terrain tool's
/// (or the painter's, or the asset brush's).
pub fn is_terrain_tool(return_address: usize) -> bool {
    let at = DO_APPLY_RETURN.load(std::sync::atomic::Ordering::Acquire);
    at != 0 && return_address == at
}

/// Keeps what a terrain tool queued at click `click` (the count before it)
/// for the GUI, as [`crate::modules::record`] keeps the module editor's:
/// the grid, or why the room cannot carry it. Logs one line.
pub fn record(memory: &dyn Memory, click: u64, payload: usize) {
    let read = decode(memory, payload).and_then(|stroke| stroke.to_lua().map(|lua| (stroke, lua)));
    match &read {
        Ok((stroke, _)) => crate::log::line(&format!(
            "{}: click {click} queued {}",
            stroke.noun(),
            stroke.summary()
        )),
        Err(why) => {
            crate::log::line(&format!(
                "terrain tool: click {click} cannot go to the room: {why}"
            ));
            if why.starts_with("the asset brush") || why.contains("asset stroke") {
                for line in probe_assets(memory, payload) {
                    crate::log::line(&line);
                }
            }
        }
    }
    // A reason says whose it is, so the GUI does not take it for the module
    // editor's.
    crate::modules::keep(
        click,
        read.map(|(_, lua)| lua)
            .map_err(|why| format!("terrain tool: {why}")),
    );
}

/// The stroke armed for the next build the room's actions send, and whether
/// one was filled with it.
struct Armed {
    stroke: Option<Stroke>,
    filled: bool,
}

static ARMED: Mutex<Option<Armed>> = Mutex::new(None);

/// Arms the next build applied while the room's actions run with `stroke`.
pub fn arm(stroke: Stroke) {
    *ARMED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Armed {
        stroke: Some(stroke),
        filled: false,
    });
}

/// Disarms: whether a build was filled with the grid armed, or `None` when
/// none was armed.
pub fn disarm() -> Option<bool> {
    ARMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .map(|armed| armed.filled)
}

/// Memory a carrier is filled in: the game's, or a test's.
pub trait Heap: Memory {
    /// Writes `bytes` at `address`; false when it cannot.
    fn write(&mut self, address: usize, bytes: &[u8]) -> bool;
    /// `len` bytes from the game's own allocator, as a `std::vector` of
    /// that many bytes allocates them, or `None`.
    fn allocate(&mut self, len: usize) -> Option<usize>;
}

/// Fills the carrier at `payload`, an empty proposal, with `stroke`: its
/// grids' cells in vectors of the game's own, and their headers. Refuses a
/// carrier with anything in it.
pub fn fill(heap: &mut dyn Heap, payload: usize, stroke: &Stroke) -> Result<(), String> {
    let head = read(heap, payload, layout::PROPOSAL_LEN, "the carrier")?;
    only_terrain(&head).map_err(|why| format!("the carrier is not empty: {why}"))?;
    let grids = &head[layout::HEIGHTS..layout::PROPOSAL_LEN];
    if grids.iter().any(|b| *b != 0) {
        return Err("the carrier's terrain grids are not empty".into());
    }
    match stroke {
        Stroke::Heights(grid) => fill_heights(heap, payload, grid),
        Stroke::Paint(paint) => fill_paint(heap, payload, paint),
        // Every game builds an asset stroke's groups through the game's own
        // Lua (tpf3mp/apply.lua): never armed.
        Stroke::Assets(_) => Err("an asset stroke goes through no carrier".into()),
    }
}

/// `bytes` in a vector of the game's own, and the grid header at `at` that
/// names it.
fn put_grid(
    heap: &mut dyn Heap,
    at: usize,
    (x0, y0, width, height): (i32, i32, u32, u32),
    bytes: &[u8],
) -> Result<(), String> {
    let data = heap
        .allocate(bytes.len())
        .ok_or_else(|| "the game's allocator gave nothing".to_string())?;
    if !heap.write(data, bytes) {
        return Err("the cells could not be written".into());
    }
    let mut header = Vec::with_capacity(layout::GRID_SIZE);
    header.extend_from_slice(&x0.to_le_bytes());
    header.extend_from_slice(&y0.to_le_bytes());
    #[allow(clippy::cast_possible_wrap)]
    {
        header.extend_from_slice(&(width as i32).to_le_bytes());
        header.extend_from_slice(&(height as i32).to_le_bytes());
    }
    let end = (data + bytes.len()) as u64;
    header.extend_from_slice(&(data as u64).to_le_bytes());
    header.extend_from_slice(&end.to_le_bytes());
    header.extend_from_slice(&end.to_le_bytes());
    if !heap.write(at, &header) {
        return Err("the grid could not be written".into());
    }
    Ok(())
}

/// The paint's materials, and its mask packed into words with its count of
/// bits, in the carrier's grids.
fn fill_paint(heap: &mut dyn Heap, payload: usize, paint: &Paint) -> Result<(), String> {
    let m = &paint.materials;
    if m.cells.is_empty() || m.cells.len() != (m.width as usize) * (m.height as usize) {
        return Err("paint whose texels are not width times height".into());
    }
    put_grid(
        heap,
        payload + layout::MATERIALS,
        (m.x0, m.y0, m.width, m.height),
        &m.cells,
    )?;
    if let Some(k) = &paint.mask {
        if k.cells.is_empty() || k.cells.len() != (k.width as usize) * (k.height as usize) {
            return Err("a paint mask whose cells are not width times height".into());
        }
        let mut words = vec![0u8; k.cells.len().div_ceil(32) * layout::MASK_WORD];
        for (i, bit) in k.cells.iter().enumerate() {
            if *bit != 0 {
                words[i / 8] |= 1 << (i % 8);
            }
        }
        put_grid(
            heap,
            payload + layout::MASK,
            (k.x0, k.y0, k.width, k.height),
            &words,
        )?;
        if !heap.write(
            payload + layout::MASK + layout::MASK_BITS,
            &(k.cells.len() as u64).to_le_bytes(),
        ) {
            return Err("the paint mask's count of bits could not be written".into());
        }
    }
    Ok(())
}

/// The terraform's cells, and their header, in the carrier's height grid.
fn fill_heights(heap: &mut dyn Heap, payload: usize, grid: &Grid) -> Result<(), String> {
    let count = grid.cells.len();
    if count == 0 || count != (grid.width as usize) * (grid.height as usize) || count > MAX_CELLS {
        return Err("a grid whose cells are not width times height".into());
    }
    let mut bytes = Vec::with_capacity(count * layout::CELL_SIZE);
    for (a, b) in &grid.cells {
        bytes.extend_from_slice(&a.to_le_bytes());
        bytes.extend_from_slice(&b.to_le_bytes());
    }
    put_grid(
        heap,
        payload + layout::HEIGHTS,
        (grid.x0, grid.y0, grid.width, grid.height),
        &bytes,
    )
}

/// At the apply of a build the room's actions send: fills it with the stroke
/// armed, if one is. `Ok(true)` when it filled, `Ok(false)` when nothing was
/// armed, `Err(why)` when the build is refused (the stroke is then spent).
pub fn inject(heap: &mut dyn Heap, payload: usize) -> Result<bool, String> {
    let stroke = {
        let mut armed = ARMED.lock().unwrap_or_else(PoisonError::into_inner);
        match armed.as_mut().and_then(|a| a.stroke.take()) {
            Some(stroke) => stroke,
            None => return Ok(false),
        }
    };
    fill(heap, payload, &stroke)?;
    if let Some(armed) = ARMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        armed.filled = true;
    }
    crate::log::line(&format!(
        "{}: filled the room's carrier with {}",
        stroke.noun(),
        stroke.summary()
    ));
    Ok(true)
}

/// The running game's memory and allocator.
pub struct GameHeap;

impl Memory for GameHeap {
    fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
        crate::modules::Process.read(address, len)
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
impl Heap for GameHeap {
    fn write(&mut self, address: usize, bytes: &[u8]) -> bool {
        if address == 0 || !crate::image::readable(address, bytes.len()) {
            return false;
        }
        // SAFETY: committed memory of the game's heap (the payload being
        // applied, or a block its allocator just gave), checked readable;
        // the game's thread is inside the apply that owns it.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len()) };
        true
    }

    fn allocate(&mut self, len: usize) -> Option<usize> {
        let malloc = crt_malloc()?;
        if len < layout::BIG_ALLOCATION {
            // SAFETY: the UCRT's malloc, as the game's operator new calls it.
            let block = unsafe { malloc(len) } as usize;
            return (block != 0).then_some(block);
        }
        // SAFETY: as above.
        let block = unsafe { malloc(len + layout::BIG_EXTRA) } as usize;
        if block == 0 {
            return None;
        }
        let data = (block + layout::BIG_EXTRA) & !(layout::BIG_ALIGNMENT - 1);
        // SAFETY: `data - 8` is inside the block just allocated (at least 8
        // bytes past its start), where MSVC keeps the block's address.
        unsafe { ((data - 8) as *mut usize).write_unaligned(block) };
        Some(data)
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
impl Heap for GameHeap {
    fn write(&mut self, _address: usize, _bytes: &[u8]) -> bool {
        false
    }

    fn allocate(&mut self, _len: usize) -> Option<usize> {
        None
    }
}

/// The UCRT's `malloc`, which the game's `operator new` calls (it imports it
/// from `api-ms-win-crt-heap-l1-1-0.dll`, which forwards to `ucrtbase.dll`).
#[cfg(all(windows, target_arch = "x86_64"))]
fn crt_malloc() -> Option<unsafe extern "C" fn(usize) -> *mut u8> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    static MALLOC: OnceLock<usize> = OnceLock::new();
    let address = *MALLOC.get_or_init(|| {
        for module in ["api-ms-win-crt-heap-l1-1-0.dll", "ucrtbase.dll"] {
            let wide: Vec<u16> = module.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a NUL-terminated wide string; a loaded module's handle
            // is not ours to free.
            let handle = unsafe { GetModuleHandleW(wide.as_ptr()) };
            if handle.is_null() {
                continue;
            }
            // SAFETY: a valid module handle and a NUL-terminated name.
            if let Some(function) = unsafe { GetProcAddress(handle, c"malloc".as_ptr().cast()) } {
                return function as usize;
            }
        }
        0
    });
    // SAFETY: the UCRT's malloc, whose ABI this is.
    (address != 0).then(|| unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> *mut u8>(address)
    })
}

/// Serialises the tests that arm a grid.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::u64_at;

    /// Memory laid out by a test, with an allocator.
    struct Fake {
        regions: Vec<(usize, Vec<u8>)>,
        next: usize,
        allocated: Vec<usize>,
    }

    impl Memory for Fake {
        fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
            self.regions.iter().find_map(|(base, bytes)| {
                let offset = address.checked_sub(*base)?;
                bytes
                    .get(offset..offset.checked_add(len)?)
                    .map(<[u8]>::to_vec)
            })
        }
    }

    impl Heap for Fake {
        fn write(&mut self, address: usize, bytes: &[u8]) -> bool {
            for (base, region) in &mut self.regions {
                if let Some(offset) = address.checked_sub(*base)
                    && offset + bytes.len() <= region.len()
                {
                    region[offset..offset + bytes.len()].copy_from_slice(bytes);
                    return true;
                }
            }
            false
        }

        fn allocate(&mut self, len: usize) -> Option<usize> {
            let at = self.alloc(vec![0; len]);
            self.allocated.push(at);
            Some(at)
        }
    }

    impl Fake {
        fn new() -> Self {
            Self {
                regions: Vec::new(),
                next: 0x100_0000,
                allocated: Vec::new(),
            }
        }
        fn alloc(&mut self, bytes: Vec<u8>) -> usize {
            let at = self.next;
            self.next += ((bytes.len() + 0xfff) & !0xfff) + 0x1000;
            self.regions.push((at, bytes));
            at
        }
        fn bytes(&mut self, at: usize) -> &mut Vec<u8> {
            &mut self.regions.iter_mut().find(|(b, _)| *b == at).unwrap().1
        }
    }

    fn put_u64(bytes: &mut [u8], at: usize, value: u64) {
        bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn put_i32(bytes: &mut [u8], at: usize, value: i32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A vector of `count` elements of `stride` bytes at `offset` of `head`,
    /// its data at `base`.
    fn put_vector(head: &mut [u8], offset: usize, base: usize, count: usize, stride: usize) {
        put_u64(head, offset, base as u64);
        put_u64(head, offset + 8, (base + count * stride) as u64);
        put_u64(head, offset + 16, (base + count * stride) as u64);
    }

    /// A terrain tool's proposal: a 3 by 2 grid from cell (-10, 7), each
    /// cell `{ 100 + i, 100 }`.
    fn stroke() -> (Fake, usize) {
        let mut fake = Fake::new();
        let mut cells = Vec::new();
        for i in 0..6u8 {
            cells.extend_from_slice(&(100.0f32 + f32::from(i)).to_le_bytes());
            cells.extend_from_slice(&100.0f32.to_le_bytes());
        }
        let data = fake.alloc(cells);
        let payload = fake.alloc(vec![0; layout::PROPOSAL_LEN + 0x100]);
        let head = fake.bytes(payload);
        put_i32(head, layout::HEIGHTS + layout::GRID_X0, -10);
        put_i32(head, layout::HEIGHTS + layout::GRID_Y0, 7);
        put_i32(head, layout::HEIGHTS + layout::GRID_WIDTH, 3);
        put_i32(head, layout::HEIGHTS + layout::GRID_HEIGHT, 2);
        put_vector(head, layout::HEIGHTS + layout::GRID_DATA, data, 6, 8);
        (fake, payload)
    }

    fn heights(stroke: Stroke) -> Grid {
        match stroke {
            Stroke::Heights(grid) => grid,
            other => panic!("not heights: {other:?}"),
        }
    }

    fn painted(stroke: Stroke) -> Paint {
        match stroke {
            Stroke::Paint(paint) => paint,
            other => panic!("not paint: {other:?}"),
        }
    }

    /// A paint stroke's proposal: a materials grid of `width` by `height`
    /// texels from (-40, 28), and a mask of the same cells.
    fn paint_stroke(materials: &[u8], width: i32, mask: Option<&[u8]>) -> (Fake, usize) {
        let mut fake = Fake::new();
        let height = materials.len() as i32 / width;
        let data = fake.alloc(materials.to_vec());
        let words = mask.map(|bits| {
            let mut words = vec![0u8; bits.len().div_ceil(32) * 4];
            for (i, b) in bits.iter().enumerate() {
                words[i / 8] |= b << (i % 8);
            }
            (fake.alloc(words.clone()), words.len() / 4, bits.len())
        });
        let payload = fake.alloc(vec![0; layout::PROPOSAL_LEN + 0x100]);
        let head = fake.bytes(payload);
        for grid in [layout::MATERIALS, layout::MASK] {
            put_i32(head, grid + layout::GRID_X0, -40);
            put_i32(head, grid + layout::GRID_Y0, 28);
            put_i32(head, grid + layout::GRID_WIDTH, width);
            put_i32(head, grid + layout::GRID_HEIGHT, height);
        }
        put_vector(
            head,
            layout::MATERIALS + layout::GRID_DATA,
            data,
            materials.len(),
            1,
        );
        if let Some((at, count, bits)) = words {
            put_vector(head, layout::MASK + layout::GRID_DATA, at, count, 4);
            put_u64(head, layout::MASK + layout::MASK_BITS, bits as u64);
        }
        (fake, payload)
    }

    #[test]
    fn a_terraform_reads_as_its_height_grid() {
        let (fake, payload) = stroke();
        let grid = heights(decode(&fake, payload).unwrap());
        assert_eq!((grid.x0, grid.y0, grid.width, grid.height), (-10, 7, 3, 2));
        assert_eq!(grid.cells[0], (100.0, 100.0));
        assert_eq!(grid.cells[5], (105.0, 100.0));
        assert_eq!(
            grid.summary(),
            "3 by 2 cells from cell (-10, 7), 5 changed, heights 100.00 to 105.00 m"
        );
        // As the GUI takes it, and as the game script gives it back.
        let lua = grid.to_lua();
        let LuaValue::Table(top) = &lua else { panic!() };
        assert_eq!(Grid::from_lua(&top[0].1).unwrap(), grid);
    }

    #[test]
    fn what_is_not_only_a_height_grid_is_refused_with_why() {
        // A grid whose cells are not width times height: a misread.
        let (mut fake, payload) = stroke();
        put_i32(fake.bytes(payload), layout::HEIGHTS + layout::GRID_WIDTH, 4);
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("4 by 2 with 6 cells")
        );
        // Heights and paint at once.
        let (mut fake, payload) = stroke();
        let mask = fake.alloc(vec![1; 16]);
        put_vector(
            fake.bytes(payload),
            layout::MASK + layout::GRID_DATA,
            mask,
            4,
            4,
        );
        assert!(decode(&fake, payload).unwrap_err().contains("paints too"));
        // Heights and the asset brush's groups at once.
        let (mut fake, payload) = stroke();
        let entity = fake.alloc(vec![0; layout::ENTITY_SIZE]);
        put_vector(
            fake.bytes(payload),
            layout::TO_ADD,
            entity,
            1,
            layout::ENTITY_SIZE,
        );
        assert_eq!(
            decode(&fake, payload).unwrap_err(),
            "an asset stroke that changes the terrain too"
        );
        // A street part.
        let (mut fake, payload) = stroke();
        let node = fake.alloc(vec![0; layout::NODE_SIZE]);
        put_vector(
            fake.bytes(payload),
            layout::ADDED_NODES,
            node,
            1,
            layout::NODE_SIZE,
        );
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("the nodes added")
        );
        // A cell that is not a number.
        let (mut fake, payload) = stroke();
        let data = u64_at(fake.bytes(payload), layout::HEIGHTS + layout::GRID_DATA) as usize;
        fake.bytes(data)[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&fake, payload).unwrap_err().contains("not finite"));
    }

    #[test]
    fn a_carrier_is_filled_with_the_armed_grid_once() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (source, at) = stroke();
        let grid = decode(&source, at).unwrap();
        let mut fake = Fake::new();
        let carrier = fake.alloc(vec![0; layout::PROPOSAL_LEN]);
        // Nothing armed: the build goes as it is.
        assert_eq!(disarm(), None);
        assert_eq!(inject(&mut fake, carrier), Ok(false));
        arm(grid.clone());
        assert_eq!(inject(&mut fake, carrier), Ok(true));
        // The carrier now reads as the stroke did.
        assert_eq!(decode(&fake, carrier).unwrap(), grid);
        assert_eq!(fake.allocated.len(), 1);
        // Spent: a second build is left alone; disarming says it was used.
        let other = fake.alloc(vec![0; layout::PROPOSAL_LEN]);
        assert_eq!(inject(&mut fake, other), Ok(false));
        assert_eq!(disarm(), Some(true));
        assert_eq!(disarm(), None);
    }

    #[test]
    fn a_carrier_with_anything_in_it_is_refused() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut fake, payload) = stroke();
        let grid = decode(&fake, payload).unwrap();
        arm(grid);
        // The stroke's own proposal is no empty carrier.
        assert!(
            inject(&mut fake, payload)
                .unwrap_err()
                .contains("terrain grids are not empty")
        );
        assert_eq!(disarm(), Some(false), "armed, and filled nothing");
    }

    #[test]
    fn the_grid_the_game_script_gives_is_checked() {
        let grid = |cells: Vec<LuaValue>, width: i64| {
            let key = LuaValue::string;
            LuaValue::Table(vec![
                (key("x0"), LuaValue::Integer(0)),
                (key("y0"), LuaValue::Integer(0)),
                (key("width"), LuaValue::Integer(width)),
                (key("height"), LuaValue::Integer(1)),
                (
                    key("cells"),
                    LuaValue::Table(
                        cells
                            .into_iter()
                            .enumerate()
                            .map(|(i, v)| (LuaValue::Integer(i as i64 + 1), v))
                            .collect(),
                    ),
                ),
            ])
        };
        let ok = Grid::from_lua(&grid(vec![LuaValue::Number(1.5), LuaValue::Integer(1)], 1));
        assert_eq!(ok.unwrap().cells, vec![(1.5, 1.0)]);
        assert!(Grid::from_lua(&grid(vec![LuaValue::Number(1.5)], 1)).is_err());
        assert!(
            Grid::from_lua(&grid(
                vec![LuaValue::Number(f64::INFINITY), LuaValue::Integer(1)],
                1
            ))
            .is_err()
        );
        assert!(Grid::from_lua(&grid(vec![], 0)).is_err());
        assert!(Grid::from_lua(&LuaValue::Integer(3)).is_err());
    }

    #[test]
    fn a_paint_stroke_reads_as_its_texels_and_mask() {
        // 4 by 3 texels: the brush's disc in the middle, 255 around it.
        let materials = [
            255, 3, 3, 255, //
            3, 3, 3, 3, //
            255, 3, 255, 255,
        ];
        let mask = [0, 1, 1, 0, 1, 1, 1, 1, 0, 1, 0, 0];
        let (fake, payload) = paint_stroke(&materials, 4, Some(&mask));
        let paint = painted(decode(&fake, payload).unwrap());
        assert_eq!(
            (
                paint.materials.x0,
                paint.materials.y0,
                paint.materials.width,
                paint.materials.height
            ),
            (-40, 28, 4, 3)
        );
        assert_eq!(paint.materials.cells, materials);
        assert_eq!(paint.mask.as_ref().unwrap().cells, mask);
        assert_eq!(
            paint.summary(),
            "4 by 3 texels from (-40, 28), 7 painted, a mask of 4 by 3 with 7 set"
        );
        // One action, and back from the GUI's Lua as it was.
        let actions = paint.actions().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].materials.runs.len(), 7);
        let LuaValue::Table(top) = paint.to_lua().unwrap() else {
            panic!()
        };
        let LuaValue::Table(list) = &top[0].1 else {
            panic!()
        };
        let body = list[0].1.get("Paint").unwrap();
        assert_eq!(Paint::from_lua(body).unwrap(), paint);
        // No mask: none carried.
        let (fake, payload) = paint_stroke(&materials, 4, None);
        assert_eq!(painted(decode(&fake, payload).unwrap()).mask, None);
    }

    #[test]
    fn a_large_paint_stroke_goes_in_bands_of_whole_rows() {
        // Every texel its own run: 100 by 100 is 10,000 runs, past one
        // action's 4096 (each band also carries its mask's runs).
        let materials: Vec<u8> = (0..10_000u32).map(|i| (i % 2) as u8).collect();
        let mask: Vec<u8> = vec![1; 10_000];
        let (fake, payload) = paint_stroke(&materials, 100, Some(&mask));
        let paint = painted(decode(&fake, payload).unwrap());
        let bands = paint.actions().unwrap();
        assert!(bands.len() >= 3, "{} bands", bands.len());
        let mut row = 28;
        let mut texels = Vec::new();
        for band in &bands {
            assert_eq!(band.materials.y0, row);
            assert!(
                band.materials.runs.len() + band.mask.as_ref().unwrap().runs.len()
                    <= MAX_PAINT_RUNS
            );
            row += band.materials.rows as i32;
            texels.extend(band.materials.cells());
        }
        assert_eq!(texels, materials);
    }

    #[test]
    fn what_a_paint_stroke_cannot_be_is_refused_with_why() {
        // Texels that are not width times height: a misread.
        let (mut fake, payload) = paint_stroke(&[3; 12], 4, None);
        put_i32(
            fake.bytes(payload),
            layout::MATERIALS + layout::GRID_WIDTH,
            5,
        );
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("a paint grid of 5 by 3 with 12 cells")
        );
        // A mask whose bit count does not fit its words.
        let (mut fake, payload) = paint_stroke(&[3; 12], 4, Some(&[1; 12]));
        put_u64(fake.bytes(payload), layout::MASK + layout::MASK_BITS, 40);
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("40 bits in 1 words")
        );
        // A mask of more than bits, from the game script.
        let (fake, payload) = paint_stroke(&[3; 4], 4, Some(&[1; 4]));
        let paint = painted(decode(&fake, payload).unwrap());
        let mut action = paint.actions().unwrap().remove(0);
        action.mask.as_mut().unwrap().runs =
            BoundedVec::new(vec![PaintRun { value: 2, count: 4 }]).unwrap();
        let body = action_to_lua(&Action::Paint(action)).unwrap();
        assert!(
            Paint::from_lua(body.get("Paint").unwrap())
                .unwrap_err()
                .contains("not 0 or 1")
        );
    }

    #[test]
    fn a_carrier_is_filled_with_the_armed_paint() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let materials: Vec<u8> = (0..40u8)
            .map(|i| if i % 3 == 0 { 7 } else { 255 })
            .collect();
        let mask: Vec<u8> = (0..40u8).map(|i| u8::from(i % 3 == 0)).collect();
        let (source, at) = paint_stroke(&materials, 8, Some(&mask));
        let stroke = decode(&source, at).unwrap();
        let mut fake = Fake::new();
        let carrier = fake.alloc(vec![0; layout::PROPOSAL_LEN]);
        arm(stroke.clone());
        assert_eq!(inject(&mut fake, carrier), Ok(true));
        // The carrier now reads as the stroke did: texels, mask words and
        // their count of bits.
        assert_eq!(decode(&fake, carrier).unwrap(), stroke);
        assert_eq!(fake.allocated.len(), 2);
        assert_eq!(disarm(), Some(true));
    }

    /// An MSVC string held inline, at `at` of `bytes`.
    fn put_string(bytes: &mut [u8], at: usize, text: &str) {
        bytes[at..at + text.len()].copy_from_slice(text.as_bytes());
        put_u64(
            bytes,
            at + crate::modules::layout::STRING_LEN,
            text.len() as u64,
        );
        put_u64(bytes, at + crate::modules::layout::STRING_CAPACITY, 15);
    }

    /// An asset brush's proposal: removing group 4321, adding one group of
    /// `models`, each a turn and a place.
    fn brush(models: &[(&str, f32, f32)]) -> (Fake, usize) {
        let mut fake = Fake::new();
        let mut list = vec![0u8; models.len() * layout::MODEL_SIZE];
        for (k, (name, x, y)) in models.iter().enumerate() {
            let at = k * layout::MODEL_SIZE;
            put_string(&mut list, at + crate::modules::layout::STRING_SIZE, name);
            let m: [f32; 16] = [
                0.6, 0.8, 0.0, 0.0, -0.8, 0.6, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, *x, *y, 2.5, 1.0,
            ];
            for (j, v) in m.iter().enumerate() {
                let o = at + layout::MODEL_TRANSF + j * 4;
                list[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        let list_at = fake.alloc(list);
        let mut sub = vec![0u8; 0x200];
        put_vector(
            &mut sub,
            layout::SUB_MODELS,
            list_at,
            models.len(),
            layout::MODEL_SIZE,
        );
        let sub_at = fake.alloc(sub);
        let mut record = vec![0u8; layout::ENTITY_SIZE];
        put_vector(&mut record, layout::ASSET_SUBS, sub_at, 1, 0x200);
        let record_at = fake.alloc(record);
        let removed = fake.alloc(4321i32.to_le_bytes().to_vec());
        let payload = fake.alloc(vec![0; layout::PROPOSAL_LEN + 0x100]);
        let head = fake.bytes(payload);
        put_vector(head, layout::TO_ADD, record_at, 1, layout::ENTITY_SIZE);
        put_vector(head, layout::TO_REMOVE, removed, 1, 4);
        (fake, payload)
    }

    #[test]
    fn an_asset_brush_stroke_reads_as_its_groups_and_their_models() {
        let (fake, payload) =
            brush(&[("assets/t1.mdl", 10.0, -4.0), ("assets/t2.mdl", 12.5, -3.0)]);
        let Stroke::Assets(assets) = decode(&fake, payload).unwrap() else {
            panic!("not the asset brush's")
        };
        assert_eq!(assets.removed, [4321]);
        assert_eq!(assets.groups.len(), 1);
        assert_eq!(assets.groups[0][0].model, "::/assets/t1.mdl");
        assert_eq!(assets.groups[0][1].matrix[12..], [12.5, -3.0, 2.5, 1.0]);
        assert_eq!(
            assets.summary(),
            "1 group(s) of 2 asset(s), replacing 1 group(s)"
        );
        // As the GUI takes it.
        let lua = assets.to_lua();
        let group = lua.get("assets").unwrap().get("groups").unwrap();
        let LuaValue::Table(groups) = group else {
            panic!()
        };
        let LuaValue::Table(first) = &groups[0].1 else {
            panic!()
        };
        assert_eq!(
            first[1].1.get("model"),
            Some(&LuaValue::string("::/assets/t2.mdl"))
        );
    }

    #[test]
    fn what_an_asset_brush_stroke_cannot_be_is_refused_with_why() {
        // A model whose matrix is no placement: a misread.
        let (mut fake, payload) = brush(&[("assets/t1.mdl", 10.0, -4.0)]);
        let head = fake.bytes(payload).clone();
        let record = u64_at(&head, layout::TO_ADD) as usize;
        let sub = u64_at(fake.bytes(record), layout::ASSET_SUBS) as usize;
        let list = u64_at(fake.bytes(sub), layout::SUB_MODELS) as usize;
        let o = layout::MODEL_TRANSF + 15 * 4;
        fake.bytes(list)[o..o + 4].copy_from_slice(&0.5f32.to_le_bytes());
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("not a placement")
        );
        // A construction of a file among the groups.
        let (mut fake, payload) = brush(&[("assets/t1.mdl", 10.0, -4.0)]);
        let record = u64_at(fake.bytes(payload), layout::TO_ADD) as usize;
        put_string(
            fake.bytes(record),
            layout::ENTITY_FILE + crate::modules::layout::STRING_SIZE,
            "x.con",
        );
        assert!(
            decode(&fake, payload)
                .unwrap_err()
                .contains("a construction of ::/x.con")
        );
        // An asset stroke is never armed into a carrier.
        let (fake, payload) = brush(&[("assets/t1.mdl", 10.0, -4.0)]);
        let stroke = decode(&fake, payload).unwrap();
        let mut heap = Fake::new();
        let carrier = heap.alloc(vec![0; layout::PROPOSAL_LEN]);
        assert!(
            fill(&mut heap, carrier, &stroke)
                .unwrap_err()
                .contains("no carrier")
        );
    }
}
