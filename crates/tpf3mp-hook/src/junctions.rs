//! Read junction-only WorldBuildProposal commands at CommandList::Add.
//! The native lane/crosswalk tools have no Lua proposal event on build 40408.
//! Layouts are checked against the shipped Lua binding registrations:
//! StreetProposal at 0x22c3920, BaseNodeLaneConnectionAndEntity at 0x22c3c8b,
//! BaseNodeConfig/TrafficLightConfig/State at 0x1768253..0x17683e3,
//! LaneConnection at 0x22c56b2. See docs/HOOKS.md, Junction tools.
//! No geometry is inferred or copied: mixed proposals use ordinary capture.

use std::sync::atomic::{AtomicBool, Ordering};

use tpf3mp_proto::lua::LuaValue;

use crate::modules::{self, Memory, i32_at, ids, layout, read, vector};

pub use crate::build_data::native::junctions::CONFIG_LAYOUT;
pub use crate::build_data::native::junctions::CROSSWALK_LAYOUT;
pub use crate::build_data::native::junctions::PROPOSAL_LAYOUT;
static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

fn integer(n: i32) -> LuaValue {
    LuaValue::Integer(i64::from(n))
}
fn object(fields: impl IntoIterator<Item = (&'static str, LuaValue)>) -> LuaValue {
    LuaValue::Table(
        fields
            .into_iter()
            .map(|(k, v)| (LuaValue::string(k), v))
            .collect(),
    )
}
fn array(values: impl IntoIterator<Item = LuaValue>) -> LuaValue {
    LuaValue::Table(
        values
            .into_iter()
            .enumerate()
            .map(|(i, v)| (LuaValue::Integer(i as i64 + 1), v))
            .collect(),
    )
}
fn boolean(raw: &[u8], at: usize) -> Result<LuaValue, String> {
    match raw[at] {
        0 => Ok(LuaValue::Boolean(false)),
        1 => Ok(LuaValue::Boolean(true)),
        _ => Err("invalid junction flag".into()),
    }
}
fn number(raw: &[u8], at: usize) -> Result<LuaValue, String> {
    let value = f32::from_bits(i32_at(raw, at) as u32);
    if !value.is_finite() || value < 0.0 || value > 86_400.0 {
        return Err("invalid traffic-light duration".into());
    }
    Ok(LuaValue::Number(f64::from(value)))
}
fn integers(memory: &dyn Memory, raw: &[u8], at: usize, max: usize) -> Result<LuaValue, String> {
    let (begin, count) = vector(raw, at, 4, max, "junction indices")?;
    Ok(array(
        ids(memory, begin, count, 4, 0, "junction indices")?
            .into_iter()
            .map(integer),
    ))
}

fn crosswalks(memory: &dyn Memory, raw: &[u8]) -> Result<LuaValue, String> {
    Ok(array(crosswalk_ids(memory, raw)?.into_iter().map(integer)))
}

/// The crosswalk set of the `BaseNodeConfig` in `raw`: its edges, in the
/// order its slots hold them, the order the game's Lua lists them in.
pub(crate) fn crosswalk_ids(memory: &dyn Memory, raw: &[u8]) -> Result<Vec<i32>, String> {
    // phmap::flat_hash_set<int>, not std::vector<int>. The move/copy at
    // 0x1eda20/0x1fe1e0 and iteration at 0xa49f1a establish the layout:
    // control bytes, int slots, size, capacity. The remaining words are
    // hash-table bookkeeping, not another component field.
    let word = |at: usize| -> Result<usize, String> {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(&raw[at..at + 8]);
        usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| "crosswalk set too large".into())
    };
    let control = word(0x18)?;
    let slots = word(0x20)?;
    let count = word(0x28)?;
    let capacity = word(0x30)?;
    if count > 256 || capacity > 1023 || count > capacity {
        return Err("crosswalk set exceeds its bounds".into());
    }
    if capacity == 0 {
        return Ok(Vec::new());
    }
    if !(capacity + 1).is_power_of_two() || control == 0 || slots == 0 {
        return Err("invalid crosswalk set storage".into());
    }
    let tags = read(memory, control, capacity + 1, "crosswalk controls")?;
    let values = read(memory, slots, capacity * 4, "crosswalk slots")?;
    if tags[capacity] != 0xff {
        return Err("crosswalk set has no end sentinel".into());
    }
    let mut entries = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (i, tag) in tags[..capacity].iter().enumerate() {
        match tag {
            0..=0x7f => {
                let id = i32_at(&values, i * 4);
                if id < 0 || !seen.insert(id) {
                    return Err("invalid or duplicate crosswalk edge".into());
                }
                entries.push(id);
            }
            0x80 | 0xfe => (), // empty or deleted
            _ => return Err("invalid crosswalk control byte".into()),
        }
    }
    if entries.len() != count {
        return Err("crosswalk set size does not match its occupied slots".into());
    }
    Ok(entries)
}

fn config(memory: &dyn Memory, address: usize) -> Result<LuaValue, String> {
    // The component is first; entity is at +0x78, with an aligned 0x80 stride.
    let raw = read(memory, address, 0x80, "junction configuration")?;
    let entity = i32_at(&raw, 0x78);
    if entity < 0 {
        return Err("a junction-only edit names a new node".into());
    }
    Ok(object([
        ("entity", integer(entity)),
        ("comp", snapshot(memory, address)?),
    ]))
}

/// The standalone BaseNodeConfig in an owned Lua userdata has no entity suffix.
pub(crate) fn snapshot(memory: &dyn Memory, address: usize) -> Result<LuaValue, String> {
    let raw = read(
        memory,
        address,
        crate::build_data::native::network::NODE_CONFIG_SIZE,
        "junction snapshot",
    )?;
    let (begin, count) = vector(&raw, 0, 0x14, 256, "turn connections")?;
    let mut connections = Vec::new();
    for i in 0..count {
        let turn = read(memory, begin + i * 0x14, 0x14, "turn connection")?;
        if [0, 4, 8, 12]
            .iter()
            .any(|offset| i32_at(&turn, *offset) < 0)
        {
            return Err("negative junction edge or lane".into());
        }
        connections.push(object([
            ("segment0", integer(i32_at(&turn, 0))),
            ("lane0", integer(i32_at(&turn, 4))),
            ("segment1", integer(i32_at(&turn, 8))),
            ("lane1", integer(i32_at(&turn, 12))),
            ("withRoad", boolean(&turn, 0x10)?),
            ("withTram", boolean(&turn, 0x11)?),
        ]));
    }
    let (begin, count) = vector(&raw, 0x50, 0x28, 64, "traffic-light phases")?;
    let mut phases = Vec::new();
    for i in 0..count {
        let phase = read(memory, begin + i * 0x28, 0x28, "traffic-light phase")?;
        phases.push(object([
            ("lockedLanes", integers(memory, &phase, 0, 256)?),
            ("duration", number(&phase, 0x18)?),
            ("minDuration", number(&phase, 0x1c)?),
            ("canSkip", boolean(&phase, 0x20)?),
        ]));
    }
    Ok(object([
        ("laneConnections", array(connections)),
        ("crosswalks", crosswalks(memory, &raw)?),
        ("doubleSlipSwitch", boolean(&raw, 0x48)?),
        ("trafficLightPreference", integer(i32_at(&raw, 0x4c))),
        (
            "trafficLightConfig",
            object([
                ("states", array(phases)),
                ("trafficLightType", integer(i32_at(&raw, 0x68))),
            ]),
        ),
        ("userModifiedTrafficLightStates", boolean(&raw, 0x70)?),
    ]))
}

pub fn decode(memory: &dyn Memory, payload: usize) -> Result<Option<LuaValue>, String> {
    let head = read(memory, payload, layout::HEAD_LEN, "junction proposal")?;
    // Only junction edits. Do not substitute a partial read for a road or
    // construction proposal which also happens to contain junction data.
    // Classify before applying junction limits: a station can generate
    // hundreds of node configs. No geometry elements are read or allocated
    // here; the ordinary capture path owns their validation and limits.
    for (offset, stride) in [
        (layout::ADDED_NODES, layout::NODE_SIZE),
        (layout::ADDED_SEGMENTS, layout::SEGMENT_SIZE),
        (layout::REMOVED_NODES, layout::NODE_SIZE),
        (layout::REMOVED_SEGMENTS, layout::SEGMENT_SIZE),
        (layout::EDGE_OBJECTS_TO_REMOVE, 4),
        (layout::EDGE_OBJECTS_TO_ADD, layout::EDGE_OBJECT_SIZE),
        (layout::TO_REMOVE, 4),
        (layout::TO_ADD, layout::ENTITY_SIZE),
    ] {
        if vector(
            &head,
            offset,
            stride,
            usize::MAX,
            "junction proposal geometry",
        )?
        .1 != 0
        {
            return Ok(None);
        }
    }
    let (begin, added) = vector(&head, 0x60, 0x80, 64, "junction configurations added")?;
    let (removed_at, removed) = vector(&head, 0x78, 4, 64, "junction configurations removed")?;
    if added == 0 && removed == 0 {
        return Ok(None);
    }
    let mut configs = Vec::new();
    for i in 0..added {
        configs.push(config(memory, begin + i * 0x80)?);
    }
    let removes = ids(
        memory,
        removed_at,
        removed,
        4,
        0,
        "junction configurations removed",
    )?;
    if removes.iter().any(|id| *id < 0) {
        return Err("a junction reset names a new node".into());
    }
    Ok(Some(object([
        ("junctionEdit", LuaValue::Boolean(true)),
        ("toAdd", array([])),
        ("toRemove", array([])),
        (
            "proposal",
            object([
                ("nodeConfigsToAdd", array(configs)),
                (
                    "nodeConfigsToRemove",
                    array(removes.into_iter().map(integer)),
                ),
            ]),
        ),
    ])))
}

pub fn record(memory: &dyn Memory, click: u64, payload: usize) {
    if !ENABLED.load(Ordering::Acquire) {
        return;
    }
    match decode(memory, payload) {
        Ok(Some(proposal)) => {
            crate::log::line(&format!("junction tool: captured click {click}"));
            modules::keep(click, Ok(proposal));
        }
        Ok(None) => (),
        Err(why) => {
            crate::log::line(&format!("junction tool: click {click} did not read: {why}"));
            modules::keep(click, Err(why));
        }
    }
}
