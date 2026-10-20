//! Big Maps' package against the real game, build 40420, without running
//! it: its hooks found in the executable's code by the hook's own
//! `plugins::locate`, arranged against every target the hook's own 40420
//! profile patches (`plugins::arrange`), and the plugin's entry run with a
//! host that answers from the executable's bytes at their preferred
//! addresses, with every switch on. Every check the plugin makes in the
//! game is made here, on the same bytes.
//!
//! The executable is optional: set `TPF3MP_TF3_BUILD40420_EXE` to it (as
//! `tpf3mp-hookcore`'s `tf3_build40420_profile` test does). Without it the
//! test says so and passes; with another build it fails.

#![allow(clippy::unwrap_used, unsafe_code)]

use std::{
    ffi::c_void,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use tpf3mp_bigmap_plugin::{build40420::BUILD, manifest};
use tpf3mp_hook::plugins::{self, Found};
use tpf3mp_hookcore::{
    pe::PeHeaders,
    profile::{self, Profile},
};
use tpf3mp_nativemods::{enabled::EnabledPackage, index::HookKind};
use tpf3mp_plugin_abi as abi;

const EXE_ENV: &str = "TPF3MP_TF3_BUILD40420_EXE";
const PROFILE: &str = include_str!("../../../profiles/tf3_build40420_steam_windows/hooks.toml");

/// The executable, loaded as the game would be at its preferred base:
/// each section's bytes at `image_base + virtual_address`.
struct Image {
    base: usize,
    sections: Vec<(usize, Vec<u8>)>,
}

impl Image {
    fn read(&self, address: usize, out: &mut [u8]) -> bool {
        self.sections.iter().any(|(start, bytes)| {
            let Some(from) = address.checked_sub(*start) else {
                return false;
            };
            match bytes.get(from..from + out.len()) {
                Some(found) => {
                    out.copy_from_slice(found);
                    true
                }
                None => false,
            }
        })
    }
}

static IMAGE: OnceLock<Image> = OnceLock::new();

/// What the plugin asked for, and what it could see.
#[derive(Default)]
struct Asked {
    found: Vec<Found>,
    attached: Vec<String>,
    spliced: Vec<String>,
    rewritten: Vec<(String, Vec<u8>)>,
    lua: usize,
    lines: Vec<String>,
}

static ASKED: Mutex<Option<Asked>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Asked) -> R) -> R {
    f(ASKED.lock().unwrap().as_mut().unwrap())
}

unsafe fn text<'a>(ptr: *const u8, len: usize) -> &'a str {
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(ptr, len) }).unwrap()
}

unsafe extern "C" fn log(_: *mut c_void, line: *const u8, len: usize) {
    let line = unsafe { text(line, len) }.to_owned();
    with(|a| a.lines.push(line));
}

unsafe extern "C" fn attach(
    _: *mut c_void,
    target: *const u8,
    len: usize,
    _: *const c_void,
    _: *mut *const c_void,
) -> i32 {
    let target = unsafe { text(target, len) }.to_owned();
    with(|a| a.attached.push(target));
    abi::OK
}

unsafe extern "C" fn splice(
    _: *mut c_void,
    target: *const u8,
    len: usize,
    _: abi::SpliceFn,
    _: *mut c_void,
) -> i32 {
    let target = unsafe { text(target, len) }.to_owned();
    with(|a| a.spliced.push(target));
    abi::OK
}

unsafe extern "C" fn rewrite(
    _: *mut c_void,
    target: *const u8,
    len: usize,
    bytes: *const u8,
    bytes_len: usize,
) -> i32 {
    let target = unsafe { text(target, len) }.to_owned();
    let bytes = unsafe { std::slice::from_raw_parts(bytes, bytes_len) }.to_vec();
    with(|a| a.rewritten.push((target, bytes)));
    abi::OK
}

unsafe extern "C" fn lua(_: *mut c_void, _: *const u8, _: usize) -> i32 {
    with(|a| a.lua += 1);
    abi::OK
}

unsafe extern "C" fn address(_: *mut c_void, target: *const u8, len: usize) -> *const c_void {
    let target = unsafe { text(target, len) };
    with(|a| {
        a.found
            .iter()
            .find(|f| f.target == target)
            .map_or(std::ptr::null(), |f| f.address as *const c_void)
    })
}

unsafe extern "C" fn read(address: *const c_void, out: *mut u8, len: usize) -> i32 {
    let out = unsafe { std::slice::from_raw_parts_mut(out, len) };
    if IMAGE.get().unwrap().read(address as usize, out) {
        abi::OK
    } else {
        abi::error::UNREADABLE
    }
}

unsafe extern "C" fn update_count() -> i64 {
    -1
}

unsafe extern "C" fn service_log(line: *const u8, len: usize) {
    unsafe { log(std::ptr::null_mut(), line, len) };
}

static SERVICES: abi::Services = abi::Services {
    read,
    update_count,
    log: service_log,
};

#[test]
fn big_maps_finds_and_checks_every_site_in_build_40420() {
    let Some(path) = std::env::var_os(EXE_ENV).map(PathBuf::from) else {
        eprintln!("{EXE_ENV} is not set: Big Maps' sites are not checked against the game");
        return;
    };
    let exe = std::fs::read(&path).unwrap();
    assert_eq!(
        tpf3mp_nativemods::signed::sha256_hex(&exe),
        BUILD,
        "{} is not build 40420",
        path.display()
    );
    let pe = PeHeaders::parse(&exe).unwrap();
    let base = pe.image_base as usize;
    let image = Image {
        base,
        sections: pe
            .sections
            .iter()
            .map(|s| {
                (
                    base + s.virtual_address as usize,
                    s.raw(&exe).unwrap().to_vec(),
                )
            })
            .collect(),
    };
    let text = pe.section(".text").unwrap();
    let code = text.raw(&exe).unwrap();
    let start = image.base + text.virtual_address as usize;
    let _ = IMAGE.set(image);
    let pointer = |at: usize| {
        let mut bytes = [0u8; 8];
        IMAGE
            .get()
            .unwrap()
            .read(at, &mut bytes)
            .then(|| u64::from_le_bytes(bytes) as usize)
    };

    // Found by the hook's own code, and clear of every patch the hook makes.
    let package = EnabledPackage {
        id: manifest::ID.into(),
        version: "0.1.0".into(),
        simulation: true,
        features: Vec::new(),
        settings: Default::default(),
        files_sha256: tpf3mp_nativemods::terms::files_digest(&[]),
        identity_sha256: String::new(),
        depends: Vec::new(),
        root: PathBuf::from("native-mods/bigmap/0.1.0"),
        plugins: Vec::new(),
        hooks: manifest::hooks(),
    };
    let found = plugins::locate(&package, code, start, &pointer).unwrap();
    assert_eq!(found.len(), package.hooks.len());
    let core: Vec<usize> = profile::resolve(
        &Profile::from_toml(PROFILE).unwrap(),
        code,
        u64::from(text.virtual_address),
    )
    .unwrap()
    .targets
    .iter()
    .map(|t| base + t.address as usize)
    .collect();
    plugins::arrange(found.clone(), &core).unwrap();
    for hook in &found {
        eprintln!("{:28} {:?} at {:#x}", hook.target, hook.kind, hook.address);
    }

    // The plugin's entry, with every switch on.
    *ASKED.lock().unwrap() = Some(Asked {
        found,
        ..Asked::default()
    });
    let settings = r#"{"street_raster": true, "placement": true, "emission_every": 4, "emission_cell": 32, "memory_gate": true}"#;
    let root = "native-mods/bigmap/0.1.0";
    let host = abi::Host {
        abi: abi::ABI,
        ctx: std::ptr::null_mut(),
        root: root.as_ptr(),
        root_len: root.len(),
        settings: settings.as_ptr(),
        settings_len: settings.len(),
        log,
        attach,
        address,
        splice,
        rewrite,
        lua,
        services: &raw const SERVICES,
    };
    let status = unsafe { tpf3mp_bigmap_plugin::tpf3mp_plugin_init(&raw const host) };
    let asked = ASKED.lock().unwrap().take().unwrap();
    for line in &asked.lines {
        eprintln!("{line}");
    }
    assert_eq!(status, abi::OK);
    assert!(
        asked.lines.iter().all(|line| !line.contains(": off,")),
        "every patch is on: {:#?}",
        asked.lines
    );
    let mut attached = asked.attached.clone();
    attached.sort();
    assert_eq!(
        attached,
        [
            "bigmap::descent",
            "bigmap::emitter_slot",
            "bigmap::grid_slot",
            "bigmap::obstacle",
            "bigmap::spacing"
        ]
    );
    let mut spliced = asked.spliced.clone();
    spliced.sort();
    assert_eq!(
        spliced,
        ["bigmap::decoder", "bigmap::root_init", "bigmap::root_load"]
    );
    assert_eq!(asked.rewritten.len(), 5, "four shifts and the factor");
    assert!(asked.rewritten[..4].iter().all(|(_, bytes)| bytes[2] == 3));
    assert_eq!(asked.lua, 1);
    // Every hook the package declares is one the plugin uses or checks.
    for hook in manifest::hooks() {
        let used = asked.attached.contains(&hook.target)
            || asked.spliced.contains(&hook.target)
            || asked.rewritten.iter().any(|(t, _)| *t == hook.target)
            || hook.kind == HookKind::Find;
        assert!(used, "{} is declared but not used", hook.target);
    }
}
