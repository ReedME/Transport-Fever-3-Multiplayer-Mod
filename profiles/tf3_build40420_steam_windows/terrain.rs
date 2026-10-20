//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// Every offset read or written, from the payload (whose `Proposal` is at
/// its start, as [`crate::modules`] reads it).
pub mod layout {
    pub use crate::modules::layout::{
        ADDED_NODES, ADDED_SEGMENTS, EDGE_OBJECT_SIZE, EDGE_OBJECTS_TO_ADD, EDGE_OBJECTS_TO_REMOVE,
        ENTITY_FILE, ENTITY_SIZE, NODE_SIZE, REMOVED_NODES, REMOVED_SEGMENTS, SEGMENT_SIZE, TO_ADD,
        TO_REMOVE,
    };

    /// `Proposal.terrain.baseHeightMod`, a grid of `Vec2f`.
    pub const HEIGHTS: usize = 0x2d8;
    /// The grids a paint sets: materials (bytes) and their mask (words).
    pub const MATERIALS: usize = HEIGHTS + GRID_SIZE;
    pub const MASK: usize = MATERIALS + GRID_SIZE;
    /// The whole `Proposal`.
    pub const PROPOSAL_LEN: usize = 0x358;

    pub const GRID_SIZE: usize = 0x28;
    pub const GRID_X0: usize = 0x00;
    pub const GRID_Y0: usize = 0x04;
    pub const GRID_WIDTH: usize = 0x08;
    pub const GRID_HEIGHT: usize = 0x0c;
    pub const GRID_DATA: usize = 0x10;
    /// The mask grid is a `vector<bool>`: its 32-bit words at `GRID_DATA`,
    /// then its count of bits (TPF2's `Grid<bool>`, `tpf2-multiplayer`'s
    /// `terrain_assets.inl`; the `Proposal` ends just past it).
    pub const MASK_BITS: usize = 0x28;
    /// One mask word: 32 cells, the first in its lowest bit.
    pub const MASK_WORD: usize = 4;

    /// The asset brush's groups, each a `ConstructionEntity` of `toAdd`
    /// (seen on build 40420 with the hook's asset probe, 2026-10-16: a
    /// stroke of 10 to 18 groups, each of 1 to 11 trees): the vector at
    /// `ASSET_SUBS` of the record (its construction's subconstructions; the
    /// tool makes one), and in its first element the vector of the group's
    /// models at `SUB_MODELS`, each `MODEL_SIZE` bytes: the model's
    /// resource name (two strings) first, its world matrix (16 `f32`,
    /// columns of four) at `MODEL_TRANSF`.
    pub const ASSET_SUBS: usize = 0xa30;
    pub const SUB_MODELS: usize = 0x140;
    pub const MODEL_SIZE: usize = 0xa8;
    pub const MODEL_TRANSF: usize = 0x60;

    /// One height cell: two `f32`.
    pub const CELL_SIZE: usize = 8;
    /// MSVC's allocation of 4 KiB or more: aligned to 32, the block's own
    /// address in the 8 bytes before the data, 39 bytes more asked for.
    pub const BIG_ALLOCATION: usize = 0x1000;
    pub const BIG_ALIGNMENT: usize = 32;
    pub const BIG_EXTRA: usize = 8 + BIG_ALIGNMENT - 1;
}
