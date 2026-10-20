//! Native data for Steam Windows build 40408. See hooks.toml for identity.

/// The profile's name for `UI::StreetBuilder::UpdateEngine`'s call of
/// `CommandList::Add` (one click of the street or track tool). `Add`
/// returns 5 bytes past it.
pub const ADD_CALL: &str = "StreetBuilder::UpdateEngine/Add call";

/// The profile's name for `UpdateEngine` setting its busy byte and
/// building the click's callback, whose signature holds every offset of
/// the tool and of the callback's captures below.
pub const CAPTURE: &str = "StreetBuilder::UpdateEngine/capture";

/// The profile's name for the callback's scan of the built edges, whose
/// signature holds the result's layout below.
pub const RESULT_SCAN: &str = "StreetBuilder::UpdateEngine callback/result scan";

/// The profile's name for the callback's start of the next segment, whose
/// signature holds the start point's and the direction's layout below and
/// calls [`START_AT`].
pub const SNAP: &str = "StreetBuilder::UpdateEngine callback/snap";

/// The profile's name for the tool's "start the next segment here".
pub const START_AT: &str = "UI::StreetBuilder::StartAt";

/// The profile's name for `ecs::Engine::EntityExists`.
pub const ENTITY_EXISTS: &str = "ecs::Engine::EntityExists";

/// [`ADD_CALL`] lies this far past [`CAPTURE`], in the same function.
pub const CAPTURE_TO_ADD_CALL: usize = 0x24a;
/// [`SNAP`] lies this far past [`RESULT_SCAN`], in the same function.
pub const SCAN_TO_SNAP: usize = 0x2b9;
/// [`SNAP`]'s call of [`START_AT`], this far into it.
pub const SNAP_CALL: usize = 0x50;
/// [`START_AT`]'s call of `ecs::GetRevisionIfSet`, this far into it.
pub const START_AT_REVISION_CALL: usize = 0x55;
/// `ecs::GetRevisionIfSet`'s call of [`ENTITY_EXISTS`], this far into it.
pub const REVISION_EXISTS_CALL: usize = 0x3d;

/// The tool's busy byte: 1 from the click until its callback ran.
pub const BUSY: usize = 0x9e0;
/// Whether the tool has a start point.
pub const HAS_START: usize = 0x940;
/// Whether the tool drags a segment from its start point.
pub const DRAGGING: usize = 0x7c4;
/// The tool's settings, which the click's callback reads.
pub const SETTINGS: usize = 0x960;
/// The settings' byte that lets the tool continue from a built segment.
pub const SETTINGS_CONTINUE: usize = 0x8e0;
/// The tool's first parameter byte, which picks the network whose edges
/// the direction rule counts (street when set).
pub const KIND: usize = 0xd8;
/// The tool's state provider, and its engine within it.
pub const PROVIDER: usize = 0x208;
pub const PROVIDER_ENGINE: usize = 0x18;

/// Where a `std::function` keeps its impl pointer (MSVC x64).
pub const FUNCTION_IMPL: usize = 0x38;
/// The click's functor, after its vftable: the tool, the start point the
/// next segment takes, the clicked end, and the two settings bytes.
pub const FUNCTOR_TOOL: usize = 0x8;
pub const FUNCTOR_START: usize = 0x10;
pub const FUNCTOR_END: usize = 0x1c;
pub const FUNCTOR_CONTINUE: usize = 0x28;
pub const FUNCTOR_DIRECTION: usize = 0x29;

/// The build's edges, a vector at this offset of the command's payload.
pub const RESULTS: usize = 0x18;
/// One edge of it: its size and fields.
pub const ENTRY_SIZE: usize = 0x350;
pub const ENTRY_EDGE: usize = 0x0;
pub const ENTRY_NODE0: usize = 0x8;
pub const ENTRY_NODE1: usize = 0xc;
pub const ENTRY_POS0: usize = 0x10;
pub const ENTRY_POS1: usize = 0x1c;
pub const ENTRY_TAN0: usize = 0x28;
pub const ENTRY_TAN1: usize = 0x34;
