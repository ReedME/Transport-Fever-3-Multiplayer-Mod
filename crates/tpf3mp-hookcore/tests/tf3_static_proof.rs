//! Static proof of the Transport Fever 3 release profile.
//!
//! The profile always parses and names its build; when the executable is on
//! disk, every target resolves uniquely at the address found on release day,
//! and a modified copy is refused. The executable is READ-ONLY: this test only
//! reads it, and skips when it is absent (in CI). Set `TPF3MP_TF3_EXE` to
//! point it at the game elsewhere.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use tpf3mp_hookcore::pe::PeHeaders;
use tpf3mp_hookcore::profile::{self, BuildIdentity, Profile};

const DEFAULT_EXE: &str = r"F:\SteamLibrary\steamapps\common\Transport Fever 3\TransportFever3.exe";
const PROFILE: &str = include_str!("../../../profiles/tf3_build40408_steam_windows/hooks.toml");

/// Addresses found on release day (RVAs, image base 0x140000000).
const TARGETS: &[(&str, u64)] = &[
    ("GameSim::Step", 0x159390),
    ("CGame::Step", 0x11f3b0),
    // The batch interval (crates/tpf3mp-hook/src/interval.rs).
    ("CGame::Sync", 0x11f650),
    ("CGame::Step/Sync call", 0x11f406),
    ("CGameTime::GetSpeed", 0x2a95a0),
    ("GameSim::Step/GetSpeed call", 0x1593ee),
    ("UI::CMenuUI::StartSavegame", 0x6a2880),
    ("UI::CMenuUI::CreatePage", 0x6a2ee0),
    ("CommandList::Add::lambda", 0x9d23c0),
    ("CommandList::Add", 0x9d29c0),
    ("WorldBuildProposal apply", 0x9e1160),
    ("ModuleBuilder::MousePressed/Add call", 0x543b25),
    ("ProposalAction::DoApply/Add call", 0x549be5),
    // The stop tool (crates/tpf3mp-hook/src/stoptool.rs).
    ("StreetTerminalBuilder::MousePressed/Add call", 0x5954e8),
    ("StreetTerminalBuilder::MousePressed/busy set", 0x595305),
    ("StreetBuilder::UpdateEngine/Add call", 0x589ecf),
    ("StreetBuilder::UpdateEngine/capture", 0x589c85),
    ("StreetBuilder::UpdateEngine callback/result scan", 0x56e629),
    ("StreetBuilder::UpdateEngine callback/snap", 0x56e8e2),
    ("UI::StreetBuilder::StartAt", 0x571710),
    ("ecs::Engine::EntityExists", 0x4f7db0),
    ("BaseNodeConfig/field offsets", 0x1768337),
    ("StreetProposal/node configuration offsets", 0x22c395f),
    ("BaseNodeConfig/crosswalk set layout", 0xa4990d),
    ("luaB_print", 0x2fccd10),
    ("lua_checkstack", 0x2fbd650),
    ("lua_createtable", 0x2fbd880),
    ("lua_gettop", 0x2fbdd10),
    ("lua_next", 0x2fbe080),
    ("lua_pushboolean", 0x2fbe1d0),
    ("lua_pushcclosure", 0x2fbe1f0),
    ("lua_pushlstring", 0x2fbe340),
    ("lua_pushnil", 0x2fbe3a0),
    ("lua_pushnumber", 0x2fbe3c0),
    ("lua_pushvalue", 0x2fbe4b0),
    ("lua_rawget", 0x2fbe590),
    ("lua_rawgeti", 0x2fbe5d0),
    ("lua_rawset", 0x2fbe6b0),
    ("lua_settop", 0x2fbeaf0),
    ("lua_toboolean", 0x2fbecb0),
    ("lua_tolstring", 0x2fbed30),
    ("lua_tonumberx", 0x2fbedd0),
    ("lua_type", 0x2fbef90),
    ("lua_touserdata", 0x2fbef50),
    // The main menu's load (crates/tpf3mp-hook/src/menu.rs).
    ("UI::CMenuUI::DoStep", 0x6a0160),
    ("UI::CMenuUI::DoStep/m_game test", 0x6a01c0),
    ("UI::CMenuUI::DoStep/m_loadGameResult read", 0x6a0c84),
    ("lua_pcallk", 0x2fbe0c0),
    ("luaL_ref", 0x2fb40b0),
    ("lua_load", 0x2fbdf70),
    ("RegisterAppUsertypes", 0xdc5fa0),
    // The seeds and the order fixes (crates/tpf3mp-hook/src/seeds.rs, order.rs).
    ("ecs::LandVehicleMoveSystem::Update2/shuffle", 0xac1b70),
    ("ecs::LandVehicleMoveSystem::Update2/records", 0xac1d72),
    // The platform-order fix (crates/tpf3mp-hook/src/order.rs, `platform`).
    ("ecs::TransportVehicleSystem::Update2/visit", 0xb8bccb),
    ("FindNextFreeTerminal/candidate sort", 0xb85430),
    ("ecs::LineSystem::GetData/return", 0xad20f4),
    // The paused-tick fix (crates/tpf3mp-hook/src/ticks.rs).
    ("GameSim::Step/paused GameTime advance", 0x159412),
    ("CGameTime::Advance", 0xbace10),
    ("CGameTime::Advance/tick", 0xbace99),
    ("CGameTime::GetTickCount", 0x2a95c0),
    ("CGameTime::GetUpdateCount", 0x2a9680),
    (
        "ecs::SimEntityAtTerminalSystem::Update/vehicles at stop",
        0xb0e35c,
    ),
    (
        "ecs::TransportVehicleSystem::GetVehiclesAtLineStop",
        0xb86510,
    ),
    ("transport::EdgeReservationManager::Reserve", 0x255c2e0),
    (
        "transport::EdgeReservationManager::Reserve_simple",
        0x255c160,
    ),
    ("transport::EdgeUseManager::Add", 0x255e940),
    ("transport::EdgeUseManager::AddRange", 0x255cc70),
    ("ecs::Engine::Update", 0x2bb8a50),
    ("game_script_util::Update/lambda_1::_Do_call", 0xf45450),
    ("game_script_util::PostUpdate/lambda_1::_Do_call", 0xf449b0),
    (
        "game_script_util::HandleEvent/lambda_1/lambda_1::operator()",
        0xf41770,
    ),
    ("TownDevelopAt::Apply", 0x9dedf0),
    // The town street field's cache fix (crates/tpf3mp-hook/src/townfield.rs).
    ("StreetField::At", 0x2b76a90),
    ("StreetField::At/cache found", 0x2b76b4a),
    // The town trace (crates/tpf3mp-hook/src/towntrace.rs).
    ("TownUpdateSize::Apply", 0x9dfb10),
    ("TownUpdateSize::Apply/develop", 0x9dfc8a),
    ("TownUpdateSize::Apply/return", 0x9dfcd7),
    ("TownDeveloper::Develop", 0x8dc240),
    // The edge watch (crates/tpf3mp-hook/src/edgewatch.rs).
    ("CommandApply::One", 0x9e1c10),
    ("CommandApply::One/return", 0x9e1f62),
    // The street trace (crates/tpf3mp-hook/src/streettrace.rs).
    ("StreetDeveloper::TryCandidate", 0x967920),
    ("StreetDeveloper::TryCandidate/return", 0x9680dc),
    ("StreetDeveloper::Reject", 0x963320),
    ("StreetDeveloper::BuildStreet/errors", 0x9659ed),
    ("lua_getfield", 0x2fbdb90),
    ("lua_loadfile", 0x2fa1d50),
    // The probe of the engine's player (crates/tpf3mp-hook/src/probe.rs).
    ("probe: GUI GameState getter", 0x6aa800),
    ("probe: engine GameState getter", 0x11ffd0),
    ("probe: ProposalStreetGraph::GetPlayerOwnedPtr", 0xa46cd0),
    ("probe: street_util IsOwnedByOtherPlayer", 0x610ea0),
    // The tools' player (crates/tpf3mp-hook/src/toolplayer.rs).
    ("UI::StreetBuilder::Step", 0x585e50),
    ("UI::StreetBuilder ctor/player store", 0x56a7f4),
    ("UI::TrackModifier::Step", 0x5cbf80),
    ("UI::TrackModifier ctor/player store", 0x5b4238),
    ("UI::Bulldozer::Step", 0x4d6340),
    ("UI::Bulldozer ctor/filter", 0x4c4c32),
    ("UI::ConstructionBuilder::Step", 0x51cd60),
    ("UI::ConstructionBuilder ctor/player store", 0x50b438),
    ("UI::StreetTerminalBuilder::Step", 0x595f30),
    ("UI::StreetTerminalBuilder ctor/player store", 0x590773),
    ("UI::ModuleBuilder::Step", 0x545b50),
    ("UI::ModuleBuilder ctor/player store", 0x540431),
    ("UI::Bulldozer ctor/player store", 0x4c4a41),
    ("UI::Bulldozer ctor/owner list", 0x4c4ad5),
    ("UI::Bulldozer set owner list", 0x4d6220),
    // The views' player (crates/tpf3mp-hook/src/guiplayer.rs).
    (
        "view: HudIconManager::PreemptiveOctreeTraversal/player",
        0x67b7db,
    ),
    ("view: StationViewer::vf4/player", 0x83a608),
    ("view: CSelector pick/player", 0x839cd9),
    ("view: ViewCreator::vf1/player", 0x86712a),
    ("view: CatchmentAreaHelper/player 1", 0x8765f5),
    ("view: CatchmentAreaHelper/player 2", 0x876f36),
    ("view: CatchmentAreaHelper/player 3", 0x8770ba),
    ("view: CatchmentAreaHelper/player 4", 0x8770fd),
    ("view: CatchmentAreaHelper/owner test", 0x877b39),
    ("view: LayerManagerColorMap/player 1", 0x87b840),
    ("view: LayerManagerColorMap/player 2", 0x87b919),
    ("view: LayerManagerColorMap/player 3", 0x87b9ef),
    ("view: LayerManager colour lambda/player", 0x88307b),
    ("view: LayerManager colour/owner test", 0x885c82),
    ("view: react RendererComponentDelegate/player", 0x29f689a),
    ("view: react RailroadCrossingComp/player", 0x289e116),
    ("view: HudIconManager icon pass/owner", 0x674906),
    ("view: getPlayer binding/push", 0x24ed2d2),
    ("view: LineViewer lines of the player/call", 0x7f3f12),
    ("view: findBestDepot depot owner test", 0x2689eeb),
    ("view: findBestDepot owner test", 0x268a335),
    ("probe: StreetBulldozerAction edge test", 0x5f2a00),
    ("probe: bulldozer owner test", 0x5f7db0),
    ("probe: LineViewer route data test", 0x7f03e7),
    ("probe: LineViewer GetEdgeGeometries", 0x7f10a0),
    ("lua_cached_loadfile", 0x2fa8130),
    // The person-order fixes (crates/tpf3mp-hook/src/persons.rs).
    ("destination_util::GetTargetsByLandUse/candidates", 0x8e3d65),
    (
        "ecs::SimEntityAtBuildingSystem::Update2/leave batches",
        0xb05f92,
    ),
    ("ecs::PersonMoveSystem::Update2/arrival batch", 0xaecfcd),
    ("ecs::SimEntityNeedsPathSystem::Update/list", 0xb18213),
    (
        "ecs::SimEntityNeedsPathSystem::EntityToBeRemoved/data getter call",
        0xb18121,
    ),
    ("ecs::Engine::EndModification/free-id append", 0x2bb4fd1),
    // The other players' build previews (crates/tpf3mp-hook/src/drawing.rs).
    ("UI::RendererFactory::Create", 0x8266d0),
    ("UI::CRendererComponent::AddRenderable", 0x6ae970),
    ("UI::CRendererComponent::RemoveRenderable", 0x6afdf0),
    ("UI::BuilderRenderer::Clear", 0x7ba590),
    ("UI::BuilderRenderer::vf0", 0x7b89a0),
    ("builder_renderer_util::AddToRenderer", 0x5e2b20),
    ("CreateProposalData", 0xa1fd10),
    ("makeProposalData/CreateProposalData call", 0x25122da),
    ("UI::CGameUI::~CGameUI", 0x650470),
    ("UI::CMenuUI::StartGame/CGameUI store", 0x6a4f52),
    ("UI::CGameUI::CreateUI/RendererFactory field", 0x65be0c),
    ("UI::CGameUI::CreateUI/mainView store", 0x65b1bc),
    ("ProposalViewer/ModelData read", 0x2aa3b06),
    ("BuilderRenderer::EndHeightMod/upload flag", 0x7bbb6a),
    ("UI::BuilderRenderer::EndHeightMod", 0x7bbae0),
    ("terrain::ViewTerrain::ApplyBlocks", 0x396a00),
    ("UI::StreetBuilder::ResetProposal", 0x576330),
    // Simulation timers and the faster component lookup
    // (crates/tpf3mp-hook/src/simperf.rs, fastindex.rs).
    ("simperf: EmissionGridSystem::Update", 0xaa9230),
    ("simperf: EmissionEmitterSystem::Update2", 0xaa51c0),
    ("simperf: TownSystem::Update2", 0xb61cc0),
    ("simperf: UpdateParcelCollision", 0x9312e0),
    ("simperf: UpdateParcelCollision call", 0x25fcb9a),
    (
        "fast-component-index: Engine::GetComponentDataIndex",
        0xa4b90,
    ),
    // The fused emission grid (crates/tpf3mp-hook/src/emission).
    ("emission::EmissionGridSystem::Update", 0xaa9230),
    // Faster saves (crates/tpf3mp-hook/src/savefast.rs).
    ("save: PushCompressor level load", 0x32d464),
    ("save: PushCompressor buffer size", 0x32d5c4),
];

#[test]
fn the_release_profile_parses_and_pins_build_40408() {
    let profile = Profile::from_toml(PROFILE).unwrap();
    assert_eq!(
        profile.build.sha256,
        "de1daad3a13f3b7e9f79903361bb43769cf4f15e59271a263aefe1f075f23ef2"
    );
    assert_eq!(profile.build.size, Some(69_711_288));
    assert_eq!(profile.targets.len(), TARGETS.len());
}

#[test]
fn every_target_resolves_uniquely_in_the_installed_game() {
    let exe = std::env::var_os("TPF3MP_TF3_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EXE));
    if !exe.exists() {
        eprintln!(
            "skipping: {} is not present (expected in CI)",
            exe.display()
        );
        return;
    }
    let profile = Profile::from_toml(PROFILE).unwrap();
    let identity = BuildIdentity::of_file(&exe).unwrap();
    if profile.verify_identity(&identity).is_err() {
        eprintln!("skipping: {} is another build", exe.display());
        return;
    }
    let image = std::fs::read(&exe).unwrap();
    let pe = PeHeaders::parse(&image).unwrap();
    let text = pe.section(".text").expect("a .text section");
    let text_bytes = text.raw(&image).expect(".text raw bytes");
    let base = u64::from(text.virtual_address);

    let resolved = profile::resolve(&profile, text_bytes, base).unwrap();
    assert!(resolved.absent_optional.is_empty());
    for &(name, rva) in TARGETS {
        let target = resolved
            .get(name)
            .unwrap_or_else(|| panic!("{name} did not resolve"));
        assert_eq!(target.address, rva, "{name} resolved to the wrong RVA");
    }

    // The paused-tick fix (crates/tpf3mp-hook/src/ticks.rs): the paused
    // path's call and the running loop's both reach the advance, the paused
    // one with r8b = 0 and the running one with r8b = 1, and the advance
    // counts tickCount always and updateCount only when r8b is set.
    let at = |rva: u64| usize::try_from(rva - base).unwrap();
    let callee = |site: u64| {
        let i = at(site);
        assert_eq!(text_bytes[i], 0xE8, "a call at {site:#x}");
        let rel = i32::from_le_bytes(text_bytes[i + 1..i + 5].try_into().unwrap());
        (site as i64 + 5 + i64::from(rel)) as u64
    };
    assert_eq!(callee(0x159412), 0xbace10);
    assert_eq!(callee(0x15954b), 0xbace10);
    assert_eq!(&text_bytes[at(0x159405)..at(0x159408)], &[0x45, 0x33, 0xC0]);
    assert_eq!(&text_bytes[at(0x15953e)..at(0x159541)], &[0x41, 0xB0, 0x01]);
    assert_eq!(
        &text_bytes[at(0xbace99)..at(0xbace99) + 11],
        &[
            0xFF, 0x47, 0x3C, 0x40, 0x84, 0xED, 0x74, 0x03, 0xFF, 0x47, 0x40
        ]
    );
    // The platform-order fix: the visit loop asks FindNextFreeTerminal,
    // whose candidate sort follows the candidate site.
    assert_eq!(callee(0xb8bea1), 0xb84e20);
    assert_eq!(callee(0xb85453), 0xb76b30);
    // The road-entry fix: Add appends in place (`add qword [rcx+8], 0x14`).
    assert_eq!(
        &text_bytes[at(0x255ea6d)..at(0x255ea6d) + 5],
        &[0x48, 0x83, 0x41, 0x08, 0x14]
    );
    // The field fix: the open pass asks StreetField::At for the street's
    // end; At's found test jumps to its miss path, which computes the
    // answer and inserts it with 0x2b76660; the only branch to the site is
    // the lookup loop's.
    assert_eq!(callee(0x967e0e), 0x2b76a90);
    assert_eq!(callee(0x2b76c6d), 0x2b76660);
    assert_eq!(&text_bytes[at(0x2b76b4f)..at(0x2b76b4f) + 2], &[0x75, 0x35]);
    assert_eq!(0x2b76b51 + 0x35, 0x2b76b86);
    assert_eq!(&text_bytes[at(0x2b76b1c)..at(0x2b76b1c) + 2], &[0x75, 0x2C]);
    // The person-order fixes (crates/tpf3mp-hook/src/persons.rs).
    // candidates: the leave handler reaches GetTargetsByLandUse through the
    // destination assignment and GetRandomTargets, whose draw is
    // BinarySearchIndex's; the vector at the site is the one the copy loop
    // filled (`lea rbx, [rsp+0x70]`).
    assert_eq!(callee(0xb2ec5f), 0xb2b040);
    assert_eq!(callee(0x8e37d8), 0x8e3ac0);
    assert_eq!(callee(0x8e24ae), 0x8e4400);
    assert_eq!(
        &text_bytes[at(0x8e3cd9)..at(0x8e3cd9) + 5],
        &[0x48, 0x8D, 0x5C, 0x24, 0x70]
    );
    // departures and arrivals: their emits all reach one signal function;
    // the thread-pool loop the departures come from is the call before
    // their site; the leave handler and the arrival handler draw from a
    // generator seeded by updateCount.
    for site in [0xb05fad, 0xb05fd1, 0xaecfe8] {
        assert_eq!(callee(site), 0xaeaeb0, "{site:#x} emits the batch");
    }
    assert_eq!(callee(0xb05f8d), 0xb052c0);
    assert_eq!(callee(0xb2e9d2), 0x2a9680);
    assert_eq!(callee(0xb33abc), 0x2a9680);
    // needs-path: Update is PathFactory::Compute's caller, its results loop
    // re-reads the list by position, and the getter call reaches the
    // copy-on-write getter.
    assert_eq!(callee(0xb1833a), 0x8d11e0);
    assert_eq!(
        &text_bytes[at(0xb183c0)..at(0xb183c0) + 11],
        &[
            0x49, 0x8B, 0x45, 0x10, 0x48, 0x8B, 0x08, 0x42, 0x8B, 0x1C, 0xA1
        ]
    );
    assert_eq!(callee(0xb18121), 0xb186c0);
    assert_eq!(callee(0xb17d0a), 0xb186c0);
    // freed ids: the call after the site is the free-id deque's insert.
    assert_eq!(callee(0x2bb4ff3), 0x2bb1110);
    // The free-id trace (crates/tpf3mp-hook/src/persons.rs, freed_ids::
    // trace). That insert has no other caller: EndModification is the only
    // code that appends to the free-id queue.
    let inserts: Vec<u64> = (0..text_bytes.len().saturating_sub(5))
        .filter(|&i| matches!(text_bytes[i], 0xE8 | 0xE9))
        .filter(|&i| {
            let rel = i32::from_le_bytes(text_bytes[i + 1..i + 5].try_into().unwrap());
            (base as i64 + i as i64 + 5 + i64::from(rel)) as u64 == 0x2bb1110
        })
        .map(|i| base + i as u64)
        .collect();
    assert_eq!(inserts, vec![0x2bb4ff3]);
    // EndModification's head, 0x241 bytes before the site: three stores
    // into the caller's home slots, five pushes and `sub rsp, 0x100`, so
    // the return address is 0x128 above the site's rsp.
    assert_eq!(0x2bb4fd1 - 0x2bb4d90, 0x241);
    assert_eq!(
        &text_bytes[at(0x2bb4d90)..at(0x2bb4d90) + 36],
        &[
            0x48, 0x89, 0x5C, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x48, 0x89, 0x7C, 0x24,
            0x20, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8D, 0x6C, 0x24,
            0xC9, 0x48, 0x81, 0xEC, 0x00, 0x01, 0x00, 0x00
        ]
    );
    // `mov r13, rcx; lea r12, [rcx+0x1f0]`: r13 is the engine at the site.
    assert_eq!(
        &text_bytes[at(0x2bb4dc2)..at(0x2bb4dc2) + 10],
        &[0x4C, 0x8B, 0xE9, 0x4C, 0x8D, 0xA1, 0xF0, 0x01, 0x00, 0x00]
    );
    // AddEntity pops the queue's front when its size (+0xf8) is not 0,
    // reading the map (+0xe0), its size (+0xe8) and the offset (+0xf0) in
    // blocks of four; else the new id is the entity table's (+0x90) size
    // in 0x18-byte rows.
    assert_eq!(
        &text_bytes[at(0x2bb37fe)..at(0x2bb37fe) + 7],
        &[0x4C, 0x8B, 0x87, 0xF8, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        &text_bytes[at(0x2bb39ed)..at(0x2bb39ed) + 0x29],
        &[
            0x48, 0x8B, 0x97, 0xF0, 0x00, 0x00, 0x00, 0x48, 0x8B, 0x8F, 0xE8, 0x00, 0x00, 0x00,
            0x48, 0xFF, 0xC9, 0x48, 0x8B, 0xC2, 0x48, 0xC1, 0xE8, 0x02, 0x48, 0x23, 0xC8, 0x83,
            0xE2, 0x03, 0x48, 0x8B, 0x87, 0xE0, 0x00, 0x00, 0x00, 0x48, 0x8B, 0x0C, 0xC8
        ]
    );
    assert_eq!(
        &text_bytes[at(0x2bb3810)..at(0x2bb3810) + 7],
        &[0x4C, 0x8D, 0x97, 0x90, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        &text_bytes[at(0x2bb3821)..at(0x2bb3821) + 10],
        &[0x48, 0xB8, 0xAB, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0x2A]
    );
    // The step keeps `this` in rbp and hands ecs::Engine::Update the
    // engine at [[this+8]+0x18].
    assert_eq!(
        &text_bytes[at(0x15939b)..at(0x15939b) + 3],
        &[0x48, 0x8B, 0xE9]
    );
    assert_eq!(
        &text_bytes[at(0x159550)..at(0x159550) + 13],
        &[
            0x48, 0x8B, 0x4D, 0x08, 0xC5, 0xF8, 0x28, 0xCE, 0x48, 0x8B, 0x49, 0x18, 0xE8
        ]
    );
    assert_eq!(callee(0x15955c), 0x2bb8a50);
    assert_eq!(
        resolved.get("ecs::Engine::Update").map(|t| t.address),
        Some(0x2bb8a50)
    );
    // The town trace: the applier calls Develop between its two sites, and
    // reads updateCount for the seed through its getter.
    assert_eq!(callee(0x9dfccb), 0x8dc240);
    assert_eq!(callee(0x9dfbf0), 0x2a9680);
    // The edge watch: every path into CommandApply::One the log names
    // reaches it (the sim loop's drain, CGame's two send lambdas, and
    // GameState's command function by a tail jump), and One reads the
    // payload's kind at +0x9b8 right before it calls the dispatcher.
    for site in [0x11eb96, 0x120334, 0x1204bf] {
        assert_eq!(callee(site), 0x9e1c10, "{site:#x} calls One");
    }
    {
        let i = at(0x268ed4);
        assert_eq!(text_bytes[i], 0xE9, "a tail jump at 0x268ed4");
        let rel = i32::from_le_bytes(text_bytes[i + 1..i + 5].try_into().unwrap());
        assert_eq!((0x268ed4_i64 + 5 + i64::from(rel)) as u64, 0x9e1c10);
    }
    assert_eq!(
        &text_bytes[at(0x9e1cbf)..at(0x9e1cbf) + 8],
        &[0x49, 0x0F, 0xBE, 0x88, 0xB8, 0x09, 0x00, 0x00]
    );
    assert_eq!(callee(0x9e1cce), 0x9d7350);
    // The street trace: Develop runs the street step, which tries each
    // candidate; the try refuses through the reject function at its three
    // sites, builds through 0x9692c0 and 0x9657c0, and the errors site
    // follows the build's call of CreateProposalData.
    assert_eq!(callee(0x8dca13), 0x967720);
    assert_eq!(callee(0x96785e), 0x967920);
    for site in [0x967bd5, 0x967c5f, 0x968035] {
        assert_eq!(
            callee(site),
            0x963320,
            "{site:#x} calls the reject function"
        );
    }
    assert_eq!(callee(0x96801b), 0x9692c0);
    assert_eq!(callee(0x9695b4), 0x9657c0);
    assert_eq!(callee(0x9659e7), 0xa1fd10);
    // The land-vehicle shuffle's seed is the tickCount getter's answer.
    assert_eq!(callee(0xac1b23), 0x2a95c0);
    // The main menu's m_game test reads CMenuUI+0x6b0, the field StartGame
    // asserts clear (`cmp [rax], r15` on `lea rax, [rcx+0x6b0]`).
    assert_eq!(
        &text_bytes[at(0x6a01c0)..at(0x6a01c0) + 7],
        &[0x4C, 0x39, 0xAE, 0xB0, 0x06, 0x00, 0x00]
    );
    assert_eq!(
        &text_bytes[at(0x6a3662)..at(0x6a3662) + 7],
        &[0x48, 0x8D, 0x81, 0xB0, 0x06, 0x00, 0x00]
    );
    // The m_loadGameResult read is of CMenuUI+0x1bd0, the field StartGame
    // checks before asserting "!m_loadGameResult.Valid()".
    assert_eq!(
        &text_bytes[at(0x6a0c84)..at(0x6a0c84) + 7],
        &[0x48, 0x8B, 0x9E, 0xD0, 0x1B, 0x00, 0x00]
    );
    assert_eq!(
        &text_bytes[at(0x6a3676)..at(0x6a3676) + 7],
        &[0x48, 0x8B, 0x81, 0xD0, 0x1B, 0x00, 0x00]
    );
    // Simulation timers: the parcel walk's only call reaches it, and each
    // system's vtable slot (.rdata, the file's pointers at the preferred
    // base) holds its function; nothing in .text calls them directly.
    assert_eq!(callee(0x25fcb9a), 0x9312e0);
    let systems = [
        (0x36fcb10u64, 0xaa9230u64),
        (0x36fc490, 0xaa51c0),
        (0x3708bb8, 0xb61cc0),
    ];
    let rdata = pe.section(".rdata").expect("an .rdata section");
    let rdata_bytes = rdata.raw(&image).expect(".rdata raw bytes");
    for (slot, function) in systems {
        let i = usize::try_from(slot - u64::from(rdata.virtual_address)).unwrap();
        let pointer = u64::from_le_bytes(rdata_bytes[i..i + 8].try_into().unwrap());
        assert_eq!(pointer, 0x1_4000_0000 + function, "slot {slot:#x}");
    }
    for i in 0..text_bytes.len().saturating_sub(5) {
        if text_bytes[i] != 0xE8 && text_bytes[i] != 0xE9 {
            continue;
        }
        let rel = i32::from_le_bytes(text_bytes[i + 1..i + 5].try_into().unwrap());
        let reached = (base as i64 + i as i64 + 5 + i64::from(rel)) as u64;
        assert!(
            systems.iter().all(|&(_, function)| reached != function),
            "{reached:#x} is reached directly from {:#x}",
            base + i as u64
        );
    }

    // A required target's bytes changed: resolution fails closed.
    let mut tampered = text_bytes.to_vec();
    let at = usize::try_from(0x159390 - base).unwrap();
    tampered[at] ^= 0xff;
    assert!(profile::resolve(&profile, &tampered, base).is_err());
}
