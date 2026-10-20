//! Static candidate for Steam Windows Preview 40418. No game is executed.
//! Set TPF3MP_TF3_PREVIEW_EXE to the archived executable to prove all targets.
//! An explicitly supplied wrong build fails; CI without private input checks
//! profile identity, target coverage and the native-bundle hold.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tpf3mp_hookcore::pe::PeHeaders;
use tpf3mp_hookcore::profile::{self, BuildIdentity, Profile};

const PREVIEW: &str = include_str!("../../../profiles/tf3_build40418_steam_windows/hooks.toml");
const RELEASE: &str = include_str!("../../../profiles/tf3_build40408_steam_windows/hooks.toml");

const REVIEWED_SITES: &[(&str, u64)] = &[
    ("view: ViewCreator::vf1/player", 0x86933a),
    ("view: CatchmentAreaHelper/player 1", 0x878845),
    ("view: CatchmentAreaHelper/player 2", 0x87917c),
    ("view: CatchmentAreaHelper/player 3", 0x879300),
    ("view: CatchmentAreaHelper/player 4", 0x879343),
];

// These signatures were measured only on Release 40408. Keep the Preview
// candidate's existing target list intact without claiming new
// Preview addresses that have not been checked against its private archive.
const RELEASE_ONLY_OPTIONAL_TARGETS: &[&str] = &[
    "simperf: EmissionGridSystem::Update",
    "simperf: EmissionEmitterSystem::Update2",
    "simperf: TownSystem::Update2",
    "simperf: UpdateParcelCollision",
    "simperf: UpdateParcelCollision call",
    "fast-component-index: Engine::GetComponentDataIndex",
    "emission::EmissionGridSystem::Update",
    // Faster saves (crates/tpf3mp-hook/src/savefast.rs) have verified
    // signatures only in the Release 40408 executable.
    "save: PushCompressor level load",
    "save: PushCompressor buffer size",
    // The batch interval's (crates/tpf3mp-hook/src/interval.rs): its field
    // layout is verified for the Release builds only.
    "CGame::Sync",
    "CGame::Step/Sync call",
];

fn coverage(profile: &Profile) -> BTreeMap<String, bool> {
    profile
        .targets
        .iter()
        .map(|target| (target.name.clone(), target.required))
        .collect()
}

#[test]
fn preview_pins_its_exact_identity_and_preserves_release_target_coverage() {
    let preview = Profile::from_toml(PREVIEW).unwrap();
    let release = Profile::from_toml(RELEASE).unwrap();
    assert_eq!(
        preview.build.sha256,
        "0017c15f267bc07d465cb7a3515f9b7af6cf5fe3e3a9c9ea6669496b16f30cc0"
    );
    assert_eq!(preview.build.size, Some(69_756_856));
    assert_eq!(preview.build.pe_timestamp, Some(1_790_974_087));
    assert_eq!(preview.targets.len(), 152);
    assert_eq!(
        release.build.sha256,
        "de1daad3a13f3b7e9f79903361bb43769cf4f15e59271a263aefe1f075f23ef2"
    );
    assert_eq!(release.build.size, Some(69_711_288));
    assert_eq!(release.build.pe_timestamp, Some(0x6AB6_9FE5));
    let preview_coverage = coverage(&preview);
    let release_coverage = coverage(&release);
    let release_only = release_coverage
        .keys()
        .filter(|name| !preview_coverage.contains_key(*name))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        release_only,
        RELEASE_ONLY_OPTIONAL_TARGETS.iter().copied().collect(),
        "only the explicitly allowlisted optional 40408 performance and batch-interval targets may be Release-only"
    );
    for (name, required) in &preview_coverage {
        assert_eq!(release_coverage.get(name), Some(required), "{name}");
    }
    for name in RELEASE_ONLY_OPTIONAL_TARGETS {
        assert_eq!(
            release_coverage.get(*name),
            Some(&false),
            "{name} must stay optional"
        );
    }
    assert!(preview.verify_identity(&release.build).is_err());
    assert!(release.verify_identity(&preview.build).is_err());
}

#[test]
fn release_only_targets_resolve_as_absent_optional_hooks() {
    let release = Profile::from_toml(RELEASE).unwrap();
    let mut performance_only = release.clone();
    performance_only
        .targets
        .retain(|target| RELEASE_ONLY_OPTIONAL_TARGETS.contains(&target.name.as_str()));
    assert_eq!(
        performance_only.targets.len(),
        RELEASE_ONLY_OPTIONAL_TARGETS.len()
    );

    let resolved = profile::resolve(&performance_only, &[], 0)
        .expect("missing optional performance targets must not refuse the profile");
    assert!(resolved.targets.is_empty());
    assert_eq!(
        resolved
            .absent_optional
            .into_iter()
            .collect::<BTreeSet<_>>(),
        RELEASE_ONLY_OPTIONAL_TARGETS
            .iter()
            .map(|name| (*name).to_owned())
            .collect()
    );
}

#[test]
fn a_signature_candidate_cannot_enable_unreviewed_preview_native_data() {
    let profiles = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    assert_ne!(
        std::fs::read_to_string(profiles.join("native-build.txt"))
            .unwrap()
            .trim(),
        "tf3_build40418_steam_windows"
    );
    assert!(
        !profiles
            .join("tf3_build40418_steam_windows/native.rs")
            .exists(),
        "Add reviewed ABI data and update this hold deliberately before selection"
    );
    let selected = tpf3mp_hookcore::bundle::Bundle::selected(&profiles).unwrap();
    let compiled = Profile::from_toml(
        &std::fs::read_to_string(selected.directory.join("hooks.toml")).unwrap(),
    )
    .unwrap();
    let preview = Profile::from_toml(PREVIEW).unwrap();
    assert!(compiled.verify_identity(&preview.build).is_err());
}

#[test]
fn every_preview_target_resolves_in_the_explicit_archived_executable() {
    let Some(exe) = std::env::var_os("TPF3MP_TF3_PREVIEW_EXE").map(PathBuf::from) else {
        eprintln!("no private Preview archive supplied; structural checks run in CI");
        return;
    };
    let preview = Profile::from_toml(PREVIEW).unwrap();
    let identity =
        BuildIdentity::of_file(&exe).expect("the explicit Preview input must be readable");
    preview
        .verify_identity(&identity)
        .expect("an explicit wrong build is a failed check, never a skipped test");
    let image = std::fs::read(&exe).unwrap();
    let pe = PeHeaders::parse(&image).unwrap();
    let section = pe.section(".text").unwrap();
    let text = section.raw(&image).unwrap();
    let base = u64::from(section.virtual_address);
    let resolved = profile::resolve(&preview, text, base).unwrap();
    assert!(resolved.absent_optional.is_empty());
    for target in &preview.targets {
        assert!(resolved.get(&target.name).is_some(), "{}", target.name);
    }
    for &(name, rva) in REVIEWED_SITES {
        assert_eq!(resolved.get(name).unwrap().address, rva, "{name}");
    }
    // The changed splice still follows r8d's +0x20c player read. Its new
    // rbp-0x60 stack read must not reuse the Release's rbp-0x70 bytes.
    let site = usize::try_from(0x878845 - base).unwrap();
    assert_eq!(
        &text[site - 7..site],
        &[0x44, 0x8B, 0x80, 0x0C, 0x02, 0x00, 0x00]
    );
    assert_eq!(
        &text[site..site + 8],
        &[0x48, 0x8B, 0x5D, 0xA0, 0x48, 0x8B, 0x53, 0x10]
    );
    assert_ne!(
        &text[site..site + 8],
        &[0x48, 0x8B, 0x5D, 0x90, 0x48, 0x8B, 0x53, 0x10]
    );
    let mut patched = identity;
    patched.sha256.replace_range(..2, "ff");
    assert!(preview.verify_identity(&patched).is_err());
}
