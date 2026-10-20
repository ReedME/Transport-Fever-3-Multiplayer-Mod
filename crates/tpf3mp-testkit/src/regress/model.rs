//! A model of the game that plays the portable action schema
//! ([`tpf3mp_proto::action`]): streets and tracks, constructions, stations,
//! depots, vehicles, lines and companies, with a small deterministic
//! simulation of vehicles carrying passengers along their lines. It stands
//! in for Transport Fever 3 in the regression harness until the hook applies
//! actions to the real game, and it is held to the same rules the hook is:
//!
//! - an action is applied whole or not at all. One that does not fit this
//!   world (an edge that is not there, a line of someone else's, too little
//!   money) changes nothing and is counted as ignored, the same on every
//!   replica, since every replica applies the same events to the same world;
//! - references resolve by position within the tolerances `docs/BUILDING.md`
//!   gives, and canonical ids count up in the order the room applied what
//!   created them.
//!
//! Its state is compared through checkpoint lanes, and
//! [`ModelWorld::with_drift`] perturbs its simulation the way a platform's
//! float difference would, so a test can show the harness catches it.

use std::collections::{BTreeMap, BTreeSet};

use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};
use tpf3mp_proto::{
    Event, EventBody, FixedBytes, LaneDigest, PlayerId, Seal,
    action::{
        Action, Bulldoze, CompanyId, CompanyOp, ConstructionBuild, ConstructionRef, EdgeEnds,
        JunctionChange, JunctionConfig, LineChange, LineId, LoanOp, Network, Polyline, Pos,
        Prospect, Renamed, ReplaceVehicle, ResName, Resolve, StationId, Structure, Terraform, Tint,
        VehicleChange, VehicleId,
    },
};

use crate::rng::SplitMix64;

pub const START_MONEY: i64 = 10_000_000;
pub const ROAD_COST_PER_M: i64 = 20;
pub const TRACK_COST_PER_M: i64 = 100;
pub const STATION_COST: i64 = 40_000;
pub const DEPOT_COST: i64 = 20_000;
pub const CONSTRUCTION_COST: i64 = 10_000;
/// Per car of a consist.
pub const VEHICLE_COST: i64 = 30_000;
pub const TERRAFORM_COST_PER_CELL: i64 = 5;
/// Earned per passenger delivered.
pub const FARE: i64 = 20;
/// Each vehicle costs this every [`UPKEEP_EVERY`] steps.
pub const UPKEEP: i64 = 20;
pub const UPKEEP_EVERY: u64 = 100;
/// Passengers a car carries.
pub const CAR_CAPACITY: u32 = 40;
/// Most passengers waiting at one station.
pub const MAX_WAITING: u32 = 1_000;

/// Steps a prospection takes before its outcome, the model's stand-in for
/// TF3's six months. Far more than a scenario's acts take to be ordered
/// (some 100 steps each at the regression runs' pace, more on a loaded
/// machine), so the prospecting scenario's prospections are all still under
/// way when its last one is.
pub const PROSPECTION_STEPS: u64 = 2_000;
/// In hundredths: how often a prospection finds an industry.
pub const PROSPECTION_CHANCE: u64 = 60;

/// Company progression, the model's stand-in for the mod's rule
/// (`mod/tpf3mp_1/content/scripts/tpf3mp/progression.lua`, D23 proposed):
/// each station is a town of [`TOWN_POPULATION`] people, a company's share
/// of a town is its share of the passengers delivered there, its rating in
/// every town is [`TOWN_RATING`] (the model keeps no ratings), and its score
/// is the sum over the towns of population x share x rating / 100. With one
/// company its score is the world's population, as TF3's own. Recomputed
/// every [`PROGRESSION_EVERY`] steps; experience never falls.
pub const TOWN_POPULATION: u64 = 1_000;
pub const TOWN_RATING: u64 = 100;
pub const PROGRESSION_EVERY: u64 = 100;
/// Experience a rank needs above the one before, the model's stand-in for
/// TF3's thresholds.
pub const RANK_EXPERIENCE: u64 = 1_000;
/// TF3's highest rank (`company_progression_util.getMaxRank`).
pub const MAX_RANK: u8 = 15;

/// Tolerances of `docs/BUILDING.md`, in millimetres.
const NODE_TOLERANCE: i64 = 1_500;
/// How far above or below its node a vertex naming it may be.
const HEIGHT_TOLERANCE: i64 = 500;
const EDGE_TOLERANCE: i64 = 1_000;
const CONSTRUCTION_TOLERANCE: i64 = 2_000;

/// Lanes a replica reports at checkpoints.
pub mod lane {
    /// Streets, tracks and terrain.
    pub const NETWORK: u16 = 0;
    /// Constructions, stations and stops.
    pub const CONSTRUCTIONS: u16 = 1;
    pub const LINES: u16 = 2;
    /// Vehicles where they are, and the passengers waiting.
    pub const VEHICLES: u16 = 3;
    /// Companies, money and the simulation's generator.
    pub const ECONOMY: u16 = 4;
}

type P = [i32; 3];
/// An edge: its network and its ends, the lesser first, so the key does not
/// depend on which end a game calls `node0`.
type EdgeKey = (u8, P, P);

fn p(pos: &Pos) -> P {
    [pos.x, pos.y, pos.z]
}

fn net(network: Network) -> u8 {
    match network {
        Network::Street => 0,
        Network::Track => 1,
    }
}

fn edge_key(network: u8, a: P, b: P) -> EdgeKey {
    if a <= b {
        (network, a, b)
    } else {
        (network, b, a)
    }
}

fn dist2(a: P, b: P) -> i128 {
    (0..3)
        .map(|i| {
            let d = i128::from(a[i]) - i128::from(b[i]);
            d * d
        })
        .sum()
}

/// A player as the mod names one: 64 lowercase hex digits.
fn hex(player: &PlayerId) -> String {
    player
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn within(a: P, b: P, tolerance: i64) -> bool {
    dist2(a, b) <= i128::from(tolerance) * i128::from(tolerance)
}

fn within_horizontally(a: P, b: P, tolerance: i64) -> bool {
    within([a[0], a[1], 0], [b[0], b[1], 0], tolerance)
}

/// Whole metres between two points, at least one.
fn metres(a: P, b: P) -> i64 {
    let mm = dist2(a, b).unsigned_abs().isqrt();
    i64::try_from(mm / 1000).unwrap_or(i64::MAX).max(1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Company {
    name: String,
    money: i64,
    /// TF3's company progression: the highest score it reached, the rank
    /// that reaches and the rank it took.
    progress: Progress,
    /// Who founded it: its head while they play for it.
    founder: Option<PlayerId>,
    /// Its players, in the order they joined: the first is its head once
    /// the founder has gone (DECISIONS.md, D22, proposed).
    members: Vec<PlayerId>,
    /// The seal of its password (scope and tag), if it has one.
    lock: Option<(u64, [u8; 32])>,
    /// Whether other companies' lines may stop at its stations: the
    /// default, for every company without a choice of its own.
    open: bool,
    /// Its head's choice for single companies: whether their lines may
    /// stop at its stations, whatever the default says.
    access: BTreeMap<u32, bool>,
}

impl Company {
    /// Whether company `other`'s lines may stop at this company's stations
    /// (D22, proposed).
    fn lets(&self, other: u32) -> bool {
        self.access.get(&other).copied().unwrap_or(self.open)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Progress {
    experience: u64,
    potential: u8,
    level: u8,
}

impl Progress {
    /// As TF3's company growth script (`company_growth.script.tl`): a
    /// company begins at rank 1, and its experience never falls.
    const fn new() -> Self {
        Self {
            experience: 0,
            potential: 1,
            level: 1,
        }
    }
}

fn rank_for(experience: u64) -> u8 {
    u8::try_from(experience / RANK_EXPERIENCE)
        .unwrap_or(MAX_RANK)
        .min(MAX_RANK)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Edge {
    kind: String,
    structure: String,
    /// Nobody's, as a town's streets are, or a company's.
    owner: Option<u32>,
}

/// A stop, signal or waypoint on an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EdgeObject {
    model: String,
    left: bool,
    owner: u32,
    /// The station a stop is.
    station: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Kind {
    Station(u32),
    Depot,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Construction {
    kind: Kind,
    owner: u32,
    name: String,
    basis: [i32; 9],
    params: usize,
    /// Depot indexes the construction exposes. Stock airfields and airports
    /// expose their nested default hangar as index 0.
    depot_count: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Station {
    at: P,
    waiting: u32,
    /// As its window names it: the construction's name, until renamed.
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Line {
    owner: u32,
    name: String,
    color: [i32; 3],
    stops: Vec<(u32, Option<u16>)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Vehicle {
    owner: u32,
    consist: Vec<String>,
    line: Option<u32>,
    next_stop: u16,
    progress: u32,
    load: u32,
    /// As its window names it: the game's default, until renamed.
    name: String,
    /// The tint its window gave it, if any.
    color: Option<[i32; 3]>,
}

/// A prospection under way: TF3's company script keeps these
/// (`pendingProspections`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Prospection {
    company: u32,
    town: u32,
    cargo: String,
    industries: Vec<String>,
    began: u64,
}

/// An industry a prospection found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Industry {
    kind: String,
    town: u32,
    at: P,
}

/// Everything a save holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    /// In the order they joined: the harness's actors by index.
    players: Vec<PlayerId>,
    member_of: BTreeMap<PlayerId, u32>,
    companies: BTreeMap<u32, Company>,
    edges: BTreeMap<EdgeKey, Edge>,
    junctions: BTreeMap<(u8, P), JunctionConfig>,
    objects: BTreeMap<(EdgeKey, P), EdgeObject>,
    constructions: BTreeMap<(String, P), Construction>,
    stations: BTreeMap<u32, Station>,
    lines: BTreeMap<u32, Line>,
    vehicles: BTreeMap<u32, Vehicle>,
    terrain: BTreeMap<(i32, i32), i32>,
    /// In the order they began.
    prospections: Vec<Prospection>,
    industries: BTreeMap<u32, Industry>,
    next_industry: u32,
    /// The last step simulated.
    now: u64,
    next_company: u32,
    next_station: u32,
    next_line: u32,
    next_vehicle: u32,
    delivered: u64,
    /// Passengers each company delivered at each station, by (company,
    /// station): its deliveries for its share of the station's town.
    served: BTreeMap<(u32, u32), u64>,
    /// Actions that changed nothing.
    ignored: u64,
    rng: u64,
}

/// What an action that changes nothing says about why.
type Refusal = String;

macro_rules! refuse {
    ($($arg:tt)*) => {
        return Err(format!($($arg)*))
    };
}

impl State {
    fn join(&mut self, player: PlayerId) {
        if self.member_of.contains_key(&player) {
            return;
        }
        self.players.push(player);
        let company = self.found(format!("Company {}", self.next_company + 1), player);
        self.set_member(player, company);
    }

    fn found(&mut self, name: String, founder: PlayerId) -> u32 {
        let id = self.next_company;
        self.next_company += 1;
        self.companies.insert(
            id,
            Company {
                name,
                money: START_MONEY,
                progress: Progress::new(),
                founder: Some(founder),
                members: Vec::new(),
                lock: None,
                open: true,
                access: BTreeMap::new(),
            },
        );
        id
    }

    /// `player` plays for `company` from now on, last in its join order.
    fn set_member(&mut self, player: PlayerId, company: u32) {
        if let Some(old) = self.member_of.insert(player, company)
            && let Some(entry) = self.companies.get_mut(&old)
        {
            entry.members.retain(|p| *p != player);
        }
        if let Some(entry) = self.companies.get_mut(&company) {
            entry.members.push(player);
        }
    }

    /// A company's head: its founder while they play for it, else its
    /// longest-standing player.
    fn head(&self, company: u32) -> Option<PlayerId> {
        let entry = self.companies.get(&company)?;
        entry
            .founder
            .filter(|founder| entry.members.contains(founder))
            .or_else(|| entry.members.first().copied())
    }

    /// Who owns station `id`: the company of the stop or construction that
    /// is it.
    fn station_owner(&self, id: u32) -> Option<u32> {
        self.objects
            .values()
            .find(|o| o.station == Some(id))
            .map(|o| o.owner)
            .or_else(|| {
                self.constructions
                    .values()
                    .find(|c| c.kind == Kind::Station(id))
                    .map(|c| c.owner)
            })
    }

    fn charge(&mut self, company: u32, amount: i64) -> Result<(), Refusal> {
        let Some(entry) = self.companies.get_mut(&company) else {
            refuse!("company-{company} is gone");
        };
        if entry.money < amount {
            refuse!("company-{company} has {} and needs {amount}", entry.money);
        }
        entry.money -= amount;
        Ok(())
    }

    fn act(
        &mut self,
        player: &PlayerId,
        action: &Action,
        seal: Option<&Seal>,
    ) -> Result<(), Refusal> {
        if let Action::CompanyOp(op) = action {
            return self.company_op(player, op, seal);
        }
        let Some(&company) = self.member_of.get(player) else {
            refuse!("the player has no company");
        };
        match action {
            Action::BuildRoad(road) => self.build(
                Network::Street,
                road.street.as_str(),
                &road.polyline,
                company,
                ROAD_COST_PER_M,
            ),
            Action::BuildTrack(track) => self.build(
                Network::Track,
                track.track.as_str(),
                &track.polyline,
                company,
                TRACK_COST_PER_M,
            ),
            Action::Bulldoze(bulldoze) => self.bulldoze(bulldoze, company),
            Action::BuildConstruction(build) => self.construct(build, company),
            Action::BuyVehicle(buy) => {
                let key = self.find_construction(&buy.depot)?;
                let depot = &self.constructions[&key];
                if depot.depot_count == 0 {
                    refuse!("{} is not a depot", buy.depot.file);
                }
                if depot.owner != company {
                    refuse!("the depot is company-{}'s", depot.owner);
                }
                if buy.depot_index >= depot.depot_count {
                    refuse!(
                        "{} has {} depot(s), and no depot index {}",
                        buy.depot.file,
                        depot.depot_count,
                        buy.depot_index
                    );
                }
                if buy.consist.is_empty() {
                    refuse!("a vehicle of no cars");
                }
                let cars = i64::try_from(buy.consist.len()).unwrap_or(i64::MAX);
                self.charge(company, VEHICLE_COST.saturating_mul(cars))?;
                let id = self.next_vehicle;
                self.next_vehicle += 1;
                self.vehicles.insert(
                    id,
                    Vehicle {
                        owner: company,
                        consist: buy
                            .consist
                            .iter()
                            .map(|part| part.model.as_str().to_owned())
                            .collect(),
                        line: None,
                        next_stop: 0,
                        progress: 0,
                        load: 0,
                        name: format!("Vehicle {id}"),
                        color: None,
                    },
                );
                Ok(())
            }
            Action::SellVehicle { vehicles } => {
                let ids = self.own_vehicles(vehicles.iter().map(|v| v.0), company)?;
                for id in ids {
                    let vehicle = self.vehicles.remove(&id).expect("checked above");
                    let cars = i64::try_from(vehicle.consist.len()).unwrap_or(i64::MAX);
                    let refund = VEHICLE_COST.saturating_mul(cars) / 2;
                    if let Some(owner) = self.companies.get_mut(&company) {
                        owner.money += refund;
                    }
                }
                Ok(())
            }
            Action::CreateLine(create) => {
                let stops = self.stops(create.line.stops.iter(), company)?;
                let id = self.next_line;
                self.next_line += 1;
                self.lines.insert(
                    id,
                    Line {
                        owner: company,
                        name: create.name.as_str().to_owned(),
                        color: [create.color.r, create.color.g, create.color.b],
                        stops,
                    },
                );
                Ok(())
            }
            Action::EditLine(edit) => {
                let line = self.own_line(edit.line, company)?;
                match &edit.change {
                    LineChange::Rename(name) => {
                        self.lines.get_mut(&line).expect("checked").name = name.as_str().to_owned();
                    }
                    LineChange::Recolor(color) => {
                        self.lines.get_mut(&line).expect("checked").color =
                            [color.r, color.g, color.b];
                    }
                    LineChange::Update(line_data) => {
                        let stops = self.stops(line_data.stops.iter(), company)?;
                        let count = stops.len();
                        self.lines.get_mut(&line).expect("checked").stops = stops;
                        for vehicle in self.vehicles.values_mut() {
                            if vehicle.line == Some(line) && usize::from(vehicle.next_stop) >= count
                            {
                                vehicle.next_stop = 0;
                                vehicle.progress = 0;
                            }
                        }
                    }
                    LineChange::Delete => {
                        self.lines.remove(&line);
                        for vehicle in self.vehicles.values_mut() {
                            if vehicle.line == Some(line) {
                                vehicle.line = None;
                            }
                        }
                    }
                }
                Ok(())
            }
            Action::AssignLine(assign) => {
                let ids = self.own_vehicles(assign.vehicles.iter().map(|v| v.0), company)?;
                if ids.is_empty() {
                    refuse!("an assignment of no vehicles");
                }
                if let (Some(line), Some(first)) = (assign.line, assign.first_stop) {
                    let line = self.own_line(line, company)?;
                    if usize::from(first) >= self.lines[&line].stops.len() {
                        refuse!("line-{line} has no stop {first}");
                    }
                } else if let Some(line) = assign.line {
                    self.own_line(line, company)?;
                }
                for id in ids {
                    let vehicle = self.vehicles.get_mut(&id).expect("checked above");
                    vehicle.line = assign.line.map(|line| line.0);
                    // The game's choice: the model's vehicles all reach
                    // every stop, so the first.
                    vehicle.next_stop = assign.first_stop.unwrap_or(0);
                    vehicle.progress = 0;
                }
                Ok(())
            }
            // The model keeps no vehicle state beyond its line and its
            // window's colour: a vehicle sent to its depot leaves its line.
            // One sold on arrival never gets here: `Action::validate`
            // refuses it (the game crashes).
            Action::VehicleOp(op) => {
                let ids = self.own_vehicles(std::iter::once(op.vehicle.0), company)?;
                match op.change {
                    VehicleChange::ToDepot { .. } => {
                        for id in ids {
                            self.vehicles.get_mut(&id).expect("checked above").line = None;
                        }
                    }
                    VehicleChange::Recolor(tint) => {
                        for id in ids {
                            self.vehicles.get_mut(&id).expect("checked above").color =
                                Some([tint.r, tint.g, tint.b]);
                        }
                    }
                    _ => {}
                }
                Ok(())
            }
            Action::ReplaceVehicle(replace) => self.replace(replace, company),
            Action::PlaceStop(stop) => {
                let key = self.find_edge(net(stop.edge.network), &stop.edge.ends)?;
                let at = p(&stop.at);
                if self
                    .objects
                    .keys()
                    .any(|(edge, there)| *edge == key && within(*there, at, EDGE_TOLERANCE))
                {
                    refuse!("something already stands there on the edge");
                }
                let model = stop.model.as_str().to_owned();
                let station = model.contains("stop").then(|| {
                    let id = self.next_station;
                    self.next_station += 1;
                    self.stations.insert(
                        id,
                        Station {
                            at,
                            waiting: 0,
                            name: format!("Stop {id}"),
                        },
                    );
                    id
                });
                self.charge(company, CONSTRUCTION_COST)?;
                self.objects.insert(
                    (key, at),
                    EdgeObject {
                        model,
                        left: stop.left,
                        owner: company,
                        station,
                    },
                );
                Ok(())
            }
            Action::Terraform(terraform) => self.terraform(terraform, company),
            // A loan pays its amount in, and paying it back takes it out;
            // the model keeps no interest.
            Action::Loan(op) => match op.as_ref() {
                LoanOp::Take { offer, .. } => {
                    if offer.amount <= 0 {
                        refuse!("a loan of {}", offer.amount);
                    }
                    if let Some(entry) = self.companies.get_mut(&company) {
                        entry.money = entry.money.saturating_add(offer.amount);
                    }
                    Ok(())
                }
                LoanOp::Repay { loan } => self.charge(company, loan.amount.max(0)),
            },
            Action::Prospect(prospect) => self.prospect(prospect, company),
            // A notification's sound played: nothing the model keeps.
            Action::NotificationSeen { .. } => Ok(()),
            Action::ApplyRank { level } => self.apply_rank(company, *level),
            Action::EditJunctions(edit) => self.edit_junctions(&edit.changes, company),
            // The game's subsidy script decides offers and their money; the
            // model has no subsidies.
            Action::Subsidy(_) => Ok(()),
            Action::Rename { what, name } => self.rename(what, name.as_str(), company),
            // A company perk: the model keeps no permits, towns' reputations
            // or emissions.
            Action::Perk(_) => Ok(()),
            // A town building's preservation: the model keeps no towns'
            // buildings.
            Action::Preserve(_) => Ok(()),
            // The model has no native calendar.
            Action::CalendarSpeed { .. } => Ok(()),
            // Signals a mod spaces along a track: the model keeps no
            // signals' places along their edges.
            Action::PlaceSignals(_) => Ok(()),
            // Terrain paint: the model keeps no ground textures.
            Action::Paint(_) => Ok(()),
            // Trees and rocks: the model keeps no assets.
            Action::PlantAssets(_) => Ok(()),
            Action::SetCrossingType(_) => Ok(()),
            Action::CompanyOp(_) => unreachable!("handled above"),
        }
    }

    /// As TF3's company growth script's `applyLevel`: a rank above the one
    /// taken, and reached.
    fn apply_rank(&mut self, company: u32, level: u8) -> Result<(), Refusal> {
        let Some(entry) = self.companies.get_mut(&company) else {
            refuse!("company-{company} is gone");
        };
        let progress = &mut entry.progress;
        if level <= progress.level || level > progress.potential {
            refuse!(
                "company-{company} has rank {} of {} reached, not {level}",
                progress.level,
                progress.potential
            );
        }
        progress.level = level;
        Ok(())
    }

    /// The companies' scores, as the mod's rule (see [`TOWN_POPULATION`]):
    /// with one company the world's population, as TF3's own; with more,
    /// each town's population split by the passengers each delivered
    /// there, times its rating there over 100.
    fn progress(&mut self) {
        let towns: Vec<u32> = self.stations.keys().copied().collect();
        let world = TOWN_POPULATION.saturating_mul(u64::try_from(towns.len()).unwrap_or(0));
        let single = self.companies.len() <= 1;
        let mut scores: BTreeMap<u32, u64> = BTreeMap::new();
        if !single {
            for town in towns {
                let here = || self.served.iter().filter(move |((_, at), _)| *at == town);
                let all: u64 = here().map(|(_, n)| *n).sum();
                if all == 0 {
                    continue;
                }
                for ((company, _), n) in here() {
                    let part =
                        u128::from(TOWN_POPULATION) * u128::from(*n) * u128::from(TOWN_RATING)
                            / (u128::from(all) * 100);
                    let entry = scores.entry(*company).or_default();
                    *entry = entry.saturating_add(u64::try_from(part).unwrap_or(u64::MAX));
                }
            }
        }
        for (id, company) in &mut self.companies {
            let score = if single {
                world
            } else {
                scores.get(id).copied().unwrap_or(0)
            };
            let progress = &mut company.progress;
            progress.experience = progress.experience.max(score);
            progress.potential = progress.potential.max(rank_for(progress.experience));
        }
    }

    /// As TF3's company script: one prospection per company, town and cargo
    /// at a time; it costs a permit, which the model does not keep.
    fn prospect(&mut self, prospect: &Prospect, company: u32) -> Result<(), Refusal> {
        let (town, cargo) = (prospect.town.0, prospect.cargo.as_str());
        if self
            .prospections
            .iter()
            .any(|p| p.company == company && p.town == town && p.cargo == cargo)
        {
            refuse!("company-{company} prospects for {cargo} near town-{town} already");
        }
        self.prospections.push(Prospection {
            company,
            town,
            cargo: cargo.to_owned(),
            industries: prospect
                .industries
                .iter()
                .map(|kind| kind.as_str().to_owned())
                .collect(),
            began: self.now,
        });
        Ok(())
    }

    /// The prospections whose time is up: each finds an industry of one of
    /// its types near its town, or nothing, by the simulation's generator.
    fn prospections_end(&mut self, rng: &mut SplitMix64) {
        let now = self.now;
        let (due, going): (Vec<Prospection>, Vec<Prospection>) =
            std::mem::take(&mut self.prospections)
                .into_iter()
                .partition(|p| now.saturating_sub(p.began) >= PROSPECTION_STEPS);
        self.prospections = going;
        for prospection in due {
            let found = rng.below(100) < PROSPECTION_CHANCE;
            let count = u64::try_from(prospection.industries.len()).unwrap_or(0);
            if !found || count == 0 {
                continue;
            }
            let pick = usize::try_from(rng.below(count)).unwrap_or(0);
            let offset = |r: &mut SplitMix64| i32::try_from(r.below(2_000)).unwrap_or(0) * 1_000;
            let base = i32::try_from(prospection.town).unwrap_or(0) * 100_000;
            let at = [base + offset(rng), base + offset(rng), 0];
            let id = self.next_industry;
            self.next_industry += 1;
            self.industries.insert(
                id,
                Industry {
                    kind: prospection.industries[pick].clone(),
                    town: prospection.town,
                    at,
                },
            );
        }
    }

    /// The company rules of D21 and D22 (proposed), as the mod keeps them
    /// (`tpf3mp/companies.lua`): a password's seal to join a locked company,
    /// and its head alone to lock, unlock, dismiss or share.
    fn company_op(
        &mut self,
        player: &PlayerId,
        op: &CompanyOp,
        seal: Option<&Seal>,
    ) -> Result<(), Refusal> {
        let current = self.member_of.get(player).copied();
        let head_of = |state: &Self, company: u32| -> Result<(), Refusal> {
            if !state.companies.contains_key(&company) {
                refuse!("no company-{company}");
            }
            if state.head(company) != Some(*player) {
                refuse!("only the head of company-{company} does that");
            }
            Ok(())
        };
        match op {
            CompanyOp::Create { name } => {
                let company = self.found(name.as_str().to_owned(), *player);
                self.set_member(*player, company);
            }
            CompanyOp::Join(CompanyId(company)) => {
                let Some(entry) = self.companies.get(company) else {
                    refuse!("no company-{company}");
                };
                if current != Some(*company)
                    && let Some(lock) = entry.lock
                    && seal.map(|s| (s.scope, s.tag.0)) != Some(lock)
                {
                    refuse!("the password for company-{company} is not right");
                }
                if current != Some(*company) {
                    self.set_member(*player, *company);
                }
            }
            CompanyOp::Rename {
                company: CompanyId(company),
                name,
            } => {
                if current != Some(*company) {
                    refuse!("the player is not in company-{company}");
                }
                self.companies.get_mut(company).expect("a member's").name =
                    name.as_str().to_owned();
            }
            CompanyOp::Recolor {
                company: CompanyId(company),
                ..
            } => {
                if current != Some(*company) {
                    refuse!("the player is not in company-{company}");
                }
            }
            CompanyOp::Delete(CompanyId(company)) => {
                if current != Some(*company) {
                    refuse!("the player is not in company-{company}");
                }
                if self.member_of.values().filter(|c| *c == company).count() > 1 {
                    refuse!("company-{company} has other members");
                }
                let owns = self.edges.values().any(|e| e.owner == Some(*company))
                    || self.objects.values().any(|o| o.owner == *company)
                    || self.constructions.values().any(|c| c.owner == *company)
                    || self.lines.values().any(|l| l.owner == *company)
                    || self.vehicles.values().any(|v| v.owner == *company);
                if owns {
                    refuse!("company-{company} still owns something");
                }
                self.companies.remove(company);
                self.member_of.remove(player);
            }
            CompanyOp::Lock(CompanyId(company)) => {
                head_of(self, *company)?;
                let Some(seal) = seal.filter(|s| s.scope == u64::from(*company)) else {
                    refuse!("a password for company-{company} comes sealed by the room");
                };
                self.companies.get_mut(company).expect("checked").lock =
                    Some((seal.scope, seal.tag.0));
            }
            CompanyOp::Unlock(CompanyId(company)) => {
                head_of(self, *company)?;
                self.companies.get_mut(company).expect("checked").lock = None;
            }
            CompanyOp::Dismiss {
                company: CompanyId(company),
                player: dismissed,
            } => {
                head_of(self, *company)?;
                let Some(&other) = self.companies[company]
                    .members
                    .iter()
                    .find(|p| hex(p) == dismissed.as_str())
                else {
                    refuse!("that player does not play for company-{company}");
                };
                if other == *player {
                    refuse!("the head leaves by joining another company");
                }
                // The model has no shared first company: they found their
                // own, as on joining the room.
                let own = self.found(format!("Company {}", self.next_company + 1), other);
                self.set_member(other, own);
            }
            CompanyOp::ShareStations {
                company: CompanyId(company),
                open,
            } => {
                head_of(self, *company)?;
                self.companies.get_mut(company).expect("checked").open = *open;
            }
            CompanyOp::StationAccess {
                company: CompanyId(company),
                other: CompanyId(other),
                open,
            } => {
                head_of(self, *company)?;
                if other == company {
                    refuse!("company-{company}'s stations are always its own");
                }
                if !self.companies.contains_key(other) {
                    refuse!("no company-{other}");
                }
                let access = &mut self.companies.get_mut(company).expect("checked").access;
                match open {
                    Some(open) => access.insert(*other, *open),
                    None => access.remove(other),
                };
            }
        }
        Ok(())
    }

    fn edit_junctions(&mut self, changes: &[JunctionChange], company: u32) -> Result<(), Refusal> {
        for change in changes {
            let network = net(change.node.network);
            let at = p(&change.node.at);
            let incident: Vec<_> = self
                .edges
                .iter()
                .filter(|((n, a, b), _)| *n == network && (*a == at || *b == at))
                .collect();
            if incident.is_empty() {
                refuse!("the junction no longer exists");
            }
            if incident
                .iter()
                .any(|(_, e)| e.owner.is_some_and(|owner| owner != company))
            {
                refuse!("the junction touches another company's edge");
            }
            if let Some(config) = &change.config {
                for reference in config
                    .connections
                    .iter()
                    .flat_map(|c| [&c.incoming, &c.outgoing])
                    .chain(config.crosswalks.iter())
                {
                    let key = edge_key(
                        net(reference.network),
                        p(&reference.ends.a),
                        p(&reference.ends.b),
                    );
                    if !self.edges.contains_key(&key) || (key.1 != at && key.2 != at) {
                        refuse!("a lane or crosswalk outside its junction");
                    }
                }
                self.junctions.insert((network, at), config.clone());
            } else {
                self.junctions.remove(&(network, at));
            }
        }
        Ok(())
    }

    fn build(
        &mut self,
        network: Network,
        kind: &str,
        polyline: &Polyline,
        company: u32,
        cost_per_m: i64,
    ) -> Result<(), Refusal> {
        let n = net(network);
        // Existing nodes are found before anything is removed: within one
        // build a node outlives its edges, as in the game's proposal.
        let mut at: Vec<Option<P>> = Vec::with_capacity(polyline.vertices.len());
        for vertex in polyline.vertices.iter() {
            let pos = p(&vertex.pos);
            at.push(match &vertex.resolve {
                Resolve::New => Some(pos),
                Resolve::Node(of) => match self.find_node(net(*of), pos) {
                    Some(node) => Some(node),
                    None => refuse!("no {of:?} node at {pos:?}"),
                },
                Resolve::Split(_) => None,
            });
        }
        // The nodes it removes, found before anything is.
        let mut gone = Vec::with_capacity(polyline.removed_nodes.len());
        for node in polyline.removed_nodes.iter() {
            let pos = p(&node.at);
            let Some(found) = self.find_node(net(node.network), pos) else {
                refuse!("no {:?} node to remove at {pos:?}", node.network);
            };
            let joined = polyline
                .vertices
                .iter()
                .zip(&at)
                .any(|(v, r)| matches!(v.resolve, Resolve::Node(_)) && *r == Some(found));
            if joined {
                refuse!("the build removes a node it joins");
            }
            gone.push((net(node.network), found));
        }
        // An edge with something on it goes only where a link rebuilds it in
        // place, between the same ends: a road or track modifier's upgrade,
        // whose new edge keeps what stood on the old (docs/BUILDING.md).
        let mut carried = Vec::new();
        for removal in polyline.removals.iter() {
            let key = self.find_edge(net(removal.network), &removal.ends)?;
            if self.unobstructed(&key).is_err() {
                carried.push(key);
            }
            self.edges.remove(&key);
        }
        // A node goes with its last edge; the game refuses to remove one
        // that still has any.
        for (network, node) in gone {
            let kept = self
                .edges
                .keys()
                .any(|(n, a, b)| *n == network && (*a == node || *b == node));
            if kept {
                refuse!("the node at {node:?} still has edges");
            }
        }
        for (vertex, slot) in polyline.vertices.iter().zip(at.iter_mut()) {
            if let Resolve::Split(edge) = &vertex.resolve {
                let pos = p(&vertex.pos);
                let key = self.find_edge(net(edge.network), &edge.ends)?;
                self.unobstructed(&key)?;
                if pos == key.1 || pos == key.2 {
                    refuse!("a split at the end of an edge");
                }
                let split = self.edges.remove(&key).expect("found above");
                self.edges
                    .insert(edge_key(key.0, key.1, pos), split.clone());
                self.edges.insert(edge_key(key.0, pos, key.2), split);
                *slot = Some(pos);
            }
        }
        let at: Vec<P> = at
            .into_iter()
            .map(|pos| pos.expect("every vertex resolved above"))
            .collect();
        let mut cost: i64 = 0;
        for link in polyline.links.iter() {
            let (a, b) = (at[usize::from(link.from)], at[usize::from(link.to)]);
            if a == b {
                refuse!("an edge from a point to itself");
            }
            // The build's own kind, or the kind the link keeps: a piece of
            // the street it joins, rebuilt through the new junction.
            let (ln, own) = match &link.kind {
                Some(other) => (net(other.network), other.template.as_str()),
                None => (n, kind),
            };
            let key = edge_key(ln, a, b);
            if self.edges.contains_key(&key) {
                refuse!("the edge {a:?}-{b:?} is already built");
            }
            let structure = match &link.structure {
                Structure::Ground => "ground".to_owned(),
                Structure::Bridge(kind) => format!("bridge:{kind}"),
                Structure::Tunnel(kind) => format!("tunnel:{kind}"),
            };
            self.edges.insert(
                key,
                Edge {
                    kind: own.to_owned(),
                    structure,
                    owner: Some(company),
                },
            );
            cost = cost.saturating_add(metres(a, b).saturating_mul(cost_per_m));
        }
        for key in carried {
            if !self.edges.contains_key(&key) {
                refuse!("something stands on the edge");
            }
        }
        self.edit_junctions(&polyline.junctions, company)?;
        self.charge(company, cost)
    }

    fn bulldoze(&mut self, bulldoze: &Bulldoze, company: u32) -> Result<(), Refusal> {
        match bulldoze {
            Bulldoze::Edges {
                network,
                edges,
                buildings,
            } => {
                if edges.is_empty() {
                    refuse!("a bulldoze of nothing");
                }
                // The model's world has no towns: no town building stands by
                // its streets, so a bulldoze that names one names nothing.
                if !buildings.is_empty() {
                    refuse!("no town building there");
                }
                for ends in edges.iter() {
                    let key = self.find_edge(net(*network), ends)?;
                    let owner = self.edges[&key].owner;
                    if owner.is_some_and(|owner| owner != company) {
                        refuse!("the edge is company-{}'s", owner.unwrap_or_default());
                    }
                    self.unobstructed(&key)?;
                    self.edges.remove(&key);
                }
            }
            Bulldoze::Assets(_) => refuse!("no asset group there"),
            Bulldoze::Construction(reference) => {
                let key = self.find_construction(reference)?;
                let construction = &self.constructions[&key];
                if construction.owner != company {
                    refuse!("the construction is company-{}'s", construction.owner);
                }
                if let Kind::Station(station) = construction.kind {
                    self.unserved(station)?;
                    self.stations.remove(&station);
                }
                self.constructions.remove(&key);
            }
            Bulldoze::EdgeObject { edge, at, model } => {
                let key = self.find_edge(net(edge.network), &edge.ends)?;
                let object = self.edge_object(key, at, model, &[], company)?;
                self.remove_objects(&[object])?;
            }
            Bulldoze::EdgeObjects { edge, objects } => {
                let key = self.find_edge(net(edge.network), &edge.ends)?;
                let mut found = Vec::new();
                for object in objects.iter() {
                    found.push(self.edge_object(
                        key,
                        &object.at,
                        &object.model,
                        &found,
                        company,
                    )?);
                }
                self.remove_objects(&found)?;
            }
        }
        Ok(())
    }

    fn construct(&mut self, build: &ConstructionBuild, company: u32) -> Result<(), Refusal> {
        if build.name.as_str().is_empty() {
            refuse!("an unnamed construction");
        }
        let file = build.file.as_str().to_owned();
        let origin = p(&build.transform.origin);
        let mut kind = if file.contains("depot") {
            Kind::Depot
        } else if file.contains("station") {
            Kind::Station(u32::MAX)
        } else {
            Kind::Other
        };
        if let Some(replaced) = &build.replaces {
            let key = self.find_construction(replaced)?;
            let old = self.constructions.remove(&key).expect("found above");
            if old.owner != company {
                refuse!("the construction is company-{}'s", old.owner);
            }
            // A module edit keeps the station, and the lines stopping there.
            match (old.kind, kind) {
                (Kind::Station(id), Kind::Station(_)) => {
                    kind = Kind::Station(id);
                    if let Some(station) = self.stations.get_mut(&id) {
                        station.at = origin;
                    }
                }
                (Kind::Station(id), _) => {
                    self.unserved(id)?;
                    self.stations.remove(&id);
                }
                _ => {}
            }
        }
        if self
            .constructions
            .keys()
            .any(|(other, at)| *other == file && within(*at, origin, CONSTRUCTION_TOLERANCE))
        {
            refuse!("a {file} already stands there");
        }
        if kind == Kind::Station(u32::MAX) {
            let id = self.next_station;
            self.next_station += 1;
            self.stations.insert(
                id,
                Station {
                    at: origin,
                    waiting: 0,
                    name: build.name.as_str().to_owned(),
                },
            );
            kind = Kind::Station(id);
        }
        let cost = match kind {
            Kind::Station(_) => STATION_COST,
            Kind::Depot => DEPOT_COST,
            Kind::Other => CONSTRUCTION_COST,
        };
        self.charge(company, cost)?;
        let depot_count = if kind == Kind::Depot
            || file.ends_with("/air/airfield.con")
            || file.ends_with("/air/airport.con")
        {
            1
        } else {
            0
        };
        self.constructions.insert(
            (file, origin),
            Construction {
                kind,
                owner: company,
                name: build.name.as_str().to_owned(),
                basis: build.transform.basis,
                params: build.params.len(),
                depot_count,
            },
        );
        // The streets the tool built with it, in the same proposal: every
        // link names its kind, a construction having none of its own.
        if let Some(connection) = &build.connection {
            let mut kinds = connection.links.iter().map(|link| link.kind.as_ref());
            let Some(Some(first)) = kinds.next() else {
                refuse!("a connection link of no kind");
            };
            if kinds.any(|kind| kind.is_none()) {
                refuse!("a connection link of no kind");
            }
            let cost = match first.network {
                Network::Street => ROAD_COST_PER_M,
                Network::Track => TRACK_COST_PER_M,
            };
            let template = first.template.as_str().to_owned();
            self.build(first.network, &template, connection, company, cost)?;
        }
        Ok(())
    }

    fn terraform(&mut self, terraform: &Terraform, company: u32) -> Result<(), Refusal> {
        let columns = i64::from(terraform.columns);
        let cell = i64::from(terraform.cell);
        for (index, cell_height) in terraform.cells.iter().enumerate() {
            let index = i64::try_from(index).unwrap_or(i64::MAX);
            let x = i64::from(terraform.origin.x) + (index % columns) * cell;
            let y = i64::from(terraform.origin.y) + (index / columns) * cell;
            let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                refuse!("a terrain cell off the map");
            };
            self.terrain.insert((x, y), cell_height.target);
        }
        let cells = i64::try_from(terraform.cells.len()).unwrap_or(i64::MAX);
        self.charge(company, cells.saturating_mul(TERRAFORM_COST_PER_CELL))
    }

    /// The node of `network` nearest `pos`, within the node tolerance
    /// horizontally and the height tolerance in height.
    fn find_node(&self, network: u8, pos: P) -> Option<P> {
        self.edges
            .keys()
            .filter(|(n, _, _)| *n == network)
            .flat_map(|(_, a, b)| [*a, *b])
            .filter(|end| {
                within_horizontally(*end, pos, NODE_TOLERANCE)
                    && (i64::from(end[2]) - i64::from(pos[2])).abs() <= HEIGHT_TOLERANCE
            })
            .min_by_key(|end| (dist2(*end, pos), *end))
    }

    /// The object of `model` on edge `key` nearest `at`, within
    /// EDGE_TOLERANCE, other than those `taken`; the company's own.
    fn edge_object(
        &self,
        key: EdgeKey,
        at: &Pos,
        model: &ResName,
        taken: &[(EdgeKey, P)],
        company: u32,
    ) -> Result<(EdgeKey, P), Refusal> {
        let at = p(at);
        let found = self
            .objects
            .iter()
            .filter(|(k @ (on, there), object)| {
                *on == key
                    && !taken.contains(k)
                    && object.model == model.as_str()
                    && within(*there, at, EDGE_TOLERANCE)
            })
            .min_by_key(|((_, there), _)| dist2(*there, at))
            .map(|(k, object)| (*k, object.owner));
        let Some((object, owner)) = found else {
            refuse!("no {model} there");
        };
        if owner != company {
            refuse!("the {model} is company-{owner}'s");
        }
        Ok(object)
    }

    /// Removes edge objects with their stations, none unless every station
    /// is unserved.
    fn remove_objects(&mut self, objects: &[(EdgeKey, P)]) -> Result<(), Refusal> {
        for object in objects {
            if let Some(station) = self.objects[object].station {
                self.unserved(station)?;
            }
        }
        for object in objects {
            if let Some(object) = self.objects.remove(object)
                && let Some(station) = object.station
            {
                self.stations.remove(&station);
            }
        }
        Ok(())
    }

    fn find_edge(&self, network: u8, ends: &EdgeEnds) -> Result<EdgeKey, Refusal> {
        let (a, b) = (p(&ends.a), p(&ends.b));
        let matches = |x: P, y: P| {
            (within(x, a, EDGE_TOLERANCE) && within(y, b, EDGE_TOLERANCE))
                || (within(x, b, EDGE_TOLERANCE) && within(y, a, EDGE_TOLERANCE))
        };
        match self
            .edges
            .keys()
            .find(|(n, x, y)| *n == network && matches(*x, *y))
        {
            Some(key) => Ok(*key),
            None => refuse!("no edge {a:?}-{b:?}"),
        }
    }

    fn find_construction(&self, reference: &ConstructionRef) -> Result<(String, P), Refusal> {
        let at = p(&reference.at);
        self.constructions
            .keys()
            .filter(|(file, there)| {
                file == reference.file.as_str() && within(*there, at, CONSTRUCTION_TOLERANCE)
            })
            .min_by_key(|(_, there)| dist2(*there, at))
            .cloned()
            .ok_or_else(|| format!("no {} at {at:?}", reference.file))
    }

    /// Refuses when a stop or signal stands on the edge.
    fn unobstructed(&self, key: &EdgeKey) -> Result<(), Refusal> {
        if self.objects.keys().any(|(edge, _)| edge == key) {
            refuse!("something stands on the edge");
        }
        Ok(())
    }

    /// Refuses when a line stops at the station.
    fn unserved(&self, station: u32) -> Result<(), Refusal> {
        if let Some((id, _)) = self
            .lines
            .iter()
            .find(|(_, line)| line.stops.iter().any(|(s, _)| *s == station))
        {
            refuse!("line-{id} stops at station-{station}");
        }
        Ok(())
    }

    /// A line's stops, for `company`: at stations no company owns, its own,
    /// or another company's that lets `company` stop there (its choice for
    /// `company`, else its default; D22, proposed).
    fn stops<'a>(
        &self,
        stops: impl Iterator<Item = &'a tpf3mp_proto::action::LineStop>,
        company: u32,
    ) -> Result<Vec<(u32, Option<u16>)>, Refusal> {
        let stops: Vec<_> = stops
            .map(|s| (s.group.0, Some(s.terminal.terminal)))
            .collect();
        if stops.len() < 2 {
            refuse!("a line of fewer than two stops");
        }
        for (station, _) in &stops {
            if !self.stations.contains_key(station) {
                refuse!("no station-{station}");
            }
            if let Some(owner) = self.station_owner(*station)
                && owner != company
                && self.companies.get(&owner).is_some_and(|c| !c.lets(company))
            {
                refuse!(
                    "station-{station} is company-{owner}'s, which keeps its stations to itself"
                );
            }
        }
        Ok(stops)
    }

    fn own_line(&self, line: LineId, company: u32) -> Result<u32, Refusal> {
        match self.lines.get(&line.0) {
            None => refuse!("no {line}"),
            Some(found) if found.owner != company => {
                refuse!("{line} is company-{}'s", found.owner)
            }
            Some(_) => Ok(line.0),
        }
    }

    /// A name given from an entity's window or the line manager, as the
    /// mod replays it (`tpf3mp/apply.lua`, `HANDLERS.Rename`): the
    /// company's own vehicle, a station or construction no other company
    /// owns, any town. The model has no towns to name.
    fn rename(&mut self, what: &Renamed, name: &str, company: u32) -> Result<(), Refusal> {
        match what {
            Renamed::Vehicle(vehicle) => {
                let [id] = self.own_vehicles(std::iter::once(vehicle.0), company)?[..] else {
                    unreachable!("one vehicle named, one checked")
                };
                self.vehicles.get_mut(&id).expect("checked above").name = name.to_owned();
                Ok(())
            }
            Renamed::Station(station) => {
                let id = station.0;
                if !self.stations.contains_key(&id) {
                    refuse!("no station-{id}");
                }
                if let Some(owner) = self.station_owner(id).filter(|owner| *owner != company) {
                    refuse!("station-{id} is company-{owner}'s");
                }
                self.stations.get_mut(&id).expect("checked above").name = name.to_owned();
                Ok(())
            }
            Renamed::Town(_) => Ok(()),
            Renamed::Construction(reference) => {
                let key = self.find_construction(reference)?;
                let owner = self.constructions[&key].owner;
                if owner != company {
                    refuse!("the construction is company-{owner}'s");
                }
                self.constructions.get_mut(&key).expect("found above").name = name.to_owned();
                Ok(())
            }
        }
    }

    /// A vehicle's consist swapped for another, as TF3's vehicle window
    /// does: the company's own vehicle, a consist of cars whose groups add
    /// up to it, each kept car one of the vehicle's own of the same model,
    /// kept once. New cars are paid for, cars left out are sold at half; the
    /// vehicle keeps its id and its line.
    fn replace(&mut self, replace: &ReplaceVehicle, company: u32) -> Result<(), Refusal> {
        let [id] = self.own_vehicles(std::iter::once(replace.vehicle.0), company)?[..] else {
            unreachable!("one vehicle named, one checked")
        };
        if replace.consist.is_empty() {
            refuse!("a replacement of no cars");
        }
        let grouped: usize = replace.groups.iter().map(|g| usize::from(*g)).sum();
        if grouped != replace.consist.len() || replace.groups.contains(&0) {
            refuse!(
                "groups of {grouped} cars for a consist of {}",
                replace.consist.len()
            );
        }
        if replace.multiple_units.len() != replace.groups.len() {
            refuse!(
                "{} multiple units for {} groups",
                replace.multiple_units.len(),
                replace.groups.len()
            );
        }
        let old = &self.vehicles[&id].consist;
        let mut kept = BTreeSet::new();
        for (index, part) in replace.consist.iter().enumerate() {
            let Some(from) = part.kept else { continue };
            match old.get(usize::from(from)) {
                None => refuse!("car {index} keeps car {from}, which vehicle-{id} does not have"),
                Some(model) if model != part.part.model.as_str() => {
                    refuse!(
                        "car {index} keeps car {from}, a {model}, as a {}",
                        part.part.model
                    )
                }
                Some(_) => {}
            }
            if !kept.insert(from) {
                refuse!("car {from} kept twice");
            }
        }
        let bought = i64::try_from(replace.consist.len() - kept.len()).unwrap_or(i64::MAX);
        let sold = i64::try_from(old.len() - kept.len()).unwrap_or(i64::MAX);
        // Net of what the cars left out bring: a negative charge pays in.
        self.charge(
            company,
            VEHICLE_COST
                .saturating_mul(bought)
                .saturating_sub(VEHICLE_COST.saturating_mul(sold) / 2),
        )?;
        let vehicle = self.vehicles.get_mut(&id).expect("checked above");
        vehicle.consist = replace
            .consist
            .iter()
            .map(|part| part.part.model.as_str().to_owned())
            .collect();
        vehicle.load = vehicle
            .load
            .min(CAR_CAPACITY.saturating_mul(u32::try_from(vehicle.consist.len()).unwrap_or(0)));
        Ok(())
    }

    fn own_vehicles(
        &self,
        ids: impl Iterator<Item = u32>,
        company: u32,
    ) -> Result<Vec<u32>, Refusal> {
        let mut seen = BTreeSet::new();
        for id in ids {
            match self.vehicles.get(&id) {
                None => refuse!("no vehicle-{id}"),
                Some(vehicle) if vehicle.owner != company => {
                    refuse!("vehicle-{id} is company-{}'s", vehicle.owner)
                }
                Some(_) => {}
            }
            if !seen.insert(id) {
                refuse!("vehicle-{id} named twice");
            }
        }
        Ok(seen.into_iter().collect())
    }

    fn simulate(&mut self, step: u64) {
        let mut rng = SplitMix64::new(self.rng);
        self.now = step;
        self.prospections_end(&mut rng);
        let served: BTreeSet<u32> = self
            .lines
            .values()
            .flat_map(|line| line.stops.iter().map(|(station, _)| *station))
            .collect();
        for id in served {
            if let Some(station) = self.stations.get_mut(&id) {
                let arrivals = u32::try_from(rng.below(3)).unwrap_or(0);
                station.waiting = (station.waiting + arrivals).min(MAX_WAITING);
            }
        }
        let Self {
            lines,
            stations,
            vehicles,
            companies,
            delivered,
            served,
            ..
        } = self;
        for vehicle in vehicles.values_mut() {
            let Some(line) = vehicle.line.and_then(|id| lines.get(&id)) else {
                continue;
            };
            let count = line.stops.len();
            if count < 2 {
                continue;
            }
            let next = usize::from(vehicle.next_stop) % count;
            let previous = (next + count - 1) % count;
            let (Some(from), Some(to)) = (
                stations.get(&line.stops[previous].0),
                stations.get(&line.stops[next].0),
            ) else {
                continue;
            };
            let segment = u32::try_from(metres(from.at, to.at) / 10)
                .unwrap_or(u32::MAX)
                .max(10);
            vehicle.progress += 1 + u32::try_from(rng.below(2)).unwrap_or(0);
            if vehicle.progress < segment {
                continue;
            }
            vehicle.progress = 0;
            *delivered += u64::from(vehicle.load);
            if vehicle.load > 0 {
                *served
                    .entry((vehicle.owner, line.stops[next].0))
                    .or_default() += u64::from(vehicle.load);
            }
            if let Some(owner) = companies.get_mut(&vehicle.owner) {
                owner.money += i64::from(vehicle.load) * FARE;
            }
            let capacity = CAR_CAPACITY
                .saturating_mul(u32::try_from(vehicle.consist.len()).unwrap_or(u32::MAX));
            vehicle.load = 0;
            if let Some(station) = stations.get_mut(&line.stops[next].0) {
                vehicle.load = capacity.min(station.waiting);
                station.waiting -= vehicle.load;
            }
            vehicle.next_stop = u16::try_from((next + 1) % count).unwrap_or(0);
        }
        if step.is_multiple_of(UPKEEP_EVERY) {
            for vehicle in vehicles.values() {
                if let Some(owner) = companies.get_mut(&vehicle.owner) {
                    owner.money -= UPKEEP;
                }
            }
        }
        if step.is_multiple_of(PROGRESSION_EVERY) {
            self.progress();
        }
        self.rng = rng.state();
    }
}

/// A line as a check sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineView {
    pub name: String,
    pub stops: usize,
    pub vehicles: usize,
}

/// A vehicle as a check sees it: its name, and its colour if its window
/// gave it one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VehicleView {
    pub name: String,
    pub color: Option<Tint>,
}

/// What a replica shows of its world, for the harness's checks. The real
/// game's hook answers the same questions from the game's own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub junctions: usize,
    /// The money of each player's company, in the order they joined, so
    /// actors first.
    pub money: Vec<Option<i64>>,
    /// Each player's company, in the order they joined.
    pub company_of: Vec<Option<CompanyId>>,
    pub companies: usize,
    pub street_edges: usize,
    pub track_edges: usize,
    pub constructions: usize,
    pub stations: usize,
    pub depots: usize,
    /// Stops, signals and waypoints on edges.
    pub edge_objects: usize,
    pub lines: BTreeMap<LineId, LineView>,
    /// Each vehicle's name and colour.
    pub vehicle_views: BTreeMap<VehicleId, VehicleView>,
    /// Each station's name.
    pub station_names: BTreeMap<StationId, String>,
    pub vehicles: usize,
    /// Vehicles on no line.
    pub idle: usize,
    pub delivered: u64,
    pub ignored: u64,
    pub terrain_cells: usize,
    /// Prospections under way.
    pub prospections: usize,
    /// Industries prospecting found.
    pub industries: usize,
    /// Each player's company's rank, taken and reached, in the order they
    /// joined.
    pub ranks: Vec<Option<(u8, u8)>>,
}

/// One replica of the model.
#[derive(Debug, Clone)]
pub struct ModelWorld {
    state: State,
    drift_at: Option<u64>,
    /// Why each ignored action was ignored, by event sequence number. Not
    /// part of the world: for the harness's report.
    ignored: Vec<(u64, String)>,
}

impl ModelWorld {
    pub fn new(seed: u64) -> Self {
        Self {
            state: State {
                rng: seed,
                ..State::default()
            },
            drift_at: None,
            ignored: Vec::new(),
        }
    }

    /// Makes this replica's simulation deviate once at `step`.
    pub fn with_drift(mut self, step: u64) -> Self {
        self.drift_at = Some(step);
        self
    }

    pub fn save(&self) -> Vec<u8> {
        postcard::to_stdvec(&self.state).expect("model state always encodes")
    }

    pub fn load(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            state: postcard::from_bytes(bytes).ok()?,
            drift_at: None,
            ignored: Vec::new(),
        })
    }

    /// The players in the order they joined.
    pub fn players(&self) -> &[PlayerId] {
        &self.state.players
    }

    /// Why each action that changed nothing was ignored.
    pub fn ignored(&self) -> &[(u64, String)] {
        &self.ignored
    }

    pub fn apply(&mut self, event: &Event) {
        match &event.body {
            EventBody::PlayerJoined { player, .. } => self.state.join(*player),
            EventBody::PlayerLeft { .. } | EventBody::Save => {}
            EventBody::Command {
                player,
                payload,
                seal,
                ..
            } => {
                let outcome = Action::from_payload(payload)
                    .map_err(|error| error.to_string())
                    .and_then(|action| {
                        let mut next = self.state.clone();
                        next.act(player, &action, seal.as_ref())?;
                        Ok(next)
                    });
                match outcome {
                    Ok(next) => self.state = next,
                    Err(why) => {
                        self.state.ignored += 1;
                        self.ignored.push((event.seq, why));
                    }
                }
            }
        }
    }

    pub fn step(&mut self, step: u64) {
        self.state.simulate(step);
        if self.drift_at == Some(step) {
            let mut rng = SplitMix64::new(self.state.rng);
            rng.next_u64();
            self.state.rng = rng.state();
        }
    }

    pub fn lanes(&self) -> Vec<LaneDigest> {
        let s = &self.state;
        let waiting: Vec<(u32, u32)> = s
            .stations
            .iter()
            .map(|(id, st)| (*id, st.waiting))
            .collect();
        let stations: Vec<(u32, P, &str)> = s
            .stations
            .iter()
            .map(|(id, st)| (*id, st.at, st.name.as_str()))
            .collect();
        vec![
            lane_digest(lane::NETWORK, &(&s.edges, &s.terrain, &s.junctions)),
            lane_digest(
                lane::CONSTRUCTIONS,
                &(
                    &s.constructions,
                    &s.objects,
                    &stations,
                    s.next_station,
                    &s.prospections,
                    &s.industries,
                    s.next_industry,
                ),
            ),
            lane_digest(lane::LINES, &(&s.lines, s.next_line)),
            lane_digest(lane::VEHICLES, &(&s.vehicles, &waiting, s.next_vehicle)),
            lane_digest(
                lane::ECONOMY,
                &(
                    &s.players,
                    &s.member_of,
                    &s.companies,
                    s.next_company,
                    s.delivered,
                    &s.served,
                    s.ignored,
                    s.rng,
                ),
            ),
        ]
    }

    pub fn observe(&self) -> Observation {
        let s = &self.state;
        let lines = s
            .lines
            .iter()
            .map(|(id, line)| {
                let vehicles = s.vehicles.values().filter(|v| v.line == Some(*id)).count();
                (
                    LineId(*id),
                    LineView {
                        name: line.name.clone(),
                        stops: line.stops.len(),
                        vehicles,
                    },
                )
            })
            .collect();
        let edges = |n: u8| s.edges.keys().filter(|(of, _, _)| *of == n).count();
        Observation {
            junctions: self.state.junctions.len(),
            money: s
                .players
                .iter()
                .map(|player| {
                    let company = s.member_of.get(player)?;
                    Some(s.companies.get(company)?.money)
                })
                .collect(),
            company_of: s
                .players
                .iter()
                .map(|player| s.member_of.get(player).map(|c| CompanyId(*c)))
                .collect(),
            companies: s.companies.len(),
            street_edges: edges(net(Network::Street)),
            track_edges: edges(net(Network::Track)),
            constructions: s.constructions.len(),
            stations: s.stations.len(),
            depots: s
                .constructions
                .values()
                .map(|construction| usize::from(construction.depot_count))
                .sum(),
            edge_objects: s.objects.len(),
            lines,
            vehicle_views: s
                .vehicles
                .iter()
                .map(|(id, vehicle)| {
                    (
                        VehicleId(*id),
                        VehicleView {
                            name: vehicle.name.clone(),
                            color: vehicle.color.map(|[r, g, b]| Tint { r, g, b }),
                        },
                    )
                })
                .collect(),
            station_names: s
                .stations
                .iter()
                .map(|(id, station)| (StationId(*id), station.name.clone()))
                .collect(),
            vehicles: s.vehicles.len(),
            idle: s.vehicles.values().filter(|v| v.line.is_none()).count(),
            delivered: s.delivered,
            ignored: s.ignored,
            terrain_cells: s.terrain.len(),
            prospections: s.prospections.len(),
            industries: s.industries.len(),
            ranks: s
                .players
                .iter()
                .map(|player| {
                    let progress = s.companies.get(s.member_of.get(player)?)?.progress;
                    Some((progress.level, progress.potential))
                })
                .collect(),
        }
    }
}

fn lane_digest<T: Serialize>(lane: u16, value: &T) -> LaneDigest {
    let bytes = postcard::to_stdvec(value).expect("model state always encodes");
    let hash = digest(&SHA256, &bytes);
    let mut out = [0; 32];
    out.copy_from_slice(hash.as_ref());
    LaneDigest {
        lane,
        digest: FixedBytes(out),
    }
}

#[cfg(test)]
mod tests {
    use tpf3mp_proto::{Platform, Text, action::LoanTerms};

    use super::*;

    fn event(seq: u64, body: EventBody) -> Event {
        Event {
            seq,
            step: seq,
            body,
        }
    }

    fn loan(amount: i64) -> LoanTerms {
        LoanTerms {
            kind: Text::new("Small").unwrap(),
            amount,
            duration: 1_095_000,
            percentage: 30_000,
            birth_day: None,
            cooldown_until: None,
            last_pay_day: None,
            times_paid: None,
            id: None,
        }
    }

    fn act(world: &mut ModelWorld, seq: u64, player: PlayerId, action: &Action) {
        act_sealed(world, seq, player, action, None);
    }

    fn act_sealed(
        world: &mut ModelWorld,
        seq: u64,
        player: PlayerId,
        action: &Action,
        seal: Option<Seal>,
    ) {
        world.apply(&event(
            seq,
            EventBody::Command {
                player,
                client_seq: seq,
                payload: action.to_payload().unwrap(),
                seal,
            },
        ));
    }

    fn joined(players: &[PlayerId]) -> ModelWorld {
        let mut world = ModelWorld::new(1);
        for (seq, player) in players.iter().enumerate() {
            world.apply(&event(
                seq as u64 + 1,
                EventBody::PlayerJoined {
                    player: *player,
                    name: Text::new("p").unwrap(),
                    platform: Platform::current(),
                },
            ));
        }
        world
    }

    #[test]
    fn junction_settings_change_the_network_digest_and_survive_save_load() {
        use crate::regress::{library, script::Item};
        let player = PlayerId(FixedBytes([7; 32]));
        let mut world = joined(&[player]);
        let scenario = library::scenarios()
            .into_iter()
            .find(|s| s.name == "junctions")
            .unwrap();
        let mut actions = scenario.items.iter().filter_map(|item| match item {
            Item::Act { action, .. } => Some(action),
            _ => None,
        });
        act(&mut world, 2, player, actions.next().unwrap());
        let before = world.lanes()[0];
        let edit = actions.next().unwrap();
        let Action::EditJunctions(edit_data) = edit else {
            panic!("junction action");
        };
        act(&mut world, 3, player, edit);
        assert!(world.ignored().is_empty());
        assert_ne!(world.lanes()[0], before);
        assert_eq!(
            world.state.junctions.values().next(),
            edit_data.changes[0].config.as_ref()
        );
        let restored = ModelWorld::load(&world.save()).unwrap();
        assert_eq!(restored.state.junctions, world.state.junctions);
        assert_eq!(restored.lanes(), world.lanes());
    }

    fn stations(world: &mut ModelWorld, count: u32) {
        for id in 0..count {
            world.state.stations.insert(
                id,
                Station {
                    at: [i32::try_from(id).unwrap() * 1_000_000, 0, 0],
                    waiting: 0,
                    name: format!("Station {id}"),
                },
            );
        }
    }

    /// With one company its score is the world's population, as TF3's own,
    /// and it takes the ranks that reaches, one above another.
    #[test]
    fn one_company_ranks_by_the_worlds_population() {
        let player = PlayerId(FixedBytes([1; 32]));
        let mut world = joined(&[player]);
        stations(&mut world, 2);
        world.step(PROGRESSION_EVERY);
        assert_eq!(world.observe().ranks, [Some((1, 2))]);
        act(&mut world, 3, player, &Action::ApplyRank { level: 3 });
        act(&mut world, 4, player, &Action::ApplyRank { level: 2 });
        act(&mut world, 5, player, &Action::ApplyRank { level: 2 });
        assert_eq!(world.observe().ranks, [Some((2, 2))]);
        assert_eq!(world.ignored().len(), 2, "{:?}", world.ignored());
    }

    /// With two, each town's population is split by what each delivered
    /// there, and each company's score is its parts' sum.
    #[test]
    fn two_companies_split_each_towns_population_by_their_deliveries() {
        let (one, two) = (PlayerId(FixedBytes([1; 32])), PlayerId(FixedBytes([2; 32])));
        let mut world = joined(&[one, two]);
        stations(&mut world, 3);
        // Town 0: 30 to 10; towns 1 and 2 the second company's alone.
        for (key, n) in [((0, 0), 30), ((1, 0), 10), ((1, 1), 5), ((1, 2), 7)] {
            world.state.served.insert(key, n);
        }
        world.step(PROGRESSION_EVERY);
        let s = &world.state;
        assert_eq!(s.companies[&0].progress.experience, 750);
        assert_eq!(s.companies[&1].progress.experience, 2_250);
        assert_eq!(world.observe().ranks, [Some((1, 1)), Some((1, 2))]);
        // Experience never falls.
        world.state.served.clear();
        world.step(2 * PROGRESSION_EVERY);
        assert_eq!(world.state.companies[&1].progress.experience, 2_250);
        act(&mut world, 3, two, &Action::ApplyRank { level: 2 });
        act(&mut world, 4, one, &Action::ApplyRank { level: 2 });
        assert_eq!(world.observe().ranks, [Some((1, 1)), Some((2, 2))]);
    }

    /// D22 (proposed): a locked company takes a player only with its
    /// password's seal; its head alone locks, dismisses and shares; the
    /// head's place passes on when the founder leaves.
    #[test]
    fn a_locked_company_takes_only_the_right_seal_and_its_head_rules_it() {
        let players: Vec<PlayerId> = (1..=3).map(|n| PlayerId(FixedBytes([n; 32]))).collect();
        let (ann, bob, cat) = (players[0], players[1], players[2]);
        let mut world = ModelWorld::new(1);
        for (seq, player) in players.iter().enumerate() {
            world.apply(&event(
                seq as u64 + 1,
                EventBody::PlayerJoined {
                    player: *player,
                    name: Text::new(format!("p{seq}")).unwrap(),
                    platform: Platform::current(),
                },
            ));
        }
        let seal = |scope: u64, byte: u8| Seal {
            scope,
            tag: FixedBytes([byte; 32]),
        };
        let op = |op: CompanyOp| Action::CompanyOp(op);
        // Ann's company is company 0. Bob cannot lock it; Ann can, only with
        // a seal for it.
        act_sealed(
            &mut world,
            10,
            bob,
            &op(CompanyOp::Lock(CompanyId(0))),
            Some(seal(0, 1)),
        );
        act(&mut world, 11, ann, &op(CompanyOp::Lock(CompanyId(0))));
        act_sealed(
            &mut world,
            12,
            ann,
            &op(CompanyOp::Lock(CompanyId(0))),
            Some(seal(1, 1)),
        );
        act_sealed(
            &mut world,
            13,
            ann,
            &op(CompanyOp::Lock(CompanyId(0))),
            Some(seal(0, 1)),
        );
        assert_eq!(world.ignored().len(), 3);
        // Bob joins with the wrong password, then the right one; Cat without.
        act_sealed(
            &mut world,
            14,
            bob,
            &op(CompanyOp::Join(CompanyId(0))),
            Some(seal(0, 2)),
        );
        act(&mut world, 15, cat, &op(CompanyOp::Join(CompanyId(0))));
        act_sealed(
            &mut world,
            16,
            bob,
            &op(CompanyOp::Join(CompanyId(0))),
            Some(seal(0, 1)),
        );
        assert_eq!(world.ignored().len(), 5);
        assert_eq!(world.state.member_of[&bob], 0);
        // Bob is no head: he cannot dismiss or close the stations.
        let dismiss_ann = op(CompanyOp::Dismiss {
            company: CompanyId(0),
            player: Text::new(hex(&ann)).unwrap(),
        });
        act(&mut world, 17, bob, &dismiss_ann);
        act(
            &mut world,
            18,
            bob,
            &op(CompanyOp::ShareStations {
                company: CompanyId(0),
                open: false,
            }),
        );
        assert_eq!(world.ignored().len(), 7);
        // Ann leaves: Bob, the next to have joined, heads company 0 now.
        act(&mut world, 19, ann, &op(CompanyOp::Join(CompanyId(1))));
        act(
            &mut world,
            20,
            bob,
            &op(CompanyOp::ShareStations {
                company: CompanyId(0),
                open: false,
            }),
        );
        assert_eq!(world.ignored().len(), 7, "{:?}", world.ignored());
        assert!(!world.state.companies[&0].open);
        // Ann returns only with the password; Bob unlocks, and Cat joins.
        act(&mut world, 21, ann, &op(CompanyOp::Join(CompanyId(0))));
        act(&mut world, 22, bob, &op(CompanyOp::Unlock(CompanyId(0))));
        act(&mut world, 23, cat, &op(CompanyOp::Join(CompanyId(0))));
        assert_eq!(world.ignored().len(), 8);
        assert_eq!(world.state.member_of[&cat], 0);
        // Bob dismisses Cat, who plays for a company of her own again.
        let dismiss_cat = op(CompanyOp::Dismiss {
            company: CompanyId(0),
            player: Text::new(hex(&cat)).unwrap(),
        });
        act(&mut world, 24, bob, &dismiss_cat);
        assert_ne!(world.state.member_of[&cat], 0);
        assert_eq!(world.ignored().len(), 8);
    }

    /// D22 (proposed): a company's head lets single companies stop at its
    /// stations, or not, whatever the default; a company without a choice
    /// of its own, one founded later included, follows the default.
    #[test]
    fn a_head_chooses_station_access_per_company_over_the_default() {
        let players: Vec<PlayerId> = (1..=3).map(|n| PlayerId(FixedBytes([n; 32]))).collect();
        let (ann, bob) = (players[0], players[1]);
        let mut world = ModelWorld::new(1);
        for (seq, player) in players.iter().enumerate() {
            world.apply(&event(
                seq as u64 + 1,
                EventBody::PlayerJoined {
                    player: *player,
                    name: Text::new(format!("p{seq}")).unwrap(),
                    platform: Platform::current(),
                },
            ));
        }
        let access = |other: u32, open: Option<bool>| {
            Action::CompanyOp(CompanyOp::StationAccess {
                company: CompanyId(0),
                other: CompanyId(other),
                open,
            })
        };
        let lets = |world: &ModelWorld, other: u32| world.state.companies[&0].lets(other);
        // Ann heads company 0: she shuts company 1 out; 2 keeps the default.
        act(&mut world, 10, ann, &access(1, Some(false)));
        assert!(!lets(&world, 1) && lets(&world, 2));
        // Closed by default: 2 too, and a company founded later; then 2 let
        // in again on its own.
        act(
            &mut world,
            11,
            ann,
            &Action::CompanyOp(CompanyOp::ShareStations {
                company: CompanyId(0),
                open: false,
            }),
        );
        assert!(!lets(&world, 2) && !lets(&world, 9));
        act(&mut world, 12, ann, &access(2, Some(true)));
        act(&mut world, 13, ann, &access(1, None));
        assert!(lets(&world, 2) && !lets(&world, 1), "1 follows the default");
        assert_eq!(world.ignored().len(), 0, "{:?}", world.ignored());
        // Not Bob's to choose, nor for itself or a company there is not.
        act(&mut world, 14, bob, &access(1, Some(true)));
        act(&mut world, 15, ann, &access(0, Some(false)));
        act(&mut world, 16, ann, &access(42, Some(true)));
        assert_eq!(world.ignored().len(), 3, "{:?}", world.ignored());
        assert!(!lets(&world, 1));
    }

    #[test]
    fn a_loan_pays_its_amount_in_and_paying_it_back_takes_it_out() {
        let player = PlayerId(FixedBytes([1; 32]));
        let mut world = ModelWorld::new(1);
        world.apply(&event(
            1,
            EventBody::PlayerJoined {
                player,
                name: Text::new("p1").unwrap(),
                platform: Platform::current(),
            },
        ));
        let take = Action::Loan(Box::new(LoanOp::Take {
            next: loan(7_000_000),
            offer: loan(5_000_000),
        }));
        act(&mut world, 2, player, &take);
        assert_eq!(world.observe().money[0], Some(START_MONEY + 5_000_000));
        act(
            &mut world,
            3,
            player,
            &Action::Loan(Box::new(LoanOp::Repay {
                loan: loan(5_000_000),
            })),
        );
        assert_eq!(world.observe().money[0], Some(START_MONEY));
        // More than the company has is refused, and changes nothing.
        act(
            &mut world,
            4,
            player,
            &Action::Loan(Box::new(LoanOp::Repay {
                loan: loan(START_MONEY + 1),
            })),
        );
        assert_eq!(world.observe().money[0], Some(START_MONEY));
        assert_eq!(world.ignored().len(), 1);
    }

    /// A replacement is the company's own vehicle's, keeps only cars the
    /// vehicle has, each once and of its own model, and pays for the cars it
    /// buys net of those it leaves out; the vehicle keeps its id.
    #[test]
    fn a_replacement_keeps_the_vehicles_own_cars_and_pays_for_new_ones() {
        use crate::regress::library::{
            BUS_DEPOT, LOCOMOTIVE, WAGON, at, buy, construction, replace,
        };

        let (one, two) = (PlayerId(FixedBytes([1; 32])), PlayerId(FixedBytes([2; 32])));
        let mut world = ModelWorld::new(1);
        for (seq, (player, name)) in [(one, "p1"), (two, "p2")].into_iter().enumerate() {
            world.apply(&event(
                seq as u64 + 1,
                EventBody::PlayerJoined {
                    player,
                    name: Text::new(name).unwrap(),
                    platform: Platform::current(),
                },
            ));
        }
        act(
            &mut world,
            3,
            one,
            &construction(BUS_DEPOT, at(0, 0), "Yard"),
        );
        act(
            &mut world,
            4,
            one,
            &buy(BUS_DEPOT, at(0, 0), &[LOCOMOTIVE, WAGON]),
        );
        let money = world.observe().money[0].unwrap();
        let refusals = [
            // Someone else's vehicle; one that is not there.
            (two, replace(0, &[(LOCOMOTIVE, Some(0))])),
            (one, replace(5, &[(LOCOMOTIVE, Some(0))])),
            // No cars; a car the vehicle does not have; another model kept;
            // a car kept twice.
            (one, replace(0, &[])),
            (one, replace(0, &[(LOCOMOTIVE, Some(2))])),
            (one, replace(0, &[(WAGON, Some(0))])),
            (one, replace(0, &[(WAGON, Some(1)), (WAGON, Some(1))])),
        ];
        for (seq, (player, action)) in refusals.iter().enumerate() {
            act(&mut world, 5 + seq as u64, *player, action);
        }
        let why: Vec<String> = world.ignored().iter().map(|(_, w)| w.clone()).collect();
        assert_eq!(
            why,
            [
                "vehicle-0 is company-0's".to_owned(),
                "no vehicle-5".to_owned(),
                "a replacement of no cars".to_owned(),
                "car 0 keeps car 2, which vehicle-0 does not have".to_owned(),
                format!("car 0 keeps car 0, a {LOCOMOTIVE}, as a {WAGON}"),
                "car 1 kept twice".to_owned(),
            ]
        );
        assert_eq!(
            world.observe().money[0],
            Some(money),
            "refusals cost nothing"
        );
        // The locomotive kept, the coach left out, two new coaches: two
        // bought, one sold at half.
        act(
            &mut world,
            20,
            one,
            &replace(0, &[(LOCOMOTIVE, Some(0)), (WAGON, None), (WAGON, None)]),
        );
        assert_eq!(world.ignored().len(), 6, "{:?}", world.ignored());
        assert_eq!(
            world.observe().money[0],
            Some(money - 2 * VEHICLE_COST + VEHICLE_COST / 2)
        );
        assert_eq!(world.state.vehicles[&0].consist.len(), 3);
    }

    #[test]
    fn a_prospection_ends_alike_on_every_replica_after_its_time() {
        use crate::regress::library::prospect;

        let player = PlayerId(FixedBytes([1; 32]));
        let replica = || {
            let mut world = ModelWorld::new(7);
            world.apply(&event(
                1,
                EventBody::PlayerJoined {
                    player,
                    name: Text::new("p1").unwrap(),
                    platform: Platform::current(),
                },
            ));
            world
        };
        let (mut a, mut b) = (replica(), replica());
        // Many towns, so some prospections find an industry and some not.
        for (seq, town) in (2..).zip(0..20u32) {
            for world in [&mut a, &mut b] {
                act(
                    world,
                    seq,
                    player,
                    &prospect(town, "coal", &["mine", "pit"]),
                );
            }
        }
        // A second one for the same town and cargo changes nothing.
        act(&mut a, 30, player, &prospect(3, "coal", &["mine"]));
        assert_eq!(a.observe().prospections, 20);
        assert_eq!(a.ignored().len(), 1);
        act(&mut b, 30, player, &prospect(3, "coal", &["mine"]));
        for step in 1..PROSPECTION_STEPS {
            a.step(step);
            b.step(step);
        }
        assert_eq!(a.observe().prospections, 20, "not before its time");
        a.step(PROSPECTION_STEPS);
        b.step(PROSPECTION_STEPS);
        let seen = a.observe();
        assert_eq!(seen.prospections, 0);
        assert!(
            seen.industries > 0 && seen.industries < 20,
            "{} found",
            seen.industries
        );
        assert_eq!(
            a.lanes(),
            b.lanes(),
            "the same industries, at the same places"
        );
    }

    /// A street drawn onto another's middle, as TF3's street tool proposes
    /// it: the old street's node there goes with its two edges, and the old
    /// street is rebuilt through the new junction in its own kind.
    #[test]
    fn a_junction_rebuilds_the_street_it_joins_in_its_own_kind() {
        use tpf3mp_proto::{
            BoundedVec,
            action::{EdgeKind, EdgeRef, Link, NodeRef, RoadBuild, Tram},
        };

        use crate::regress::library::{at, new, node, polyline, road};

        let player = PlayerId(FixedBytes([1; 32]));
        let mut world = ModelWorld::new(1);
        world.apply(&event(
            1,
            EventBody::PlayerJoined {
                player,
                name: Text::new("p1").unwrap(),
                platform: Platform::current(),
            },
        ));
        // The street A-M-B.
        act(
            &mut world,
            2,
            player,
            &road(vec![new(at(400, 0)), new(at(450, 0)), new(at(500, 0))]),
        );
        assert_eq!(world.observe().street_edges, 2);
        let street_edge = |a, b| EdgeRef {
            network: Network::Street,
            ends: EdgeEnds { a, b },
        };
        let junction = |removals: Vec<EdgeRef>, removed_nodes: Vec<NodeRef>| {
            let mut lines = polyline(
                vec![
                    new(at(450, 100)),
                    new(at(452, 0)),
                    node(at(400, 0), Network::Street),
                    node(at(500, 0), Network::Street),
                ],
                &Structure::Ground,
            );
            // The new street's one link, then the old street rebuilt.
            let mut links = vec![lines.links.to_vec()[0].clone()];
            for (from, to) in [(2, 1), (1, 3)] {
                links.push(Link {
                    from,
                    to,
                    kind: Some(EdgeKind {
                        network: Network::Street,
                        template: Text::new("street/country.street_template").unwrap(),
                        style: None,
                    }),
                    ..links[0].clone()
                });
            }
            lines = Polyline::new(
                lines.vertices,
                BoundedVec::new(links).unwrap(),
                BoundedVec::new(removals).unwrap(),
            )
            .unwrap()
            .with_removed_nodes(BoundedVec::new(removed_nodes).unwrap());
            Action::BuildRoad(RoadBuild {
                street: Text::new("street/town.street_template").unwrap(),
                style: None,
                bus_lane: false,
                tram: Tram::None,
                polyline: lines,
                crossing_type: None,
            })
        };
        let both = || {
            vec![
                street_edge(at(400, 0), at(450, 0)),
                street_edge(at(450, 0), at(500, 0)),
            ]
        };
        let street_node = |pos| NodeRef {
            network: Network::Street,
            at: pos,
        };
        // Removing a node the build joins, or one that keeps an edge, is
        // refused and changes nothing.
        act(
            &mut world,
            3,
            player,
            &junction(both(), vec![street_node(at(400, 0))]),
        );
        act(
            &mut world,
            4,
            player,
            &junction(
                vec![street_edge(at(400, 0), at(450, 0))],
                vec![street_node(at(450, 0))],
            ),
        );
        assert_eq!(world.observe().street_edges, 2);
        let why: Vec<&str> = world.ignored().iter().map(|(_, w)| w.as_str()).collect();
        assert_eq!(why[0], "the build removes a node it joins");
        assert!(why[1].ends_with("still has edges"), "{why:?}");
        // The tool's own: the old junction's node goes.
        act(
            &mut world,
            5,
            player,
            &junction(both(), vec![street_node(at(450, 0))]),
        );
        assert_eq!(world.ignored().len(), 2, "{:?}", world.ignored());
        assert_eq!(world.observe().street_edges, 3);
        let kinds: Vec<&str> = world
            .state
            .edges
            .values()
            .map(|e| e.kind.as_str())
            .collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == "street/country.street_template")
                .count(),
            2,
            "{kinds:?}"
        );
    }
}
