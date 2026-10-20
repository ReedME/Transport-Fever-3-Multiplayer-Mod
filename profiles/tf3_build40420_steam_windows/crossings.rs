//! Native data for Steam Windows build 40420. See hooks.toml for identity.

/// The profile's name for the level crossing window's call of
/// `CommandList::Add` (its type selection, `RailroadCrossingComp`'s
/// `<lambda_5>` -> `sub_28a35a0`). `Add` returns 5 bytes past it.
pub const CROSSING_ADD_CALL: &str = "RailroadCrossingComp/type selection Add call";

/// The profile's name for the StreetProposal constructor's two
/// `std::unordered_map<int,int>` at [`layout::NODE_TYPES`] and
/// [`layout::CROSSING_TYPES`], whose signature holds both offsets: a build
/// that moved them does not resolve.
pub const MAPS_LAYOUT: &str = "StreetProposal/level crossing maps";

/// The profile's name for the window's insert into the crossing map
/// (`UpdateRailRoadCrossingProposal`, 0x28a42cf: `lea rcx,[r15+0xd0];
/// lea r8,[key]; lea rdx,[out]; call insert; mov rcx,[rax]; mov eax,[type];
/// mov [rcx+0x14],eax`). The insert it calls (`sub_567540`, MSVC
/// `unordered_map<int,int>::try_emplace`: map, `pair<iterator, bool>` out,
/// key; returns the out) has byte-identical twins, so it is found through
/// this call; the room's replay fills its carrier with it.
pub const MAP_INSERT_CALL: &str = "UpdateRailRoadCrossingProposal/crossing map insert";

/// The insert's first instructions, checked where the call leads: `mov
/// [rsp+8],rbx; mov [rsp+0x10],rbp; mov [rsp+0x20],rsi; mov [rsp+0x18],r8`.
pub const INSERT_HEAD: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x08, 0x48, 0x89, 0x6C, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x20, 0x4C,
    0x89, 0x44, 0x24, 0x18,
];

/// Every offset read.
pub mod layout {
    /// `node2rcType`, node -> type index: what the game's Lua sets
    /// (`SimpleStreetProposal.node2rcType` converts to it) and what the
    /// window sets with a rebuild of the crossing's segments.
    pub const NODE_TYPES: usize = 0x90;
    /// Crossing entity -> type index, set by the window alone
    /// (`UpdateRailRoadCrossingProposal`, 0x28a42bc); no Lua binding.
    pub const CROSSING_TYPES: usize = 0xd0;
    /// The proposal's node configurations added and removed.
    pub const CONFIGS_ADDED: usize = 0x60;
    pub const CONFIGS_REMOVED: usize = 0x78;
    pub const CONFIG_SIZE: usize = 0x80;
    /// Where a configuration added names its node
    /// (`BaseNodeLaneConnectionAndEntity`, docs/HOOKS.md "Junction tools").
    pub const CONFIG_NODE: usize = 0x78;

    /// MSVC `std::unordered_map`: the list's sentinel node and its size
    /// (`sub_28b250`).
    pub const MAP_HEAD: usize = 0x08;
    pub const MAP_SIZE: usize = 0x10;
    /// A list node: next, previous, then the pair (`sub_567540`).
    pub const NODE_NEXT: usize = 0x00;
    pub const NODE_PREV: usize = 0x08;
    pub const NODE_KEY: usize = 0x10;
    pub const NODE_VALUE: usize = 0x14;
    pub const NODE_LEN: usize = 0x18;
}
