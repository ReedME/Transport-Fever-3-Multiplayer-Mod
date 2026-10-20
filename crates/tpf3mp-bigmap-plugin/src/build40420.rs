//! Steam Windows build 40420's data for Big Maps' patches, from Big Maps'
//! own branch (`profiles/tf3_build40420_steam_windows/bigmap.rs` and
//! `simswitch.rs` on `feat/bigmaps-40420`;
//! investigation/TF3_BIGMAPS_256KM_2026-10-05.md and
//! TF3_BIG_MAP_SIM_COST_2026-10-05.md there), with each target named as the
//! package's hook (`crate::manifest`) instead of the hook profile's.
//!
//! A root site's hook is the splice: the instruction after the site's
//! call, [`ROOT_NEXT_OFFSET`] bytes past the site.

/// The game build these are read on: its executable's SHA-256.
pub const BUILD: &str = "74861ac43b041aebc5179154345b3cf1ec83154c8e6cc58e0d9e02ff5fa602e4";

/// `OctreeSystem::Resize(octree, depth, half)`: stores the root box and
/// the depth, nothing else.
pub const RESIZE: &str = "bigmap::resize";

/// The octree root's site on the new-game path (`InitGame`).
pub const ROOT_INIT: &str = "bigmap::root_init";

/// The octree root's site on the load path (`GameState::Load`).
pub const ROOT_LOAD: &str = "bigmap::root_load";

/// Where the site's `call Resize` is, from the site.
pub const ROOT_CALL_OFFSET: usize = 17;

/// Where the instruction after the call is, from the site: the splice.
pub const ROOT_NEXT_OFFSET: usize = 22;

/// `mov edx, 10` at the site's +12: the stock large-map depth.
pub const ROOT_DEPTH_BYTES: [u8; 5] = [0xBA, 0x0A, 0x00, 0x00, 0x00];

/// Where the site's owner load, `mov rcx,[owner+0x38]`, is, from the site.
pub const ROOT_OWNER_OFFSET: usize = 8;

/// The owner's field holding the `OctreeSystem*`.
pub const OCTREE_FIELD: usize = 0x38;

/// A general-purpose register a site keeps a pointer in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    Rbp,
    Rsi,
}

/// One root site: which register holds the world's tile counts (two
/// `int32`, x then y), which the owner, the owner load's bytes, and the
/// instruction after the call, which is spliced (5 bytes, no RIP-relative
/// operand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootSite {
    pub target: &'static str,
    pub tiles: Reg,
    pub owner: Reg,
    pub owner_load: [u8; 4],
    pub next: [u8; 5],
}

/// init (0x244c0b): tiles in rbp, owner in rsi; next `vmovss xmm2,[rbp+0x20]`.
pub const INIT_SITE: RootSite = RootSite {
    target: ROOT_INIT,
    tiles: Reg::Rbp,
    owner: Reg::Rsi,
    owner_load: [0x48, 0x8B, 0x4E, 0x38],
    next: [0xC5, 0xFA, 0x10, 0x55, 0x20],
};

/// load (0x2026fd): tiles in rsi, owner in rbp; next `vmovss xmm2,[rsi+0x20]`.
pub const LOAD_SITE: RootSite = RootSite {
    target: ROOT_LOAD,
    tiles: Reg::Rsi,
    owner: Reg::Rbp,
    owner_load: [0x48, 0x8B, 0x4D, 0x38],
    next: [0xC5, 0xFA, 0x10, 0x56, 0x20],
};

/// Where `Resize` stores the depth, in the `OctreeSystem`.
pub const OCTREE_DEPTH_FIELD: usize = 0x28;

/// Where `Resize` stores `+half` (x), in the `OctreeSystem`.
pub const OCTREE_HALF_FIELD: usize = 0x1c;

/// `Obstacle::Obstacle(this, box, cell)`: the street and obstacle raster.
pub const OBSTACLE: &str = "bigmap::obstacle";

/// The constructor's body from the box copy to its resize call
/// (0x8d0a77..0x8d0afe): the counts `floor(extent / cell) + 1`, stored at
/// `+0x40`/`+0x44`, and the 32-bit `imul` the resize is sized with. The
/// hook's cell rule models exactly this; a build that changed it is left
/// alone.
pub const OBSTACLE_BODY_OFFSET: usize = 0x17;
pub const OBSTACLE_BODY: [u8; 135] = [
    0xC5, 0xF8, 0x10, 0x02, // vmovups xmm0,[rdx]
    0xC5, 0xF8, 0x11, 0x41, 0x08, // vmovups [rcx+8],xmm0
    0xC5, 0xFA, 0x11, 0x51, 0x18, // vmovss [rcx+0x18],xmm2
    0x48, 0x83, 0xC1, 0x20, // add rcx,0x20
    0x33, 0xC0, // xor eax,eax
    0x48, 0x89, 0x01, // mov [rcx],rax
    0x48, 0x89, 0x41, 0x08, // mov [rcx+8],rax
    0x48, 0x89, 0x41, 0x10, // mov [rcx+0x10],rax
    0x48, 0x89, 0x41, 0x18, // mov [rcx+0x18],rax
    0xC5, 0xFA, 0x10, 0x52, 0x08, // vmovss xmm2,[rdx+8]
    0xC5, 0xFA, 0x10, 0x42, 0x0C, // vmovss xmm0,[rdx+0xc]
    0xC5, 0xFA, 0x5C, 0x4B, 0x0C, // vsubss xmm1,xmm0,[rbx+0xc]
    0xC5, 0xF2, 0x5E, 0x63, 0x18, // vdivss xmm4,xmm1,[rbx+0x18]
    0xC5, 0x7A, 0x2C, 0xC4, // vcvttss2si r8d,xmm4
    0xC5, 0xEA, 0x5C, 0x43, 0x08, // vsubss xmm0,xmm2,[rbx+8]
    0xC5, 0xFA, 0x5E, 0x4B, 0x18, // vdivss xmm1,xmm0,[rbx+0x18]
    0xC5, 0xFA, 0x2C, 0xD1, // vcvttss2si edx,xmm1
    0x8D, 0x42, 0xFF, // lea eax,[rdx-1]
    0xC5, 0xF8, 0x57, 0xC0, // vxorps xmm0,xmm0,xmm0
    0xC5, 0xF8, 0x2F, 0xC8, // vcomiss xmm1,xmm0
    0x0F, 0x42, 0xD0, // cmovb edx,eax
    0x41, 0x8D, 0x40, 0xFF, // lea eax,[r8-1]
    0xC5, 0xF8, 0x2F, 0xE0, // vcomiss xmm4,xmm0
    0x44, 0x0F, 0x42, 0xC0, // cmovb r8d,eax
    0xFF, 0xC2, // inc edx
    0x89, 0x54, 0x24, 0x38, // mov [rsp+0x38],edx
    0x41, 0x8D, 0x40, 0x01, // lea eax,[r8+1]
    0x89, 0x44, 0x24, 0x3C, // mov [rsp+0x3c],eax
    0x48, 0x8B, 0x44, 0x24, 0x38, // mov rax,[rsp+0x38]
    0x48, 0x89, 0x43, 0x40, // mov [rbx+0x40],rax
    0x48, 0xC1, 0xE8, 0x20, // shr rax,0x20
    0x0F, 0xAF, 0xC2, // imul eax,edx
    0x48, 0x63, 0xD0, // movsxd rdx,eax
    0x45, 0x33, 0xC0, // xor r8d,r8d
];

/// The placement spacing score.
pub const SPACING: &str = "bigmap::spacing";

/// The score's two int32 squarings, by their offset in it: candidate
/// pairs (`imul edx,edx; imul eax,eax` at 0x8d52ab) and exclusions
/// (`imul ecx,ecx; imul eax,eax` at 0x8d530b). The hook replaces the score
/// only where both are as read.
pub const SPACING_SQUARES: [(usize, [u8; 6]); 2] = [
    (0x15b, [0x0F, 0xAF, 0xD2, 0x0F, 0xAF, 0xC0]),
    (0x1bb, [0x0F, 0xAF, 0xC9, 0x0F, 0xAF, 0xC0]),
];

/// The score a candidate closer than the minimum gets (`[0x36c4ea8]`).
pub const SPACING_TOO_CLOSE: f32 = 99_999.0;

/// The octree's descent (`detail::EcsOctreeIterator`), recursive.
pub const DESCENT: &str = "bigmap::descent";

/// The descent's only other caller, which starts it at the root.
pub const DESCENT_START: &str = "bigmap::descent_start";

/// The descent's body where the depth-12 ids rely on it, by offset:
/// where it reads its box (entry `rsp+0x48`) and its node (`+0x50`), the
/// remaining depth's decrement (`+0x40`) and the leaf stop, and the child
/// id `8·id + 1 + octant` in 32 bits.
pub const DESCENT_CHECKS: [(usize, &[u8]); 4] = [
    (0x85, &[0x4C, 0x8B, 0xAD, 0x60, 0x01, 0x00, 0x00]),
    (
        0xa7,
        &[0x8B, 0x9D, 0x68, 0x01, 0x00, 0x00, 0x83, 0xFB, 0xFF],
    ),
    (
        0x37b,
        &[0x83, 0xAD, 0x58, 0x01, 0x00, 0x00, 0x01, 0x75, 0x27],
    ),
    (
        0x4c9,
        &[
            0x44, 0x8D, 0x0C, 0xC5, 0x01, 0x00, 0x00, 0x00, 0x45, 0x03, 0xCC,
        ],
    ),
];

/// The starter's reads of the EcsOctree: `mov eax,[rcx+0x24]` (the root
/// entity) at +0x5a and `mov edx,[rcx+0x20]` (the depth) at +0x60, and its
/// call of the descent at +0xf8.
pub const DESCENT_START_CHECKS: [(usize, &[u8]); 2] =
    [(0x5a, &[0x8B, 0x41, 0x24]), (0x60, &[0x8B, 0x51, 0x20])];
pub const DESCENT_START_CALL: usize = 0xf8;

/// The EcsOctree the descent's `rcx` points at: the root box (min x, y, z,
/// max x, y, z) at +8 and the depth at +0x20.
pub const ECS_OCTREE_BOX: usize = 0x08;
pub const ECS_OCTREE_DEPTH: usize = 0x20;

/// The level decoder's splice: `mov r12d,r9d; cmp edx,r8d`, then `jl` past
/// the loop.
pub const DECODER: &str = "bigmap::decoder";
pub const DECODER_STOLEN: [u8; 6] = [0x45, 0x8B, 0xE1, 0x41, 0x3B, 0xD0];

// The simulation switches (a proposal on Big Maps' branch).

/// `ecs::EmissionGridSystem::Update(Engine*, INodeList*, float dt) const`
/// (0xaaa700): the noise and pollution grids' diffusion, wind and average.
pub const GRID_UPDATE: &str = "bigmap::grid_update";

/// `lea rax,[EmissionGridSystem::vftable]` in the system's constructor
/// (0xaa9379).
pub const GRID_VTABLE: &str = "bigmap::grid_slot";

/// `Update`'s slot in that vtable: `ecs::Engine::Update` (0x2bbdb20) calls
/// every system's slot 11 (`call [r9+0x58]`) in turn, on the step's thread.
pub const GRID_SLOT: usize = 11;

/// `ecs::EmissionEmitterSystem::Update2(Engine*, int, float dt) const`
/// (0xaa6570): every emitter splatted into both grids.
pub const EMITTER_UPDATE2: &str = "bigmap::emitter_update";

/// `lea rax,[EmissionEmitterSystem::vftable]` in the system's constructor
/// (0xaa52be).
pub const EMITTER_VTABLE: &str = "bigmap::emitter_slot";

/// `Update2`'s slot: the system's own slot 11 (0xaa6cd0) calls it through
/// the vtable (`call [r9+0x60]`).
pub const EMITTER_SLOT: usize = 12;

/// `lea rax,[rip+disp32]`: the vtable loads' first three bytes.
pub const LEA_RAX_RIP: [u8; 3] = [0x48, 0x8D, 0x05];

/// `ecs::component::CreateEmissionGrid` (0xba0a20, EmissionGrid.cpp): the
/// noise and pollution grids of a new world, 16 cells per tile. Called only
/// from the new-game path (0x155670); a load takes the grid from the save.
pub const CREATE_GRID: &str = "bigmap::create_grid";

/// Where the checked body starts, from the function: the first `shl`.
pub const CREATE_GRID_BODY_OFFSET: usize = 0x91;

/// The body from the first `shl` to the gridPointSize multiplies
/// (0xba0ab1..0xba0b1b): `X0`, `Y0`, `width`, `height` as `tiles << 4`,
/// the `> 2` checks, the two tile-size calls, then `vmovss xmm2,[0.0625]`
/// and the two multiplies. The load's displacement (`+0x5d..+0x61` here)
/// is checked through the constant it reads instead.
pub const CREATE_GRID_BODY: [u8; 106] = [
    0xC1, 0xE0, 0x04, 0x89, 0x03, 0x49, 0x8B, 0xCF, 0xE8, 0x02, 0xBD, 0x00, 0x00, 0x48, 0xC1, 0xE8,
    0x20, 0xC1, 0xE0, 0x04, 0x89, 0x43, 0x04, 0x41, 0x8B, 0x07, 0xC1, 0xE0, 0x04, 0x89, 0x43, 0x08,
    0x41, 0x8B, 0x4F, 0x04, 0xC1, 0xE1, 0x04, 0x89, 0x4B, 0x0C, 0x83, 0xF8, 0x02, 0x0F, 0x8E, 0xB1,
    0x01, 0x00, 0x00, 0x83, 0xF9, 0x02, 0x0F, 0x8E, 0x87, 0x01, 0x00, 0x00, 0x49, 0x8B, 0xCF, 0xE8,
    0x9B, 0xBC, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0x34, 0x49, 0x8B, 0xCF, 0xE8, 0x8E, 0xBC, 0x00,
    0x00, 0x48, 0x89, 0x84, 0x24, 0xB8, 0x00, 0x00, 0x00, 0xC5, 0xFA, 0x10, 0x15, 0xD2, 0xBF, 0xB0,
    0x02, 0xC5, 0xEA, 0x59, 0x9C, 0x24, 0xB8, 0x00, 0x00, 0x00,
];

/// The four `shl reg,4` (cells per tile), from the function: X0, Y0,
/// width (`shl eax,4`) and height (`shl ecx,4`).
pub const CREATE_GRID_SHIFTS: [usize; 4] = [0x91, 0xa2, 0xab, 0xb5];

/// `vmovss xmm2,[rip+disp32]` loading the cell factor, from the function
/// (0xba0b0a); 8 bytes, the displacement in the last four.
pub const CREATE_GRID_FACTOR_LOAD: usize = 0xea;

/// The load's opcode bytes before its displacement.
pub const VMOVSS_XMM2_RIP: [u8; 4] = [0xC5, 0xFA, 0x10, 0x15];

/// From the 0.0625f the load reads (0x36acae4, shared by ten other
/// instructions, never written) to a 0.125f in the same section
/// (0x36bc024, read by 22 instructions), which the load is pointed at.
pub const CREATE_GRID_COARSE_FACTOR_DELTA: usize = 0x36bc024 - 0x36acae4;

/// The four shifts' rewrite hooks, in [`CREATE_GRID_SHIFTS`]' order.
pub const CREATE_GRID_SHIFT_HOOKS: [&str; 4] = [
    "bigmap::grid_shift_x0",
    "bigmap::grid_shift_y0",
    "bigmap::grid_shift_width",
    "bigmap::grid_shift_height",
];

/// The cell factor load's rewrite hook.
pub const CREATE_GRID_FACTOR_HOOK: &str = "bigmap::grid_factor";
