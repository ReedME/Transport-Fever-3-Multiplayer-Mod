//! Verified mod installation for the portable Windows package under Wine/Proton.
//! Native Windows keeps its readable installer; both paths check the same output.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ring::digest::{Context as Digest, SHA256};

const MOD: &str = "tpf3mp_1";
const RECORD: &str = "installed.json";

pub(crate) fn wine() -> bool {
    #[cfg(windows)]
    #[allow(unsafe_code)]
    {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let name: Vec<u16> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
        // SAFETY: querying an already loaded module with terminated names.
        unsafe {
            GetProcAddress(
                GetModuleHandleW(name.as_ptr()),
                c"wine_get_version".as_ptr().cast(),
            )
            .is_some()
        }
    }
    #[cfg(not(windows))]
    false
}

/// Query failure refuses maintenance, just as finding a running game does.
pub(crate) fn check_names(names: Result<Vec<String>>, extra: Option<&str>) -> Result<()> {
    for name in
        names.context("cannot check whether Transport Fever 3 is closed; no files changed")?
    {
        if [
            "TransportFever3.exe",
            "Transport Fever 3.exe",
            "TransportFever3",
            "Transport Fever 3",
        ]
        .iter()
        .any(|game| name.eq_ignore_ascii_case(game))
            || extra.is_some_and(|game| name.eq_ignore_ascii_case(game))
        {
            bail!("Close Transport Fever 3 first: {name} is running.");
        }
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn game_closed() -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES, GetLastError, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    let extra = tpf3mp_agent::steam::find_game(None).and_then(|game| {
        tpf3mp_launch::find_executable(&game.dir).and_then(|exe| {
            exe.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
    });
    // SAFETY: a process-list snapshot, with its handle closed on every outcome.
    let names = unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            Err(std::io::Error::last_os_error()).context("enumerating game processes")
        } else {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize =
                u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX);
            let mut names = Vec::new();
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                names.push(String::from_utf16_lossy(&entry.szExeFile[..end]));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            let error = GetLastError();
            CloseHandle(snapshot);
            if error == ERROR_NO_MORE_FILES {
                Ok(names)
            } else {
                bail!("process enumeration failed ({error})")
            }
        }
    };
    check_names(names, extra.as_deref())
}

#[cfg(not(windows))]
pub(crate) fn game_closed() -> Result<()> {
    check_names(
        Err(anyhow::anyhow!(
            "The Windows package's maintenance requires the Windows launcher."
        )),
        None,
    )
}

type Tree = BTreeMap<PathBuf, Option<(u64, Vec<u8>)>>;

/// Links and special files are refused in both input and installed trees.
fn tree(root: &Path) -> Result<Tree> {
    fn walk(root: &Path, relative: &Path, out: &mut Tree) -> Result<()> {
        let at = root.join(relative);
        let metadata =
            fs::symlink_metadata(&at).with_context(|| format!("reading {}", at.display()))?;
        if metadata.is_symlink() {
            bail!("mod contains a symbolic link: {}", at.display());
        }
        if metadata.is_dir() {
            if !relative.as_os_str().is_empty() {
                out.insert(relative.to_owned(), None);
            }
            for entry in fs::read_dir(at)? {
                walk(root, &relative.join(entry?.file_name()), out)?;
            }
        } else if metadata.is_file() {
            let mut file = fs::File::open(&at)?;
            let mut digest = Digest::new(&SHA256);
            let mut buffer = [0; 32 * 1024];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
            out.insert(
                relative.to_owned(),
                Some((metadata.len(), digest.finish().as_ref().to_vec())),
            );
        } else {
            bail!("mod contains a special file: {}", at.display());
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, Path::new(""), &mut out)?;
    Ok(out)
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let expected = tree(source)?;
    fs::create_dir_all(target)?;
    for (relative, file) in &expected {
        let destination = target.join(relative);
        if file.is_none() {
            fs::create_dir_all(destination)?;
        } else {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source.join(relative), destination)?;
        }
    }
    if tree(target)? != expected {
        bail!("copied mod differs from its source");
    }
    Ok(())
}

fn record(data: &Path) -> Result<(String, PathBuf)> {
    if fs::symlink_metadata(data.join(RECORD))?.is_symlink() {
        bail!("mod installation record is a symbolic link");
    }
    let text = fs::read_to_string(data.join(RECORD)).context("reading mod installation record")?;
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    let version = json["version"]
        .as_str()
        .context("mod record has no version")?
        .to_owned();
    let target = PathBuf::from(json["mod"].as_str().context("mod record has no path")?);
    if !target.is_absolute()
        || target.file_name().is_none_or(|name| name != MOD)
        || target
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|name| name != "staging_area")
        || target.components().any(|part| part == Component::ParentDir)
    {
        bail!("mod installation record names something other than TPF3-MP");
    }
    Ok((version, target))
}

pub(crate) fn recorded_target(data: &Path) -> Result<PathBuf> {
    record(data).map(|(_, target)| target)
}

pub(crate) fn verify(root: &Path, data: &Path, mods: &Path, version: &str) -> Result<()> {
    let (installed, target) = record(data)?;
    if installed != version || fs::canonicalize(&target)? != fs::canonicalize(mods.join(MOD))? {
        bail!("installer did not record the expected mod and version");
    }
    let source = root.join("mod").join(MOD);
    if !source.join("mod.json").is_file() {
        bail!("package has no Transport Fever 3 mod");
    }
    if tree(&source)? != tree(&target)? {
        bail!("installed mod differs from this package; repair installation");
    }
    Ok(())
}

/// A successful script exit is insufficient (Wine's PowerShell stub returns 0).
pub(crate) fn checked_script_install(
    root: &Path,
    data: &Path,
    mods: &Path,
    version: &str,
    run: impl FnOnce() -> Result<()>,
) -> Result<()> {
    run()?;
    verify(root, data, mods, version).context("verifying the installer's output")
}

pub(crate) fn install(root: &Path, data: &Path, mods: &Path, version: &str) -> Result<()> {
    install_with(root, data, mods, version, game_closed, verify)
}

fn install_with(
    root: &Path,
    data: &Path,
    mods: &Path,
    version: &str,
    mut closed: impl FnMut() -> Result<()>,
    confirm: impl FnOnce(&Path, &Path, &Path, &str) -> Result<()>,
) -> Result<()> {
    closed()?;
    if !mods.is_absolute() || mods.file_name().is_none_or(|name| name != "staging_area") {
        bail!("choose the game's full staging_area path");
    }
    let source = root.join("mod").join(MOD);
    if !source.join("mod.json").is_file() {
        bail!("package has no Transport Fever 3 mod");
    }
    let _ = tree(&source)?;
    if data.join(RECORD).exists() {
        let _ = record(data)?;
    }
    fs::create_dir_all(mods)?;
    fs::create_dir_all(data.join("backups"))?;
    let backups = tempfile::Builder::new()
        .prefix("mod-")
        .tempdir_in(data.join("backups"))?
        .keep();
    let staged = tempfile::Builder::new()
        .prefix(".tpf3mp-install-")
        .tempdir_in(mods)?;
    copy_tree(&source, staged.path())?;
    let mod_hold = tempfile::Builder::new()
        .prefix(".tpf3mp-previous-")
        .tempdir_in(mods)?;
    let record_hold = tempfile::Builder::new()
        .prefix(".tpf3mp-record-")
        .tempdir_in(data)?;
    let target = mods.join(MOD);
    let record_path = data.join(RECORD);
    // Back up everything before the last process check, so even a long copy
    // cannot separate that check from the swap of the live files.
    if target.exists() {
        copy_tree(&target, &backups.join(MOD))?;
    }
    if record_path.exists() {
        fs::copy(&record_path, backups.join(RECORD))?;
    }
    let (mut moved_mod, mut moved_record, mut new_mod, mut new_record) =
        (false, false, false, false);
    let result = (|| {
        closed()?;
        if target.exists() {
            fs::rename(&target, mod_hold.path().join(MOD))?;
            moved_mod = true;
        }
        if record_path.exists() {
            fs::rename(&record_path, record_hold.path().join(RECORD))?;
            moved_record = true;
        }
        fs::rename(staged.path(), &target)?;
        new_mod = true;
        let json =
            serde_json::to_vec_pretty(&serde_json::json!({ "version": version, "mod": target }))?;
        new_record = true;
        fs::write(&record_path, json)?;
        confirm(root, data, mods, version)
    })();
    if let Err(error) = result {
        let rollback: Result<()> = (|| {
            if new_mod {
                fs::remove_dir_all(&target)?;
            }
            if moved_mod {
                fs::rename(mod_hold.path().join(MOD), &target)?;
            }
            if (new_record || moved_record) && record_path.is_file() {
                fs::remove_file(&record_path)?;
            }
            if moved_record {
                fs::rename(record_hold.path().join(RECORD), &record_path)?;
            }
            Ok(())
        })();
        if let Err(undo) = rollback {
            bail!(
                "mod install failed: {error:#}; rollback failed: {undo:#}; backups: {}",
                backups.display()
            );
        }
        return Err(error.context("mod install failed; previous installation restored"));
    }
    Ok(())
}

pub(crate) fn uninstall(data: &Path) -> Result<()> {
    uninstall_with(data, game_closed)
}

fn uninstall_with(data: &Path, mut closed: impl FnMut() -> Result<()>) -> Result<()> {
    closed()?;
    let (_, target) = record(data)?;
    fs::create_dir_all(data.join("backups"))?;
    let backups = tempfile::Builder::new()
        .prefix("mod-uninstall-")
        .tempdir_in(data.join("backups"))?
        .keep();
    if target.exists() {
        copy_tree(&target, &backups.join(MOD))?;
    }
    fs::copy(data.join(RECORD), backups.join(RECORD))?;
    // Record first: if moving the mod fails, restore it and report failure.
    let hold = tempfile::Builder::new()
        .prefix(".tpf3mp-uninstall-")
        .tempdir_in(data)?;
    closed()?;
    fs::rename(data.join(RECORD), hold.path().join(RECORD))?;
    if target.exists()
        && let Err(error) = fs::remove_dir_all(&target)
    {
        fs::rename(hold.path().join(RECORD), data.join(RECORD))?;
        // A failed recursive removal may have removed some files already.
        copy_tree(&backups.join(MOD), &target)?;
        return Err(error.into());
    }
    if target.exists() || data.join(RECORD).exists() {
        bail!("uninstall did not remove the recorded mod");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(temp: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let root = temp.join("package");
        let data = temp.join("data");
        let mods = temp.join("game/local/staging_area");
        fs::create_dir_all(root.join("mod/tpf3mp_1/content")).unwrap();
        fs::write(root.join("mod/tpf3mp_1/mod.json"), b"{}").unwrap();
        fs::write(root.join("mod/tpf3mp_1/content/game.lua"), b"current mod").unwrap();
        (root, data, mods)
    }

    #[test]
    fn zero_exit_without_installing_never_reports_success() {
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        assert!(checked_script_install(&root, &data, &mods, "1.0", || Ok(())).is_err());
    }

    #[test]
    fn install_repair_verifies_every_byte_and_uninstall_preserves_saves_and_backups() {
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        let save = mods.parent().unwrap().join("save/keep.sav");
        fs::create_dir_all(save.parent().unwrap()).unwrap();
        fs::write(&save, b"my save").unwrap();
        install_with(&root, &data, &mods, "1.0", || Ok(()), verify).unwrap();
        fs::write(mods.join("tpf3mp_1/content/game.lua"), b"broken mod").unwrap();
        assert!(verify(&root, &data, &mods, "1.0").is_err());
        install_with(&root, &data, &mods, "1.1", || Ok(()), verify).unwrap();
        verify(&root, &data, &mods, "1.1").unwrap();
        uninstall_with(&data, || Ok(())).unwrap();
        assert!(!mods.join(MOD).exists());
        assert!(!data.join(RECORD).exists());
        assert_eq!(fs::read(save).unwrap(), b"my save");
        assert!(fs::read_dir(data.join("backups")).unwrap().count() >= 3);
        assert!(fs::read_dir(data.join("backups")).unwrap().any(|entry| {
            fs::read(entry.unwrap().path().join("tpf3mp_1/content/game.lua"))
                .is_ok_and(|bytes| bytes == b"broken mod")
        }));
    }

    #[test]
    fn failed_postcondition_restores_the_previous_mod_and_record() {
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        install_with(&root, &data, &mods, "1.0", || Ok(()), verify).unwrap();
        let old_record = fs::read(data.join(RECORD)).unwrap();
        let old_mod = tree(&mods.join(MOD)).unwrap();
        fs::write(root.join("mod/tpf3mp_1/content/game.lua"), b"new mod").unwrap();
        assert!(
            install_with(
                &root,
                &data,
                &mods,
                "1.1",
                || Ok(()),
                |_, _, _, _| bail!("fixture postcondition failure")
            )
            .is_err()
        );
        assert_eq!(fs::read(data.join(RECORD)).unwrap(), old_record);
        assert_eq!(tree(&mods.join(MOD)).unwrap(), old_mod);
    }

    #[test]
    fn running_or_unknown_game_state_changes_nothing() {
        for names in [
            Ok(vec!["TransportFever3.exe".into()]),
            Err(anyhow::anyhow!("query failed")),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (root, data, mods) = fixture(temp.path());
            let mut names = Some(names);
            assert!(
                install_with(
                    &root,
                    &data,
                    &mods,
                    "1.0",
                    || check_names(names.take().unwrap(), None),
                    verify
                )
                .is_err()
            );
            assert!(!data.exists());
            assert!(!mods.exists());
        }
        assert!(
            check_names(
                Ok(vec!["renamed-game.exe".into()]),
                Some("renamed-game.exe")
            )
            .is_err()
        );
        assert!(check_names(Ok(vec!["steam.exe".into()]), None).is_ok());
    }

    #[test]
    fn game_started_during_copy_cannot_replace_the_previous_mod() {
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        install_with(&root, &data, &mods, "1.0", || Ok(()), verify).unwrap();
        let previous = fs::read(data.join(RECORD)).unwrap();
        let mut calls = 0;
        assert!(
            install_with(
                &root,
                &data,
                &mods,
                "1.1",
                || {
                    calls += 1;
                    if calls == 1 {
                        Ok(())
                    } else {
                        bail!("game now running")
                    }
                },
                verify
            )
            .is_err()
        );
        assert_eq!(fs::read(data.join(RECORD)).unwrap(), previous);
        verify(&root, &data, &mods, "1.0").unwrap();
    }

    #[test]
    fn corrupted_records_and_extra_or_missing_files_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        install_with(&root, &data, &mods, "1.0", || Ok(()), verify).unwrap();
        fs::write(mods.join("tpf3mp_1/content/extra.lua"), b"unexpected").unwrap();
        assert!(verify(&root, &data, &mods, "1.0").is_err());
        fs::remove_file(mods.join("tpf3mp_1/content/extra.lua")).unwrap();
        fs::remove_file(mods.join("tpf3mp_1/content/game.lua")).unwrap();
        assert!(verify(&root, &data, &mods, "1.0").is_err());
        fs::write(
            data.join(RECORD),
            serde_json::to_vec(&serde_json::json!({"version":"1.0", "mod":mods.parent().unwrap()}))
                .unwrap(),
        )
        .unwrap();
        assert!(uninstall_with(&data, || Ok(())).is_err());
        assert!(mods.join(MOD).exists());
        // A matching final directory name is insufficient to authorize
        // deletion of some unrelated folder outside the game's staging area.
        let unrelated = temp.path().join(MOD);
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(unrelated.join("keep.txt"), b"not an installed mod").unwrap();
        fs::write(
            data.join(RECORD),
            serde_json::to_vec(&serde_json::json!({"version":"1.0", "mod":unrelated})).unwrap(),
        )
        .unwrap();
        assert!(uninstall_with(&data, || Ok(())).is_err());
        assert!(unrelated.join("keep.txt").is_file());
    }

    #[test]
    fn game_started_during_uninstall_backup_keeps_the_live_mod() {
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        install_with(&root, &data, &mods, "1.0", || Ok(()), verify).unwrap();
        let mut calls = 0;
        assert!(
            uninstall_with(&data, || {
                calls += 1;
                if calls == 1 {
                    Ok(())
                } else {
                    bail!("game now running")
                }
            })
            .is_err()
        );
        verify(&root, &data, &mods, "1.0").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_cannot_redirect_package_copy_or_recorded_removal() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let (root, data, mods) = fixture(temp.path());
        let outside = temp.path().join("outside.lua");
        fs::write(&outside, b"keep this file").unwrap();
        symlink(&outside, root.join("mod/tpf3mp_1/content/link.lua")).unwrap();
        assert!(install_with(&root, &data, &mods, "1.0", || Ok(()), verify).is_err());
        assert!(!mods.exists());
        fs::remove_file(root.join("mod/tpf3mp_1/content/link.lua")).unwrap();
        install_with(&root, &data, &mods, "1.0", || Ok(()), verify).unwrap();
        let record_copy = temp.path().join("record.json");
        fs::rename(data.join(RECORD), &record_copy).unwrap();
        symlink(&record_copy, data.join(RECORD)).unwrap();
        assert!(uninstall_with(&data, || Ok(())).is_err());
        assert!(mods.join(MOD).exists());
        assert_eq!(fs::read(outside).unwrap(), b"keep this file");
    }
}
