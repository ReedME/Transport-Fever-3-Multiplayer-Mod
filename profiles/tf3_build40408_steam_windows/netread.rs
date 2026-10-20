//! Native data for Steam Windows build 40408. See hooks.toml for identity.
//!
//! The engine's component store and the street network's components as
//! `crates/tpf3mp-hook/src/netread.rs` reads them
//! (investigation/TF3_NATIVE_NETWORK_2026-10-04.md).

/// `CGameTime+8`: the engine (`crate::ticks` reads it the same way).
pub const GAME_TIME_ENGINE: usize = 8;

/// The engine's component pools: a `std::vector` of `CompVec<T>*`, indexed
/// by the component's type id.
pub const POOLS: usize = 0x78;
/// The entity table: a `std::vector` of 24-byte records, each itself a
/// `std::vector<{int32 typeId, int32 dataIndex}>` (`sub_a4b90`).
pub const ENTITIES: usize = 0x90;
pub const ENTITY_RECORD: usize = 24;
pub const COMPONENT_PAIR: usize = 8;
/// Each entity's component bits: 16 bytes an entity, bit = type id
/// (`HasComponent`, `sub_2bb60a0`).
pub const BITS: usize = 0xc0;
pub const BITS_PER_ENTITY: usize = 16;

/// `CompVec<T>`: the dense vector of `T` and the page table of 16-byte
/// `{T* page, ctrl}` entries, 32 slots a page.
pub const POOL_HEAD: usize = 0xa0;
pub const POOL_DENSE: usize = 0x68;
pub const POOL_PAGES: usize = 0x80;
pub const PAGE_ENTRY: usize = 16;
pub const PAGE_SLOTS: usize = 32;
/// A data index at or above it is a paged slot's.
pub const PAGED_FROM: u32 = 0x4000_0000;

/// `ecs::ComponentManager::CompVec<struct ecs::component::BaseEdge>::vftable`.
pub const BASE_EDGE_POOL_VTABLE: usize = 0x367fac8;

/// `BaseEdge` (0x118): its two ends, its lane configs and its road template.
pub const BASE_EDGE_SIZE: usize = 0x118;
pub const EDGE_POSITION0: usize = 0x08;
pub const EDGE_POSITION1: usize = 0x14;
pub const EDGE_LANE_CONFIGS: usize = 0x70;
pub const EDGE_ROAD_TEMPLATE: usize = 0x98;

/// `LaneConfig` (0x18).
pub const LANE_CONFIG_SIZE: usize = 0x18;
pub const LANE_SPEED: usize = 0x00;
pub const LANE_WIDTH: usize = 0x04;
pub const LANE_HEIGHT: usize = 0x08;
pub const LANE_FORWARD: usize = 0x0c;
pub const LANE_MODES: usize = 0x10;
pub const LANE_OFFSET: usize = 0x14;

/// `BaseEdge`'s nodes and its network (`roadType`: TRACK 0, STREET 1;
/// asserts in `sub_5bc120`, `sub_904f50`).
pub const EDGE_NODE0: usize = 0x00;
pub const EDGE_NODE1: usize = 0x04;
pub const EDGE_ROAD_TYPE: usize = 0x90;
pub const ROAD_TYPE_TRACK: i32 = 0;
pub const ROAD_TYPE_STREET: i32 = 1;

/// `ecs::ComponentManager::CompVec<struct ecs::component::BaseNode>::vftable`;
/// `BaseNode` (0x14, `sub_2806a0`): its position first.
pub const BASE_NODE_POOL_VTABLE: usize = 0x367fb38;
pub const BASE_NODE_SIZE: usize = 0x14;
pub const NODE_POSITION: usize = 0x00;

/// `ecs::ComponentManager::CompVec<struct ecs::component::BaseNodeConfig>::vftable`;
/// `BaseNodeConfig` (0x78, `sub_281290`), as `crate::junctions` reads it
/// in a proposal: its turns, its crosswalk set (`crate::junctions`), its
/// flags, its preference, its light's phases and type.
pub const BASE_NODE_CONFIG_POOL_VTABLE: usize = 0x367fba8;
pub const BASE_NODE_CONFIG_SIZE: usize = 0x78;
pub const CONFIG_TURNS: usize = 0x00;
pub const CONFIG_DOUBLE_SLIP: usize = 0x48;
pub const CONFIG_PREFERENCE: usize = 0x4c;
pub const CONFIG_PHASES: usize = 0x50;
pub const CONFIG_LIGHT_TYPE: usize = 0x68;
pub const CONFIG_CUSTOM_PHASES: usize = 0x70;

/// `LaneConnection` (0x14).
pub const TURN_SIZE: usize = 0x14;
pub const TURN_SEGMENT0: usize = 0x00;
pub const TURN_LANE0: usize = 0x04;
pub const TURN_SEGMENT1: usize = 0x08;
pub const TURN_LANE1: usize = 0x0c;
pub const TURN_ROAD: usize = 0x10;
pub const TURN_TRAM: usize = 0x11;

/// `TrafficLightState` (0x28).
pub const PHASE_SIZE: usize = 0x28;
pub const PHASE_LOCKED: usize = 0x00;
pub const PHASE_DURATION: usize = 0x18;
pub const PHASE_MINIMUM: usize = 0x1c;
pub const PHASE_SKIP: usize = 0x20;

/// `ecs::ComponentManager::CompVec<struct ecs::component::Construction>::vftable`;
/// `Construction` (0x288, `imul rax, 0x288` in `sub_2807d0`; its fields as
/// the Lua binding registers them, `sub_18829c0`): its file a `ResName` at
/// +0, its `transf` 16 float32 at +0x58, column-major, the translation's x
/// and y elements 12 and 13 (`UI::ModuleBuilder::SetConstruction`,
/// `sub_544f10`, 0x5450ba-0x54515f).
pub const CONSTRUCTION_POOL_VTABLE: usize = 0x3680510;
pub const CONSTRUCTION_SIZE: usize = 0x288;
pub const CONSTRUCTION_FILE: usize = 0x00;
pub const CONSTRUCTION_X: usize = 0x88;
pub const CONSTRUCTION_Y: usize = 0x8c;
