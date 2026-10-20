//! What a player did, in terms every replica can resolve against its own
//! world: the portable action schema. `docs/BUILDING.md` ("The action
//! schema") explains what each action carries and why.
//!
//! An action names nothing by engine entity id, which differs between games
//! and is recycled within one. It uses three kinds of reference only:
//!
//! - **world positions**, as fixed-point integers in millimetres ([`Pos`]);
//!   geometry is matched by position within the tolerances BUILDING.md gives;
//! - **resource file names** ([`ResName`]), such as a street type or a
//!   vehicle model, which are the same in every game with the same content;
//! - **canonical ids** ([`CompanyId`], [`LineId`], [`VehicleId`],
//!   [`StationId`]), which the server assigns to what an action creates.
//!
//! The acting player, and so their company, is not part of an action: the
//! event that carries it names the player.
//!
//! An action travels inside the opaque [`Payload`] of an intent, behind the
//! [`ACTION_SCHEMA_VERSION`] ([`Action::to_payload`]). Decoding enforces every
//! bound, and a polyline's links must name its own vertices, so a decoded
//! action is well-formed. Variants are identified by position: append, never
//! reorder, and bump the schema version when an existing variant changes.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BoundedVec, Text,
    bytes::{Payload, PayloadTooLarge},
};

/// Version of the action schema, the first thing in an action's payload.
/// Players in one room run the same mod, so their versions match; a payload
/// of any other version is refused, never guessed at.
pub const ACTION_SCHEMA_VERSION: u32 = 28;

/// Most vertices, and most links, in one road or track build. A 23-segment
/// track was the longest single TPF2 build measured.
pub const MAX_VERTICES: usize = 512;
pub const MAX_LINKS: usize = 512;
/// Decorations one edge carries.
pub const MAX_DECORATIONS: usize = 8;
/// Lanes one edge has.
pub const MAX_LANES: usize = 32;
/// Junctions changed by one tool stroke, and connections/phases per junction.
pub const MAX_JUNCTIONS: usize = 64;
pub const MAX_CONNECTIONS: usize = 256;
pub const MAX_PHASES: usize = 64;
/// Most edges one action removes or bulldozes.
pub const MAX_EDGES: usize = 256;
/// How far an edge's own end may lie from its vertex, in millimetres, on
/// each axis ([`Link::position0`] or [`Link::position1`], schema 27). The
/// track tool starts a track on a street's tram lane, 2 m beside the street's
/// end node (build 40420); the widest street's outer lane is well within this.
pub const MAX_END_OFFSET: i32 = 32_000;
/// Most town buildings one bulldoze of streets removes with them.
pub const MAX_BUILDINGS: usize = 64;
/// Most assets removed by one action.
pub const MAX_ASSETS: usize = 64;
/// Most parameters of one construction, nested modules counted one by one.
pub const MAX_PARAMS: usize = 1024;
/// Most construction parameters of one stop or signal (the signal tool's
/// settings: the base game's `oneWay` and a mod's own, schema 26).
pub const MAX_OBJECT_PARAMS: usize = 32;
/// Most edges one [`PlaceSignals`] rebuilds. Auto Signals walks up to 1000
/// edges from the signal placed (schema 26).
pub const MAX_SIGNAL_EDGES: usize = 1024;
/// Most signals one [`PlaceSignals`] adds to, or removes from, one edge.
pub const MAX_EDGE_SIGNALS: usize = 64;
/// Most vehicle models in one consist.
pub const MAX_CONSIST: usize = 64;
/// Most vehicles one sell or line assignment names.
pub const MAX_VEHICLES: usize = 256;
/// Most stops on a line.
pub const MAX_LINE_STOPS: usize = 256;
/// Most cells in one terraform stroke. TPF2's largest measured, a smooth, was
/// 83 by 59.
pub const MAX_TERRAIN_CELLS: usize = 8192;
/// Most model files one [`AssetStroke`] names: a brush paints a handful of
/// kinds of tree or rock.
pub const MAX_STROKE_MODELS: usize = 64;
/// Most asset groups one [`AssetStroke`] replaces, and most it adds.
pub const MAX_STROKE_GROUPS: usize = 256;
/// Most assets one group an [`AssetStroke`] adds holds. A stroke's groups
/// measured 1 to 11 trees each on build 40420.
pub const MAX_GROUP_ASSETS: usize = 512;
/// Most runs in one [`PaintGrid`]. A paint stroke's grid is the brush's
/// bounding box of 1 m texels, mostly unchanged: a few runs a row (TPF2's
/// strokes packed about 30 to 1, `tpf2-multiplayer`'s `terrain_assets.inl`).
/// The hook cuts a larger stroke into bands of rows, each its own action.
pub const MAX_PAINT_RUNS: usize = 4096;
/// Most compartments of one vehicle.
pub const MAX_COMPARTMENTS: usize = 16;
/// Most cargo types a stop's loading rules list.
pub const MAX_CARGOS: usize = 64;
/// Most other terminals one line stop may use.
pub const MAX_ALTERNATIVES: usize = 32;
/// Most transport modes a line lists.
pub const MAX_MODES: usize = 32;
/// Most waypoints after one stop of a line.
pub const MAX_WAYPOINTS: usize = 32;
/// Most industry types one prospection may find. Build 40408's economy has
/// at most a handful per cargo.
pub const MAX_INDUSTRY_TYPES: usize = 32;
/// Most nodes one level crossing names. TF3 builds a crossing on the two
/// ends of its connector.
pub const MAX_CROSSING_NODES: usize = 8;

/// A resource file name as the game lists it, such as
/// `street/standard/town_medium_new.lua` or a vehicle's `.mdl`.
pub type ResName = Text<128>;
/// A name a player gives something: a construction, a line, a company.
pub type ObjectName = Text<64>;

/// A position in the world, in millimetres, on the game's own axes. `i32`
/// reaches ±2,147 km, far past the largest map (65.5 km on a side).
/// Capture rounds metres to the nearest millimetre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// A position on the ground plane, in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pos2 {
    pub x: i32,
    pub y: i32,
}

/// A curve's Hermite tangent at one end, in millimetres: its length is part
/// of the curve's shape, so it is not normalised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tangent {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// A direction as a unit vector, in millionths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnitDir {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

macro_rules! canonical_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        ///
        /// Assigned by the server when the thing is created; never an engine
        /// entity id. Each replica maps it to its own entity.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

canonical_id!(
    /// A company.
    CompanyId,
    "company-"
);
canonical_id!(
    /// A line.
    LineId,
    "line-"
);
canonical_id!(
    /// A vehicle, a whole consist.
    VehicleId,
    "vehicle-"
);
canonical_id!(
    /// A station group, which line stops name.
    StationId,
    "station-"
);
canonical_id!(
    /// A town. Towns come with the room's world, not from an action: every
    /// game binds them, lowest entity first, at the room's first update.
    TownId,
    "town-"
);
canonical_id!(
    /// An industry, by its construction. Industries come with the room's
    /// world or from a prospection, and every game binds them as it binds
    /// towns (`tpf3mp/registry.lua`, `industries`).
    IndustryId,
    "industry-"
);

/// The two transport networks. A road node and a track node can stand at
/// the same place (a level crossing), so every reference to a node or edge
/// says which network it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Network {
    Street,
    Track,
}

/// An existing edge, named by its end positions. Orientation is not part of
/// the identity: which end is `node0` differs between games.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeEnds {
    pub a: Pos,
    pub b: Pos,
}

/// An existing edge in a given network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeRef {
    pub network: Network,
    pub ends: EdgeEnds,
}

/// An existing node, named by its position, in a given network: matched
/// within 0.5 m horizontally and in height, the nearest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeRef {
    pub network: Network,
    pub at: Pos,
}

/// A turn through a junction. Lane indices are counted from the junction,
/// as in TF3, so reversing an edge's local entity numbering changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneConnection {
    pub incoming: EdgeRef,
    pub lane_in: u16,
    pub outgoing: EdgeRef,
    pub lane_out: u16,
    pub road: bool,
    pub tram: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrafficPreference {
    Auto,
    Yes,
    No,
}

/// Durations in milliseconds. Locked indices address connections followed
/// by crosswalks, in the order carried by JunctionConfig (never entity ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficPhase {
    pub locked: BoundedVec<u16, MAX_CONNECTIONS>,
    pub duration: u32,
    pub minimum: u32,
    pub skip: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JunctionConfig {
    pub connections: BoundedVec<LaneConnection, MAX_CONNECTIONS>,
    pub crosswalks: BoundedVec<EdgeRef, MAX_CONNECTIONS>,
    pub preference: TrafficPreference,
    /// None is the game's default light type (-1); otherwise a resource name.
    pub light: Option<ResName>,
    pub phases: BoundedVec<TrafficPhase, MAX_PHASES>,
    pub double_slip: bool,
    pub custom_phases: bool,
}

/// None removes a configuration and restores the game's defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JunctionChange {
    pub node: NodeRef,
    pub config: Option<JunctionConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JunctionEdit {
    pub changes: BoundedVec<JunctionChange, MAX_JUNCTIONS>,
}

/// An existing construction, named by its file and its position (the
/// transform's origin). Matched within 2 m.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstructionRef {
    pub file: ResName,
    pub at: Pos,
}

/// What the originator's engine made of a polyline vertex. Receivers repeat
/// the decision instead of re-deriving it from a world that may have drifted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolve {
    /// A new node, attached to nothing that already existed.
    New,
    /// The existing node of this network at the vertex (within 1.5 m
    /// horizontally and 0.5 m in height, the nearest). A track vertex on a
    /// street node is a level crossing.
    Node(Network),
    /// A new node splitting this existing edge at the vertex. The halves
    /// keep the split edge's own type and flags.
    Split(EdgeRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vertex {
    pub pos: Pos,
    pub resolve: Resolve,
}

/// What an edge is built as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Structure {
    Ground,
    /// A bridge of this bridge type.
    Bridge(ResName),
    /// A tunnel of this tunnel type.
    Tunnel(ResName),
}

/// What an edge is built as when it is not the build's own street or track:
/// a piece of an existing street or track that the tool rebuilds around a new
/// junction or crossing keeps that road's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeKind {
    pub network: Network,
    /// Its road template (TF3's `BaseEdge.roadTemplate`).
    pub template: ResName,
    /// Its road style; none for the template's own.
    pub style: Option<ResName>,
}

/// One lane of an edge, as TF3's `LaneConfig` has it: its speed (in the
/// game's units, thousandths), width, height and offset in millimetres, its
/// direction, and the transport modes it carries, a bit for each
/// `TransportMode` value (a tram track or bus lane is a lane's modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub speed: i32,
    pub width: i32,
    pub height: i32,
    pub offset: i32,
    pub forward: bool,
    pub modes: u32,
}

/// A decoration along an edge (TF3's `BaseEdge.edgeDecorations`: a noise
/// barrier, an alley of trees), by its resource, with the game's flag for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoration {
    pub name: ResName,
    pub flag: bool,
}

/// One new edge between two vertices of its polyline, by index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub from: u16,
    pub to: u16,
    pub tangent0: Tangent,
    pub tangent1: Tangent,
    pub structure: Structure,
    /// None for the build's own street or track, in the build's style.
    pub kind: Option<EdgeKind>,
    /// Its decorations, as the tool left them.
    #[serde(default)]
    pub decorations: BoundedVec<Decoration, MAX_DECORATIONS>,
    /// Locked against the towns' road development
    /// (`roadDevelopmentLocked`).
    #[serde(default)]
    pub locked: bool,
    /// Owned by the acting company (`PLAYER_OWNED`), as the tool made it.
    #[serde(default)]
    pub owned: bool,
    /// Its lanes, as the tool made them (a tram track, a bus lane); empty
    /// for its template's own.
    #[serde(default)]
    pub lanes: BoundedVec<Lane, MAX_LANES>,
    /// A street's precedence at each end, as the tool set it
    /// (`BaseEdgeStreet.precedenceNode0`, `precedenceNode1`, the game's
    /// `PrecedencePreference` values); none for a track, or where the tool
    /// set none.
    #[serde(default)]
    pub precedence: Option<Precedence>,
    /// Where the edge itself ends at `from`, when that is not its vertex
    /// (TF3's `BaseEdge.position0`); none where it is. The track tool
    /// starts a track laid onto a street's end on the street's tram lane,
    /// beside the node, and only there does the game join the tram lane to
    /// the track (build 40420, schema 27).
    #[serde(default)]
    pub position0: Option<Pos>,
    /// The same at `to` (`BaseEdge.position1`).
    #[serde(default)]
    pub position1: Option<Pos>,
    /// The signals this edge takes from an edge the build removes, as the
    /// tool moved them: a track split where a signal stands, the signal on
    /// the piece it stood on (schema 27). Empty for an edge that takes
    /// none, or that rebuilds a removed edge in place, which takes all its
    /// objects.
    #[serde(default)]
    pub kept: BoundedVec<KeptSignal, MAX_EDGE_SIGNALS>,
}

/// A signal an edge takes from an edge the build removes: the one signal of
/// this construction on the polyline's removal number `removal` (from 0)
/// within a quarter of a metre of `at`, its place along that edge from the
/// removal's `a` end. None, or more than one, refuses the whole build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptSignal {
    pub removal: u16,
    pub at: Fraction,
    pub model: ResName,
}

/// A street's precedence at its two ends, the game's own values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precedence {
    pub node0: i32,
    pub node1: i32,
}

/// The geometry of one road or track build, as the tool proposed it: new
/// edges between vertices, and the existing edges and nodes it removes (an
/// upgrade's, a span the build passes under, the stretch of road TF3's tools
/// rebuild around a new junction or crossing). A split vertex names the edge
/// it splits, which is then no removal: the receiver removes it as it splits
/// it.
///
/// Decoding checks that there is at least one link and that every link joins
/// two different vertices of this polyline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PolylineFields")]
pub struct Polyline {
    pub vertices: BoundedVec<Vertex, MAX_VERTICES>,
    pub links: BoundedVec<Link, MAX_LINKS>,
    /// Existing edges removed, of either network.
    pub removals: BoundedVec<EdgeRef, MAX_EDGES>,
    /// Existing nodes removed: TF3's tools move a junction that was near the
    /// new one onto it.
    pub removed_nodes: BoundedVec<NodeRef, MAX_EDGES>,
    /// Full junction settings, including those a rebuilt edge must preserve.
    pub junctions: BoundedVec<JunctionChange, MAX_JUNCTIONS>,
}

#[derive(Deserialize)]
struct PolylineFields {
    vertices: BoundedVec<Vertex, MAX_VERTICES>,
    links: BoundedVec<Link, MAX_LINKS>,
    removals: BoundedVec<EdgeRef, MAX_EDGES>,
    removed_nodes: BoundedVec<NodeRef, MAX_EDGES>,
    #[serde(default)]
    junctions: BoundedVec<JunctionChange, MAX_JUNCTIONS>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PolylineError {
    #[error("a build with no edges")]
    NoLinks,
    #[error("link {0} names a vertex the polyline does not have")]
    NoSuchVertex(usize),
    #[error("link {0} joins a vertex to itself")]
    Loop(usize),
    #[error("link {0} ends too far from its vertex")]
    EndTooFar(usize),
    #[error("link {0} keeps a signal of a removal the polyline does not have, or off its edge")]
    KeptSignal(usize),
}

impl Polyline {
    /// A polyline that removes no nodes; see [`Polyline::with_removed_nodes`].
    pub fn new(
        vertices: BoundedVec<Vertex, MAX_VERTICES>,
        links: BoundedVec<Link, MAX_LINKS>,
        removals: BoundedVec<EdgeRef, MAX_EDGES>,
    ) -> Result<Self, PolylineError> {
        if links.is_empty() {
            return Err(PolylineError::NoLinks);
        }
        for (index, link) in links.iter().enumerate() {
            let count = vertices.len();
            if usize::from(link.from) >= count || usize::from(link.to) >= count {
                return Err(PolylineError::NoSuchVertex(index));
            }
            if link.from == link.to {
                return Err(PolylineError::Loop(index));
            }
            let ends = [(link.from, link.position0), (link.to, link.position1)];
            for (vertex, end) in ends {
                let Some(end) = end else { continue };
                let at = vertices[usize::from(vertex)].pos;
                let far = |a: i32, b: i32| {
                    (i64::from(a) - i64::from(b)).abs() > i64::from(MAX_END_OFFSET)
                };
                if far(end.x, at.x) || far(end.y, at.y) || far(end.z, at.z) {
                    return Err(PolylineError::EndTooFar(index));
                }
            }
            if link.kept.iter().any(|k| {
                usize::from(k.removal) >= removals.len() || !(0..=1_000_000).contains(&k.at.0)
            }) {
                return Err(PolylineError::KeptSignal(index));
            }
        }
        Ok(Self {
            vertices,
            links,
            removals,
            removed_nodes: BoundedVec::empty(),
            junctions: BoundedVec::empty(),
        })
    }

    /// This polyline, removing these existing nodes too.
    #[must_use]
    pub fn with_removed_nodes(mut self, nodes: BoundedVec<NodeRef, MAX_EDGES>) -> Self {
        self.removed_nodes = nodes;
        self
    }

    #[must_use]
    pub fn with_junctions(mut self, changes: BoundedVec<JunctionChange, MAX_JUNCTIONS>) -> Self {
        self.junctions = changes;
        self
    }
}

impl TryFrom<PolylineFields> for Polyline {
    type Error = PolylineError;

    fn try_from(fields: PolylineFields) -> Result<Self, PolylineError> {
        Ok(Self::new(fields.vertices, fields.links, fields.removals)?
            .with_removed_nodes(fields.removed_nodes)
            .with_junctions(fields.junctions))
    }
}

/// The tram track a street carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tram {
    None,
    Plain,
    Electric,
}

/// Transport Fever 3 types every street and track by a road template and a
/// road style (`BaseEdge.roadTemplate`, `roadStyle`); TPF2 had one type
/// file. A build names its template where TPF2 named the type, and its
/// style where the game has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadBuild {
    /// The street's type: its road template on TF3.
    pub street: ResName,
    /// The street's road style, on TF3.
    pub style: Option<ResName>,
    pub bus_lane: bool,
    pub tram: Tram,
    pub polyline: Polyline,
    /// The level crossing type the tool's toolbar picked (`rcType`), by its
    /// resource file, for the crossings the build lays anew; `None` where
    /// the tool picked none (docs/HOOKS.md, "Level crossing types").
    /// Appended under schema version 28.
    #[serde(default)]
    pub crossing_type: Option<ResName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackBuild {
    /// The track's type: its road template on TF3.
    pub track: ResName,
    /// The track's road style, on TF3.
    pub style: Option<ResName>,
    pub catenary: bool,
    pub polyline: Polyline,
    /// As [`RoadBuild::crossing_type`]. Appended under schema version 28.
    #[serde(default)]
    pub crossing_type: Option<ResName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bulldoze {
    /// Edges of one network, matched by their ends within 0.5 m,
    /// horizontally and in height. Nodes left with no edge go with them.
    Edges {
        network: Network,
        edges: BoundedVec<EdgeEnds, MAX_EDGES>,
        /// The town buildings the game removes with the edges, as the
        /// player's bulldozer showed them (schema 15): every game removes
        /// these, by file within 2 m of where each stands, and refuses the
        /// bulldoze if its game would remove any other or not all of them.
        buildings: BoundedVec<ConstructionRef, MAX_BUILDINGS>,
    },
    Construction(ConstructionRef),
    /// The stop, signal or waypoint of this model on this edge, nearest to
    /// `at`.
    EdgeObject {
        edge: EdgeRef,
        at: Pos,
        model: ResName,
    },
    /// Trees and other assets taken out of their asset group, which every
    /// game builds again from its own copy without them (schema 25).
    Assets(AssetRemoval),
    /// The stops one bulldozer click removes from one edge together: both
    /// of a two-sided stop (schema 27). Each is the stop of its model on
    /// the edge nearest its `at`, every one a different object; every game
    /// rebuilds the edge once without them.
    EdgeObjects {
        edge: EdgeRef,
        objects: BoundedVec<EdgeObjectAt, MAX_REMOVED_STOPS>,
    },
}

/// Most stops one bulldoze of an edge's objects removes: a two-sided stop's.
pub const MAX_REMOVED_STOPS: usize = 2;

/// A stop, signal or waypoint on an edge: its construction's file and where
/// it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeObjectAt {
    pub at: Pos,
    pub model: ResName,
}

/// One asset of an asset group: its model's file and where it stands,
/// matched within 5 mm.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetRef {
    pub model: ResName,
    pub at: Pos,
}

/// The asset bulldozer's removal: the group, named by its first asset and
/// how many it holds, and the assets taken out of it. Every game finds the
/// group that holds exactly that many assets, the first one and every one
/// removed among them, and builds it again from its own copy without them;
/// any other group refuses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRemoval {
    pub first: AssetRef,
    pub count: u32,
    pub removed: BoundedVec<AssetRef, MAX_ASSETS>,
    /// Each asset's turn goes the other way round in the game's matrix
    /// than `[cos, sin; -sin, cos]` (which way the tool built them, read
    /// off its proposal).
    pub mirrored: bool,
    /// Whether the tool's rebuilt group was owned by the player.
    pub owned: bool,
}

/// Where a construction stands: the game's 4x4 matrix as its rotation and
/// scale part, in millionths, and its origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transform {
    /// Elements 1-3, 5-7 and 9-11 of the game's matrix, in its order.
    pub basis: [i32; 9],
    /// Elements 13-15.
    pub origin: Pos,
}

/// A construction parameter's value. Lua numbers with no fraction are
/// `Int`; others are `Fixed`, in millionths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamValue {
    Int(i64),
    Fixed(i64),
    Bool(bool),
    Text(Text<128>),
}

/// One leaf of a construction's parameter table. Nested tables flatten
/// into paths: `modules[3801].name` is the `name` field of the entry at
/// integer key 3801 of `modules`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub key: Text<128>,
    pub value: ParamValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstructionBuild {
    pub file: ResName,
    pub transform: Transform,
    /// Every parameter, `seed` included: without it the game refuses.
    pub params: BoundedVec<Param, MAX_PARAMS>,
    /// Never empty in practice: an unnamed construction's children crash
    /// the game when clicked.
    pub name: ObjectName,
    /// The construction this one replaces: a module edit or an upgrade.
    pub replaces: Option<ConstructionRef>,
    /// The street and track changes the tool made with it, built in the same
    /// proposal: a station placed by a road joins it through a junction the
    /// road is rebuilt around, and an entrance edge to the station's own
    /// street node, which the construction then meets at the same place.
    /// Every link names its kind: a construction has no street of its own.
    pub connection: Option<Box<Polyline>>,
}

/// A fraction, in millionths: in Lua a plain number (0.25 is 250,000),
/// wherever it stands, a list included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fraction(pub i32);

/// A colour as the game keeps one (a `Vec3f`): red, green and blue, each a
/// fraction in millionths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tint {
    pub r: i32,
    pub g: i32,
    pub b: i32,
}

/// What one compartment of a vehicle loads (the game's `LoadConfig`): which
/// of its model's load configurations, and the cargo type, by the game's
/// numbering of its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Load {
    pub config: i32,
    pub cargo: i32,
}

/// One vehicle of a consist, as the depot's store configures it (the game's
/// `VehiclePart`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsistPart {
    /// Its model, as the game lists it (`api.res.modelRep`).
    pub model: ResName,
    /// It faces backwards.
    pub reversed: bool,
    /// Each compartment's load, in order.
    pub loads: BoundedVec<Load, MAX_COMPARTMENTS>,
    pub color: Tint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuyVehicle {
    pub depot: ConstructionRef,
    /// The consist, front to back.
    pub consist: BoundedVec<ConsistPart, MAX_CONSIST>,
    /// Its groups, front to back: each the number of its vehicles (a multiple
    /// unit is one group).
    pub groups: BoundedVec<u8, MAX_CONSIST>,
    /// For each group, the multiple unit's file, or empty.
    pub multiple_units: BoundedVec<Text<128>, MAX_CONSIST>,
    /// Which of the construction's depots, from 0: its `CONSTRUCTION.depots`,
    /// then its subconstructions that are depots (an airfield's or
    /// airport's hangar module), as the mod's `capture.depotsOf` lists
    /// them; an airport's second hangar, say. Added under schema version 20.
    #[serde(default)]
    pub depot_index: u8,
}

/// One vehicle of a replacement consist: the part as a purchase carries it,
/// and whether it is one the vehicle has already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacedPart {
    pub part: ConsistPart,
    /// The index, from 0, of the vehicle's own part this one keeps, with its
    /// age and wear, of the same model; none for a part bought new. TF3's
    /// store keeps a part the player left in the consist as it was (its
    /// purchase time), and buys the rest.
    pub kept: Option<u8>,
}

/// The vehicle window's "modify" and "replace" (`makeVehicleReplaceCmd`):
/// one vehicle's consist swapped for another, the vehicle staying the one
/// its line and orders name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceVehicle {
    pub vehicle: VehicleId,
    /// The new consist, front to back.
    pub consist: BoundedVec<ReplacedPart, MAX_CONSIST>,
    /// Its groups, as [`BuyVehicle::groups`].
    pub groups: BoundedVec<u8, MAX_CONSIST>,
    /// For each group, the multiple unit's file, or empty.
    pub multiple_units: BoundedVec<Text<128>, MAX_CONSIST>,
}

/// How long vehicles load at a stop (the game's `Line.LoadMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadMode {
    LoadIfAvailable,
    FullLoadAny,
    FullLoadAll,
    LegacyUnloadOnly,
}

/// What vehicles load and unload at a stop (the game's `Line.StopConfig`),
/// cargo type by cargo type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopRules {
    pub load: BoundedVec<bool, MAX_CARGOS>,
    /// The most of a vehicle's capacity each cargo type may take.
    pub max_load: BoundedVec<Fraction, MAX_CARGOS>,
    pub force_unload: bool,
    pub destroy_for_config_change: bool,
    pub destroy_for_refresh: bool,
}

/// A terminal of a station group: the station's place in the group, and the
/// terminal's in the station, both from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    pub station: u16,
    pub terminal: u16,
}

/// One stop of a line, as the game keeps it (`Line.Stop`), its station group
/// by canonical id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineStop {
    pub group: StationId,
    /// The terminal the line uses.
    pub terminal: Terminal,
    /// Other terminals it may use.
    pub alternatives: BoundedVec<Terminal, MAX_ALTERNATIVES>,
    pub load_mode: LoadMode,
    /// Waiting times, in the game's seconds, in millionths.
    pub min_wait: i64,
    pub max_wait: i64,
    pub max_extra_wait: i64,
    pub rules: StopRules,
    /// The waypoints after this stop, in order (`Line.Stop.waypoints`).
    /// Added under schema version 18.
    #[serde(default)]
    pub waypoints: BoundedVec<Waypoint, MAX_WAYPOINTS>,
}

/// Whose transport network a waypoint's lane is in: a street or track edge,
/// its ends in the originator's own order (node 0, then node 1: the lane's
/// index and place along it depend on which way the edge runs), or a
/// construction's, a station's tracks among them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkOf {
    Edge(EdgeRef),
    Construction(ConstructionRef),
}

/// Where a line's waypoint is (TF3's `Waypoint`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaypointAt {
    /// On a street or track: lane `index` of `of`'s transport network
    /// (`EdgePos.edgeId`), at `param` along it.
    Lane {
        of: NetworkOf,
        index: u16,
        param: Fraction,
    },
    /// A place in the open, which ships and aircraft are routed through
    /// (`Waypoint.pos`).
    Open(Pos),
}

/// A waypoint of a line, and the tag the line manager gave it, which names
/// it across edits (`Waypoint.tag`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waypoint {
    pub at: WaypointAt,
    pub tag: i32,
}

/// A line as the game keeps it (`Line`): its stops, the transport modes that
/// may run it (by the game's numbering), and its settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineData {
    pub stops: BoundedVec<LineStop, MAX_LINE_STOPS>,
    pub modes: BoundedVec<u16, MAX_MODES>,
    pub custom_filters: bool,
    /// In millionths.
    pub reservation_priority: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLine {
    pub name: ObjectName,
    pub color: Tint,
    pub line: LineData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineChange {
    Rename(ObjectName),
    Recolor(Tint),
    /// The whole line anew, as the line editor built it.
    Update(LineData),
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditLine {
    pub line: LineId,
    pub change: LineChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignLine {
    pub vehicles: BoundedVec<VehicleId, MAX_VEHICLES>,
    /// The line, or none to take the vehicles off their line.
    pub line: Option<LineId>,
    /// Index of the stop the vehicles head for first; none for the game's
    /// choice, the next stop each vehicle can reach (the line manager's
    /// "Next Reachable Stop", stop index -1 in TF3's command).
    pub first_stop: Option<u16>,
}

/// What the vehicle window does to one vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VehicleChange {
    /// Stopped by the player (true), or running again (false).
    Stop(bool),
    /// To the nearest depot. `sell` (sold on arrival) must be false:
    /// [`Action::validate`] refuses it, as build 40408 crashes when such a
    /// vehicle reaches its depot. The field stays for the wire's bytes.
    ToDepot {
        sell: bool,
    },
    Reverse,
    /// Leaves its terminal now.
    Depart,
    /// Waits at its stops until told to leave (true), or leaves by the
    /// line's own rules again (false): the game's manual departure, which a
    /// timetable mod holds and releases vehicles with (docs/MODS.md).
    /// Appended under schema version 10: the variants before it keep their
    /// bytes.
    ManualDeparture(bool),
    /// Its colour, as the vehicle window's and the line manager's colour
    /// buttons set it (`makeEntitySetColorCmd` on the vehicle). Appended
    /// under schema version 17.
    Recolor(Tint),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleOp {
    pub vehicle: VehicleId,
    pub change: VehicleChange,
}

/// A stop placed on an existing edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceStop {
    pub edge: EdgeRef,
    /// Where along the edge.
    pub at: Pos,
    /// The engine's side flag, which is not the geometric side.
    pub left: bool,
    /// The edge's direction at `at` on the originator; a receiver whose edge
    /// runs the other way flips `left`.
    pub direction: UnitDir,
    /// The stop's construction (Transport Fever 3 builds a stop as one,
    /// e.g. `stations/street/small_stops/small_new.con`).
    pub model: ResName,
    /// A stop on both sides at once (a `_twosided` construction): `left`
    /// names the side the originator's tool put first.
    #[serde(default)]
    pub two_sided: bool,
    /// What it is: a stop, or a waypoint or signal on a track (the game's
    /// edge object category).
    #[serde(default)]
    pub object: EdgeObjectKind,
    /// A one-way signal (the signal tool's "oneWay").
    #[serde(default)]
    pub one_way: bool,
    /// The name the originator's tool gave the stop
    /// (`street_util::MakeEdgeObjectName`: a street name from the town's
    /// name list, else "Stop #n"), which every game builds it with. None
    /// where the tool gave none the schema can carry: every game then names
    /// it by the mod's own rule.
    #[serde(default)]
    pub name: Option<ObjectName>,
    /// The construction parameters the originator's tool built it with
    /// (`EdgeObjectBuilder.params`: the construction's own keys, values as
    /// the tool's controls hold them), which every game builds it with
    /// (`SimpleStreetProposal.EdgeObject.params`, build 40408). A mod's
    /// settings on a signal travel here, Auto Signals' spacing among them.
    /// Empty: the construction's defaults. Schema 26.
    #[serde(default)]
    pub params: BoundedVec<Param, MAX_OBJECT_PARAMS>,
}

/// Signals added to, and removed from, existing tracks in one build: what
/// a mod's script sends after its player's signal (Auto Signals spaces
/// signals along the track from it; docs/MODS.md). Every edge named is
/// rebuilt in place, with its other objects kept, in one proposal: every
/// game builds all of it or none. Schema 26.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceSignals {
    /// The new signals' construction, the same for all of them (e.g.
    /// `infrastructure/signal/signal_path_c.con`).
    pub model: ResName,
    /// Whether the new signals are one-way.
    pub one_way: bool,
    /// The new signals' construction parameters; empty for the
    /// construction's defaults.
    pub params: BoundedVec<Param, MAX_OBJECT_PARAMS>,
    /// The tracks rebuilt, each once.
    pub edges: BoundedVec<SignalEdge, MAX_SIGNAL_EDGES>,
}

/// One track a [`PlaceSignals`] rebuilds: its ends in the originator's own
/// order, `a` its node 0. A game whose edge runs from `b` to `a` reads each
/// place as `1 - at` and flips `left`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalEdge {
    pub edge: EdgeEnds,
    pub add: BoundedVec<NewSignal, MAX_EDGE_SIGNALS>,
    pub remove: BoundedVec<OldSignal, MAX_EDGE_SIGNALS>,
}

/// A signal added: where along its edge from `a` (the game's
/// `EdgeObject.param`), and the engine's side flag there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSignal {
    pub at: Fraction,
    pub left: bool,
}

/// A signal removed: the one signal of this construction on the edge
/// within a quarter of a metre of `at`; none, or more than one, refuses
/// the whole build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldSignal {
    pub at: Fraction,
    pub model: ResName,
}

impl PlaceSignals {
    /// At least one signal added or removed; every place on its edge
    /// (0 to 1); no edge named twice, whichever way round its ends are.
    pub fn validate(&self) -> Result<(), ActionError> {
        if self
            .edges
            .iter()
            .all(|e| e.add.is_empty() && e.remove.is_empty())
        {
            return Err(ActionError::Signals("no signal added or removed"));
        }
        let mut seen = std::collections::HashSet::new();
        for edge in self.edges.iter() {
            let places = edge.add.iter().map(|s| s.at);
            if places
                .chain(edge.remove.iter().map(|s| s.at))
                .any(|at| !(0..=1_000_000).contains(&at.0))
            {
                return Err(ActionError::Signals("a place off its edge"));
            }
            let (a, b) = (edge.edge.a, edge.edge.b);
            let key = if (a.x, a.y, a.z) <= (b.x, b.y, b.z) {
                (a, b)
            } else {
                (b, a)
            };
            if !seen.insert(key) {
                return Err(ActionError::Signals("an edge named twice"));
            }
        }
        Ok(())
    }
}

/// What an edge object placed with the stop and signal tool is (TF3's
/// `EdgeObject.category`: 0, 1, 2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeObjectKind {
    #[default]
    Stop,
    Waypoint,
    Signal,
}

/// One terrain cell: the height it is set to and the height it had, in
/// millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerrainCell {
    pub target: i32,
    pub before: i32,
}

/// A terraform stroke as the grid the game computed. Decoding checks that
/// the cells fill whole rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TerraformFields")]
pub struct Terraform {
    /// The corner of the first cell.
    pub origin: Pos2,
    /// The side of a cell, in millimetres.
    pub cell: u32,
    /// Cells per row.
    pub columns: u16,
    /// Row by row, heights in millimetres.
    pub cells: BoundedVec<TerrainCell, MAX_TERRAIN_CELLS>,
}

#[derive(Deserialize)]
struct TerraformFields {
    origin: Pos2,
    cell: u32,
    columns: u16,
    cells: BoundedVec<TerrainCell, MAX_TERRAIN_CELLS>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{cells} cells do not fill rows of {columns}, or cells have no size")]
pub struct GridError {
    pub cells: usize,
    pub columns: u16,
}

impl Terraform {
    pub fn new(
        origin: Pos2,
        cell: u32,
        columns: u16,
        cells: BoundedVec<TerrainCell, MAX_TERRAIN_CELLS>,
    ) -> Result<Self, GridError> {
        let whole_rows = columns != 0 && cells.len().is_multiple_of(usize::from(columns));
        if cell == 0 || cells.is_empty() || !whole_rows {
            return Err(GridError {
                cells: cells.len(),
                columns,
            });
        }
        Ok(Self {
            origin,
            cell,
            columns,
            cells,
        })
    }
}

impl TryFrom<TerraformFields> for Terraform {
    type Error = GridError;

    fn try_from(fields: TerraformFields) -> Result<Self, GridError> {
        Self::new(fields.origin, fields.cell, fields.columns, fields.cells)
    }
}

/// A run of `count` cells of one value, row by row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaintRun {
    pub value: u8,
    pub count: u32,
}

/// One of a paint stroke's grids: its first cell and size in the grid's
/// own cells, and its values as runs that fill it exactly. Decoding checks
/// that they do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PaintGridFields")]
pub struct PaintGrid {
    pub x0: i32,
    pub y0: i32,
    pub columns: u32,
    pub rows: u32,
    pub runs: BoundedVec<PaintRun, MAX_PAINT_RUNS>,
}

#[derive(Deserialize)]
struct PaintGridFields {
    x0: i32,
    y0: i32,
    columns: u32,
    rows: u32,
    runs: BoundedVec<PaintRun, MAX_PAINT_RUNS>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("runs of {cells} cells do not fill a grid of {columns} by {rows}")]
pub struct PaintGridError {
    pub cells: u64,
    pub columns: u32,
    pub rows: u32,
}

impl PaintGrid {
    pub fn new(
        x0: i32,
        y0: i32,
        columns: u32,
        rows: u32,
        runs: BoundedVec<PaintRun, MAX_PAINT_RUNS>,
    ) -> Result<Self, PaintGridError> {
        let cells: u64 = runs.iter().map(|r| u64::from(r.count)).sum();
        let empty_run = runs.iter().any(|r| r.count == 0);
        if columns == 0 || rows == 0 || empty_run || cells != u64::from(columns) * u64::from(rows) {
            return Err(PaintGridError {
                cells,
                columns,
                rows,
            });
        }
        Ok(Self {
            x0,
            y0,
            columns,
            rows,
            runs,
        })
    }

    /// Every cell's value, row by row.
    pub fn cells(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((self.columns as usize) * (self.rows as usize));
        for run in self.runs.iter() {
            out.extend(std::iter::repeat_n(run.value, run.count as usize));
        }
        out
    }
}

impl TryFrom<PaintGridFields> for PaintGrid {
    type Error = PaintGridError;

    fn try_from(f: PaintGridFields) -> Result<Self, PaintGridError> {
        Self::new(f.x0, f.y0, f.columns, f.rows, f.runs)
    }
}

/// One asset the asset brush places: its model, by its index in the
/// stroke's `models` from 0, and its world matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedAsset {
    pub model: u16,
    pub transform: Transform,
}

/// An asset group a stroke replaces, named as the asset bulldozer names
/// one: its first asset and how many it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetGroupRef {
    pub first: AssetRef,
    pub count: u32,
}

/// An asset brush stroke (trees, rocks, bushes: `UI::ProposalAction`, as
/// the terrain tools): the groups it replaces, and the groups it builds,
/// each its assets (the ones a replaced group kept among them). Every game
/// removes the same groups and builds the same ones. Appended under schema
/// version 27.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetStroke {
    pub models: BoundedVec<ResName, MAX_STROKE_MODELS>,
    pub removed: BoundedVec<AssetGroupRef, MAX_STROKE_GROUPS>,
    pub groups: BoundedVec<BoundedVec<PlacedAsset, MAX_GROUP_ASSETS>, MAX_STROKE_GROUPS>,
}

impl AssetStroke {
    /// Whether it does something, and names only models it lists.
    pub fn validate(&self) -> Result<(), ActionError> {
        if self.removed.is_empty() && self.groups.is_empty() {
            return Err(ActionError::Assets("a stroke that changes nothing"));
        }
        if self.groups.iter().any(|g| g.is_empty()) {
            return Err(ActionError::Assets("a group of no assets"));
        }
        let models = self.models.len();
        if self
            .groups
            .iter()
            .flat_map(|g| g.iter())
            .any(|a| usize::from(a.model) >= models)
        {
            return Err(ActionError::Assets(
                "an asset of a model the stroke does not name",
            ));
        }
        Ok(())
    }
}

/// A terrain paint stroke (the terrain painter, `UI::ProposalAction`) as
/// the grids the game computed: each texel's material (255 leaves it as it
/// is), and the mask grid beside it, each cell 0 or 1, where the stroke had
/// one. Appended under schema version 27.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerrainPaint {
    pub materials: PaintGrid,
    pub mask: Option<PaintGrid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompanyOp {
    Create {
        name: ObjectName,
    },
    /// The player plays for this company from now on.
    Join(CompanyId),
    Rename {
        company: CompanyId,
        name: ObjectName,
    },
    Delete(CompanyId),
    /// The company's colour, for its vehicles and lines. Appended under
    /// schema version 8: the variants before it keep their bytes.
    Recolor {
        company: CompanyId,
        color: Tint,
    },
    /// The company's head gives it the password the intent carries beside
    /// it (a [`crate::Secret`] whose scope is the company), or a new one:
    /// joining it then needs the password. Every game keeps only the
    /// password's seal. Appended under schema version 9, as the variants
    /// after it.
    Lock(CompanyId),
    /// The company's head takes its password away: anyone may join again.
    Unlock(CompanyId),
    /// The company's head sends a player out of it: they play for the
    /// room's first company again.
    Dismiss {
        company: CompanyId,
        /// The player, as the mod names players: 64 lowercase hex digits.
        player: PlayerHex,
    },
    /// The company's head opens its stations to other companies' lines, or
    /// closes them. A company's stations start open. This is the default:
    /// it holds for every company without a choice of its own
    /// ([`CompanyOp::StationAccess`]), those founded later included.
    ShareStations {
        company: CompanyId,
        open: bool,
    },
    /// The company's head lets the lines of one other company stop at its
    /// stations (`Some(true)`), or not (`Some(false)`), whatever the default
    /// says; `None` leaves that company to the default again. Per company,
    /// not per player: a company's players share everything it owns.
    /// Appended under schema version 23: the variants before it keep their
    /// bytes.
    StationAccess {
        company: CompanyId,
        other: CompanyId,
        open: Option<bool>,
    },
}

/// A player as the mod names one: their id's 64 lowercase hex digits.
pub type PlayerHex = Text<64>;

/// A loan on its terms, as Transport Fever 3's loan script keeps it
/// (`game_mechanics/finance/loan.d.tl`), field for field. Nothing in it
/// names an entity: a loan is the acting player's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoanTerms {
    /// "Small", "Medium", "Large", "ExtraLarge" or "Custom".
    #[serde(rename = "type")]
    pub kind: Text<16>,
    /// In the game's money.
    pub amount: i64,
    /// In the game's milliseconds.
    pub duration: i64,
    /// The interest a year, in millionths: 0.03 is 30 000.
    pub percentage: i64,
    #[serde(rename = "birthDay")]
    pub birth_day: Option<i64>,
    #[serde(rename = "cooldownUntil")]
    pub cooldown_until: Option<i64>,
    #[serde(rename = "lastPayDay")]
    pub last_pay_day: Option<i64>,
    #[serde(rename = "timesPaid")]
    pub times_paid: Option<i64>,
    pub id: Option<i64>,
}

/// Taking or paying back a loan: the loan script's two events, with the
/// parameters the game's finance window sends them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoanOp {
    /// Take the loan `offer`; the game's window also sends the offer it
    /// drew to follow it (`next`), whose kind the script puts on cooldown.
    Take { next: LoanTerms, offer: LoanTerms },
    /// Pay back a loan taken, named by its terms and id.
    Repay { loan: LoanTerms },
}

/// A subsidy the game offers, as Transport Fever 3's subsidy script keeps
/// it (`game_mechanics/subventions/subventions.script.tl`): its own number
/// (`uid`) and its kind, the subsidy resource that drew it (`id`, such as
/// `::/game_mechanics/subventions/deliver_cargo/deliver_cargo.res`). Every
/// game draws the same offers from the same world; the kind is there so a
/// game whose offer under that number is another kind refuses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubsidyRef {
    pub uid: i64,
    pub kind: ResName,
}

/// Answering a subsidy offer: what TF3's subsidy window sends the subsidy
/// script (`Subvention` `onAccept` and `onDecline`, `subventions_gui.tl`).
/// Appended under schema version 13.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubsidyOp {
    /// The acting player's company takes the offer: the first in the
    /// room's order to accept it gets it, every later one is refused alike
    /// in every game.
    Accept(SubsidyRef),
    /// The offer is declined: it is gone for every company, as in single
    /// player.
    Decline(SubsidyRef),
}

/// Prospecting near a town for one cargo: what TF3's construction menu sends
/// the company script when the player picks a town with a prospection
/// (`gui/construction/construction_react_util.tl`, the event `Companies`
/// `spawnIndustry`), field for field. The game decides the rest, months
/// later, from its own state and the game time, the same in every game
/// (investigation/TPF3_PROSPECTING_2026-09-30.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prospect {
    pub town: TownId,
    /// The cargo prospected for, a cargo resource (`cargoType`).
    pub cargo: ResName,
    /// The industry types that may be found, as the economy tags them, in
    /// the originator's order: the game shuffles them with draws taken in
    /// this order (`types`).
    pub industries: BoundedVec<ResName, MAX_INDUSTRY_TYPES>,
    /// The company permit it uses (`permitKey`), if it names one.
    pub permit: Option<ResName>,
}

/// What an entity window's title, or the line manager's vehicle list,
/// renames (`makeEntitySetNameCmd`), when it is not a line or a company
/// (those are [`LineChange::Rename`] and [`CompanyOp::Rename`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Renamed {
    Vehicle(VehicleId),
    /// A station group, which the station's window names.
    Station(StationId),
    Town(TownId),
    /// Any other construction: a depot, an industry, a landmark.
    Construction(ConstructionRef),
}

/// A company perk the construction menu's perk tools use on a town or an
/// industry (`gui/construction/tools/*.script.tl`): an event to TF3's
/// company script, which spends the perk's permit for the acting company
/// and passes the perk on to the towns or emissions script. Appended under
/// schema version 24.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerkOp {
    /// Industry Greenification (`Companies` `MakeGreen`,
    /// `industry_greenify_tool.script.tl`): the industry's emissions cut.
    Greenify {
        industry: IndustryId,
        /// The company permit it spends (`permitKey`), if it names one.
        permit: Option<ResName>,
    },
    /// A marketing campaign in a town (`Companies`
    /// `startMarketingCampaign`, `marketing_campaign_tool.script.tl`),
    /// with the campaign's terms as the tool's metadata gives them, and its
    /// cost, which the tool books to the company once the campaign started.
    Marketing {
        town: TownId,
        /// How long it runs, in the game's milliseconds (`durationMs`).
        duration_ms: i64,
        /// `lineCostFactor`.
        line_cost_factor: Fraction,
        permit: Option<ResName>,
        /// In the game's money, as the tool priced it at the click.
        cost: i64,
    },
}

/// A town building's Historic Preservation checkbox
/// (`makeTownBuildingSetBlockedDevelopmentCmd`,
/// `gui/entity_window/town_building/town_building.tl`): the building keeps
/// its look but still levels up. Appended under schema version 24.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preservation {
    /// The construction the town building stands in, by its file and place.
    pub building: ConstructionRef,
    /// Which of that construction's town buildings, from 0.
    pub index: u8,
    /// Preserved (true), or free to change again (false).
    pub preserved: bool,
}

/// A level crossing's type, chosen in the crossing's window (the native
/// `RailroadCrossingComp`, whose build maps the crossing to the type: docs/
/// HOOKS.md, "Level crossing types"). Appended under schema version 27.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossingType {
    /// The crossing, by the places of all its nodes, sorted and each once:
    /// a crossing whose nodes are not exactly these is another one.
    pub nodes: BoundedVec<Pos, MAX_CROSSING_NODES>,
    /// The type, by its resource file (`railroadCrossingTypeRep`'s name),
    /// never its index, which each game assigns as it loads.
    pub crossing_type: ResName,
}

impl CrossingType {
    /// Refuses a crossing named by no node, by unsorted or repeated ones, or
    /// a type with no name.
    pub fn validate(&self) -> Result<(), ActionError> {
        if self.nodes.is_empty() {
            return Err(ActionError::Crossing("a crossing with no node"));
        }
        let key = |p: &Pos| (p.x, p.y, p.z);
        if self.nodes.windows(2).any(|w| key(&w[0]) >= key(&w[1])) {
            return Err(ActionError::Crossing(
                "nodes not sorted, or one named twice",
            ));
        }
        if self.crossing_type.as_str().is_empty() {
            return Err(ActionError::Crossing("a type with no name"));
        }
        Ok(())
    }
}

/// Refuses a build's level crossing type with no name.
fn no_unnamed(crossing_type: &Option<ResName>) -> Result<(), ActionError> {
    match crossing_type {
        Some(name) if name.as_str().is_empty() => Err(ActionError::Crossing("a type with no name")),
        _ => Ok(()),
    }
}

/// One player action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    BuildRoad(RoadBuild),
    BuildTrack(TrackBuild),
    Bulldoze(Bulldoze),
    BuildConstruction(ConstructionBuild),
    BuyVehicle(BuyVehicle),
    SellVehicle {
        vehicles: BoundedVec<VehicleId, MAX_VEHICLES>,
    },
    CreateLine(CreateLine),
    EditLine(EditLine),
    AssignLine(AssignLine),
    PlaceStop(PlaceStop),
    Terraform(Terraform),
    CompanyOp(CompanyOp),
    /// Boxed: its terms are larger than every other action.
    Loan(Box<LoanOp>),
    VehicleOp(VehicleOp),
    ReplaceVehicle(ReplaceVehicle),
    Prospect(Prospect),
    /// A notification's popup played its first sound: the game's
    /// Notifications script marks it so (its `initialSound` event), in every
    /// game, so the sound is not played again.
    NotificationSeen {
        notification: u32,
    },
    /// Taking a company rank the company has reached: what TF3's company
    /// window sends the company growth script (`Companies` `applyLevel`,
    /// `game_mechanics/company/company.tl`), the rank being `level` there.
    /// The acting player's company takes it. Appended under schema version
    /// 9, after `NotificationSeen`: the variants before it keep their bytes.
    ApplyRank {
        /// The rank to take, 1 to 15 in the game.
        level: u8,
    },
    /// Crosswalks, turning lanes and the full traffic-light configuration.
    EditJunctions(JunctionEdit),
    /// Accepting or declining a subsidy offer (`SubsidyOp`). Appended under
    /// schema version 13: the variants before it keep their bytes.
    Subsidy(SubsidyOp),
    /// Renaming what is not a line or a company (`Renamed`): a vehicle, a
    /// station, a town, a construction. Appended under schema version 17:
    /// the variants before it keep their bytes.
    Rename {
        what: Renamed,
        name: ObjectName,
    },
    /// A company perk used on a town or an industry (`PerkOp`). Appended
    /// under schema version 24: the variants before it keep their bytes.
    Perk(PerkOp),
    /// A town building's Historic Preservation (`Preservation`). Appended
    /// under schema version 24: the variants before it keep their bytes.
    Preserve(Preservation),
    /// Shared calendar pace, independent of simulation speed. Zero pauses
    /// the date; positive values are the game's day length in milliseconds.
    CalendarSpeed {
        millis_per_day: u32,
    },
    /// Signals added to and removed from existing tracks in one build
    /// (`PlaceSignals`). Appended under schema version 26: the variants
    /// before it keep their bytes.
    PlaceSignals(PlaceSignals),
    /// A terrain paint stroke (`TerrainPaint`). Appended under schema
    /// version 27: the variants before it keep their bytes.
    Paint(TerrainPaint),
    /// An asset brush stroke (`AssetStroke`). Appended under schema version
    /// 27, after `Paint`.
    PlantAssets(AssetStroke),
    /// A level crossing's type ([`CrossingType`]). Appended after the other
    /// unreleased schema-27 actions; earlier action tags stay fixed.
    SetCrossingType(CrossingType),
}

#[derive(Debug, Error)]
pub enum ActionError {
    #[error("calendar day length exceeds the game's signed integer range")]
    CalendarSpeed,
    #[error("invalid junction configuration: {0}")]
    Junction(&'static str),
    #[error("action schema {found}; this game speaks {ACTION_SCHEMA_VERSION}")]
    Schema { found: u32 },
    #[error("malformed action: {0}")]
    Malformed(#[from] postcard::Error),
    #[error("{0} unexpected bytes after the action")]
    TrailingBytes(usize),
    #[error(transparent)]
    TooLarge(#[from] PayloadTooLarge),
    /// A vehicle sent to its depot to be sold there. Build 40408 sells it on
    /// arrival (`Engine::RemoveEntity`) and then asks the removed vehicle
    /// where its depot is, which fails the engine's assertion
    /// (`Engine.h:323`, `GetComponentDataIndex`) and ends every game in the
    /// room at the same step. TF3's own windows only ever send `false`.
    #[error(
        "selling a vehicle when it reaches its depot (the game crashes there; sell it instead)"
    )]
    SellOnArrival,
    /// A [`PlaceSignals`] that builds nothing, names an edge twice or a
    /// place off its edge.
    #[error("invalid signals: {0}")]
    Signals(&'static str),
    /// A [`TerrainPaint`] whose mask holds more than bits.
    #[error("invalid paint: {0}")]
    Paint(&'static str),
    /// An [`AssetStroke`] that changes nothing or names a model it lacks.
    #[error("invalid asset stroke: {0}")]
    Assets(&'static str),
    /// A two-sided stop removal that does not carry one same-construction pair.
    #[error("invalid bulldoze: {0}")]
    Bulldoze(&'static str),
    /// A [`CrossingType`] naming no node, a node twice or out of order, or
    /// no type.
    #[error("invalid level crossing: {0}")]
    Crossing(&'static str),
}

impl Action {
    /// The action's kind, as its variant is named (and as the mod's Lua
    /// tables name it), for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Action::BuildRoad(_) => "BuildRoad",
            Action::BuildTrack(_) => "BuildTrack",
            Action::Bulldoze(_) => "Bulldoze",
            Action::BuildConstruction(_) => "BuildConstruction",
            Action::BuyVehicle(_) => "BuyVehicle",
            Action::SellVehicle { .. } => "SellVehicle",
            Action::CreateLine(_) => "CreateLine",
            Action::EditLine(_) => "EditLine",
            Action::AssignLine(_) => "AssignLine",
            Action::PlaceStop(_) => "PlaceStop",
            Action::Terraform(_) => "Terraform",
            Action::CompanyOp(_) => "CompanyOp",
            Action::Loan(_) => "Loan",
            Action::VehicleOp(_) => "VehicleOp",
            Action::ReplaceVehicle(_) => "ReplaceVehicle",
            Action::Prospect(_) => "Prospect",
            Action::NotificationSeen { .. } => "NotificationSeen",
            Action::ApplyRank { .. } => "ApplyRank",
            Action::EditJunctions(_) => "EditJunctions",
            Action::Subsidy(_) => "Subsidy",
            Action::Rename { .. } => "Rename",
            Action::Perk(_) => "Perk",
            Action::Preserve(_) => "Preserve",
            Action::CalendarSpeed { .. } => "CalendarSpeed",
            Action::PlaceSignals(_) => "PlaceSignals",
            Action::Paint(_) => "Paint",
            Action::PlantAssets(_) => "PlantAssets",
            Action::SetCrossingType(_) => "SetCrossingType",
        }
    }

    /// Validate relationships the wire's bounds do not: a two-sided stop
    /// removal's pair, the relationships within a junction, and no vehicle
    /// sent to be sold on arrival ([`ActionError::SellOnArrival`]).
    pub fn validate(&self) -> Result<(), ActionError> {
        if let Self::Bulldoze(Bulldoze::EdgeObjects { objects, .. }) = self {
            if objects.len() != 2 {
                return Err(ActionError::Bulldoze(
                    "a two-sided stop removal must name both objects",
                ));
            }
            if objects[0] == objects[1] {
                return Err(ActionError::Bulldoze(
                    "a two-sided stop removal must name distinct objects",
                ));
            }
            if objects[0].model != objects[1].model {
                return Err(ActionError::Bulldoze(
                    "a two-sided stop removal must name one construction",
                ));
            }
        }
        let changes = match self {
            Self::CalendarSpeed { millis_per_day } if *millis_per_day > i32::MAX as u32 => {
                return Err(ActionError::CalendarSpeed);
            }
            Self::VehicleOp(VehicleOp {
                change: VehicleChange::ToDepot { sell: true },
                ..
            }) => return Err(ActionError::SellOnArrival),
            Self::EditJunctions(edit) => {
                if edit.changes.is_empty() {
                    return Err(ActionError::Junction("empty edit"));
                }
                &edit.changes
            }
            Self::BuildRoad(road) => {
                no_unnamed(&road.crossing_type)?;
                &road.polyline.junctions
            }
            Self::BuildTrack(track) => {
                no_unnamed(&track.crossing_type)?;
                &track.polyline.junctions
            }
            Self::BuildConstruction(build) => match &build.connection {
                Some(line) => &line.junctions,
                None => return Ok(()),
            },
            Self::PlaceSignals(signals) => return signals.validate(),
            Self::PlantAssets(stroke) => return stroke.validate(),
            Self::Paint(paint) => {
                let bits = paint
                    .mask
                    .as_ref()
                    .is_none_or(|mask| mask.runs.iter().all(|r| r.value <= 1));
                return if bits {
                    Ok(())
                } else {
                    Err(ActionError::Paint("a mask cell that is not 0 or 1"))
                };
            }
            Self::SetCrossingType(crossing) => return crossing.validate(),
            _ => return Ok(()),
        };
        for (i, change) in changes.iter().enumerate() {
            if changes[..i].iter().any(|other| other.node == change.node) {
                return Err(ActionError::Junction("node changed twice"));
            }
            let Some(config) = &change.config else {
                continue;
            };
            for turn in config.connections.iter() {
                if usize::from(turn.lane_in) >= MAX_LANES || usize::from(turn.lane_out) >= MAX_LANES
                {
                    return Err(ActionError::Junction("lane index out of bounds"));
                }
            }
            let count = config.connections.len() + config.crosswalks.len();
            for phase in config.phases.iter() {
                if phase.minimum > phase.duration || phase.duration > 86_400_000 {
                    return Err(ActionError::Junction("invalid phase duration"));
                }
                for (j, lane) in phase.locked.iter().enumerate() {
                    if usize::from(*lane) >= count || phase.locked[..j].contains(lane) {
                        return Err(ActionError::Junction("invalid or duplicate locked lane"));
                    }
                }
            }
        }
        Ok(())
    }
    /// The payload of an intent carrying this action: the schema version,
    /// then the action, both postcard-encoded.
    pub fn to_payload(&self) -> Result<Payload, ActionError> {
        self.validate()?;
        let mut bytes = postcard::to_stdvec(&ACTION_SCHEMA_VERSION)?;
        bytes.extend(postcard::to_stdvec(self)?);
        Ok(Payload::new(bytes)?)
    }

    /// The action a payload carries. Refuses another schema version, and
    /// bytes after the action.
    pub fn from_payload(payload: &Payload) -> Result<Self, ActionError> {
        let (version, rest): (u32, _) = postcard::take_from_bytes(payload.as_bytes())?;
        if version != ACTION_SCHEMA_VERSION {
            return Err(ActionError::Schema { found: version });
        }
        let (action, rest): (Self, _) = postcard::take_from_bytes(rest)?;
        if !rest.is_empty() {
            return Err(ActionError::TrailingBytes(rest.len()));
        }
        action.validate()?;
        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: i32, y: i32, z: i32) -> Pos {
        Pos { x, y, z }
    }

    fn link(from: u16, to: u16) -> Link {
        Link {
            precedence: None,
            position0: None,
            position1: None,
            kept: BoundedVec::default(),

            from,
            to,
            tangent0: Tangent { x: 1, y: 0, z: 0 },
            tangent1: Tangent { x: 1, y: 0, z: 0 },
            structure: Structure::Ground,
            kind: None,
            decorations: BoundedVec::default(),
            locked: false,
            owned: false,
            lanes: BoundedVec::default(),
        }
    }

    fn vertices(count: usize) -> BoundedVec<Vertex, MAX_VERTICES> {
        let list = (0..count)
            .map(|i| Vertex {
                pos: pos(i32::try_from(i).unwrap() * 1000, 0, 0),
                resolve: Resolve::New,
            })
            .collect();
        BoundedVec::new(list).unwrap()
    }

    #[test]
    fn polyline_links_must_name_its_vertices() {
        let links = |list| BoundedVec::new(list).unwrap();
        assert_eq!(
            Polyline::new(vertices(2), links(vec![]), BoundedVec::empty()),
            Err(PolylineError::NoLinks)
        );
        assert_eq!(
            Polyline::new(vertices(2), links(vec![link(0, 2)]), BoundedVec::empty()),
            Err(PolylineError::NoSuchVertex(0))
        );
        assert_eq!(
            Polyline::new(
                vertices(2),
                links(vec![link(0, 1), link(1, 1)]),
                BoundedVec::empty()
            ),
            Err(PolylineError::Loop(1))
        );
        assert!(Polyline::new(vertices(2), links(vec![link(1, 0)]), BoundedVec::empty()).is_ok());
        // A kept signal names one of the polyline's removals, on its edge.
        let keeping = |removal, at| Link {
            kept: BoundedVec::new(vec![KeptSignal {
                removal,
                at: Fraction(at),
                model: Text::new("s").unwrap(),
            }])
            .unwrap(),
            ..link(0, 1)
        };
        let one = || {
            BoundedVec::new(vec![EdgeRef {
                network: Network::Track,
                ends: EdgeEnds {
                    a: pos(0, 0, 0),
                    b: pos(1, 0, 0),
                },
            }])
            .unwrap()
        };
        assert_eq!(
            Polyline::new(vertices(2), links(vec![keeping(0, 0)]), BoundedVec::empty()),
            Err(PolylineError::KeptSignal(0))
        );
        assert_eq!(
            Polyline::new(vertices(2), links(vec![keeping(1, 0)]), one()),
            Err(PolylineError::KeptSignal(0))
        );
        assert_eq!(
            Polyline::new(vertices(2), links(vec![keeping(0, 1_000_001)]), one()),
            Err(PolylineError::KeptSignal(0))
        );
        assert!(Polyline::new(vertices(2), links(vec![keeping(0, 1_000_000)]), one()).is_ok());
    }

    #[test]
    fn a_link_ends_near_its_vertex() {
        let links = |list| BoundedVec::new(list).unwrap();
        // Vertex 1 is at (1000, 0, 0): an end 2 m beside it, on a street's
        // tram lane, is the track tool's; one 40 m off is no edge's end.
        let mut beside = link(0, 1);
        beside.position1 = Some(pos(1000, -2000, 0));
        assert!(Polyline::new(vertices(2), links(vec![beside]), BoundedVec::empty()).is_ok());
        let mut far = link(0, 1);
        far.position0 = Some(pos(0, 0, MAX_END_OFFSET + 1));
        assert_eq!(
            Polyline::new(
                vertices(2),
                links(vec![link(0, 1), far]),
                BoundedVec::empty()
            ),
            Err(PolylineError::EndTooFar(1))
        );
    }

    #[test]
    fn decoding_checks_the_polyline() {
        // Encode an invalid polyline through the unchecked field struct's
        // layout: the same fields, in the same order.
        #[derive(Serialize)]
        struct Raw {
            vertices: BoundedVec<Vertex, MAX_VERTICES>,
            links: BoundedVec<Link, MAX_LINKS>,
            removals: BoundedVec<EdgeRef, MAX_EDGES>,
            removed_nodes: BoundedVec<NodeRef, MAX_EDGES>,
        }
        let raw = Raw {
            vertices: vertices(1),
            links: BoundedVec::new(vec![link(0, 1)]).unwrap(),
            removals: BoundedVec::empty(),
            removed_nodes: BoundedVec::empty(),
        };
        let bytes = postcard::to_stdvec(&raw).unwrap();
        assert!(postcard::from_bytes::<Polyline>(&bytes).is_err());
    }

    #[test]
    fn terraform_cells_fill_rows() {
        let cells = |n| {
            BoundedVec::new(vec![
                TerrainCell {
                    target: 1,
                    before: 0
                };
                n
            ])
            .unwrap()
        };
        let origin = Pos2 { x: 0, y: 0 };
        assert!(Terraform::new(origin, 4000, 3, cells(6)).is_ok());
        assert!(Terraform::new(origin, 4000, 3, cells(7)).is_err());
        assert!(Terraform::new(origin, 4000, 0, cells(6)).is_err());
        assert!(Terraform::new(origin, 0, 3, cells(6)).is_err());
        assert!(Terraform::new(origin, 4000, 3, cells(0)).is_err());
    }

    /// Guards against accidental wire changes, which the Lua mod's encoder
    /// would not notice. Update deliberately, with ACTION_SCHEMA_VERSION.
    #[test]
    fn wire_format_is_stable() {
        let action = Action::SellVehicle {
            vehicles: BoundedVec::new(vec![VehicleId(3), VehicleId(300)]).unwrap(),
        };
        let payload = action.to_payload().unwrap();
        assert_eq!(
            payload.as_bytes(),
            [
                28, // schema version
                5,  // Action::SellVehicle
                2, 3, 0xac, 0x02, // two ids, varints
            ]
        );
        let track = Action::BuildTrack(TrackBuild {
            track: Text::new("t").unwrap(),
            style: Some(Text::new("s").unwrap()),
            catenary: true,
            polyline: Polyline::new(
                BoundedVec::new(vec![
                    Vertex {
                        pos: pos(-1, 1, 0),
                        resolve: Resolve::Node(Network::Street),
                    },
                    Vertex {
                        pos: pos(0, 0, 0),
                        resolve: Resolve::New,
                    },
                ])
                .unwrap(),
                BoundedVec::new(vec![link(0, 1)]).unwrap(),
                BoundedVec::new(vec![EdgeRef {
                    network: Network::Street,
                    ends: EdgeEnds {
                        a: pos(1, 0, 0),
                        b: pos(0, 1, 0),
                    },
                }])
                .unwrap(),
            )
            .unwrap()
            .with_removed_nodes(
                BoundedVec::new(vec![NodeRef {
                    network: Network::Track,
                    at: pos(0, 0, 1),
                }])
                .unwrap(),
            ),
            crossing_type: Some(Text::new("c").unwrap()),
        });
        assert_eq!(
            track.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                1,  // Action::BuildTrack
                1, b't', 1, 1, b's', 1, // track, style Some("s"), catenary
                2, // two vertices
                1, 2, 0, 1, 0, // (-1, 1, 0) zigzag, Resolve::Node(Street)
                0, 0, 0, 0, // (0, 0, 0), Resolve::New
                1, // one link
                0, 1, 2, 0, 0, 2, 0, 0, 0, // 0 -> 1, tangents, Structure::Ground
                0, // the build's own kind
                0, 0, 0, 0, 0, // no decorations, lock, ownership, lanes or precedence
                0, 0, // no edge end offset at either vertex
                0, // no kept signals (schema 27)
                1, 0, 2, 0, 0, 0, 2, 0, // a removal: Street, (1, 0, 0), (0, 1, 0)
                1, 1, 0, 0, 2, // a removed node: Track, (0, 0, 1)
                0, // no junction changes
                1, 1, b'c', // crossing type Some("c"), schema 28
            ]
        );
        // Final schema 27 link order after precedence: position0, position1, kept.
        let keeping = Link {
            position0: Some(pos(1, -2, 3)),
            position1: Some(pos(4, 5, 6)),
            kept: BoundedVec::new(vec![KeptSignal {
                removal: 1,
                at: Fraction(750_000),
                model: Text::new("s").unwrap(),
            }])
            .unwrap(),
            ..link(0, 1)
        };
        assert_eq!(
            postcard::to_stdvec(&keeping).unwrap()[15..],
            [
                1, 2, 3, 6, // Some(position0): (1, -2, 3)
                1, 8, 10, 12, // Some(position1): (4, 5, 6)
                1,  // one kept signal
                1,  // removal 1
                0xe0, 0xc6, 0x5b, // at 750,000, zigzag varint
                1, b's', // its construction
            ]
        );
        // Appended with Prospect under schema version 7: the variants
        // before them keep their bytes.
        let replace = Action::ReplaceVehicle(ReplaceVehicle {
            vehicle: VehicleId(3),
            consist: BoundedVec::new(vec![ReplacedPart {
                part: ConsistPart {
                    model: Text::new("m").unwrap(),
                    reversed: true,
                    loads: BoundedVec::empty(),
                    color: Tint { r: 1, g: 0, b: 0 },
                },
                kept: Some(2),
            }])
            .unwrap(),
            groups: BoundedVec::new(vec![1]).unwrap(),
            multiple_units: BoundedVec::new(vec![Text::new("").unwrap()]).unwrap(),
        });
        assert_eq!(
            replace.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                14, // Action::ReplaceVehicle
                3,  // vehicle-3
                1, 1, b'm', 1, 0, 2, 0, 0, // one part: model, reversed, no loads, colour
                1, 2, // kept: Some(2)
                1, 1, 1, 0, // groups { 1 }, multiple units { "" }
            ]
        );
        let prospect = Action::Prospect(Prospect {
            town: TownId(3),
            cargo: Text::new("c").unwrap(),
            industries: BoundedVec::new(vec![Text::new("m").unwrap(), Text::new("q").unwrap()])
                .unwrap(),
            permit: None,
        });
        assert_eq!(
            prospect.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                15, // Action::Prospect
                3,  // town-3
                1, b'c', // cargo
                2, 1, b'm', 1, b'q', // two industry types, in order
                0,    // no permit
            ]
        );
        let recolor = Action::CompanyOp(CompanyOp::Recolor {
            company: CompanyId(2),
            color: Tint { r: 1, g: 0, b: 0 },
        });
        assert_eq!(
            recolor.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                11, // Action::CompanyOp
                4,  // CompanyOp::Recolor, appended under schema version 8
                2,  // company-2
                2, 0, 0, // the colour, zigzag
            ]
        );
        let rank = Action::ApplyRank { level: 6 };
        assert_eq!(
            rank.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                17, // Action::ApplyRank, appended under schema version 9
                6,  // the rank
            ]
        );
        let accept = Action::Subsidy(SubsidyOp::Accept(SubsidyRef {
            uid: 1_234_560_000,
            kind: Text::new("s").unwrap(),
        }));
        assert_eq!(
            accept.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                19, // Action::Subsidy, appended under schema version 13
                0,  // SubsidyOp::Accept
                0x80, 0x90, 0xaf, 0x99, 0x09, // the uid, zigzag varint
                1, b's', // the kind
            ]
        );
        // Appended under schema version 9: the company's head's own.
        let cases: [(CompanyOp, &[u8]); 5] = [
            (CompanyOp::Lock(CompanyId(2)), &[5, 2]),
            (CompanyOp::Unlock(CompanyId(2)), &[6, 2]),
            (
                CompanyOp::Dismiss {
                    company: CompanyId(2),
                    player: Text::new("ab").unwrap(),
                },
                &[7, 2, 2, b'a', b'b'],
            ),
            (
                CompanyOp::ShareStations {
                    company: CompanyId(2),
                    open: false,
                },
                &[8, 2, 0],
            ),
            // Appended under schema version 23.
            (
                CompanyOp::StationAccess {
                    company: CompanyId(2),
                    other: CompanyId(3),
                    open: Some(false),
                },
                &[9, 2, 3, 1, 0],
            ),
        ];
        for (op, bytes) in cases {
            let payload = Action::CompanyOp(op).to_payload().unwrap();
            assert_eq!(payload.as_bytes()[..2], [28, 11]);
            assert_eq!(&payload.as_bytes()[2..], bytes);
        }
        // Appended under schema version 24: the perk tools take the next tag.
        let green = Action::Perk(PerkOp::Greenify {
            industry: IndustryId(5),
            permit: None,
        });
        assert_eq!(green.to_payload().unwrap().as_bytes(), [28, 21, 0, 5, 0]);
        // Appended under schema version 24: Historic Preservation takes the
        // next tag.
        let preserve = Action::Preserve(Preservation {
            building: ConstructionRef {
                file: Text::new("b").unwrap(),
                at: pos(1, 0, 0),
            },
            index: 0,
            preserved: true,
        });
        assert_eq!(
            preserve.to_payload().unwrap().as_bytes(),
            [28, 22, 1, b'b', 2, 0, 0, 0, 1]
        );
        // Appended under schema version 26: signals on existing tracks take
        // the next tag.
        let signals = Action::PlaceSignals(PlaceSignals {
            model: Text::new("s").unwrap(),
            one_way: true,
            params: BoundedVec::empty(),
            edges: BoundedVec::new(vec![SignalEdge {
                edge: EdgeEnds {
                    a: pos(0, 0, 0),
                    b: pos(1, 0, 0),
                },
                add: BoundedVec::new(vec![NewSignal {
                    at: Fraction(5),
                    left: true,
                }])
                .unwrap(),
                remove: BoundedVec::empty(),
            }])
            .unwrap(),
        });
        assert_eq!(
            signals.to_payload().unwrap().as_bytes(),
            [28, 24, 1, b's', 1, 0, 1, 0, 0, 0, 2, 0, 0, 1, 10, 1, 0]
        );
        // Schema 27 appends the terrain and asset actions, then the
        // two-sided stop removal within Bulldoze. Earlier tags stay fixed.
        let paint = Action::Paint(TerrainPaint {
            materials: PaintGrid::new(
                -1,
                2,
                2,
                1,
                BoundedVec::new(vec![PaintRun { value: 3, count: 2 }]).unwrap(),
            )
            .unwrap(),
            mask: None,
        });
        assert_eq!(
            paint.to_payload().unwrap().as_bytes(),
            [28, 25, 1, 4, 2, 1, 1, 3, 2, 0]
        );
        let plant = Action::PlantAssets(AssetStroke {
            models: BoundedVec::new(vec![Text::new("t").unwrap()]).unwrap(),
            removed: BoundedVec::empty(),
            groups: BoundedVec::new(vec![
                BoundedVec::new(vec![PlacedAsset {
                    model: 0,
                    transform: Transform {
                        basis: [1, 0, 0, 0, 1, 0, 0, 0, 1],
                        origin: pos(1, 0, 0),
                    },
                }])
                .unwrap(),
            ])
            .unwrap(),
        });
        assert_eq!(
            plant.to_payload().unwrap().as_bytes(),
            [
                28, 26, 1, 1, b't', 0, 1, 1, 0, 2, 0, 0, 0, 2, 0, 0, 0, 2, 2, 0, 0
            ]
        );
        let stops = Action::Bulldoze(Bulldoze::EdgeObjects {
            edge: EdgeRef {
                network: Network::Street,
                ends: EdgeEnds {
                    a: pos(0, 0, 0),
                    b: pos(1, 0, 0),
                },
            },
            objects: BoundedVec::new(vec![
                EdgeObjectAt {
                    at: pos(0, 1, 0),
                    model: Text::new("s").unwrap(),
                },
                EdgeObjectAt {
                    at: pos(0, -1, 0),
                    model: Text::new("s").unwrap(),
                },
            ])
            .unwrap(),
        });
        assert_eq!(
            stops.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                2,  // Action::Bulldoze
                4,  // Bulldoze::EdgeObjects
                0, 0, 0, 0, 2, 0, 0, // Street, (0, 0, 0), (1, 0, 0)
                2, 0, 2, 0, 1, b's', 0, 1, 0, 1, b's', // two stops, (0, 1, 0) and (0, -1, 0)
            ]
        );
        // The next schema-27 top-level tag carries a level crossing's type.
        let crossing = Action::SetCrossingType(CrossingType {
            nodes: BoundedVec::new(vec![pos(0, 0, 0), pos(1, 0, 0)]).unwrap(),
            crossing_type: Text::new("c").unwrap(),
        });
        assert_eq!(
            crossing.to_payload().unwrap().as_bytes(),
            [28, 27, 2, 0, 0, 0, 2, 0, 0, 1, b'c']
        );
        let hold = Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(7),
            change: VehicleChange::ManualDeparture(true),
        });
        assert_eq!(
            hold.to_payload().unwrap().as_bytes(),
            [
                28, // schema version
                13, // Action::VehicleOp
                7,  // vehicle-7
                4,  // VehicleChange::ManualDeparture, appended under schema version 10
                1,  // held
            ]
        );
        assert_eq!(
            Action::from_payload(&hold.to_payload().unwrap()).unwrap(),
            hold
        );
    }

    #[test]
    fn payload_refuses_other_schemas_and_trailing_bytes() {
        let action = Action::CompanyOp(CompanyOp::Join(CompanyId(2)));
        let payload = action.to_payload().unwrap();
        assert_eq!(Action::from_payload(&payload).unwrap(), action);

        let mut other = payload.as_bytes().to_vec();
        other[0] = 1;
        assert!(matches!(
            Action::from_payload(&Payload::new(other).unwrap()),
            Err(ActionError::Schema { found: 1 })
        ));

        let mut padded = payload.as_bytes().to_vec();
        padded.push(0);
        assert!(matches!(
            Action::from_payload(&Payload::new(padded).unwrap()),
            Err(ActionError::TrailingBytes(1))
        ));
    }

    #[test]
    fn ids_display_with_their_kind() {
        assert_eq!(LineId(7).to_string(), "line-7");
        assert_eq!(StationId(12).to_string(), "station-12");
        assert_eq!(IndustryId(3).to_string(), "industry-3");
    }
}
