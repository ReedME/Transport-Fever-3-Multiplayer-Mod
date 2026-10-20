//! The level crossing window's proposal, read from checked native memory.
#![allow(clippy::unwrap_used)]

use tpf3mp_hook::{
    crossings::{
        Change, ToolType, arm, decode, decode_tool, disarm, fill, is_crossing_window, layout,
        record_tool, set_call, set_layout, take_tool,
    },
    modules::{Memory, layout as street},
    terrain::Heap,
};

struct Image(Vec<u8>);
impl Memory for Image {
    fn read(&self, address: usize, len: usize) -> Option<Vec<u8>> {
        Some(self.0.get(address..address.checked_add(len)?)?.to_vec())
    }
}
impl Heap for Image {
    fn write(&mut self, address: usize, bytes: &[u8]) -> bool {
        match self.0.get_mut(address..address + bytes.len()) {
            Some(at) => {
                at.copy_from_slice(bytes);
                true
            }
            None => false,
        }
    }
    fn allocate(&mut self, _len: usize) -> Option<usize> {
        None
    }
}
impl Image {
    fn word(&mut self, at: usize, n: u64) {
        self.0[at..at + 8].copy_from_slice(&n.to_le_bytes());
    }
    fn int(&mut self, at: usize, n: i32) {
        self.0[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    /// A map at `offset` of the proposal whose list's sentinel is at
    /// `sentinel`, holding `pairs` in nodes 0x20 apart after it.
    fn map(&mut self, offset: usize, sentinel: usize, pairs: &[(i32, i32)]) {
        self.word(PROPOSAL + offset + layout::MAP_HEAD, sentinel as u64);
        self.word(PROPOSAL + offset + layout::MAP_SIZE, pairs.len() as u64);
        let nodes: Vec<usize> = (0..pairs.len())
            .map(|i| sentinel + 0x20 * (i + 1))
            .collect();
        let ring: Vec<usize> = std::iter::once(sentinel)
            .chain(nodes.iter().copied())
            .collect();
        for (i, at) in ring.iter().enumerate() {
            let next = ring[(i + 1) % ring.len()];
            let prev = ring[(i + ring.len() - 1) % ring.len()];
            self.word(at + layout::NODE_NEXT, next as u64);
            self.word(at + layout::NODE_PREV, prev as u64);
        }
        for (at, (key, value)) in nodes.iter().zip(pairs) {
            self.int(at + layout::NODE_KEY, *key);
            self.int(at + layout::NODE_VALUE, *value);
        }
    }
}

const PROPOSAL: usize = 0x100;
const NODE_MAP: usize = 0x800;
const CROSSING_MAP: usize = 0x900;

/// The window's proposal for an existing crossing: nothing but the crossing
/// mapped to its new type.
fn window(pairs: &[(i32, i32)]) -> Image {
    let mut m = Image(vec![0; 0x1000]);
    m.map(layout::NODE_TYPES, NODE_MAP, &[]);
    m.map(layout::CROSSING_TYPES, CROSSING_MAP, pairs);
    m
}

#[test]
fn the_window_s_type_change_reads_as_the_crossing_and_its_type() {
    assert_eq!(
        decode(&window(&[(51_234, 5)]), PROPOSAL),
        Ok(Change::Crossing {
            entity: 51_234,
            kind: 5
        })
    );
}

const SEGMENTS: usize = 0x1000;

/// The window's rebuild: `removed` segments (entities) removed and as many
/// laid again, the crossing's nodes mapped to `types`.
fn rebuild(removed: &[i32], added: usize, types: &[(i32, i32)]) -> Image {
    let mut m = window(&[]);
    m.0.resize(SEGMENTS + 16 * street::SEGMENT_SIZE, 0);
    m.map(layout::NODE_TYPES, NODE_MAP, types);
    let at = SEGMENTS;
    for (i, e) in removed.iter().enumerate() {
        m.int(at + i * street::SEGMENT_SIZE, *e);
    }
    let end = (at + removed.len() * street::SEGMENT_SIZE) as u64;
    m.word(PROPOSAL + street::REMOVED_SEGMENTS, at as u64);
    m.word(PROPOSAL + street::REMOVED_SEGMENTS + 8, end);
    m.word(PROPOSAL + street::REMOVED_SEGMENTS + 16, end);
    let laid = SEGMENTS + 8 * street::SEGMENT_SIZE;
    let end = (laid + added * street::SEGMENT_SIZE) as u64;
    m.word(PROPOSAL + street::ADDED_SEGMENTS, laid as u64);
    m.word(PROPOSAL + street::ADDED_SEGMENTS + 8, end);
    m.word(PROPOSAL + street::ADDED_SEGMENTS + 16, end);
    m
}

/// A window rebuild maps nodes to the type while laying segments and may
/// reset junction settings. The type-only replay cannot prove it preserves
/// those changes, so even a well-formed rebuild is refused.
#[test]
fn the_window_s_rebuild_is_refused_even_with_a_matching_segment_count() {
    // This has the observed valid shape: two segments laid again, the
    // crossing's nodes mapped to one type, and junctions at segment ends.
    let mut m = rebuild(&[900, 901], 2, &[(700, 5), (701, 5)]);
    let configs = 0xe00;
    m.int(configs + layout::CONFIG_NODE, 701);
    m.int(configs + layout::CONFIG_SIZE + layout::CONFIG_NODE, 650);
    let end = (configs + 2 * layout::CONFIG_SIZE) as u64;
    m.word(PROPOSAL + layout::CONFIGS_ADDED, configs as u64);
    m.word(PROPOSAL + layout::CONFIGS_ADDED + 8, end);
    m.word(PROPOSAL + layout::CONFIGS_ADDED + 16, end);
    let resets = 0xf40;
    m.int(resets, 650);
    m.word(PROPOSAL + layout::CONFIGS_REMOVED, resets as u64);
    m.word(PROPOSAL + layout::CONFIGS_REMOVED + 8, resets as u64 + 4);
    m.word(PROPOSAL + layout::CONFIGS_REMOVED + 16, resets as u64 + 4);
    assert_eq!(
        decode(&m, PROPOSAL),
        Err("a crossing rebuild with segments or junctions is not carried".to_owned())
    );
    assert_eq!(
        decode(&rebuild(&[900, 901], 2, &[(700, 5), (701, 5)]), PROPOSAL),
        Err("a crossing rebuild with segments or junctions is not carried".to_owned())
    );
    // Junctions without a rebuild are not the window's.
    let mut m = window(&[(51_234, 5)]);
    m.word(PROPOSAL + layout::CONFIGS_REMOVED, resets as u64);
    m.word(PROPOSAL + layout::CONFIGS_REMOVED + 8, resets as u64 + 4);
    m.word(PROPOSAL + layout::CONFIGS_REMOVED + 16, resets as u64 + 4);
    assert_eq!(
        decode(&m, PROPOSAL),
        Err("a crossing change that also changes junctions".to_owned())
    );
    for m in [
        rebuild(&[900, 901], 1, &[(700, 5), (701, 5)]),
        rebuild(&[], 0, &[(700, 5)]),
        rebuild(&[900], 1, &[(700, 5), (701, 6)]),
        rebuild(&[-1], 1, &[(700, 5)]),
        rebuild(&[900], 1, &[]),
    ] {
        assert!(decode(&m, PROPOSAL).is_err());
    }
    // Both maps at once.
    let mut m = rebuild(&[900], 1, &[(700, 5)]);
    m.map(layout::CROSSING_TYPES, CROSSING_MAP, &[(51_234, 5)]);
    assert_eq!(
        decode(&m, PROPOSAL),
        Err("a crossing change in both of the proposal's maps".to_owned())
    );
    // A node added.
    let mut m = rebuild(&[900], 1, &[(700, 5)]);
    m.word(PROPOSAL + street::ADDED_NODES, 0xe00);
    m.word(
        PROPOSAL + street::ADDED_NODES + 8,
        0xe00 + street::NODE_SIZE as u64,
    );
    m.word(
        PROPOSAL + street::ADDED_NODES + 16,
        0xe00 + street::NODE_SIZE as u64,
    );
    assert_eq!(
        decode(&m, PROPOSAL),
        Err("a crossing change that also changes nodes".to_owned())
    );
}

#[test]
fn anything_but_one_crossing_and_type_is_refused() {
    assert_eq!(
        decode(&window(&[]), PROPOSAL),
        Err("a crossing change that changes no crossing".to_owned())
    );
    assert_eq!(
        decode(&window(&[(1, 2), (3, 4)]), PROPOSAL),
        Err("a change of more than one crossing".to_owned())
    );
    assert_eq!(
        decode(&window(&[(-1, 2)]), PROPOSAL),
        Err("a crossing or type that is not one".to_owned())
    );
    assert_eq!(
        decode(&window(&[(1, -1)]), PROPOSAL),
        Err("a crossing or type that is not one".to_owned())
    );
}

#[test]
fn a_map_that_does_not_walk_to_its_size_is_refused() {
    // A size the list does not have.
    let mut m = window(&[(51_234, 5)]);
    m.word(PROPOSAL + layout::CROSSING_TYPES + layout::MAP_SIZE, 2);
    assert!(
        decode(&m, PROPOSAL)
            .unwrap_err()
            .contains("shorter than its size")
    );
    let mut m = window(&[(51_234, 5)]);
    m.word(PROPOSAL + layout::CROSSING_TYPES + layout::MAP_SIZE, 0);
    assert!(
        decode(&m, PROPOSAL)
            .unwrap_err()
            .contains("longer than its size")
    );
    // A back link that does not answer.
    let mut m = window(&[(51_234, 5)]);
    m.word(CROSSING_MAP + 0x20 + layout::NODE_PREV, 0x1230);
    assert!(
        decode(&m, PROPOSAL)
            .unwrap_err()
            .contains("does not answer")
    );
    // A next link out of the image.
    let mut m = window(&[(51_234, 5)]);
    m.word(CROSSING_MAP + 0x20 + layout::NODE_NEXT, 0x10_0000);
    assert!(decode(&m, PROPOSAL).is_err());
    // No list at all, and a size past any crossing.
    let mut m = window(&[(51_234, 5)]);
    m.word(PROPOSAL + layout::CROSSING_TYPES + layout::MAP_HEAD, 0);
    assert!(decode(&m, PROPOSAL).unwrap_err().contains("no list"));
    let mut m = window(&[(51_234, 5)]);
    m.word(
        PROPOSAL + layout::CROSSING_TYPES + layout::MAP_SIZE,
        1 << 40,
    );
    assert!(
        decode(&m, PROPOSAL)
            .unwrap_err()
            .contains("more than a crossing has")
    );
    // The same key twice.
    let m = window(&[(7, 5), (7, 6)]);
    assert!(decode(&m, PROPOSAL).unwrap_err().contains("a key twice"));
}

#[test]
fn only_the_window_s_own_call_is_read_as_a_crossing_change() {
    set_call(0x1_428a_386c, true);
    assert!(is_crossing_window(0x1_428a_3871));
    assert!(!is_crossing_window(0x1_428a_386c));
    assert!(!is_crossing_window(0));
}

/// The game's insert as the fakes do it: a node at `node` keyed
/// `key_written`, linked before the map's sentinel, its value 0; answers
/// the node.
fn inserting(
    node: usize,
    key_written: i32,
) -> impl FnMut(&mut dyn Heap, usize, i32) -> Result<usize, String> {
    move |heap: &mut dyn Heap, map: usize, _key: i32| {
        let sentinel = u64::from_le_bytes(heap.read(map + 8, 8).unwrap().try_into().unwrap());
        let mut put = |at: usize, bytes: &[u8]| assert!(heap.write(at, bytes));
        put(node + layout::NODE_NEXT, &sentinel.to_le_bytes());
        put(node + layout::NODE_PREV, &sentinel.to_le_bytes());
        put(node + layout::NODE_KEY, &key_written.to_le_bytes());
        put(node + layout::NODE_VALUE, &0i32.to_le_bytes());
        let sentinel = sentinel as usize;
        put(sentinel + layout::NODE_NEXT, &(node as u64).to_le_bytes());
        put(sentinel + layout::NODE_PREV, &(node as u64).to_le_bytes());
        put(map + layout::MAP_SIZE, &1u64.to_le_bytes());
        Ok(node)
    }
}

#[test]
fn the_room_s_carrier_gets_the_crossing_and_its_type_as_the_window_s_build() {
    let mut m = window(&[]);
    fill(&mut m, &mut inserting(0xc00, 51_234), PROPOSAL, 51_234, 5).unwrap();
    assert_eq!(
        decode(&m, PROPOSAL),
        Ok(Change::Crossing {
            entity: 51_234,
            kind: 5
        }),
        "as the window's own"
    );
}

#[test]
fn a_carrier_with_anything_in_it_or_a_wrong_insert_is_refused() {
    let mut m = window(&[(7, 1)]);
    let why = fill(&mut m, &mut inserting(0xc00, 51_234), PROPOSAL, 51_234, 5).unwrap_err();
    assert_eq!(why, "the carrier's crossing map is not empty");
    let mut m = window(&[]);
    m.map(layout::NODE_TYPES, NODE_MAP, &[(700, 5)]);
    let why = fill(&mut m, &mut inserting(0xc00, 51_234), PROPOSAL, 51_234, 5).unwrap_err();
    assert!(why.starts_with("the carrier is not empty"), "{why}");
    // An insert that keyed another crossing.
    let mut m = window(&[]);
    let why = fill(&mut m, &mut inserting(0xc00, 99), PROPOSAL, 51_234, 5).unwrap_err();
    assert_eq!(why, "the carrier's crossing map does not hold the crossing");
}

#[test]
fn arming_needs_a_crossing_a_type_and_the_game_s_insert() {
    assert!(arm(-1, 5).is_err());
    assert!(arm(5, -1).is_err());
    // This test binary's hook found no insert in a game.
    assert_eq!(
        arm(51_234, 5),
        Err("the profile does not lead to this build's crossing map".to_owned())
    );
    assert_eq!(disarm(), None);
}

/// A street or track tool's proposal: its crossing nodes mapped as the tool
/// maps them (`pairs`), nothing in the window's map.
fn tool(pairs: &[(i32, i32)]) -> Image {
    let mut m = Image(vec![0; 0x1000]);
    m.map(layout::NODE_TYPES, NODE_MAP, pairs);
    m.map(layout::CROSSING_TYPES, CROSSING_MAP, &[]);
    m
}

/// The tool maps each crossing node of its polyline to -2 - rcType (build
/// 40420, UI::StreetBuilder): that reads as the type; nothing mapped (an
/// automatic choice, or no crossing) as none, -1 entries ignored.
#[test]
fn the_tool_s_crossing_type_reads_as_one_type_or_none() {
    assert_eq!(
        decode_tool(&tool(&[(-5, -7), (-6, -7), (712, -7)]), PROPOSAL),
        Ok(ToolType::Type(5))
    );
    assert_eq!(
        decode_tool(&tool(&[(-5, -2)]), PROPOSAL),
        Ok(ToolType::Type(0))
    );
    assert_eq!(decode_tool(&tool(&[]), PROPOSAL), Ok(ToolType::None));
    assert_eq!(
        decode_tool(&tool(&[(-5, -1)]), PROPOSAL),
        Ok(ToolType::None)
    );
    assert_eq!(
        decode_tool(&tool(&[(-5, -1), (-6, -9)]), PROPOSAL),
        Ok(ToolType::Type(7))
    );
}

/// A forced type (the crossing window's encoding), types that differ, or a
/// map that does not walk are refused, never read as a type.
#[test]
fn a_forced_mixed_or_broken_tool_map_is_refused() {
    assert_eq!(
        decode_tool(&tool(&[(-5, 5)]), PROPOSAL),
        Err("a build that forces a level crossing's type".to_owned())
    );
    assert_eq!(
        decode_tool(&tool(&[(-5, -7), (-6, -8)]), PROPOSAL),
        Err("a build that gives its level crossings different types".to_owned())
    );
    let mut m = tool(&[(-5, -7)]);
    m.word(PROPOSAL + layout::NODE_TYPES + layout::MAP_SIZE, 2);
    assert!(
        decode_tool(&m, PROPOSAL)
            .unwrap_err()
            .contains("shorter than its size")
    );
    assert!(
        decode_tool(&tool(&[(7, -7), (7, -7)]), PROPOSAL)
            .unwrap_err()
            .contains("a key twice")
    );
}

/// Each click's reading is kept for the GUI and taken once; without the
/// profile's maps it is unknown, not none.
#[test]
fn the_tool_s_reading_is_kept_by_click_and_taken_once() {
    // Clicks far from any other test's.
    set_layout(false);
    record_tool(&tool(&[(-5, -7)]), 7_000_001, PROPOSAL);
    assert_eq!(take_tool(7_000_001), Some(Ok(ToolType::Unknown)));
    set_layout(true);
    record_tool(&tool(&[(-5, -7)]), 7_000_002, PROPOSAL);
    record_tool(&tool(&[(-5, 5)]), 7_000_003, PROPOSAL);
    assert_eq!(take_tool(7_000_002), Some(Ok(ToolType::Type(5))));
    assert_eq!(take_tool(7_000_002), None, "taken once");
    assert!(take_tool(7_000_003).unwrap().is_err());
    assert_eq!(take_tool(7_000_004), None, "nothing kept");
}
