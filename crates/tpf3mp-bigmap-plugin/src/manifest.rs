//! Big Maps' package: what its `tpf3mp-native.json` says (docs/NATIVE_MODS.md,
//! "Plugins"): the hooks its plugin uses on build 40420, each found by the
//! signature Big Maps' hook profile used (`profiles/tf3_build40420_steam_windows/hooks.toml`
//! on `feat/bigmaps-40420`) and checked against the bytes read there; its
//! settings and their defaults; its plugin and its game mod's files.
//! [`package`] makes the whole package; the `tpf3mp-bigmap-package`
//! binary writes it.

use std::collections::BTreeMap;

use tpf3mp_nativemods::{
    index::{
        FileEntry, HookDecl, HookKind, HookUse, PLUGIN_ABI, Package, Plugin, Setting, SlotDecl,
    },
    signed::sha256_hex,
};

use crate::build40420::*;

/// The package's id.
pub const ID: &str = "bigmap";
/// Its game mod, among its files.
pub const GAME_MOD: &str = "mod/tpf3mp_bigmap_1";
/// Where its plugin is, in its folder.
pub const PLUGIN_PATH: &str = "plugin/tpf3mp_bigmap.dll";

// The signatures Big Maps' profile finds its targets by.
const RESIZE_SIG: &str = "48 89 5C 24 10 57 48 83 EC 30 C5 F8 29 74 24 20 C5 F8 28 F2";
const ROOT_INIT_SIG: &str = "81 7D 04 80 00 00 00 7E 16 C5 FA 10 15 ?? ?? ?? ?? 48 8B 4E 38 BA 0A 00 00 00 E8 ?? ?? ?? ?? C5 FA 10 55 20";
const ROOT_LOAD_SIG: &str = "81 7E 04 80 00 00 00 7E 16 C5 FA 10 15 ?? ?? ?? ?? 48 8B 4D 38 BA 0A 00 00 00 E8 ?? ?? ?? ?? C5 FA 10 56 20";
/// From a root signature's match to its site (the profile's target).
const ROOT_SITE_OFFSET: i64 = 9;
const OBSTACLE_SIG: &str =
    "48 89 4C 24 08 53 48 83 EC 20 48 8B D9 48 8D 05 ?? ?? ?? ?? 48 89 01 C5 F8 10 02";
const SPACING_SIG: &str = "4C 8B DC 4D 89 4B 20 49 89 4B 08 57 41 55";
const DESCENT_SIG: &str = "48 8B C4 48 89 58 18 55 56 57 41 54 41 55 41 56 41 57 48 8D A8 E8 FE FF FF 48 81 EC E0 01 00 00 C5 F8 29 70 B8 C5 F8 29 78 A8 C5 78 29 40 98 C5 78 29 48 88 C5 78 29 90 78 FF FF FF 48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 85 80 00 00 00 45 8B D1";
const DESCENT_START_SIG: &str = "4C 8B DC 53 48 81 EC E0 00 00 00 C4 C1 78 29 73 E8";
const DECODER_SIG: &str =
    "45 33 C9 4C 89 64 24 38 41 B8 01 00 00 00 4C 89 7C 24 20 45 8B E1 41 3B D0 7C 23";
const GRID_UPDATE_SIG: &str = "48 8B C4 48 89 58 18 48 89 50 10 48 89 48 08 55 56 57 41 54 41 55 41 56 41 57 48 8D 68 A1 48 81 EC 00 01 00 00 C5 F8 29 70 B8 C5 F8 28 F3";
const GRID_VTABLE_SIG: &str =
    "48 8B F2 48 8B D9 33 ED 48 8D 05 ?? ?? ?? ?? 48 89 01 48 C7 44 24 48 FF FF FF FF 8D 4D 18";
const EMITTER_UPDATE_SIG: &str = "C5 FA 11 5C 24 20 55 53 56 57 41 54 41 55 41 56 41 57 48 8D 6C 24 98 48 81 EC 68 01 00 00 48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 45 58 44 89 44 24 78";
const EMITTER_VTABLE_SIG: &str = "48 89 5C 24 20 48 89 4C 24 08 55 56 57 48 83 EC 30 48 8B F9 33 ED 48 89 69 08 48 89 69 10 48 8D 05 ?? ?? ?? ?? 48 89 01 48 89 51 18";
const CREATE_GRID_SIG: &str =
    "48 89 5C 24 18 48 89 4C 24 08 55 56 57 41 56 41 57 48 81 EC 80 00 00 00";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn hook(target: &str, kind: HookKind, signature: &str, offset: i64, expected: &[u8]) -> HookDecl {
    HookDecl {
        target: target.into(),
        kind,
        signature: signature.into(),
        offset,
        expected: hex(expected),
        access: HookUse::Chain,
        steal: None,
        replacement: None,
        slot: None,
    }
}

/// The hooks Big Maps' plugin uses on build 40420.
pub fn hooks() -> Vec<HookDecl> {
    let splice = |target: &str, signature: &str, offset: i64, stolen: &[u8]| HookDecl {
        steal: Some(stolen.len() as u8),
        ..hook(target, HookKind::Splice, signature, offset, stolen)
    };
    let slot = |target: &str, signature: &str, offset: i64, lea: &[u8], index: usize| HookDecl {
        slot: Some(SlotDecl {
            disp_at: LEA_RAX_RIP.len() as u8,
            length: 7,
            index: index as u16,
        }),
        ..hook(target, HookKind::Slot, signature, offset, lea)
    };
    let body =
        |offset: usize, len: usize| &CREATE_GRID_BODY[offset - CREATE_GRID_BODY_OFFSET..][..len];
    let mut hooks = vec![
        // The octree: Resize is called, the two root sites spliced.
        hook(
            RESIZE,
            HookKind::Find,
            RESIZE_SIG,
            0,
            &[
                0x48, 0x89, 0x5C, 0x24, 0x10, 0x57, 0x48, 0x83, 0xEC, 0x30, 0xC5, 0xF8, 0x29, 0x74,
                0x24, 0x20,
            ],
        ),
        splice(
            ROOT_INIT,
            ROOT_INIT_SIG,
            ROOT_SITE_OFFSET + ROOT_NEXT_OFFSET as i64,
            &INIT_SITE.next,
        ),
        splice(
            ROOT_LOAD,
            ROOT_LOAD_SIG,
            ROOT_SITE_OFFSET + ROOT_NEXT_OFFSET as i64,
            &LOAD_SITE.next,
        ),
        // Depth 12: the descent detoured, its starter checked, the decoder
        // spliced.
        hook(
            DESCENT,
            HookKind::Detour,
            DESCENT_SIG,
            0,
            &[
                0x48, 0x8B, 0xC4, 0x48, 0x89, 0x58, 0x18, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55,
            ],
        ),
        hook(
            DESCENT_START,
            HookKind::Find,
            DESCENT_START_SIG,
            0,
            &[
                0x4C, 0x8B, 0xDC, 0x53, 0x48, 0x81, 0xEC, 0xE0, 0x00, 0x00, 0x00, 0xC4, 0xC1, 0x78,
                0x29, 0x73, 0xE8,
            ],
        ),
        splice(DECODER, DECODER_SIG, 19, &DECODER_STOLEN),
        // The street raster's constructor.
        hook(
            OBSTACLE,
            HookKind::Detour,
            OBSTACLE_SIG,
            0,
            &[
                0x48, 0x89, 0x4C, 0x24, 0x08, 0x53, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0xD9,
            ],
        ),
        // The placement score, replaced.
        HookDecl {
            access: HookUse::Replace,
            ..hook(
                SPACING,
                HookKind::Detour,
                SPACING_SIG,
                0,
                &[
                    0x4C, 0x8B, 0xDC, 0x4D, 0x89, 0x4B, 0x20, 0x49, 0x89, 0x4B, 0x08, 0x57, 0x41,
                    0x55,
                ],
            )
        },
        // The emission throttle: two vtable slots, each checked to hold
        // its system's update.
        hook(
            GRID_UPDATE,
            HookKind::Find,
            GRID_UPDATE_SIG,
            0,
            &[
                0x48, 0x8B, 0xC4, 0x48, 0x89, 0x58, 0x18, 0x48, 0x89, 0x50, 0x10, 0x48, 0x89, 0x48,
                0x08,
            ],
        ),
        slot(
            GRID_VTABLE,
            GRID_VTABLE_SIG,
            8,
            &[0x48, 0x8D, 0x05, 0xB8, 0xCA, 0xC5, 0x02],
            GRID_SLOT,
        ),
        hook(
            EMITTER_UPDATE2,
            HookKind::Find,
            EMITTER_UPDATE_SIG,
            0,
            &[
                0xC5, 0xFA, 0x11, 0x5C, 0x24, 0x20, 0x55, 0x53, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55,
            ],
        ),
        slot(
            EMITTER_VTABLE,
            EMITTER_VTABLE_SIG,
            30,
            &[0x48, 0x8D, 0x05, 0xEB, 0x04, 0xC6, 0x02],
            EMITTER_SLOT,
        ),
        // The emission cells: the function checked, its four shifts and
        // its factor load rewritten.
        hook(
            CREATE_GRID,
            HookKind::Find,
            CREATE_GRID_SIG,
            0,
            &[
                0x48, 0x89, 0x5C, 0x24, 0x18, 0x48, 0x89, 0x4C, 0x24, 0x08, 0x55, 0x56, 0x57, 0x41,
                0x56,
            ],
        ),
    ];
    for (name, offset) in CREATE_GRID_SHIFT_HOOKS.iter().zip(CREATE_GRID_SHIFTS) {
        hooks.push(hook(
            name,
            HookKind::Rewrite,
            CREATE_GRID_SIG,
            offset as i64,
            body(offset, 3),
        ));
    }
    hooks.push(hook(
        CREATE_GRID_FACTOR_HOOK,
        HookKind::Rewrite,
        CREATE_GRID_SIG,
        CREATE_GRID_FACTOR_LOAD as i64,
        body(CREATE_GRID_FACTOR_LOAD, 8),
    ));
    hooks
}

/// The package's settings and their defaults: every switch off but the
/// memory gate, so a room that sets none runs only the octree root, which
/// stock-sized worlds never reach.
pub fn settings() -> BTreeMap<String, Setting> {
    BTreeMap::from([
        ("street_raster".to_owned(), Setting::Bool(false)),
        ("placement".to_owned(), Setting::Bool(false)),
        ("emission_every".to_owned(), Setting::Int(1)),
        ("emission_cell".to_owned(), Setting::Int(16)),
        ("memory_gate".to_owned(), Setting::Bool(true)),
    ])
}

/// A file of the package as released: its path in the package, its bytes
/// and where it is downloaded from.
pub struct Released<'a> {
    pub path: String,
    pub bytes: &'a [u8],
    pub url: String,
}

fn entry(file: &Released<'_>) -> FileEntry {
    FileEntry {
        path: file.path.clone(),
        url: file.url.clone(),
        size: file.bytes.len() as u64,
        sha256: sha256_hex(file.bytes),
    }
}

/// The package of `version`, with its plugin (the Windows DLL) and its game
/// mod's files as released.
pub fn package(
    version: &str,
    plugin: &Released<'_>,
    files: &[Released<'_>],
    homepage: Option<String>,
) -> Package {
    let plugin = entry(plugin);
    Package {
        id: ID.into(),
        version: version.into(),
        name: "Big Maps".into(),
        description: "Bigger worlds: size rows up to 45 km on the New Game page, a deeper octree \
                      root for worlds past 65 km, and switches for the street raster, town \
                      spacing and emissions on very large maps."
            .into(),
        homepage,
        authors: vec!["silver2127".into()],
        preview: None,
        simulation: true,
        builds: vec![BUILD.into()],
        features: Vec::new(),
        settings: settings(),
        depends: Vec::new(),
        conflicts: Vec::new(),
        files: files.iter().map(entry).collect(),
        plugins: vec![Plugin {
            path: plugin.path,
            url: plugin.url,
            size: plugin.size,
            sha256: plugin.sha256,
            abi: PLUGIN_ABI,
            platform: "windows-x86_64".into(),
        }],
        hooks: hooks(),
        game_mods: vec![GAME_MOD.into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_is_one_the_index_accepts() {
        let dll = b"MZ the plugin";
        let page = b"-- page";
        let package = package(
            "0.1.0",
            &Released {
                path: PLUGIN_PATH.into(),
                bytes: dll,
                url: "https://example.com/tpf3mp_bigmap.dll".into(),
            },
            &[Released {
                path: format!("{GAME_MOD}/content/gui/menu/new_game_or_map_settings_page.tl"),
                bytes: page,
                url: "https://example.com/page.tl".into(),
            }],
            Some("https://github.com/silver2127/tpf3mp-bigmap".into()),
        );
        let index = tpf3mp_nativemods::index::Index {
            format: 1,
            serial: 1,
            packages: vec![package],
            listings: Default::default(),
        };
        let json = serde_json::to_vec(&index).unwrap();
        let parsed = tpf3mp_nativemods::index::Index::parse(&json).unwrap();
        assert_eq!(parsed.packages[0].hooks.len(), 18);
    }

    #[test]
    fn the_hooks_name_the_targets_the_plugin_asks_for() {
        let names: Vec<String> = hooks().into_iter().map(|h| h.target).collect();
        for name in [
            RESIZE,
            ROOT_INIT,
            ROOT_LOAD,
            DESCENT,
            DESCENT_START,
            DECODER,
            OBSTACLE,
            SPACING,
            GRID_UPDATE,
            GRID_VTABLE,
            EMITTER_UPDATE2,
            EMITTER_VTABLE,
            CREATE_GRID,
            CREATE_GRID_FACTOR_HOOK,
        ]
        .into_iter()
        .chain(CREATE_GRID_SHIFT_HOOKS)
        {
            assert!(names.iter().any(|n| n == name), "{name}");
        }
    }
}
