//! The scenarios the harness plays, and builders that keep them readable.
//! Positions are in metres; stations, depots and vehicles are numbered by
//! canonical id, which counts up from 0 in the order they were made.

use std::sync::Arc;

use tpf3mp_proto::{
    BoundedVec, Text,
    action::{
        Action, AssignLine, Bulldoze, BuyVehicle, CompanyId, CompanyOp, ConsistPart,
        ConstructionBuild, ConstructionRef, CreateLine, Decoration, EdgeEnds, EdgeKind,
        EdgeObjectKind, EdgeRef, EditLine, Lane, LineChange, LineData, LineId, LineStop, Link,
        LoadMode, Network, Param, ParamValue, PlaceStop, Polyline, Pos, Pos2, Prospect, Renamed,
        ReplaceVehicle, ReplacedPart, Resolve, RoadBuild, StationId, StopRules, Structure, Tangent,
        Terminal, Terraform, TerrainCell, Tint, TownId, TrackBuild, Tram, Transform, UnitDir,
        VehicleChange, VehicleId, VehicleOp, Vertex,
    },
};

use super::{
    model::{PROSPECTION_STEPS, START_MONEY},
    script::{Check, Item, Scenario},
};

pub const STREET: &str = "street/standard/town_medium_new.lua";
pub const TRACK: &str = "track/standard.lua";
pub const BRIDGE: &str = "bridge/stone.lua";
pub const BUS_STATION: &str = "station/street/bus_station.con";
pub const TRAIN_STATION: &str = "station/rail/train_station.con";
pub const AIRFIELD: &str = "station/air/airfield.con";
pub const AIRPORT: &str = "station/air/airport.con";
pub const BUS_DEPOT: &str = "depot/bus_depot.con";
pub const TRAIN_DEPOT: &str = "depot/train_depot.con";
pub const BUS: &str = "vehicle/bus/standard_bus.mdl";
pub const LOCOMOTIVE: &str = "vehicle/train/standard_loco.mdl";
pub const WAGON: &str = "vehicle/waggon/standard_coach.mdl";
pub const SMALL_AIRCRAFT: &str = "vehicle/plane/f13.mdl";
pub const STREET_STOP: &str = "station/street/street_stop.mdl";

/// A point on the ground, in metres.
pub fn at(x: i32, y: i32) -> Pos {
    Pos {
        x: x * 1000,
        y: y * 1000,
        z: 0,
    }
}

fn text<const N: usize>(value: &str) -> Text<N> {
    Text::new(value).expect("names in scenarios are short")
}

fn list<T, const N: usize>(items: Vec<T>) -> BoundedVec<T, N> {
    BoundedVec::new(items).expect("scenarios stay within the schema's bounds")
}

/// A vertex where nothing was.
pub fn new(pos: Pos) -> Vertex {
    Vertex {
        pos,
        resolve: Resolve::New,
    }
}

/// A vertex on the existing node of `network` there.
pub fn node(pos: Pos, network: Network) -> Vertex {
    Vertex {
        pos,
        resolve: Resolve::Node(network),
    }
}

/// A vertex splitting the edge `a`-`b` of `network` at `pos`.
pub fn split(pos: Pos, network: Network, a: Pos, b: Pos) -> Vertex {
    Vertex {
        pos,
        resolve: Resolve::Split(EdgeRef {
            network,
            ends: EdgeEnds { a, b },
        }),
    }
}

/// Straight edges through the vertices in order.
pub fn polyline(vertices: Vec<Vertex>, structure: &Structure) -> Polyline {
    let links = vertices
        .windows(2)
        .enumerate()
        .map(|(i, pair)| {
            let (a, b) = (pair[0].pos, pair[1].pos);
            let tangent = Tangent {
                x: b.x - a.x,
                y: b.y - a.y,
                z: b.z - a.z,
            };
            Link {
                precedence: None,
                position0: None,
                position1: None,
                kept: BoundedVec::default(),

                from: u16::try_from(i).expect("few vertices"),
                to: u16::try_from(i + 1).expect("few vertices"),
                tangent0: tangent,
                tangent1: tangent,
                structure: structure.clone(),
                kind: None,
                decorations: BoundedVec::default(),
                locked: false,
                owned: false,
                lanes: BoundedVec::default(),
            }
        })
        .collect();
    Polyline::new(list(vertices), list(links), BoundedVec::empty())
        .expect("a polyline of at least two vertices")
}

pub fn road(vertices: Vec<Vertex>) -> Action {
    Action::BuildRoad(RoadBuild {
        street: text(STREET),
        style: None,
        bus_lane: false,
        tram: Tram::None,
        polyline: polyline(vertices, &Structure::Ground),
        crossing_type: None,
    })
}

pub fn track(vertices: Vec<Vertex>, structure: &Structure) -> Action {
    Action::BuildTrack(TrackBuild {
        track: text(TRACK),
        style: None,
        catenary: true,
        polyline: polyline(vertices, structure),
        crossing_type: None,
    })
}

/// A construction of `file` at `pos`, facing along x.
pub fn construction(file: &str, pos: Pos, name: &str) -> Action {
    Action::BuildConstruction(ConstructionBuild {
        file: text(file),
        transform: Transform {
            basis: [1_000_000, 0, 0, 0, 1_000_000, 0, 0, 0, 1_000_000],
            origin: pos,
        },
        params: list(vec![
            Param {
                key: text("seed"),
                value: ParamValue::Int(0),
            },
            Param {
                key: text("length"),
                value: ParamValue::Int(2),
            },
        ]),
        name: text(name),
        replaces: None,
        connection: None,
    })
}

fn reference(file: &str, pos: Pos) -> ConstructionRef {
    ConstructionRef {
        file: text(file),
        at: pos,
    }
}

pub fn buy_indexed(depot: &str, pos: Pos, depot_index: u8, consist: &[&str]) -> Action {
    Action::BuyVehicle(BuyVehicle {
        depot: reference(depot, pos),
        consist: list(
            consist
                .iter()
                .map(|model| ConsistPart {
                    model: text(model),
                    reversed: false,
                    loads: BoundedVec::empty(),
                    color: Tint { r: 0, g: 0, b: 0 },
                })
                .collect(),
        ),
        groups: list(vec![u8::try_from(consist.len()).expect("a short consist")]),
        multiple_units: list(vec![text("")]),
        depot_index,
    })
}

pub fn buy(depot: &str, pos: Pos, consist: &[&str]) -> Action {
    buy_indexed(depot, pos, 0, consist)
}

/// A vehicle's consist replaced: each car a model and, for one the vehicle
/// has already, the index of the car it keeps.
pub fn replace(vehicle: u32, consist: &[(&str, Option<u8>)]) -> Action {
    Action::ReplaceVehicle(ReplaceVehicle {
        vehicle: VehicleId(vehicle),
        consist: list(
            consist
                .iter()
                .map(|(model, kept)| ReplacedPart {
                    part: ConsistPart {
                        model: text(model),
                        reversed: false,
                        loads: BoundedVec::empty(),
                        color: Tint { r: 0, g: 0, b: 0 },
                    },
                    kept: *kept,
                })
                .collect(),
        ),
        groups: list(vec![u8::try_from(consist.len()).expect("a short consist")]),
        multiple_units: list(vec![text("")]),
    })
}

/// The colour the scenarios paint a vehicle.
const RED: Tint = Tint {
    r: 900_000,
    g: 100_000,
    b: 100_000,
};

/// A name given from an entity's window or the line manager.
pub fn rename(what: Renamed, name: &str) -> Action {
    Action::Rename {
        what,
        name: text(name),
    }
}

/// A vehicle's colour from its window.
pub fn recolor(vehicle: u32, tint: Tint) -> Action {
    Action::VehicleOp(VehicleOp {
        vehicle: VehicleId(vehicle),
        change: VehicleChange::Recolor(tint),
    })
}

pub fn sell(vehicles: &[u32]) -> Action {
    Action::SellVehicle {
        vehicles: list(vehicles.iter().map(|v| VehicleId(*v)).collect()),
    }
}

/// A line through the stations, each at its first terminal, loading what is
/// there.
fn line_data(stations: &[u32]) -> LineData {
    LineData {
        stops: list(
            stations
                .iter()
                .map(|s| LineStop {
                    group: StationId(*s),
                    terminal: Terminal {
                        station: 0,
                        terminal: 0,
                    },
                    alternatives: BoundedVec::empty(),
                    load_mode: LoadMode::LoadIfAvailable,
                    min_wait: 0,
                    max_wait: 180_000_000,
                    max_extra_wait: 0,
                    rules: StopRules {
                        load: BoundedVec::empty(),
                        max_load: BoundedVec::empty(),
                        force_unload: false,
                        destroy_for_config_change: false,
                        destroy_for_refresh: false,
                    },
                    waypoints: BoundedVec::empty(),
                })
                .collect(),
        ),
        modes: BoundedVec::empty(),
        custom_filters: false,
        reservation_priority: 0,
    }
}

pub fn line(name: &str, stations: &[u32]) -> Action {
    Action::CreateLine(CreateLine {
        name: text(name),
        color: Tint {
            r: 800_000,
            g: 160_000,
            b: 160_000,
        },
        line: line_data(stations),
    })
}

pub fn edit(line: u32, change: LineChange) -> Action {
    Action::EditLine(EditLine {
        line: LineId(line),
        change,
    })
}

pub fn set_stops(line: u32, stations: &[u32]) -> Action {
    edit(line, LineChange::Update(line_data(stations)))
}

pub fn assign(vehicles: &[u32], line: Option<u32>, first_stop: Option<u16>) -> Action {
    Action::AssignLine(AssignLine {
        vehicles: list(vehicles.iter().map(|v| VehicleId(*v)).collect()),
        line: line.map(LineId),
        first_stop,
    })
}

pub fn bulldoze_edges(network: Network, edges: &[(Pos, Pos)]) -> Action {
    Action::Bulldoze(Bulldoze::Edges {
        network,
        edges: list(
            edges
                .iter()
                .map(|(a, b)| EdgeEnds { a: *a, b: *b })
                .collect(),
        ),
        buildings: list(Vec::new()),
    })
}

pub fn bulldoze_construction(file: &str, pos: Pos) -> Action {
    Action::Bulldoze(Bulldoze::Construction(reference(file, pos)))
}

fn street_edge(a: Pos, b: Pos) -> EdgeRef {
    EdgeRef {
        network: Network::Street,
        ends: EdgeEnds { a, b },
    }
}

/// A street stop on the street `a`-`b`, at `pos`.
pub fn place_stop(a: Pos, b: Pos, pos: Pos) -> Action {
    Action::PlaceStop(PlaceStop {
        edge: street_edge(a, b),
        at: pos,
        left: false,
        direction: UnitDir {
            x: 1_000_000,
            y: 0,
            z: 0,
        },
        model: text(STREET_STOP),
        two_sided: false,
        object: EdgeObjectKind::Stop,
        one_way: false,
        name: None,
        params: BoundedVec::empty(),
    })
}

pub fn bulldoze_stop(a: Pos, b: Pos, pos: Pos) -> Action {
    Action::Bulldoze(Bulldoze::EdgeObject {
        edge: street_edge(a, b),
        at: pos,
        model: text(STREET_STOP),
    })
}

/// A road or track modifier's upgrade of the straight edge `a`-`b` of
/// `network`: rebuilt in place in `template`, with one lane carrying
/// `modes` (a bit per transport mode: a tram track, catenary), a noise
/// barrier, locked and owned, the old edge removed.
pub fn upgrade(network: Network, a: Pos, b: Pos, template: &str, modes: u32) -> Action {
    let mut polyline = polyline(vec![node(a, network), node(b, network)], &Structure::Ground);
    let mut links = polyline.links.to_vec();
    links[0].kind = Some(EdgeKind {
        network,
        template: text(template),
        style: None,
    });
    links[0].decorations = list(vec![Decoration {
        name: text("::/infrastructure/edge_addons/barrier_b.edge"),
        flag: false,
    }]);
    links[0].locked = true;
    links[0].owned = true;
    links[0].lanes = list(vec![Lane {
        speed: 22_222,
        width: 3_000,
        height: 0,
        offset: 0,
        forward: true,
        modes,
    }]);
    polyline = Polyline::new(
        polyline.vertices.clone(),
        list(links),
        list(vec![EdgeRef {
            network,
            ends: EdgeEnds { a, b },
        }]),
    )
    .expect("two vertices");
    match network {
        Network::Street => Action::BuildRoad(RoadBuild {
            street: text(STREET),
            style: None,
            bus_lane: false,
            tram: Tram::None,
            polyline,
            crossing_type: None,
        }),
        Network::Track => Action::BuildTrack(TrackBuild {
            track: text(TRACK),
            style: None,
            catenary: false,
            polyline,
            crossing_type: None,
        }),
    }
}

/// Raises a `columns` by `rows` grid of `cell`-metre cells to `height` mm.
pub fn terraform(x: i32, y: i32, cell: u32, columns: u16, rows: u16, height: i32) -> Action {
    let cells = vec![
        TerrainCell {
            target: height,
            before: 0,
        };
        usize::from(columns) * usize::from(rows)
    ];
    Action::Terraform(
        Terraform::new(
            Pos2 {
                x: x * 1000,
                y: y * 1000,
            },
            cell * 1000,
            columns,
            list(cells),
        )
        .expect("whole rows"),
    )
}

pub fn company(op: CompanyOp) -> Action {
    Action::CompanyOp(op)
}

/// Prospecting near town `town` for `cargo`, as TF3's construction menu
/// sends it.
pub fn prospect(town: u32, cargo: &str, industries: &[&str]) -> Action {
    Action::Prospect(Prospect {
        town: TownId(town),
        cargo: text(cargo),
        industries: list(industries.iter().map(|kind| text(kind)).collect()),
        permit: Some(text(cargo)),
    })
}

impl Scenario {
    /// The room has exactly the actors in it.
    fn exact(mut self) -> Self {
        self.exact_players = true;
        self
    }
}

/// Builds a scenario's items.
#[derive(Default)]
pub struct Script {
    items: Vec<Item>,
}

impl Script {
    pub fn act(mut self, actor: usize, action: Action) -> Self {
        self.items.push(Item::Act { actor, action });
        self
    }

    pub fn run(mut self, steps: u64) -> Self {
        self.items.push(Item::Run(steps));
        self
    }

    pub fn expect(mut self, check: Check) -> Self {
        self.items.push(Item::Expect(check));
        self
    }

    /// Each check in turn.
    pub fn expect_all(mut self, checks: impl IntoIterator<Item = Check>) -> Self {
        self.items.extend(checks.into_iter().map(Item::Expect));
        self
    }

    pub fn scenario(self, name: &str, about: &str, smoke: bool, actors: usize) -> Scenario {
        Scenario {
            name: name.to_owned(),
            about: about.to_owned(),
            smoke,
            actors,
            exact_players: false,
            items: self.items,
        }
    }
}

/// A bus line for `actor` between two new stations at `x0` and `x1` along
/// the street at height `y`, with `buses` buses: stations `first`,
/// `first + 1`, line `line`, vehicles from `vehicle`.
fn bus_line(script: Script, actor: usize, y: i32, buses: u32, ids: (u32, u32, u32)) -> Script {
    let (first_station, line_id, first_vehicle) = ids;
    let (a, b) = (at(0, y), at(1500, y));
    let mut script = script
        .act(actor, road(vec![new(a), new(b)]))
        .act(actor, construction(BUS_STATION, at(0, y + 20), "West"))
        .act(actor, construction(BUS_STATION, at(1500, y + 20), "East"))
        .act(actor, construction(BUS_DEPOT, at(700, y + 20), "Depot"));
    for _ in 0..buses {
        script = script.act(actor, buy(BUS_DEPOT, at(700, y + 20), &[BUS]));
    }
    let vehicles: Vec<u32> = (first_vehicle..first_vehicle + buses).collect();
    script
        .act(actor, line("Bus", &[first_station, first_station + 1]))
        .act(actor, assign(&vehicles, Some(line_id), Some(0)))
}

fn bus_line_scenario() -> Scenario {
    let (a, b, c) = (at(0, 0), at(1000, 0), at(1000, 1000));
    Script::default()
        .act(0, road(vec![new(a), new(b)]))
        .expect(Check::StreetEdges(1))
        .act(0, road(vec![node(b, Network::Street), new(c)]))
        .expect(Check::StreetEdges(2))
        .act(0, construction(BUS_STATION, at(0, 20), "Harbour"))
        .act(0, construction(BUS_STATION, at(1000, 1020), "Hill"))
        .expect_all([Check::Stations(2), Check::Constructions(2)])
        .act(0, construction(BUS_DEPOT, at(500, 20), "Depot"))
        .expect(Check::Depots(1))
        .act(0, buy(BUS_DEPOT, at(500, 20), &[BUS]))
        .act(0, buy(BUS_DEPOT, at(500, 20), &[BUS]))
        .act(0, buy(BUS_DEPOT, at(500, 20), &[BUS]))
        .expect_all([Check::Vehicles(3), Check::Idle(3)])
        .act(0, line("Harbour - Hill", &[0, 1]))
        .expect(Check::Lines(1))
        .act(0, assign(&[0, 1, 2], Some(0), Some(0)))
        .expect_all([
            Check::Line {
                line: LineId(0),
                stops: 2,
                vehicles: 3,
            },
            Check::Idle(0),
            Check::MoneyBelow {
                actor: 0,
                money: START_MONEY,
            },
        ])
        .run(2_000)
        .expect_all([Check::DeliveredAtLeast(1), Check::Ignored(0)])
        .scenario(
            "bus-line",
            "build streets, two bus stations and a depot, buy buses, make a line, assign them, carry passengers",
            true,
            1,
        )
}

fn rail_line_scenario() -> Scenario {
    let track_net = Network::Track;
    let (t0, t1, t2) = (at(0, 0), at(2000, 0), at(4000, 0));
    let (mid, far) = (at(1000, 0), at(1000, 1000));
    Script::default()
        .act(0, track(vec![new(t0), new(t1), new(t2)], &Structure::Ground))
        .expect(Check::TrackEdges(2))
        // A branch over a bridge, from a switch splitting the first edge.
        .act(
            0,
            track(
                vec![split(mid, track_net, t0, t1), new(far)],
                &Structure::Bridge(text(BRIDGE)),
            ),
        )
        .expect(Check::TrackEdges(4))
        // A level crossing on the track's middle node.
        .act(0, road(vec![new(at(2000, -1000)), node(t1, track_net)]))
        .expect(Check::StreetEdges(1))
        .act(0, construction(TRAIN_STATION, at(0, 30), "West"))
        .act(0, construction(TRAIN_STATION, at(4000, 30), "East"))
        .act(0, construction(TRAIN_DEPOT, at(2000, 30), "Yard"))
        .expect_all([Check::Stations(2), Check::Depots(1)])
        .act(
            0,
            buy(TRAIN_DEPOT, at(2000, 30), &[LOCOMOTIVE, WAGON, WAGON, WAGON]),
        )
        .act(0, buy(TRAIN_DEPOT, at(2000, 30), &[LOCOMOTIVE, WAGON, WAGON]))
        .act(0, line("West - East", &[0, 1]))
        .act(0, assign(&[0], Some(0), Some(0)))
        .act(0, assign(&[1], Some(0), Some(1)))
        .expect(Check::Line {
            line: LineId(0),
            stops: 2,
            vehicles: 2,
        })
        .run(3_000)
        .expect(Check::DeliveredAtLeast(1))
        // The second train lengthened in the vehicle window: its own cars
        // kept, a coach bought; it stays vehicle-1, on its line.
        .act(
            0,
            replace(
                1,
                &[
                    (LOCOMOTIVE, Some(0)),
                    (WAGON, Some(1)),
                    (WAGON, Some(2)),
                    (WAGON, None),
                ],
            ),
        )
        .expect_all([
            Check::Vehicles(2),
            Check::Line {
                line: LineId(0),
                stops: 2,
                vehicles: 2,
            },
        ])
        .act(0, bulldoze_edges(track_net, &[(far, mid)]))
        .expect_all([Check::TrackEdges(3), Check::Ignored(0)])
        .scenario(
            "rail-line",
            "lay track with a switch, a bridge and a level crossing, build stations and a yard, run two trains, lengthen one",
            true,
            1,
        )
}

fn two_companies_scenario() -> Scenario {
    let script = bus_line(Script::default(), 0, 0, 1, (0, 0, 0));
    let script = script
        .expect_all([
            Check::CompanyOf {
                actor: 0,
                company: Some(CompanyId(0)),
            },
            Check::CompanyOf {
                actor: 1,
                company: Some(CompanyId(1)),
            },
            Check::MoneyBelow {
                actor: 0,
                money: START_MONEY,
            },
        ])
        // Nobody uses what another company owns.
        .act(1, assign(&[0], Some(0), Some(0)))
        .expect(Check::Ignored(1))
        .act(
            1,
            bulldoze_edges(Network::Street, &[(at(0, 0), at(1500, 0))]),
        )
        .expect_all([Check::Ignored(2), Check::StreetEdges(1)])
        .act(1, buy(BUS_DEPOT, at(700, 20), &[BUS]))
        .expect_all([Check::Ignored(3), Check::Vehicles(1)])
        // Nor renames or recolours it: the bus and the station keep their
        // names and the bus its own colours.
        .act(1, rename(Renamed::Vehicle(VehicleId(0)), "Mine"))
        .act(1, rename(Renamed::Station(StationId(0)), "Mine"))
        .act(1, recolor(0, RED))
        .expect_all([
            Check::Ignored(6),
            Check::VehicleName {
                vehicle: VehicleId(0),
                name: "Vehicle 0".into(),
            },
            Check::VehicleColor {
                vehicle: VehicleId(0),
                color: None,
            },
            Check::StationName {
                station: StationId(0),
                name: "West".into(),
            },
        ]);
    bus_line(script, 1, 3000, 2, (2, 1, 1))
        .expect_all([
            Check::Lines(2),
            Check::Line {
                line: LineId(1),
                stops: 2,
                vehicles: 2,
            },
            Check::Line {
                line: LineId(0),
                stops: 2,
                vehicles: 1,
            },
            Check::MoneyBelow {
                actor: 1,
                money: START_MONEY,
            },
        ])
        // Its own it may name.
        .act(1, rename(Renamed::Station(StationId(2)), "Far West"))
        .expect(Check::StationName {
            station: StationId(2),
            name: "Far West".into(),
        })
        .run(1_500)
        .expect_all([Check::DeliveredAtLeast(1), Check::Ignored(6)])
        .scenario(
            "two-companies",
            "two players build lines side by side; neither can touch, rename or recolour what the other owns",
            true,
            2,
        )
}

fn airport_support_scenario() -> Scenario {
    Script::default()
        .act(0, construction(AIRFIELD, at(0, 0), "West Airfield"))
        .act(0, construction(AIRPORT, at(5000, 0), "East Airport"))
        .expect_all([Check::Constructions(2), Check::Stations(2), Check::Depots(2)])
        // Each default construction has its hangar as depot 0. The airfield
        // has no depot 1, and another company cannot buy or remove either.
        .act(0, buy_indexed(AIRFIELD, at(0, 0), 1, &[SMALL_AIRCRAFT]))
        .expect(Check::Ignored(1))
        .act(1, buy_indexed(AIRFIELD, at(0, 0), 0, &[SMALL_AIRCRAFT]))
        .act(1, bulldoze_construction(AIRPORT, at(5000, 0)))
        .expect(Check::Ignored(3))
        .act(0, buy_indexed(AIRFIELD, at(0, 0), 0, &[SMALL_AIRCRAFT]))
        .act(0, buy_indexed(AIRPORT, at(5000, 0), 0, &[SMALL_AIRCRAFT]))
        .expect_all([
            Check::Vehicles(2),
            Check::Idle(2),
            Check::Ignored(3),
            Check::MoneyBelow {
                actor: 0,
                money: START_MONEY,
            },
        ])
        .act(0, line("Air shuttle", &[0, 1]))
        .act(0, assign(&[0, 1], Some(0), Some(0)))
        .expect_all([
            Check::Line {
                line: LineId(0),
                stops: 2,
                vehicles: 2,
            },
            Check::Idle(0),
        ])
        .act(0, edit(0, LineChange::Rename(text("Island air"))))
        .expect(Check::LineName {
            line: LineId(0),
            name: "Island air".into(),
        })
        // A route edit naming a station that does not exist is refused and
        // leaves both stops intact.
        .act(0, edit(0, LineChange::Update(line_data(&[1, 9]))))
        .expect(Check::Line {
            line: LineId(0),
            stops: 2,
            vehicles: 2,
        })
        .expect(Check::Ignored(4))
        // An airport still used by a line cannot be removed; neither can a
        // company change another company's line.
        .act(0, bulldoze_construction(AIRPORT, at(5000, 0)))
        .act(1, edit(0, LineChange::Rename(text("Stolen"))))
        .expect_all([
            Check::Ignored(6),
            Check::LineName {
                line: LineId(0),
                name: "Island air".into(),
            },
        ])
        .act(0, edit(0, LineChange::Delete))
        .expect_all([Check::Lines(0), Check::Idle(2)])
        .act(0, sell(&[0, 1]))
        .expect(Check::Vehicles(0))
        .act(0, bulldoze_construction(AIRFIELD, at(0, 0)))
        .act(0, bulldoze_construction(AIRPORT, at(5000, 0)))
        .expect_all([
            Check::Constructions(0),
            Check::Stations(0),
            Check::Depots(0),
            Check::Ignored(6),
        ])
        .scenario(
            "airport-support",
            "build a default airfield and airport, buy aircraft through their hangars, edit and assign an air line, refuse invalid ownership and removal, then clean up",
            true,
            2,
        )
}

fn line_editing_scenario() -> Scenario {
    bus_line(Script::default(), 0, 0, 2, (0, 0, 0))
        .act(0, construction(BUS_STATION, at(750, 400), "Middle"))
        .act(0, set_stops(0, &[0, 2, 1]))
        .expect(Check::Line {
            line: LineId(0),
            stops: 3,
            vehicles: 2,
        })
        .act(0, edit(0, LineChange::Rename(text("Ring"))))
        .expect(Check::LineName {
            line: LineId(0),
            name: "Ring".into(),
        })
        .act(
            0,
            edit(
                0,
                LineChange::Recolor(Tint {
                    r: 0,
                    g: 350_000,
                    b: 780_000,
                }),
            ),
        )
        // A bus and a station renamed from their windows, the bus
        // recoloured; a vehicle that does not exist is refused.
        .act(0, rename(Renamed::Vehicle(VehicleId(0)), "Ring One"))
        .act(0, recolor(0, RED))
        .act(0, rename(Renamed::Station(StationId(0)), "Ring West"))
        .expect_all([
            Check::VehicleName {
                vehicle: VehicleId(0),
                name: "Ring One".into(),
            },
            Check::VehicleColor {
                vehicle: VehicleId(0),
                color: Some(RED),
            },
            Check::StationName {
                station: StationId(0),
                name: "Ring West".into(),
            },
        ])
        .act(0, rename(Renamed::Vehicle(VehicleId(9)), "Nobody"))
        .expect(Check::Ignored(1))
        .act(0, set_stops(0, &[0, 9]))
        .expect(Check::Ignored(2))
        .act(0, set_stops(0, &[0]))
        .expect(Check::Ignored(3))
        .run(500)
        .act(0, edit(0, LineChange::Delete))
        .expect_all([Check::Lines(0), Check::Idle(2)])
        .act(0, sell(&[0, 1]))
        .expect(Check::Vehicles(0))
        .act(0, sell(&[0]))
        .expect(Check::Ignored(4))
        .scenario(
            "line-editing",
            "change a line's stops, name and colour, rename and recolour a bus and a station, refuse bad edits, delete it, sell its vehicles",
            false,
            1,
        )
}

fn demolition_scenario() -> Scenario {
    let (a, b, c) = (at(0, 0), at(1000, 0), at(2000, 0));
    let (stop1, stop2) = (at(500, 0), at(1500, 0));
    Script::default()
        .act(0, road(vec![new(a), new(b), new(c)]))
        .act(0, place_stop(a, b, stop1))
        .expect_all([Check::EdgeObjects(1), Check::Stations(1)])
        .act(0, place_stop(a, b, stop1))
        .expect(Check::Ignored(1))
        // A street with a stop on it stays.
        .act(0, bulldoze_edges(Network::Street, &[(a, b)]))
        .expect_all([Check::Ignored(2), Check::StreetEdges(2)])
        .act(0, place_stop(b, c, stop2))
        .act(0, line("Stops", &[0, 1]))
        // A stop a line uses stays.
        .act(0, bulldoze_stop(b, c, stop2))
        .expect_all([Check::Ignored(3), Check::EdgeObjects(2)])
        .act(0, edit(0, LineChange::Delete))
        .act(0, bulldoze_stop(b, c, stop2))
        .expect_all([Check::EdgeObjects(1), Check::Stations(1)])
        .act(0, bulldoze_edges(Network::Street, &[(c, b)]))
        .expect(Check::StreetEdges(1))
        .act(0, construction(BUS_STATION, at(0, 40), "Gone soon"))
        .expect(Check::Constructions(1))
        .act(0, bulldoze_construction(BUS_STATION, at(1, 41)))
        .expect_all([Check::Constructions(0), Check::Stations(1)])
        .act(0, bulldoze_construction(BUS_STATION, at(0, 40)))
        .expect(Check::Ignored(4))
        .scenario(
            "demolition",
            "place and remove stops, bulldoze streets and stations, refuse what is still in use",
            false,
            1,
        )
}

fn companies_scenario() -> Scenario {
    Script::default()
        .act(0, terraform(100, 100, 4, 8, 8, 2_500))
        .expect(Check::TerrainCells(64))
        .act(
            1,
            company(CompanyOp::Create {
                name: text("Rivals"),
            }),
        )
        .expect_all([
            Check::Companies(3),
            Check::MoneyAtLeast {
                actor: 1,
                money: START_MONEY,
            },
        ])
        .act(
            1,
            company(CompanyOp::Rename {
                company: CompanyId(2),
                name: text("Rivals Ltd"),
            }),
        )
        .act(
            1,
            company(CompanyOp::Rename {
                company: CompanyId(0),
                name: text("Mine now"),
            }),
        )
        .expect(Check::Ignored(1))
        .act(1, company(CompanyOp::Delete(CompanyId(2))))
        .expect_all([
            Check::CompanyOf {
                actor: 1,
                company: None,
            },
            Check::Companies(2),
        ])
        .act(1, road(vec![new(at(0, 0)), new(at(100, 0))]))
        .expect_all([Check::Ignored(2), Check::StreetEdges(0)])
        .act(1, company(CompanyOp::Join(CompanyId(1))))
        .act(0, company(CompanyOp::Delete(CompanyId(0))))
        .expect_all([
            Check::CompanyOf {
                actor: 0,
                company: None,
            },
            Check::Companies(1),
        ])
        .act(0, company(CompanyOp::Join(CompanyId(1))))
        .act(1, company(CompanyOp::Delete(CompanyId(1))))
        .expect_all([Check::Ignored(3), Check::Companies(1)])
        .act(0, road(vec![new(at(0, 0)), new(at(100, 0))]))
        .expect_all([
            Check::StreetEdges(1),
            Check::CompanyOf {
                actor: 0,
                company: Some(CompanyId(1)),
            },
        ])
        .scenario(
            "companies",
            "terraform; found, rename, leave, join and delete companies",
            false,
            2,
        )
        .exact()
}

/// Two players prospect; each prospection ends after its time, with an
/// industry or without, alike on every replica.
fn prospecting_scenario() -> Scenario {
    const COAL: &str = "::/cargos/coal/coal.cargo";
    const GRAIN: &str = "::/cargos/grain/grain.cargo";
    Script::default()
        .act(0, prospect(1, COAL, &["coal_mine", "coal_mine_large"]))
        .act(0, prospect(1, COAL, &["coal_mine"]))
        .act(1, prospect(1, COAL, &["coal_mine"]))
        .act(1, prospect(2, GRAIN, &["farm_grain"]))
        .expect_all([Check::Prospections(3), Check::Ignored(1)])
        .run(PROSPECTION_STEPS + 10)
        .expect_all([
            Check::Prospections(0),
            Check::IndustriesAtMost(3),
            Check::Ignored(1),
        ])
        .scenario(
            "prospecting",
            "prospect near towns; one prospection per company, town and cargo; each ends in an industry or nothing",
            true,
            2,
        )
}

fn refusals_scenario() -> Scenario {
    let (a, b) = (at(0, 0), at(500, 0));
    Script::default()
        .act(0, road(vec![new(a), new(b)]))
        .act(0, road(vec![new(a), new(b)]))
        .expect_all([Check::Ignored(1), Check::StreetEdges(1)])
        .act(0, road(vec![node(at(3000, 3000), Network::Street), new(a)]))
        .act(0, line("Nowhere", &[4, 5]))
        .act(0, assign(&[7], None, Some(0)))
        .act(0, replace(7, &[(BUS, None)]))
        .act(0, construction(BUS_STATION, at(0, 20), "Station"))
        .act(0, buy(BUS_STATION, at(0, 20), &[BUS]))
        .act(
            0,
            track(
                vec![new(at(0, 1000)), new(at(200_000, 1000))],
                &Structure::Ground,
            ),
        )
        .act(0, construction(BUS_DEPOT, at(0, 50), ""))
        .act(0, construction(BUS_STATION, at(1, 20), "Again"))
        .act(0, bulldoze_edges(Network::Track, &[(a, b)]))
        .act(
            0,
            road(vec![split(a, Network::Street, a, b), new(at(0, 300))]),
        )
        .expect_all([
            Check::Ignored(11),
            Check::StreetEdges(1),
            Check::TrackEdges(0),
            Check::Stations(1),
            Check::Vehicles(0),
            Check::Lines(0),
            Check::Depots(0),
        ])
        .scenario(
            "refusals",
            "actions that do not fit the world change nothing, alike on every replica",
            true,
            1,
        )
}

/// A town of `blocks` by `blocks` street blocks, served by bus lines.
fn town_scenario(blocks: i32) -> Scenario {
    const BLOCK: i32 = 200;
    let mut script = Script::default();
    for row in 0..=blocks {
        let vertices = (0..=blocks)
            .map(|col| new(at(col * BLOCK, row * BLOCK)))
            .collect();
        script = script.act(0, road(vertices));
    }
    for col in 0..=blocks {
        let vertices = (0..=blocks)
            .map(|row| node(at(col * BLOCK, row * BLOCK), Network::Street))
            .collect();
        script = script.act(0, road(vertices));
    }
    let edges = usize::try_from(2 * blocks * (blocks + 1)).expect("small town");
    script = script.expect(Check::StreetEdges(edges));
    let stations = blocks;
    for i in 0..stations {
        script = script.act(
            0,
            construction(
                BUS_STATION,
                at(i * BLOCK + 20, (i % 3) * BLOCK + 20),
                "Stop",
            ),
        );
    }
    script = script
        .act(
            0,
            construction(BUS_DEPOT, at(20, blocks * BLOCK - 20), "North depot"),
        )
        .act(
            0,
            construction(BUS_DEPOT, at(blocks * BLOCK - 20, 20), "South depot"),
        );
    let buses = u32::try_from(blocks * 2).expect("small town");
    for bus in 0..buses {
        let depot = if bus % 2 == 0 {
            at(20, blocks * BLOCK - 20)
        } else {
            at(blocks * BLOCK - 20, 20)
        };
        script = script.act(0, buy(BUS_DEPOT, depot, &[BUS]));
    }
    let stations = u32::try_from(stations).expect("small town");
    let lines = stations / 2;
    for l in 0..lines {
        let a = l * 2;
        let route = [a, (a + 1) % stations, (a + 3) % stations];
        script = script.act(0, line("Town", &route));
    }
    for bus in 0..buses {
        script = script.act(0, assign(&[bus], Some(bus % lines), Some(0)));
    }
    let buses = usize::try_from(buses).expect("small town");
    script
        .expect_all([
            Check::Lines(usize::try_from(lines).expect("small town")),
            Check::Vehicles(buses),
            Check::Idle(0),
        ])
        .run(5_000)
        .expect_all([Check::DeliveredAtLeast(100), Check::Ignored(0)])
        .scenario(
            "town",
            "a street grid, many stations, two depots, many buses on many lines, run long",
            false,
            1,
        )
}

/// Four players each build a bus line, their actions interleaved.
fn crowd_scenario() -> Scenario {
    let mut script = Script::default();
    for actor in 0..4 {
        let y = i32::try_from(actor).expect("few actors") * 3000;
        script = script.act(actor, road(vec![new(at(0, y)), new(at(1500, y))]));
    }
    for actor in 0..4 {
        let y = i32::try_from(actor).expect("few actors") * 3000;
        script = script
            .act(actor, construction(BUS_STATION, at(0, y + 20), "West"))
            .act(actor, construction(BUS_STATION, at(1500, y + 20), "East"))
            .act(actor, construction(BUS_DEPOT, at(700, y + 20), "Depot"));
    }
    for actor in 0..4u32 {
        let y = i32::try_from(actor).expect("few actors") * 3000;
        let idx = usize::try_from(actor).expect("few actors");
        script = script
            .act(idx, buy(BUS_DEPOT, at(700, y + 20), &[BUS]))
            .act(idx, line("Bus", &[actor * 2, actor * 2 + 1]))
            .act(idx, assign(&[actor], Some(actor), Some(0)));
    }
    script
        .expect_all([Check::Lines(4), Check::Vehicles(4), Check::Idle(0)])
        .run(2_000)
        .expect_all([
            Check::DeliveredAtLeast(1),
            Check::Ignored(0),
            Check::CompanyOf {
                actor: 3,
                company: Some(CompanyId(3)),
            },
        ])
        .scenario(
            "crowd",
            "four players build at once, their actions interleaved",
            false,
            4,
        )
}

/// The upgrade tools and terraforming: a street with a stop upgraded in
/// place (a tram track, a noise barrier, locked and owned), the stop kept;
/// a track electrified in another template; a road that would move the
/// stop refused; a stroke cut into two bands of rows (the mod's
/// `capture.terraform`).
fn upgrades_scenario() -> Scenario {
    const TRAM_STREET: &str = "street/standard/town_medium_tram.lua";
    const CATENARY_TRACK: &str = "track/high_speed.lua";
    // PERSON and TRAM_TRACK; TRAIN and ELECTRIC_TRAIN.
    const TRAM_LANE: u32 = 1 | 1 << 14;
    const ELECTRIC: u32 = 1 << 7 | 1 << 8;
    let (a, b, stop) = (at(0, 0), at(1000, 0), at(500, 0));
    let (t1, t2) = (at(0, 400), at(1000, 400));
    Script::default()
        .act(0, road(vec![new(a), new(b)]))
        .act(0, place_stop(a, b, stop))
        .expect_all([Check::StreetEdges(1), Check::EdgeObjects(1)])
        .act(0, upgrade(Network::Street, a, b, TRAM_STREET, TRAM_LANE))
        .expect_all([Check::StreetEdges(1), Check::EdgeObjects(1), Check::Ignored(0)])
        .act(1, track(vec![new(t1), new(t2)], &Structure::Ground))
        .act(1, upgrade(Network::Track, t1, t2, CATENARY_TRACK, ELECTRIC))
        .expect_all([Check::TrackEdges(1), Check::Ignored(0)])
        // A road from the stop's street elsewhere, its edge removed and not
        // rebuilt: the stop would stand on nothing.
        .act(
            0,
            Action::BuildRoad(RoadBuild {
                street: text(STREET),
                style: None,
                bus_lane: false,
                tram: Tram::None,
                polyline: Polyline::new(
                    list(vec![node(a, Network::Street), new(at(0, 300))]),
                    polyline(vec![new(a), new(at(0, 300))], &Structure::Ground).links,
                    list(vec![street_edge(a, b)]),
                )
                .expect("two vertices"),
                crossing_type: None,
            }),
        )
        .expect_all([Check::Ignored(1), Check::StreetEdges(1), Check::EdgeObjects(1)])
        .act(1, terraform(2000, 2000, 4, 16, 10, 3_500))
        .act(1, terraform(2000, 2040, 4, 16, 3, 3_500))
        .expect(Check::TerrainCells(16 * 13))
        .scenario(
            "upgrades",
            "upgrade a street with a stop and electrify a track in place, refuse moving the stop, terraform in bands",
            true,
            2,
        )
        .exact()
}

/// Junction settings through the room, with ownership refusal and reset.
fn junctions_scenario() -> Scenario {
    use tpf3mp_proto::action::{
        JunctionChange, JunctionConfig, JunctionEdit, LaneConnection, NodeRef, TrafficPhase,
        TrafficPreference,
    };
    let (a, b, c) = (at(0, 0), at(100, 0), at(100, 100));
    let incoming = EdgeRef {
        network: Network::Street,
        ends: EdgeEnds { a, b },
    };
    let outgoing = EdgeRef {
        network: Network::Street,
        ends: EdgeEnds { a: b, b: c },
    };
    let node = NodeRef {
        network: Network::Street,
        at: b,
    };
    let edit = Action::EditJunctions(JunctionEdit {
        changes: list(vec![JunctionChange {
            node,
            config: Some(JunctionConfig {
                connections: list(vec![LaneConnection {
                    incoming,
                    lane_in: 0,
                    outgoing,
                    lane_out: 1,
                    road: true,
                    tram: true,
                }]),
                crosswalks: list(vec![incoming]),
                preference: TrafficPreference::Yes,
                light: None,
                phases: list(vec![TrafficPhase {
                    locked: list(vec![0, 1]),
                    duration: 12_375,
                    minimum: 4_125,
                    skip: true,
                }]),
                double_slip: false,
                custom_phases: true,
            }),
        }]),
    });
    Script::default()
        .act(0, road(vec![new(a), new(b), new(c)]))
        .act(0, edit.clone())
        .expect_all([Check::Junctions(1), Check::Ignored(0)])
        .run(100)
        .act(1, edit)
        .expect_all([Check::Junctions(1), Check::Ignored(1)])
        .act(
            0,
            Action::EditJunctions(JunctionEdit {
                changes: list(vec![JunctionChange { node, config: None }]),
            }),
        )
        .expect_all([
            Check::Junctions(0),
            Check::StreetEdges(2),
            Check::Ignored(1),
        ])
        .scenario(
            "junctions",
            "turns, crosswalks and timed lights; ownership refusal and reset",
            true,
            2,
        )
}

/// The junction edit a room carries before the junction acceptance: the
/// junction window's double slip switch where two tracks cross, turned on
/// and off again, the preference set to No with it as the window does.
fn double_slip_scenario() -> Scenario {
    use tpf3mp_proto::action::{
        JunctionChange, JunctionConfig, JunctionEdit, NodeRef, TrafficPreference,
    };
    let (west, crossing, east) = (at(0, 0), at(1000, 0), at(2000, 0));
    let (south, north) = (at(1000, -1000), at(1000, 1000));
    let switch = |double_slip| {
        Action::EditJunctions(JunctionEdit {
            changes: list(vec![JunctionChange {
                node: NodeRef {
                    network: Network::Track,
                    at: crossing,
                },
                config: Some(JunctionConfig {
                    connections: list(vec![]),
                    crosswalks: list(vec![]),
                    preference: TrafficPreference::No,
                    light: None,
                    phases: list(vec![]),
                    double_slip,
                    custom_phases: false,
                }),
            }]),
        })
    };
    Script::default()
        .act(
            0,
            track(
                vec![new(west), new(crossing), new(east)],
                &Structure::Ground,
            ),
        )
        .act(
            0,
            track(
                vec![new(south), node(crossing, Network::Track), new(north)],
                &Structure::Ground,
            ),
        )
        .expect(Check::TrackEdges(4))
        .act(0, switch(true))
        .expect_all([Check::Junctions(1), Check::Ignored(0)])
        .act(0, switch(false))
        .expect_all([Check::Junctions(1), Check::Ignored(0)])
        .scenario(
            "double_slip",
            "a double slip switch where two tracks cross, turned on and off again",
            true,
            2,
        )
}

/// Every scenario, the quick ones first.
pub fn scenarios() -> Vec<Arc<Scenario>> {
    let mut all = vec![
        bus_line_scenario(),
        rail_line_scenario(),
        junctions_scenario(),
        double_slip_scenario(),
        two_companies_scenario(),
        airport_support_scenario(),
        refusals_scenario(),
        prospecting_scenario(),
        line_editing_scenario(),
        demolition_scenario(),
        companies_scenario(),
        upgrades_scenario(),
        crowd_scenario(),
        town_scenario(12),
    ];
    all.sort_by_key(|s| !s.smoke);
    all.into_iter().map(Arc::new).collect()
}
