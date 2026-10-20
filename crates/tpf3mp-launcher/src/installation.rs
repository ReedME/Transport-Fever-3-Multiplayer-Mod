//! Windows setup using the readable installer, or verified Rust copying in Proton.
//! Downloads use the updater's signed manifest, never an executable from a peer.

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
};

use anyhow::{Context, Result, bail};
use eframe::egui;

use crate::{icon, installed, mod_install, theme, update};

const MANAGED: &str = "tpf3mp-managed.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Install,
    Sync,
    Repair,
    Uninstall,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub root: PathBuf,
    pub data: PathBuf,
    pub mode: Mode,
    pub managed: bool,
}

/// Missing package markers on development binaries do not start setup.
/// The release asset is named TPF3-MP.exe, exactly like the installed app.
pub fn standalone(exe: &Path) -> bool {
    standalone_at(exe, option_env!("TPF3MP_DISTRIBUTABLE") == Some("1"))
}

fn standalone_at(exe: &Path, distributable: bool) -> bool {
    (distributable
        || exe
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("TPF3-MP.exe")))
        && !exe
            .parent()
            .is_some_and(|dir| dir.join(update::PACKAGE_MARKER).is_file())
}

pub fn managed_root() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").context("Windows has no local application data folder")?,
    )
    .join("Programs")
    .join("TPF3-MP"))
}

fn data_dir() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").context("Windows has no local application data folder")?,
    )
    .join("TPF3-MP"))
}

/// Start setup before the backend opens connections or a game. Return true
/// when this process should exit (setup either opened another copy or closed).
pub fn before_launch(repair: bool, uninstall: bool) -> Result<bool> {
    if !cfg!(windows) {
        return Ok(false);
    }
    let exe = std::env::current_exe()?;
    let downloaded = standalone(&exe);
    portable_wine_package(mod_install::wine(), downloaded, false)?;
    let root = if downloaded {
        managed_root()?
    } else {
        exe.parent()
            .context("the launcher has no folder")?
            .to_owned()
    };
    let data = data_dir()?;
    let managed = downloaded || root.join(MANAGED).is_file();
    portable_wine_package(mod_install::wine(), downloaded, managed)?;
    if downloaded
        && exe != root.join("TPF3-MP.exe")
        && root.join(MANAGED).is_file()
        && root.join("TPF3-MP.exe").is_file()
        && package_version(&root).is_some()
        && !repair
        && !uninstall
    {
        // Reopening the download must work offline, too.
        if open_launcher(&root).is_ok() {
            return Ok(true);
        }
    }
    let packaged = root.join(update::PACKAGE_MARKER).is_file();
    if packaged && !downloaded && !repair && !uninstall {
        let marker: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join(update::PACKAGE_MARKER))?)?;
        if marker["version"] != update::VERSION || marker["platform"] != "windows-x64" {
            bail!(
                "The launcher and package versions differ. Close TPF3-MP and reopen it; use the standalone download to repair if needed."
            );
        }
    }
    if !downloaded && !packaged && !repair && !uninstall {
        return Ok(false);
    }
    let mode = if uninstall {
        Mode::Uninstall
    } else if repair {
        Mode::Repair
    } else if downloaded {
        Mode::Install
    } else {
        Mode::Sync
    };
    if mode == Mode::Sync
        && recorded_mods(&data)
            .is_some_and(|mods| mod_install::verify(&root, &data, &mods, update::VERSION).is_ok())
    {
        return Ok(false);
    }
    if !downloaded && !packaged {
        bail!("Repair needs an installed TPF3-MP package.");
    }
    let plan = Plan {
        root,
        data,
        mode,
        managed,
    };
    let mods = mod_folders(&tpf3mp_agent::steam::steam_roots());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TPF3-MP · Setup")
            .with_inner_size([680.0, 540.0])
            .with_min_inner_size([600.0, 510.0])
            .with_icon(icon::icon()),
        ..Default::default()
    };
    eframe::run_native(
        "TPF3-MP Setup",
        options,
        Box::new(move |_| Ok(Box::new(SetupApp::new(plan, mods)))),
    )
    .map_err(|error| anyhow::anyhow!("opening setup: {error}"))?;
    Ok(true)
}

fn portable_wine_package(wine: bool, downloaded: bool, managed: bool) -> Result<()> {
    if wine && (downloaded || managed) {
        bail!(
            "Under Proton or Wine, use the Windows ZIP package: extract it and run TPF3-MP.exe in the game's Proton prefix. The standalone EXE's Windows shortcuts and managed removal require real PowerShell and are unavailable here. No setup changes were made."
        );
    }
    Ok(())
}

fn package_version(root: &Path) -> Option<String> {
    let marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(update::PACKAGE_MARKER)).ok()?).ok()?;
    let version = marker["version"].as_str()?;
    (marker["platform"] == "windows-x64" && semver::Version::parse(version).is_ok())
        .then(|| version.to_owned())
}

pub fn open_launcher(root: &Path) -> Result<()> {
    Command::new(root.join("TPF3-MP.exe"))
        .args(
            std::env::args_os()
                .skip(1)
                .filter(|arg| arg != "--repair" && arg != "--uninstall"),
        )
        .spawn()
        .context("opening the installed launcher")?;
    Ok(())
}

/// Settings starts a separate setup process and closes the lobby window.
pub fn open_maintenance(uninstall: bool) -> Result<()> {
    Command::new(std::env::current_exe()?)
        .arg(if uninstall { "--uninstall" } else { "--repair" })
        .spawn()?;
    Ok(())
}

pub fn mod_folders(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut folders = Vec::new();
    for root in roots {
        let Ok(accounts) = std::fs::read_dir(root.join("userdata")) else {
            continue;
        };
        for account in accounts.flatten() {
            if account
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
                .is_none()
            {
                continue;
            }
            let local = account.path().join("3493540/local");
            if local.is_dir() {
                folders.push(local.join("staging_area"));
            }
        }
    }
    folders.sort();
    folders.dedup();
    folders
}

fn recorded_mods(data: &Path) -> Option<PathBuf> {
    mod_install::recorded_target(data)
        .ok()?
        .parent()
        .map(Path::to_owned)
}

/// The mods folder as typed or pasted: Explorer's "Copy as path" adds quotes.
fn typed_folder(text: &str) -> PathBuf {
    PathBuf::from(text.trim().trim_matches('"').trim())
}

/// Why setup cannot install into this folder, said under the folder, or
/// `None` when it can.
fn folder_problem(text: &str) -> Option<&'static str> {
    let folder = typed_folder(text);
    if folder.as_os_str().is_empty() {
        Some("Start the game through Steam once if its user folder hasn't been created yet.")
    } else if !folder.is_absolute() {
        Some("Enter the folder's full path, starting with its drive.")
    } else if !folder
        .file_name()
        .is_some_and(|name| name == "staging_area")
    {
        Some(
            r"Choose the folder named staging_area: <Steam>\userdata\<account>\3493540\local\staging_area.",
        )
    } else {
        None
    }
}

fn powershell(script: &Path) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn run_script(mut command: Command) -> Result<()> {
    let output = command
        .output()
        .context("starting the readable installer")?;
    if !output.status.success() {
        bail!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn check_game_closed(_root: &Path) -> Result<()> {
    mod_install::game_closed()
}

/// Restore the old package's mod if an update rolls its binaries back.
/// This also covers rollback to 0.1, whose launcher has no setup gate.
pub fn restore_mod(root: &Path) -> Result<()> {
    if !root.join("tools/install.ps1").is_file() {
        return Ok(());
    }
    let Some(mods) = recorded_mods(&data_dir()?) else {
        return Ok(());
    };
    check_game_closed(root)?;
    install_mod(root, &data_dir()?, &mods)
}

fn install_mod(root: &Path, data: &Path, mods: &Path) -> Result<()> {
    let version =
        package_version(root).context("the package has no valid Windows version marker")?;
    if mod_install::wine() {
        mod_install::install(root, data, mods, &version)
    } else {
        mod_install::checked_script_install(root, data, mods, &version, || {
            let mut command = powershell(&root.join("tools/install.ps1"));
            command.arg("-ModsDir").arg(mods);
            run_script(command)
        })
    }
}

fn perform(
    plan: &Plan,
    mods: &Path,
    desktop: bool,
    mut progress: impl FnMut(String, Option<f32>),
) -> Result<()> {
    portable_wine_package(mod_install::wine(), false, plan.managed)?;
    check_game_closed(&plan.root)?;
    if plan.mode == Mode::Uninstall {
        progress("Removing the multiplayer mod…".into(), None);
        if plan.data.join("installed.json").is_file() {
            let target = mod_install::recorded_target(&plan.data)?;
            if mod_install::wine() {
                mod_install::uninstall(&plan.data)?;
            } else {
                let mut command = powershell(&plan.root.join("tools/install.ps1"));
                command.arg("-Uninstall");
                run_script(command)?;
            }
            if target.exists() || plan.data.join("installed.json").exists() {
                bail!("The installer did not remove the recorded mod; uninstall is incomplete.");
            }
        } else {
            check_game_closed(&plan.root)?;
        }
        if plan.managed {
            // A copy outside the install can move the app to backups after
            // this process closes. The helper validates its exact root.
            std::fs::create_dir_all(&plan.data)?;
            let helper = plan
                .data
                .join(format!("uninstall-{}.ps1", std::process::id()));
            std::fs::copy(plan.root.join("tools/manage.ps1"), &helper)?;
            let mut command = powershell(&helper);
            command.current_dir(&plan.data);
            command
                .arg("-Root")
                .arg(&plan.root)
                .arg("-Uninstall")
                .arg("-WaitForPid")
                .arg(std::process::id().to_string());
            command.spawn().context("finishing launcher removal")?;
        }
        return Ok(());
    }
    if plan.mode == Mode::Install || (plan.mode == Mode::Repair && plan.managed) {
        if plan.root.join("tools/install.ps1").is_file() {
            check_game_closed(&plan.root)?;
        }
        progress("Checking the signed release…".into(), None);
        update::bootstrap(&plan.root, |version, bytes, total| {
            #[allow(clippy::cast_precision_loss)]
            let fraction = bytes as f32 / total.max(1) as f32;
            progress(
                format!(
                    "Downloading TPF3-MP {version} · {} / {} MB",
                    bytes / 1_000_000,
                    total / 1_000_000
                ),
                Some(fraction),
            );
        })?;
    }
    progress("Installing the multiplayer mod…".into(), None);
    install_mod(&plan.root, &plan.data, mods)?;
    if plan.managed {
        progress("Adding your launcher shortcut…".into(), None);
        let mut command = powershell(&plan.root.join("tools/manage.ps1"));
        command.arg("-Root").arg(&plan.root);
        if desktop {
            command.arg("-DesktopShortcut");
        }
        run_script(command)?;
        if !plan.root.join(MANAGED).is_file() {
            bail!("The shortcut installer did not create its managed-installation record.");
        }
    }
    Ok(())
}

enum Event {
    Progress(String, Option<f32>),
    Done(Result<(), String>),
}

pub struct SetupApp {
    plan: Plan,
    folders: Vec<PathBuf>,
    mods: String,
    desktop: bool,
    receiver: Option<Receiver<Event>>,
    status: String,
    fraction: Option<f32>,
    error: Option<String>,
    complete: bool,
    auto_sync: bool,
    themed: bool,
}

impl SetupApp {
    pub fn new(plan: Plan, folders: Vec<PathBuf>) -> Self {
        let previous = recorded_mods(&plan.data);
        let mods = previous.or_else(|| (folders.len() == 1).then(|| folders[0].clone()));
        let auto_sync = plan.mode == Mode::Sync
            && mods.is_some()
            && installed::installed_mod(&plan.data).is_some();
        Self {
            plan,
            folders,
            mods: mods
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            desktop: true,
            receiver: None,
            status: String::new(),
            fraction: None,
            error: None,
            complete: false,
            auto_sync,
            themed: false,
        }
    }

    fn begin(&mut self, ctx: &egui::Context) {
        self.error = None;
        self.status = "Preparing…".into();
        let plan = self.plan.clone();
        let mods = typed_folder(&self.mods);
        let desktop = self.desktop;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = perform(&plan, &mods, desktop, |text, fraction| {
                let _ = sender.send(Event::Progress(text, fraction));
                ctx.request_repaint();
            });
            let _ = sender.send(Event::Done(result.map_err(|error| format!("{error:#}"))));
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if !self.themed {
            theme::apply(ui.ctx());
            self.themed = true;
            ui.ctx().request_repaint();
            return;
        }
        if self.auto_sync {
            self.auto_sync = false;
            self.begin(ui.ctx());
        }
        let events: Vec<_> = self
            .receiver
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Progress(text, fraction) => {
                    self.status = text;
                    self.fraction = fraction;
                }
                Event::Done(result) => {
                    self.receiver = None;
                    match result {
                        Ok(()) => self.complete = true,
                        Err(error) => self.error = Some(error),
                    }
                }
            }
        }
        let busy = self.receiver.is_some();
        if busy && ui.ctx().input(|input| input.viewport().close_requested()) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG).inner_margin(30.0)).show(ui, |ui| {
            ui.label(theme::text("TRANSPORT FEVER 3 · MULTIPLAYER", theme::semibold(12.0), theme::MUTED));
            ui.add_space(14.0);
            let heading = if self.complete { "You're all set" } else { match self.plan.mode { Mode::Install => "Let's get you playing", Mode::Sync => "Set up multiplayer", Mode::Repair => "Repair TPF3-MP", Mode::Uninstall => "Uninstall TPF3-MP" } };
            ui.label(theme::text(heading, theme::semibold(30.0), theme::TEXT));
            ui.add_space(16.0);
            if self.complete {
                if self.plan.mode == Mode::Uninstall {
                    ui.label("The multiplayer mod has been removed. Your saves and settings are kept.");
                    if self.plan.managed { ui.label("Close this window to finish removing the launcher."); }
                    if ui.button("Close").clicked() { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); }
                } else {
                    ui.label("TPF3-MP is installed. Future updates also update your multiplayer mod.");
                    ui.add_space(12.0);
                    ui.label("Create a world from Multiplayer to select TPF3-MP automatically. For an existing world, keep TPF3-MP active in its Mods tab.");
                    ui.add_space(24.0);
                    if theme::primary_small(ui, true, Some("arrow-right"), "Open launcher", false).clicked() {
                        match open_launcher(&self.plan.root) {
                            Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(error) => self.error = Some(error.to_string()),
                        }
                    }
                }
            } else {
                ui.label(match self.plan.mode { Mode::Install => "One download. We'll install the launcher and multiplayer mod for your Windows account.", Mode::Uninstall => "Remove the multiplayer mod and, for a managed installation, the launcher and its shortcuts. Saves and settings stay in place.", Mode::Repair => "Restore the launcher package and reinstall the multiplayer mod. Close the game first.", Mode::Sync => "Keep the multiplayer mod on the same version as your launcher. Close the game first." });
                ui.add_space(14.0);
                ui.label(theme::text(self.plan.root.display().to_string(), theme::body(12.0), theme::MUTED));
                if self.plan.mode != Mode::Uninstall {
                    ui.add_space(14.0);
                    ui.label("Transport Fever 3 mods folder");
                    ui.add_enabled_ui(!busy, |ui| {
                        if self.folders.len() > 1 {
                            egui::ComboBox::from_id_salt("steam-account").selected_text("Choose a Steam account's folder").width(ui.available_width()).show_ui(ui, |ui| {
                                for folder in &self.folders { let path = folder.display().to_string(); ui.selectable_value(&mut self.mods, path.clone(), path); }
                            });
                        }
                        ui.add(egui::TextEdit::singleline(&mut self.mods).desired_width(f32::INFINITY).hint_text("Paste the staging_area folder if Steam wasn't found"));
                        if let Some(problem) = folder_problem(&self.mods) { ui.label(problem); }
                        if self.plan.managed { ui.checkbox(&mut self.desktop, "Also add a desktop shortcut"); }
                    });
                }
                ui.add_space(18.0);
                if busy {
                    ui.label(&self.status);
                    if let Some(fraction) = self.fraction { ui.add(egui::ProgressBar::new(fraction).show_percentage()); } else { ui.spinner(); }
                } else {
                    let valid = self.plan.mode == Mode::Uninstall || folder_problem(&self.mods).is_none();
                    let label = match self.plan.mode { Mode::Install => "Install TPF3-MP", Mode::Sync => "Install multiplayer mod", Mode::Repair => "Repair installation", Mode::Uninstall => "Uninstall" };
                    if theme::primary_small(ui, valid, Some("download"), label, false).clicked() { self.begin(ui.ctx()); }
                }
            }
            if let Some(error) = &self.error {
                ui.add_space(12.0);
                egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| { ui.colored_label(theme::BAD, error); });
            }
        });
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn development_binaries_stay_portable_and_the_download_bootstraps() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!standalone_at(
            &temp.path().join("tpf3mp-launcher.exe"),
            false
        ));
        assert!(standalone_at(&temp.path().join("TPF3-MP.exe"), false));
        assert!(standalone_at(&temp.path().join("TPF3-MP (1).exe"), true));
        assert!(standalone_at(&temp.path().join("renamed.exe"), true));
        std::fs::write(temp.path().join(update::PACKAGE_MARKER), "{}").unwrap();
        assert!(!standalone_at(&temp.path().join("TPF3-MP.exe"), true));
    }

    #[test]
    fn wine_requires_the_portable_package_before_any_setup() {
        assert!(portable_wine_package(true, true, false).is_err());
        assert!(portable_wine_package(true, false, true).is_err());
        assert!(portable_wine_package(true, false, false).is_ok());
        assert!(portable_wine_package(false, true, true).is_ok());
    }

    #[test]
    fn several_steam_accounts_need_a_choice_and_custom_folders_are_remembered() {
        let temp = tempfile::tempdir().unwrap();
        for id in ["123", "456", "not-an-account"] {
            std::fs::create_dir_all(temp.path().join(format!("userdata/{id}/3493540/local")))
                .unwrap();
        }
        let folders = mod_folders(&[temp.path().to_owned()]);
        assert_eq!(folders.len(), 2);
        let plan = Plan {
            root: temp.path().join("program"),
            data: temp.path().to_owned(),
            mode: Mode::Install,
            managed: true,
        };
        let app = SetupApp::new(plan.clone(), folders.clone());
        assert!(
            app.mods.is_empty(),
            "do not silently pick another player's account"
        );
        let custom = temp.path().join("custom/staging_area/tpf3mp_1");
        std::fs::write(
            temp.path().join("installed.json"),
            // Existing Windows records may have a UTF-8 BOM.
            format!(
                "\u{feff}{}",
                serde_json::json!({"version":"0.1.0", "mod":custom})
            ),
        )
        .unwrap();
        assert_eq!(
            SetupApp::new(plan, folders).mods,
            custom.parent().unwrap().display().to_string()
        );
    }

    #[test]
    fn setup_explains_the_install_and_exposes_one_primary_action() {
        let temp = tempfile::tempdir().unwrap();
        let app = SetupApp::new(
            Plan {
                root: temp.path().join("program"),
                data: temp.path().to_owned(),
                mode: Mode::Install,
                managed: true,
            },
            vec![],
        );
        let mut harness = Harness::builder()
            .with_size(egui::vec2(680.0, 540.0))
            .build_ui_state(
                |ui, app: &mut SetupApp| {
                    app.show(ui);
                },
                app,
            );
        harness.run_steps(3);
        assert!(harness.query_by_label("Let's get you playing").is_some());
        assert!(harness.query_by_label("Install TPF3-MP").is_some());
        assert!(
            harness
                .query_by_label("Also add a desktop shortcut")
                .is_some()
        );
    }

    #[test]
    fn setup_says_why_a_mods_folder_cannot_be_used() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("userdata/123/3493540/local/staging_area");
        assert_eq!(folder_problem(&staging.display().to_string()), None);
        // Explorer's "Copy as path" quotes the path.
        let quoted = format!("  \"{}\" ", staging.display());
        assert_eq!(folder_problem(&quoted), None);
        assert_eq!(typed_folder(&quoted), staging);

        assert!(folder_problem("").unwrap().contains("Start the game"));
        assert!(
            folder_problem("staging_area")
                .unwrap()
                .contains("full path")
        );
        let local = staging.parent().unwrap().display().to_string();
        assert!(folder_problem(&local).unwrap().contains("staging_area"));

        let mut app = SetupApp::new(
            Plan {
                root: temp.path().join("program"),
                data: temp.path().to_owned(),
                mode: Mode::Install,
                managed: true,
            },
            vec![],
        );
        app.mods = local;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(680.0, 540.0))
            .build_ui_state(
                |ui, app: &mut SetupApp| {
                    app.show(ui);
                },
                app,
            );
        harness.run_steps(3);
        assert!(
            harness
                .query_by_label_contains("Choose the folder named staging_area")
                .is_some()
        );
    }

    #[test]
    #[ignore = "renders setup with a GPU for visual review"]
    fn render_setup() {
        let temp = tempfile::tempdir().unwrap();
        let app = SetupApp::new(
            Plan {
                root: PathBuf::from(r"C:\Users\Player\AppData\Local\Programs\TPF3-MP"),
                data: temp.path().to_owned(),
                mode: Mode::Install,
                managed: true,
            },
            vec![PathBuf::from(
                r"C:\Program Files (x86)\Steam\userdata\123456\3493540\local\staging_area",
            )],
        );
        let mut harness = Harness::builder()
            .with_size(egui::vec2(680.0, 540.0))
            .wgpu()
            .build_ui_state(
                |ui, app: &mut SetupApp| {
                    app.show(ui);
                },
                app,
            );
        harness.run_steps(3);
        let image = harness.render().unwrap();
        std::fs::create_dir_all("../../target/launcher-screenshots").unwrap();
        image
            .save("../../target/launcher-screenshots/setup.png")
            .unwrap();
    }
}
