//! Writes Big Maps' package: its folder, as the launcher installs it, and
//! the `tpf3mp-native.json` a registry reads (docs/NATIVE_MODS.md, "A
//! server's own index").
//!
//! ```text
//! cargo build -p tpf3mp-bigmap-plugin --release
//! cargo run -p tpf3mp-bigmap-plugin --bin tpf3mp-bigmap-package -- \
//!     <version> <the built tpf3mp_bigmap_plugin.dll> <base address of the released files> <out folder>
//! ```
//!
//! The base address is where the folder's files will be downloaded from,
//! each at `<base>/<its path in the folder>`, such as a tag's raw files
//! (`https://raw.githubusercontent.com/<owner>/<repo>/v0.1.0/package`).

use std::{fs, path::Path};

use tpf3mp_bigmap_plugin::manifest::{self, Released};

fn files_under(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files_under(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [version, dll, base, out] = &args[..] else {
        return Err(
            "usage: tpf3mp-bigmap-package <version> <dll> <base address> <out folder>".into(),
        );
    };
    let base = base.trim_end_matches('/');
    let package_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("package");
    let out = Path::new(out);
    let error = |e: std::io::Error| e.to_string();

    let mut paths = Vec::new();
    files_under(
        &package_dir,
        &package_dir.join(manifest::GAME_MOD),
        &mut paths,
    )
    .map_err(error)?;
    paths.sort();
    let mut bytes = Vec::new();
    for path in &paths {
        let content = fs::read(package_dir.join(path)).map_err(error)?;
        let target = out.join(path);
        fs::create_dir_all(target.parent().unwrap_or(out)).map_err(error)?;
        fs::write(&target, &content).map_err(error)?;
        bytes.push(content);
    }
    let dll_bytes = fs::read(dll).map_err(error)?;
    let plugin_target = out.join(manifest::PLUGIN_PATH);
    fs::create_dir_all(plugin_target.parent().unwrap_or(out)).map_err(error)?;
    fs::write(&plugin_target, &dll_bytes).map_err(error)?;

    let files: Vec<Released<'_>> = paths
        .iter()
        .zip(&bytes)
        .map(|(path, bytes)| Released {
            path: path.clone(),
            bytes,
            url: format!("{base}/{path}"),
        })
        .collect();
    let plugin = Released {
        path: manifest::PLUGIN_PATH.into(),
        bytes: &dll_bytes,
        url: format!("{base}/{}", manifest::PLUGIN_PATH),
    };
    let package = manifest::package(version, &plugin, &files, None);
    let file = tpf3mp_nativemods::registry::ModFile {
        format: 1,
        packages: vec![package],
    };
    // What the server reads checks it as this does.
    let index = tpf3mp_nativemods::index::Index {
        format: 1,
        serial: 1,
        packages: file.packages.clone(),
        listings: Default::default(),
    };
    tpf3mp_nativemods::index::Index::parse(&serde_json::to_vec(&index).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    fs::write(out.join("tpf3mp-native.json"), json).map_err(error)?;
    println!(
        "wrote {} with {} files, the plugin and {} hooks",
        out.join("tpf3mp-native.json").display(),
        paths.len(),
        file.packages[0].hooks.len()
    );
    Ok(())
}
