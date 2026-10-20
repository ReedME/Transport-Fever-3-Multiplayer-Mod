//! Every action round-trips through postcard and through an intent's
//! payload, and whatever bytes a peer sends as an action, decoding gives an
//! error or a well-formed action, never a panic. Same method as
//! `hostile_bytes.rs`: valid samples of every variant, corrupted.

#![allow(clippy::unwrap_used)]

use proptest::{collection::vec, prelude::*, sample::Index};
use tpf3mp_proto::{
    BoundedVec, MAX_PAYLOAD, Payload, Text,
    action::{
        ACTION_SCHEMA_VERSION, Action, ActionError, AssetStroke, AssignLine, Bulldoze, BuyVehicle,
        CompanyId, CompanyOp, ConsistPart, ConstructionBuild, ConstructionRef, CreateLine,
        CrossingType, Decoration, EdgeEnds, EdgeKind, EdgeObjectAt, EdgeObjectKind, EdgeRef,
        EditLine, Fraction, LineChange, LineData, LineId, LineStop, Link, Load, LoadMode, LoanOp,
        LoanTerms, MAX_EDGES, MAX_VERTICES, Network, NewSignal, NodeRef, OldSignal, PaintGrid,
        PaintRun, Param, ParamValue, PlaceSignals, PlaceStop, PlacedAsset, Polyline, Pos, Pos2,
        Prospect, ReplaceVehicle, ReplacedPart, Resolve, RoadBuild, SignalEdge, StationId,
        StopRules, Structure, SubsidyOp, SubsidyRef, Tangent, Terminal, Terraform, TerrainCell,
        TerrainPaint, Tint, TownId, TrackBuild, Tram, Transform, UnitDir, VehicleChange, VehicleId,
        VehicleOp, Vertex,
    },
    lua,
};

fn text<const N: usize>(value: &str) -> Text<N> {
    Text::new(value).unwrap()
}

#[test]
fn calendar_speed_refuses_values_outside_the_engine_integer_range() {
    let bad = Action::CalendarSpeed {
        millis_per_day: u32::MAX,
    };
    assert!(matches!(bad.validate(), Err(ActionError::CalendarSpeed)));
    assert!(bad.to_payload().is_err());
    let bytes = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &bad)).unwrap();
    assert!(matches!(
        Action::from_payload(&Payload::new(bytes).unwrap()),
        Err(ActionError::CalendarSpeed)
    ));
    assert!(lua::action_from_lua(&lua::action_to_lua(&bad).unwrap()).is_err());
}

fn list<T, const N: usize>(items: Vec<T>) -> BoundedVec<T, N> {
    BoundedVec::new(items).unwrap()
}

fn pos(x: i32, y: i32, z: i32) -> Pos {
    Pos { x, y, z }
}

fn ends(a: Pos, b: Pos) -> EdgeEnds {
    EdgeEnds { a, b }
}

fn polyline() -> Polyline {
    Polyline::new(
        list(vec![
            Vertex {
                pos: pos(1_204_500, -88_250, 31_400),
                resolve: Resolve::Node(Network::Street),
            },
            Vertex {
                pos: pos(1_264_500, -88_250, 33_100),
                resolve: Resolve::Split(EdgeRef {
                    network: Network::Track,
                    ends: ends(
                        pos(1_260_000, -120_000, 33_000),
                        pos(1_270_000, -40_000, 33_200),
                    ),
                }),
            },
            Vertex {
                pos: pos(1_324_500, -60_000, 40_000),
                resolve: Resolve::New,
            },
        ]),
        list(vec![
            Link {
                precedence: None,
                position0: None,
                position1: None,
                kept: BoundedVec::default(),

                from: 0,
                to: 1,
                tangent0: Tangent {
                    x: 60_000,
                    y: 0,
                    z: 1_700,
                },
                tangent1: Tangent {
                    x: 60_000,
                    y: 0,
                    z: 1_700,
                },
                structure: Structure::Ground,
                kind: None,
                decorations: BoundedVec::default(),
                locked: false,
                owned: false,
                lanes: BoundedVec::default(),
            },
            Link {
                precedence: None,
                position0: None,
                position1: None,
                kept: BoundedVec::default(),

                from: 1,
                to: 2,
                tangent0: Tangent {
                    x: 55_000,
                    y: 20_000,
                    z: 0,
                },
                tangent1: Tangent {
                    x: 60_000,
                    y: 28_250,
                    z: -2_000,
                },
                structure: Structure::Bridge(text("bridge/cement.lua")),
                kind: Some(EdgeKind {
                    network: Network::Street,
                    template: text("street/country.street_template"),
                    style: None,
                }),
                decorations: list(vec![Decoration {
                    name: text("::/infrastructure/edge_addons/barrier_b.edge"),
                    flag: false,
                }]),
                locked: true,
                owned: true,
                lanes: BoundedVec::default(),
            },
        ]),
        list(vec![EdgeRef {
            network: Network::Street,
            ends: ends(pos(0, 0, 0), pos(-5_000, 12_000, 300)),
        }]),
    )
    .unwrap()
    .with_removed_nodes(list(vec![NodeRef {
        network: Network::Street,
        at: pos(-2_500, 6_000, 150),
    }]))
}

fn depot() -> ConstructionRef {
    ConstructionRef {
        file: text("depot/train_depot_era_a.con"),
        at: pos(900_000, 450_000, 12_000),
    }
}

fn line_data() -> LineData {
    let stop = |group, load_mode, rules| LineStop {
        group: StationId(group),
        terminal: Terminal {
            station: 0,
            terminal: 1,
        },
        alternatives: list(vec![Terminal {
            station: 1,
            terminal: 0,
        }]),
        load_mode,
        min_wait: 0,
        max_wait: 180_000_000,
        max_extra_wait: 30_500_000,
        rules,
        waypoints: list(vec![
            tpf3mp_proto::action::Waypoint {
                at: tpf3mp_proto::action::WaypointAt::Lane {
                    of: tpf3mp_proto::action::NetworkOf::Edge(EdgeRef {
                        network: Network::Track,
                        ends: ends(pos(0, 0, 0), pos(80_000, 0, 0)),
                    }),
                    index: 1,
                    param: Fraction(250_000),
                },
                tag: 3,
            },
            tpf3mp_proto::action::Waypoint {
                at: tpf3mp_proto::action::WaypointAt::Lane {
                    of: tpf3mp_proto::action::NetworkOf::Construction(depot()),
                    index: 12,
                    param: Fraction(1_000_000),
                },
                tag: 4,
            },
            tpf3mp_proto::action::Waypoint {
                at: tpf3mp_proto::action::WaypointAt::Open(pos(-1_200_000, 340_000, 0)),
                tag: 5,
            },
        ]),
    };
    LineData {
        stops: list(vec![
            stop(
                4,
                LoadMode::LoadIfAvailable,
                StopRules {
                    load: list(vec![true, false, true]),
                    max_load: list(vec![Fraction(1_000_000), Fraction(0), Fraction(250_000)]),
                    force_unload: false,
                    destroy_for_config_change: true,
                    destroy_for_refresh: false,
                },
            ),
            stop(
                9,
                LoadMode::FullLoadAll,
                StopRules {
                    load: BoundedVec::empty(),
                    max_load: BoundedVec::empty(),
                    force_unload: true,
                    destroy_for_config_change: false,
                    destroy_for_refresh: true,
                },
            ),
        ]),
        modes: list(vec![0, 3]),
        custom_filters: true,
        reservation_priority: 500_000,
    }
}

fn loan(amount: i64) -> LoanTerms {
    LoanTerms {
        kind: text("Small"),
        amount,
        duration: 1_095_000,
        percentage: 30_000,
        birth_day: Some(400_000),
        cooldown_until: None,
        last_pay_day: None,
        times_paid: Some(2),
        id: Some(7),
    }
}

/// One of every variant, and of every inner variant.
fn samples() -> Vec<Action> {
    let color = Tint {
        r: 784_314,
        g: 117_647,
        b: 0,
    };
    vec![
        Action::BuildRoad(RoadBuild {
            street: text("street/standard/town_medium_new.lua"),
            style: None,
            bus_lane: true,
            tram: Tram::Electric,
            polyline: polyline(),
            crossing_type: None,
        }),
        Action::BuildTrack(TrackBuild {
            track: text("high_speed.lua"),
            style: None,
            catenary: true,
            polyline: polyline(),
            crossing_type: Some(text("railroad_crossing/era_c_eu.rcr")),
        }),
        Action::Bulldoze(Bulldoze::Edges {
            network: Network::Track,
            edges: list(vec![ends(pos(1, 2, 3), pos(4, 5, 6))]),
            buildings: list(vec![]),
        }),
        Action::Bulldoze(Bulldoze::Edges {
            network: Network::Street,
            edges: list(vec![ends(pos(1, 2, 3), pos(4, 5, 6))]),
            buildings: list(vec![ConstructionRef {
                file: text("buildings/a/c1/4x4_02/a_com_l1_4x4_02.con"),
                at: pos(2_000, 9_000, 3_000),
            }]),
        }),
        Action::Bulldoze(Bulldoze::Construction(depot())),
        Action::Bulldoze(Bulldoze::EdgeObject {
            edge: EdgeRef {
                network: Network::Street,
                ends: ends(pos(10, 0, 0), pos(90_000, 0, 0)),
            },
            at: pos(45_000, 3_000, 0),
            model: text("station/street/bus_stop.mdl"),
        }),
        Action::Bulldoze(Bulldoze::EdgeObjects {
            edge: EdgeRef {
                network: Network::Street,
                ends: ends(pos(10, 0, 0), pos(90_000, 0, 0)),
            },
            objects: list(vec![
                EdgeObjectAt {
                    at: pos(45_000, 3_000, 0),
                    model: text("stations/street/small_stops/small_new_twosided.con"),
                },
                EdgeObjectAt {
                    at: pos(45_000, -3_000, 0),
                    model: text("stations/street/small_stops/small_new_twosided.con"),
                },
            ]),
        }),
        Action::BuildConstruction(ConstructionBuild {
            file: text("station/rail/modular_station/modular_station.con"),
            transform: Transform {
                basis: [0, 1_000_000, 0, -1_000_000, 0, 0, 0, 0, 1_000_000],
                origin: pos(500_000, 500_000, 20_000),
            },
            params: list(vec![
                Param {
                    key: text("seed"),
                    value: ParamValue::Int(-4_094_223_111),
                },
                Param {
                    key: text("modules[3801].name"),
                    value: ParamValue::Text(text("station/rail/modules/platform_track.module")),
                },
                Param {
                    key: text("modules[3801].variant"),
                    value: ParamValue::Fixed(2_500_000),
                },
                Param {
                    key: text("paramX"),
                    value: ParamValue::Bool(true),
                },
            ]),
            name: text("Hauptbahnhof"),
            replaces: Some(depot()),
            connection: Some(Box::new(polyline())),
        }),
        Action::BuyVehicle(BuyVehicle {
            depot: depot(),
            consist: list(vec![
                ConsistPart {
                    model: text("vehicle/train/br_101.mdl"),
                    reversed: false,
                    loads: BoundedVec::empty(),
                    color,
                },
                ConsistPart {
                    model: text("vehicle/waggon/ic_2nd.mdl"),
                    reversed: true,
                    loads: list(vec![Load {
                        config: 0,
                        cargo: 12,
                    }]),
                    color,
                },
            ]),
            groups: list(vec![1, 1]),
            multiple_units: list(vec![text(""), text("")]),
            depot_index: 1,
        }),
        Action::SellVehicle {
            vehicles: list(vec![VehicleId(1), VehicleId(70_000)]),
        },
        Action::CreateLine(CreateLine {
            name: text("Line 1"),
            color,
            line: line_data(),
        }),
        Action::EditLine(EditLine {
            line: LineId(3),
            change: LineChange::Rename(text("Airport express")),
        }),
        Action::EditLine(EditLine {
            line: LineId(3),
            change: LineChange::Recolor(color),
        }),
        Action::EditLine(EditLine {
            line: LineId(3),
            change: LineChange::Update(line_data()),
        }),
        Action::EditLine(EditLine {
            line: LineId(3),
            change: LineChange::Delete,
        }),
        Action::AssignLine(AssignLine {
            vehicles: list(vec![VehicleId(1)]),
            line: Some(LineId(3)),
            first_stop: Some(1),
        }),
        Action::AssignLine(AssignLine {
            vehicles: list(vec![VehicleId(1), VehicleId(2)]),
            line: None,
            first_stop: None,
        }),
        Action::PlaceStop(PlaceStop {
            edge: EdgeRef {
                network: Network::Street,
                ends: ends(pos(10, 0, 0), pos(90_000, 0, 0)),
            },
            at: pos(45_000, 3_000, 0),
            left: true,
            direction: UnitDir {
                x: 1_000_000,
                y: 0,
                z: 0,
            },
            model: text("stations/street/small_stops/small_new_twosided.con"),
            two_sided: true,
            object: EdgeObjectKind::Stop,
            one_way: false,
            name: Some(text("High Street")),
            params: BoundedVec::empty(),
        }),
        Action::PlaceStop(PlaceStop {
            edge: EdgeRef {
                network: Network::Track,
                ends: ends(pos(10, 0, 0), pos(90_000, 0, 0)),
            },
            at: pos(45_000, 0, 0),
            left: false,
            direction: UnitDir {
                x: 1_000_000,
                y: 0,
                z: 0,
            },
            model: text("infrastructure/signal/signal_path_a.con"),
            two_sided: false,
            object: EdgeObjectKind::Signal,
            one_way: true,
            name: None,
            params: list(vec![
                Param {
                    key: text("auto_signals_distance"),
                    value: ParamValue::Int(4),
                },
                Param {
                    key: text("oneWay"),
                    value: ParamValue::Int(1),
                },
            ]),
        }),
        Action::Terraform(
            Terraform::new(
                Pos2 {
                    x: -8_000,
                    y: 16_000,
                },
                4_000,
                2,
                list(vec![
                    TerrainCell {
                        target: 5_000,
                        before: 4_200,
                    },
                    TerrainCell {
                        target: 5_000,
                        before: 4_900,
                    },
                    TerrainCell {
                        target: 5_000,
                        before: 5_300,
                    },
                    TerrainCell {
                        target: 5_000,
                        before: -100,
                    },
                ]),
            )
            .unwrap(),
        ),
        Action::CompanyOp(CompanyOp::Create {
            name: text("Rail & Sons"),
        }),
        Action::CompanyOp(CompanyOp::Join(CompanyId(2))),
        Action::CompanyOp(CompanyOp::Rename {
            company: CompanyId(2),
            name: text("Rail & Daughters"),
        }),
        Action::CompanyOp(CompanyOp::Delete(CompanyId(2))),
        Action::CompanyOp(CompanyOp::Recolor {
            company: CompanyId(2),
            color: Tint {
                r: 130_000,
                g: 420_000,
                b: 850_000,
            },
        }),
        Action::CompanyOp(CompanyOp::Lock(CompanyId(2))),
        Action::CompanyOp(CompanyOp::Unlock(CompanyId(2))),
        Action::CompanyOp(CompanyOp::Dismiss {
            company: CompanyId(2),
            player: text(&"ab".repeat(32)),
        }),
        Action::CompanyOp(CompanyOp::ShareStations {
            company: CompanyId(2),
            open: false,
        }),
        Action::CompanyOp(CompanyOp::StationAccess {
            company: CompanyId(2),
            other: CompanyId(3),
            open: Some(true),
        }),
        Action::CompanyOp(CompanyOp::StationAccess {
            company: CompanyId(2),
            other: CompanyId(0),
            open: None,
        }),
        Action::Loan(Box::new(LoanOp::Take {
            next: loan(7_000_000),
            offer: loan(5_000_000),
        })),
        Action::Loan(Box::new(LoanOp::Repay {
            loan: loan(5_000_000),
        })),
        Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(1),
            change: VehicleChange::Stop(true),
        }),
        Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(2),
            change: VehicleChange::ToDepot { sell: false },
        }),
        Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(2),
            change: VehicleChange::Reverse,
        }),
        Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(2),
            change: VehicleChange::Depart,
        }),
        Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(2),
            change: VehicleChange::ManualDeparture(true),
        }),
        Action::VehicleOp(VehicleOp {
            vehicle: VehicleId(2),
            change: VehicleChange::Recolor(Tint {
                r: 1_000_000,
                g: 500_000,
                b: 0,
            }),
        }),
        Action::ReplaceVehicle(replacement()),
        Action::Prospect(Prospect {
            town: TownId(4),
            cargo: text("::/cargos/coal/coal.cargo"),
            industries: list(vec![text("coal_mine"), text("coal_mine_large")]),
            permit: Some(text(
                "game_mechanics/company/explorations/exploration_coal.res",
            )),
        }),
        Action::NotificationSeen { notification: 12 },
        Action::ApplyRank { level: 6 },
        junction_action(),
        Action::Subsidy(SubsidyOp::Accept(SubsidyRef {
            uid: 1_234_560_000,
            kind: text("::/game_mechanics/subventions/deliver_cargo/deliver_cargo.res"),
        })),
        Action::Subsidy(SubsidyOp::Decline(SubsidyRef {
            uid: 7,
            kind: text("::/game_mechanics/subventions/deliver_passengers/deliver_passengers.res"),
        })),
        Action::Rename {
            what: tpf3mp_proto::action::Renamed::Vehicle(VehicleId(2)),
            name: text("Blue Arrow"),
        },
        Action::Rename {
            what: tpf3mp_proto::action::Renamed::Station(StationId(5)),
            name: text("Central"),
        },
        Action::Rename {
            what: tpf3mp_proto::action::Renamed::Town(TownId(1)),
            name: text("Newtown"),
        },
        Action::Rename {
            what: tpf3mp_proto::action::Renamed::Construction(depot()),
            name: text("North depot"),
        },
        Action::Perk(tpf3mp_proto::action::PerkOp::Greenify {
            industry: tpf3mp_proto::action::IndustryId(3),
            permit: Some(text("ECO_INDUSTRY")),
        }),
        Action::Perk(tpf3mp_proto::action::PerkOp::Marketing {
            town: TownId(2),
            duration_ms: 1_095_000,
            line_cost_factor: Fraction(500_000),
            permit: Some(text("::/game_mechanics/company/permitKeys/marketing.res")),
            cost: 4_000_000,
        }),
        Action::Preserve(tpf3mp_proto::action::Preservation {
            building: depot(),
            index: 0,
            preserved: true,
        }),
        Action::CalendarSpeed { millis_per_day: 0 },
        Action::CalendarSpeed {
            millis_per_day: 2000,
        },
        Action::PlaceSignals(PlaceSignals {
            model: text("infrastructure/signal/signal_path_c.con"),
            one_way: false,
            params: BoundedVec::empty(),
            edges: list(vec![
                SignalEdge {
                    edge: ends(pos(10, 0, 0), pos(90_000, 0, 0)),
                    add: list(vec![NewSignal {
                        at: Fraction(250_000),
                        left: true,
                    }]),
                    remove: list(vec![OldSignal {
                        at: Fraction(750_000),
                        model: text("infrastructure/signal/signal_path_a.con"),
                    }]),
                },
                SignalEdge {
                    edge: ends(pos(90_000, 0, 0), pos(180_000, 0, 0)),
                    add: list(vec![
                        NewSignal {
                            at: Fraction(0),
                            left: false,
                        },
                        NewSignal {
                            at: Fraction(1_000_000),
                            left: false,
                        },
                    ]),
                    remove: BoundedVec::empty(),
                },
            ]),
        }),
        Action::Paint(TerrainPaint {
            materials: PaintGrid::new(-1, 2, 2, 1, list(vec![PaintRun { value: 3, count: 2 }]))
                .unwrap(),
            mask: None,
        }),
        Action::PlantAssets(AssetStroke {
            models: list(vec![text("tree.con")]),
            removed: BoundedVec::empty(),
            groups: list(vec![list(vec![PlacedAsset {
                model: 0,
                transform: Transform {
                    basis: [1, 0, 0, 0, 1, 0, 0, 0, 1],
                    origin: pos(1, 0, 0),
                },
            }])]),
        }),
        Action::SetCrossingType(CrossingType {
            nodes: list(vec![
                pos(-4_000, 120_500, 3_250),
                pos(4_000, 120_500, 3_250),
            ]),
            crossing_type: text("infrastructure/railroad_crossing/era_c_eu/era_c_eu.rcr"),
        }),
    ]
}

fn junction_action() -> Action {
    use tpf3mp_proto::action::{
        JunctionChange, JunctionConfig, JunctionEdit, LaneConnection, TrafficPhase,
        TrafficPreference,
    };
    let edge = EdgeRef {
        network: Network::Street,
        ends: ends(pos(0, 0, 0), pos(100_000, 0, 0)),
    };
    Action::EditJunctions(JunctionEdit {
        changes: list(vec![JunctionChange {
            node: NodeRef {
                network: Network::Street,
                at: pos(0, 0, 0),
            },
            config: Some(JunctionConfig {
                connections: list(vec![LaneConnection {
                    incoming: edge,
                    lane_in: 0,
                    outgoing: edge,
                    lane_out: 1,
                    road: true,
                    tram: false,
                }]),
                crosswalks: list(vec![edge]),
                preference: TrafficPreference::Yes,
                light: Some(text("::/light/standard.lua")),
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
    })
}

#[test]
fn invalid_junction_relationships_are_refused_on_the_wire_and_in_lua() {
    use tpf3mp_proto::action::{JunctionChange, JunctionEdit, MAX_LANES};
    let Action::EditJunctions(base) = junction_action() else {
        unreachable!()
    };
    let mut cases = vec![
        Action::EditJunctions(JunctionEdit {
            changes: list(vec![]),
        }),
        Action::EditJunctions(JunctionEdit {
            changes: list(vec![base.changes[0].clone(), base.changes[0].clone()]),
        }),
    ];
    for variant in 0..4 {
        let mut config = base.changes[0].config.clone().unwrap();
        if variant == 0 {
            let mut turn = config.connections[0].clone();
            turn.lane_in = MAX_LANES as u16;
            config.connections = list(vec![turn]);
        } else {
            let mut phase = config.phases[0].clone();
            match variant {
                1 => phase.minimum = phase.duration + 1,
                2 => phase.locked = list(vec![2]),
                _ => phase.locked = list(vec![0, 0]),
            }
            config.phases = list(vec![phase]);
        }
        cases.push(Action::EditJunctions(JunctionEdit {
            changes: list(vec![JunctionChange {
                node: base.changes[0].node,
                config: Some(config),
            }]),
        }));
    }
    for bad in cases {
        assert!(bad.to_payload().is_err());
        let wire = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &bad)).unwrap();
        assert!(Action::from_payload(&Payload::new(wire).unwrap()).is_err());
        assert!(lua::action_from_lua(&lua::action_to_lua(&bad).unwrap()).is_err());
    }
}

/// A vehicle sent to be sold on arrival at its depot crashes build 40408
/// there, in every game at once (2026-10-02, `road_vehicles` scenario): no
/// game sends it, reads it from the room or takes it from the mod.
#[test]
fn a_vehicle_sold_on_arrival_at_its_depot_is_refused_on_the_wire_and_in_lua() {
    let bad = Action::VehicleOp(VehicleOp {
        vehicle: VehicleId(2),
        change: VehicleChange::ToDepot { sell: true },
    });
    assert!(matches!(bad.validate(), Err(ActionError::SellOnArrival)));
    assert!(bad.to_payload().is_err());
    let wire = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &bad)).unwrap();
    assert!(matches!(
        Action::from_payload(&Payload::new(wire).unwrap()),
        Err(ActionError::SellOnArrival)
    ));
    assert!(lua::action_from_lua(&lua::action_to_lua(&bad).unwrap()).is_err());
    // Sent to the depot and kept, it travels.
    let good = Action::VehicleOp(VehicleOp {
        vehicle: VehicleId(2),
        change: VehicleChange::ToDepot { sell: false },
    });
    let payload = good.to_payload().unwrap();
    assert_eq!(Action::from_payload(&payload).unwrap(), good);
}

#[test]
fn a_two_sided_stop_bulldoze_requires_two_refs_for_one_construction() {
    let action = |objects: Vec<EdgeObjectAt>| {
        Action::Bulldoze(Bulldoze::EdgeObjects {
            edge: EdgeRef {
                network: Network::Street,
                ends: ends(pos(0, 0, 0), pos(10_000, 0, 0)),
            },
            objects: list(objects),
        })
    };
    let object = |x, model| EdgeObjectAt {
        at: pos(x, 0, 0),
        model: text(model),
    };
    let malformed = [
        action(vec![]),
        action(vec![object(2_000, "stations/street/twosided.con")]),
        action(vec![
            object(2_000, "stations/street/twosided.con"),
            object(2_000, "stations/street/twosided.con"),
        ]),
        action(vec![
            object(2_000, "stations/street/twosided.con"),
            object(2_000, "stations/street/other.con"),
        ]),
    ];
    for bad in malformed {
        assert!(bad.validate().is_err());
        assert!(bad.to_payload().is_err());
        let wire = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &bad)).unwrap();
        assert!(Action::from_payload(&Payload::new(wire).unwrap()).is_err());
        assert!(lua::action_from_lua(&lua::action_to_lua(&bad).unwrap()).is_err());
    }

    let good = action(vec![
        object(2_000, "stations/street/twosided.con"),
        object(8_000, "stations/street/twosided.con"),
    ]);
    assert!(good.validate().is_ok());
    assert_eq!(
        Action::from_payload(&good.to_payload().unwrap()).unwrap(),
        good
    );
}

/// A train's replacement: its locomotive kept, turned, and a new coach
/// behind it.
fn replacement() -> ReplaceVehicle {
    let color = Tint {
        r: 1_000_000,
        g: 250_000,
        b: 0,
    };
    ReplaceVehicle {
        vehicle: VehicleId(9),
        consist: list(vec![
            ReplacedPart {
                part: ConsistPart {
                    model: text("vehicle/train/br_101.mdl"),
                    reversed: true,
                    loads: BoundedVec::empty(),
                    color,
                },
                kept: Some(0),
            },
            ReplacedPart {
                part: ConsistPart {
                    model: text("vehicle/waggon/ic_2nd.mdl"),
                    reversed: false,
                    loads: list(vec![Load {
                        config: 1,
                        cargo: 0,
                    }]),
                    color,
                },
                kept: None,
            },
        ]),
        groups: list(vec![1, 1]),
        multiple_units: list(vec![text(""), text("")]),
    }
}

/// Decodes `bytes` as an action payload: an error, or an action that
/// survives a round trip.
fn check(bytes: &[u8]) {
    let Ok(payload) = Payload::new(bytes.to_vec()) else {
        return;
    };
    if let Ok(action) = Action::from_payload(&payload) {
        let again = action.to_payload().unwrap();
        assert_eq!(Action::from_payload(&again).unwrap(), action);
    }
}

#[test]
fn every_variant_round_trips() {
    let samples = samples();
    // Every top-level variant is sampled: postcard tags them 0..=27.
    let mut tags: Vec<u8> = samples
        .iter()
        .map(|action| postcard::to_stdvec(action).unwrap()[0])
        .collect();
    tags.dedup();
    assert_eq!(tags, (0..=27).collect::<Vec<u8>>());

    for action in samples {
        let bytes = postcard::to_stdvec(&action).unwrap();
        assert_eq!(postcard::from_bytes::<Action>(&bytes).unwrap(), action);

        let payload = action.to_payload().unwrap();
        assert_eq!(payload.as_bytes()[0], ACTION_SCHEMA_VERSION as u8);
        assert_eq!(Action::from_payload(&payload).unwrap(), action);
        // An intent's payload is how an action travels; it must survive the
        // payload's own encoding too.
        let wire = postcard::to_stdvec(&payload).unwrap();
        let back: Payload = postcard::from_bytes(&wire).unwrap();
        assert_eq!(Action::from_payload(&back).unwrap(), action);
    }
}

/// The kind the logs name an action by is its variant's name, as serde (and
/// so a scenario file) writes it.
#[test]
fn every_variant_s_kind_is_its_name() {
    for action in samples() {
        let json = serde_json::to_value(&action).unwrap();
        let name = match &json {
            serde_json::Value::Object(map) => map.keys().next().unwrap().clone(),
            serde_json::Value::String(name) => name.clone(),
            other => panic!("{other}"),
        };
        assert_eq!(action.kind(), name);
    }
}

#[test]
fn every_variant_round_trips_through_the_mod_s_tables() {
    for action in samples() {
        let table = lua::action_to_lua(&action).unwrap();
        assert_eq!(lua::action_from_lua(&table).unwrap(), action, "{table:?}");
    }
}

#[test]
fn the_mod_s_tables_are_in_the_game_s_units() {
    let stop = samples()
        .into_iter()
        .find(|action| matches!(action, Action::PlaceStop(_)))
        .unwrap();
    let table = lua::action_to_lua(&stop).unwrap();
    let stop = table.get("PlaceStop").unwrap();
    // The sample's stop stands at x = 45 000 mm, facing +x in millionths.
    assert_eq!(
        stop.get("at").unwrap().get("x"),
        Some(&lua::LuaValue::Number(45.0))
    );
    assert_eq!(
        stop.get("direction").unwrap().get("x"),
        Some(&lua::LuaValue::Number(1.0))
    );
}

/// Signals spaced along a track (Auto Signals) are places along each edge,
/// fractions as the game's `EdgeObject.param`, and a build that adds
/// nothing, names an edge twice (either way round) or a place off its edge
/// is refused on the wire and from the mod.
#[test]
fn signals_on_existing_tracks_are_fractions_and_each_edge_once() {
    let signals = samples()
        .into_iter()
        .find_map(|action| match action {
            Action::PlaceSignals(signals) => Some(signals),
            _ => None,
        })
        .unwrap();
    let table = lua::action_to_lua(&Action::PlaceSignals(signals.clone())).unwrap();
    let lua::LuaValue::Table(edges) = table.get("PlaceSignals").unwrap().get("edges").unwrap()
    else {
        panic!("edges are a sequence");
    };
    let lua::LuaValue::Table(add) = edges[0].1.get("add").unwrap() else {
        panic!("add is a sequence");
    };
    assert_eq!(
        add[0].1.get("at"),
        Some(&lua::LuaValue::Number(0.25)),
        "a place along the edge is the game's param"
    );

    let refused = |signals: PlaceSignals, why: &str| {
        let action = Action::PlaceSignals(signals);
        match action.validate() {
            Err(ActionError::Signals(said)) => assert_eq!(said, why),
            other => panic!("{why}: {other:?}"),
        }
        let wire = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &action)).unwrap();
        assert!(Action::from_payload(&Payload::new(wire).unwrap()).is_err());
        assert!(lua::action_from_lua(&lua::action_to_lua(&action).unwrap()).is_err());
    };
    let mut twice = signals.clone();
    let reversed = SignalEdge {
        edge: ends(twice.edges[0].edge.b, twice.edges[0].edge.a),
        ..twice.edges[1].clone()
    };
    twice.edges = list(vec![twice.edges[0].clone(), reversed]);
    refused(twice, "an edge named twice");
    let mut off = signals.clone();
    off.edges = list(vec![SignalEdge {
        add: list(vec![NewSignal {
            at: Fraction(1_000_001),
            left: true,
        }]),
        ..off.edges[1].clone()
    }]);
    refused(off, "a place off its edge");
    let mut none = signals;
    none.edges = list(vec![SignalEdge {
        add: BoundedVec::empty(),
        remove: BoundedVec::empty(),
        ..none.edges[0].clone()
    }]);
    refused(none, "no signal added or removed");
}

/// A replacement as the mod writes it: the vehicle by canonical id, each
/// part as a purchase's under `part`, a kept part by its index and a new
/// one with `kept` left out, colours as fractions.
#[test]
fn a_replacement_is_the_vehicle_s_id_and_its_parts_as_the_mod_writes_them() {
    let table = lua::action_to_lua(&Action::ReplaceVehicle(replacement())).unwrap();
    let replace = table.get("ReplaceVehicle").unwrap();
    assert_eq!(replace.get("vehicle"), Some(&lua::LuaValue::Number(9.0)));
    let lua::LuaValue::Table(consist) = replace.get("consist").unwrap() else {
        panic!("a consist is a sequence");
    };
    let first = &consist[0].1;
    assert_eq!(first.get("kept"), Some(&lua::LuaValue::Number(0.0)));
    let part = first.get("part").unwrap();
    assert_eq!(part.get("reversed"), Some(&lua::LuaValue::Boolean(true)));
    assert_eq!(
        part.get("color").unwrap().get("g"),
        Some(&lua::LuaValue::Number(0.25))
    );
    assert_eq!(consist[1].1.get("kept"), None, "a new part keeps nothing");
}

#[test]
fn oversized_lists_are_refused() {
    // A polyline claiming one vertex past the limit, then nothing: refused
    // on the length, before any vertex is read.
    let mut bytes = vec![ACTION_SCHEMA_VERSION as u8, 1];
    bytes.extend(postcard::to_stdvec(&Text::<8>::new("t").unwrap()).unwrap());
    bytes.push(0);
    bytes.extend(postcard::to_stdvec(&u32::try_from(MAX_VERTICES + 1).unwrap()).unwrap());
    assert!(Action::from_payload(&Payload::new(bytes).unwrap()).is_err());

    // One edge past the bulldoze limit, every edge well-formed.
    let edges = vec![ends(pos(0, 0, 0), pos(1, 1, 1)); MAX_EDGES + 1];
    let mut bytes = vec![ACTION_SCHEMA_VERSION as u8, 2, 0, 1];
    bytes.extend(postcard::to_stdvec(&edges).unwrap());
    assert!(Action::from_payload(&Payload::new(bytes).unwrap()).is_err());
    assert!(BoundedVec::<EdgeEnds, MAX_EDGES>::new(edges).is_err());
}

#[test]
fn oversized_and_hostile_text_is_refused() {
    let mut bytes = vec![ACTION_SCHEMA_VERSION as u8, 11, 0];
    bytes.extend(postcard::to_stdvec(&"x".repeat(65)).unwrap());
    assert!(Action::from_payload(&Payload::new(bytes).unwrap()).is_err());

    let mut bytes = vec![ACTION_SCHEMA_VERSION as u8, 11, 0];
    bytes.extend(postcard::to_stdvec(&"a\u{1b}[2J").unwrap());
    assert!(Action::from_payload(&Payload::new(bytes).unwrap()).is_err());
}

#[test]
fn a_link_to_a_missing_vertex_is_refused() {
    // A track build laid out field by field as the schema encodes it, but
    // without the polyline's checks: its second link points past the last
    // vertex.
    #[derive(serde::Serialize)]
    struct UncheckedTrack {
        track: String,
        style: Option<String>,
        catenary: bool,
        vertices: Vec<Vertex>,
        links: Vec<Link>,
        removals: Vec<EdgeRef>,
        removed_nodes: Vec<NodeRef>,
        junctions: Vec<tpf3mp_proto::action::JunctionChange>,
        crossing_type: Option<String>,
    }
    let good = polyline();
    let mut vertices = good.vertices.to_vec();
    vertices.pop();
    assert!(
        Polyline::new(
            list(vertices.clone()),
            good.links.clone(),
            BoundedVec::empty()
        )
        .is_err()
    );
    let track = UncheckedTrack {
        track: "high_speed.lua".into(),
        style: None,
        catenary: false,
        vertices,
        links: good.links.to_vec(),
        removals: Vec::new(),
        removed_nodes: Vec::new(),
        junctions: Vec::new(),
        crossing_type: None,
    };
    // The schema version, then Action::BuildTrack.
    let bytes = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, 1u32, &track)).unwrap();
    assert!(Action::from_payload(&Payload::new(bytes).unwrap()).is_err());

    // The same bytes with every vertex present decode.
    let mut track = track;
    track.vertices = good.vertices.to_vec();
    let bytes = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, 1u32, &track)).unwrap();
    assert!(Action::from_payload(&Payload::new(bytes).unwrap()).is_ok());
}

#[test]
fn the_payload_limit_holds() {
    let bytes = vec![ACTION_SCHEMA_VERSION as u8; MAX_PAYLOAD + 1];
    assert!(Payload::new(bytes).is_err());
    // The largest terraform the schema allows is refused by the payload, not
    // truncated.
    let cells = vec![
        TerrainCell {
            target: i32::MAX,
            before: i32::MIN,
        };
        8192
    ];
    let action =
        Action::Terraform(Terraform::new(Pos2 { x: 0, y: 0 }, 4_000, 64, list(cells)).unwrap());
    assert!(action.to_payload().is_err());
}

/// The mod cuts a stroke into bands of at most 4,096 cells
/// (`capture.TERRAIN_CELLS`): the largest band, at the worst heights, fits
/// one payload and round-trips, through bytes and the mod's tables.
#[test]
fn the_largest_terraform_band_the_mod_sends_fits_one_payload() {
    let cells = vec![
        TerrainCell {
            target: i32::MIN,
            before: i32::MAX,
        };
        4096
    ];
    let action = Action::Terraform(
        Terraform::new(
            Pos2 {
                x: -2_048_000,
                y: 2_048_000,
            },
            4_000,
            4096,
            list(cells),
        )
        .unwrap(),
    );
    let payload = action.to_payload().unwrap();
    assert!(payload.as_bytes().len() <= MAX_PAYLOAD);
    assert_eq!(Action::from_payload(&payload).unwrap(), action);
    let table = lua::action_to_lua(&action).unwrap();
    assert_eq!(lua::action_from_lua(&table).unwrap(), action);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    #[test]
    fn corrupted_actions_decode_or_fail_cleanly(
        pick in any::<Index>(),
        edits in vec((any::<Index>(), any::<u8>(), 0u8..4), 1..8),
    ) {
        let samples = samples();
        let sample = &samples[pick.index(samples.len())];
        let mut bytes = sample.to_payload().unwrap().as_bytes().to_vec();
        for (at, value, kind) in edits {
            match kind {
                0 | 1 if !bytes.is_empty() => {
                    let index = at.index(bytes.len());
                    bytes[index] = value;
                }
                2 => bytes.insert(at.index(bytes.len() + 1), value),
                3 if !bytes.is_empty() => bytes.truncate(at.index(bytes.len())),
                _ => {}
            }
        }
        check(&bytes);
    }

    #[test]
    fn arbitrary_bytes_decode_or_fail_cleanly(bytes in vec(any::<u8>(), 0..300)) {
        check(&bytes);
        let mut versioned = vec![ACTION_SCHEMA_VERSION as u8];
        versioned.extend(bytes);
        check(&versioned);
    }
}

/// A level crossing travels as the places of all its nodes, sorted and each
/// once, and its type by name; a crossing with no node, nodes out of order or
/// named twice, or a type with no name is refused on the wire and from the mod.
#[test]
fn a_crossing_type_names_every_node_once_in_order() {
    let crossing = samples()
        .into_iter()
        .find_map(|action| match action {
            Action::SetCrossingType(crossing) => Some(crossing),
            _ => None,
        })
        .unwrap();
    let table = lua::action_to_lua(&Action::SetCrossingType(crossing.clone())).unwrap();
    let lua::LuaValue::Table(nodes) = table.get("SetCrossingType").unwrap().get("nodes").unwrap()
    else {
        panic!("nodes are a sequence");
    };
    assert_eq!(
        nodes[0].1.get("x"),
        Some(&lua::LuaValue::Number(-4.0)),
        "a node's place is in metres in the mod"
    );

    let refused = |crossing: CrossingType, why: &str| {
        let action = Action::SetCrossingType(crossing);
        match action.validate() {
            Err(ActionError::Crossing(said)) => assert_eq!(said, why),
            other => panic!("{why}: {other:?}"),
        }
        let wire = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &action)).unwrap();
        assert!(Action::from_payload(&Payload::new(wire).unwrap()).is_err());
        assert!(lua::action_from_lua(&lua::action_to_lua(&action).unwrap()).is_err());
    };
    let mut none = crossing.clone();
    none.nodes = BoundedVec::empty();
    refused(none, "a crossing with no node");
    let mut reversed = crossing.clone();
    reversed.nodes = list(vec![crossing.nodes[1], crossing.nodes[0]]);
    refused(reversed, "nodes not sorted, or one named twice");
    let mut twice = crossing.clone();
    twice.nodes = list(vec![crossing.nodes[0], crossing.nodes[0]]);
    refused(twice, "nodes not sorted, or one named twice");
    let mut nameless = crossing;
    nameless.crossing_type = text("");
    refused(nameless, "a type with no name");
}

/// A road or track build carries the toolbar's crossing type by its file,
/// through the wire and the mod's tables alike; one with no name is refused.
#[test]
fn a_build_carries_its_toolbars_crossing_type_by_name() {
    let mut builds: Vec<Action> = samples()
        .into_iter()
        .filter(|a| matches!(a, Action::BuildRoad(_) | Action::BuildTrack(_)))
        .collect();
    assert_eq!(builds.len(), 2);
    if let Action::BuildRoad(road) = &mut builds[0] {
        road.crossing_type = Some(text("railroad_crossing/era_a_eu.rcr"));
    }
    for action in &builds {
        let payload = action.to_payload().unwrap();
        assert_eq!(&Action::from_payload(&payload).unwrap(), action);
        let table = lua::action_to_lua(action).unwrap();
        assert_eq!(&lua::action_from_lua(&table).unwrap(), action);
    }
    for mut action in builds {
        match &mut action {
            Action::BuildRoad(road) => road.crossing_type = Some(text("")),
            Action::BuildTrack(track) => track.crossing_type = Some(text("")),
            _ => unreachable!(),
        }
        match action.validate() {
            Err(ActionError::Crossing(said)) => assert_eq!(said, "a type with no name"),
            other => panic!("a nameless crossing type: {other:?}"),
        }
        let wire = postcard::to_stdvec(&(ACTION_SCHEMA_VERSION, &action)).unwrap();
        assert!(Action::from_payload(&Payload::new(wire).unwrap()).is_err());
    }
}
