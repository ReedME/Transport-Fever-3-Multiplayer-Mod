//! The launcher's window, laid out as the page silver2127 ported from
//! tearded's TPF2 Multiplayer Launcher (D20): the city across the window;
//! the logo, the server, updates and Settings along the top; the game's
//! wordmark and how to play, or the room, on the left; the Multiplayer
//! panel with one big button on the right; your game in a bar along the
//! bottom. What it shows is worked out in [`view`], from the launcher's
//! [`State`] on every frame.
//!
//! The room's lobby is in the game (D17): by default the panel starts the
//! game and the left column shows where things stand, with the room's
//! players but not its chat or buttons. "Lobby in this window" brings the
//! page's lobby back, for a game whose menu the hook cannot reach.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use eframe::egui::{
    self, Align, Color32, ComboBox, CornerRadius, Frame, Id, Layout, Margin, Modal, Rect, RichText,
    ScrollArea, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType,
    pos2, vec2,
};
use tpf3mp_agent::{
    launcher::{Action, Connection, ModHave, State, server_address},
    native_mods::NativeRow,
};

use crate::{
    backend::Backend,
    notes::{Block, Notes, ReleaseNotes},
    previews::Previews,
    probe::{Probe, Reach},
    theme::{self, Assets, Fill, Pill, Quiet},
    update::{UpdateState, Updater},
    view::{self, Does, Form, Place, Then, Tone, View},
};

/// How often the window rereads the launcher's state when nothing else
/// asks it to repaint.
const REFRESH: Duration = Duration::from_millis(250);
/// Player counts a room can be created for, as the page offers them.
const ROOM_SIZES: std::ops::RangeInclusive<u8> = 2..=16;
/// Notices kept in the session log, newest last.
const NOTICES_SHOWN: usize = 12;
/// How often TPF3-MP's data folder is looked at for what the installer
/// put in the game.
const INSTALL_CHECK: Duration = Duration::from_secs(5);
/// How long a toast stays.
const TOAST: Duration = Duration::from_millis(6500);
/// The Multiplayer panel's width.
const SIDEBAR: f32 = 340.0;
/// What the note under the panel says.
const SESSION_NOTE: &str = "Everyone in a room needs the same game build and mods.";
/// Where the room's chat and buttons are, when the lobby is in the game.
const IN_GAME_NOTE: &str =
    "Chat, Ready, Start and removing players are in the game's Multiplayer window.";

/// What the window needs besides the launcher.
pub struct Extras {
    /// Checks for and installs new versions; `None` in tests.
    pub updater: Option<Updater>,
    /// Whether the launcher's own server is up; `None` in tests and
    /// without a server of its own. A new one asks when the player changes
    /// the server.
    pub probe: Option<Probe>,
    /// The latest release's notes; `None` in tests.
    pub notes: Option<ReleaseNotes>,
    /// For tests and screenshots, what to show without an updater and
    /// without looking in TPF3-MP's data folder: the updater's state, and
    /// the installed mod's version.
    pub shown: Shown,
}

/// What a window without an updater or installer record shows.
#[derive(Debug, Clone, Default)]
pub struct Shown {
    pub update: Option<UpdateState>,
    /// `Some` to show this installed mod rather than look for one.
    pub installed_mod: Option<Option<String>>,
}

/// A question the window asks before acting.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Confirm {
    Kick { id: String, name: String },
    Leave,
    Quit,
}

/// Where the parts of the window go, for its size, as the page lays them
/// out at 1100 by 690.
struct Geometry {
    window: Rect,
    header: Rect,
    story: Rect,
    sidebar: Rect,
    files: Rect,
    footer: Rect,
}

impl Geometry {
    fn of(window: Rect) -> Self {
        let pad = if window.width() >= 1000.0 { 48.0 } else { 28.0 };
        let left = window.min.x + pad;
        let right = window.max.x - pad;
        let side = right - SIDEBAR;
        let layout_bottom = window.max.y - 122.0;
        Self {
            window,
            header: Rect::from_min_max(pos2(left, window.min.y), pos2(right, window.min.y + 76.0)),
            story: Rect::from_min_max(
                pos2(window.min.x + 20.0, window.min.y + 76.0),
                pos2(side - 36.0, layout_bottom),
            ),
            sidebar: Rect::from_min_max(
                pos2(side, window.min.y + 104.0),
                pos2(right, layout_bottom - 12.0),
            ),
            files: Rect::from_min_max(
                pos2(window.min.x + 20.0, window.max.y - 106.0),
                pos2(right, window.max.y - 37.0),
            ),
            footer: Rect::from_min_max(pos2(left, window.max.y - 36.0), pos2(right, window.max.y)),
        }
    }
}

/// The launcher's window over a [`Backend`].
pub struct LauncherApp<B> {
    backend: B,
    extras: Extras,
    /// The fields, as the player types them.
    server: String,
    name: String,
    invite: String,
    room_name: String,
    max_players: u8,
    create_password: String,
    rules: Option<String>,
    join_invite: String,
    join_password: String,
    /// The trusted region chosen for an invite, or `None` to find it across
    /// all listed regions. Kept only while the join form is open.
    join_server: Option<String>,
    /// The server and room state under which `join_server` was chosen.
    join_server_context: Option<(Option<String>, bool)>,
    chat: String,
    /// Whether the server and name fields took the launcher's first offer.
    offered: bool,
    /// A form opened with a quieter button, in place of the main one.
    open_form: Option<Form>,
    confirm: Option<Confirm>,
    /// The player confirmed quitting during a game.
    quitting: bool,
    /// An update check was started because the server is newer.
    looked_for_update: bool,
    /// The look is set on the first frame.
    styled: bool,
    settings: bool,
    /// The open mods dialog, if any: the mod manager or the mod browser.
    mods: Option<ModsPage>,
    /// What the player typed in the mod browser's search.
    mod_search: String,
    /// The native mod picked in the mods dialog's table, by id.
    selected_mod: Option<String>,
    /// Native mods' preview images, fetched as they are shown.
    previews: Previews,
    /// The server setting's field, as the player types it, and the server
    /// it was last filled from.
    server_setting: String,
    server_setting_for: Option<String>,
    toast: Option<(String, Instant)>,
    /// The error last shown as a toast.
    last_error: Option<String>,
    /// When TPF3-MP's data folder was last looked at, and the TPF3-MP
    /// version the installer recorded there.
    installed_mod: Option<(Instant, Option<String>)>,
    /// Where the player uses the room's lobby: the game's Multiplayer
    /// window, unless they asked for it here.
    place: Place,
}

impl<B: Backend> LauncherApp<B> {
    pub fn new(backend: B, extras: Extras) -> Self {
        Self {
            backend,
            extras,
            server: String::new(),
            name: String::new(),
            invite: String::new(),
            room_name: String::new(),
            max_players: 4,
            create_password: String::new(),
            rules: None,
            join_invite: String::new(),
            join_password: String::new(),
            join_server: None,
            join_server_context: None,
            chat: String::new(),
            offered: false,
            open_form: None,
            confirm: None,
            quitting: false,
            looked_for_update: false,
            styled: false,
            settings: false,
            mods: None,
            mod_search: String::new(),
            selected_mod: None,
            previews: Previews::default(),
            server_setting: String::new(),
            server_setting_for: None,
            toast: None,
            last_error: None,
            installed_mod: None,
            place: Place::Game,
        }
    }

    /// The window with the room's lobby in `place` to begin with.
    #[must_use]
    pub fn with_place(mut self, place: Place) -> Self {
        self.place = place;
        self
    }

    /// Shows `image` as the preview at `url` without fetching it.
    pub fn with_preview(mut self, url: &str, image: egui::ColorImage) -> Self {
        self.previews.insert(url, image);
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Draws the window once.
    pub fn show(&mut self, ui: &mut Ui) {
        if !self.styled {
            theme::apply(ui.ctx());
            self.styled = true;
            // egui takes new fonts from the next frame on: draw from then.
            ui.ctx().request_repaint();
            return;
        }
        let ctx = ui.ctx().clone();
        let Some(assets) = Assets::of(&ctx) else {
            return;
        };
        let state = self.backend.state();
        let join_server_context = (state.server.clone(), state.room.is_some());
        if self.join_server_context.as_ref() != Some(&join_server_context) {
            self.join_server = None;
            self.join_server_context = Some(join_server_context);
        }
        if !self.offered {
            // The server and name remembered from last time, or given.
            self.server = state.server.clone().unwrap_or_default();
            self.name = state.name.clone();
            self.offered = true;
        }
        if state.error != self.last_error {
            if let Some(error) = &state.error {
                self.toast = Some((error.clone(), Instant::now()));
            }
            self.last_error.clone_from(&state.error);
        }
        if state.outdated && !self.looked_for_update {
            // The server is newer: look for the update now, not at the next
            // regular check.
            self.looked_for_update = true;
            if let Some(updater) = &self.extras.updater {
                updater.check();
            }
        }
        self.guard_quit(&ctx, &state);
        // The server changed (the player's setting): the field shows it, and
        // the probe asks the new one.
        if self.server_setting_for != state.server {
            self.server_setting = state.server.clone().unwrap_or_default();
            self.server_setting_for.clone_from(&state.server);
        }
        if let Some(server) = state.server.as_deref().filter(|_| state.server_fixed)
            && self
                .extras
                .probe
                .as_ref()
                .is_some_and(|probe| !probe.tells_of(server))
        {
            self.extras.probe = Probe::start(server);
        }
        let reach = self
            .extras
            .probe
            .as_ref()
            .map_or(Reach::Unknown, Probe::reach);
        let update = self
            .extras
            .updater
            .as_ref()
            .map(Updater::state)
            .or_else(|| self.extras.shown.update.clone());
        let view = match self.place {
            Place::Game => view::present_in_game(&state, reach, update.as_ref()),
            Place::Launcher => view::present(&state, reach, update.as_ref()),
        };
        let geometry = Geometry::of(ctx.content_rect());
        theme::scene(ui.painter(), geometry.window, &assets);
        self.header(ui, &geometry, &state, &view, reach, &assets);
        self.story(ui, &geometry, &state, &view, &assets);
        self.sidebar(ui, &geometry, &state, &view, &assets);
        self.files(ui, &geometry, &state, &assets);
        self.footer(ui, &geometry, &state);
        if self.settings
            || self.mods.is_some()
            || self.confirm.is_some()
            || state.native_consent.is_some()
        {
            // Behind a dialog the page is blurred, as the page's
            // `::backdrop` does: the city blurred, over it all.
            let painter =
                ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, Id::new("backdrop")));
            theme::blur_behind(&painter, geometry.window, &assets);
        }
        self.settings_dialog(&ctx, &state, &view);
        self.mods_dialog(&ctx, &state);
        self.confirm_dialog(&ctx);
        self.plugin_consent_dialog(&ctx, &state);
        self.draw_toast(&ctx, geometry.window);
        ctx.request_repaint_after(REFRESH);
    }

    // ---------- along the top ----------

    /// The logo; the server, a waiting update and Settings, right.
    fn header(
        &mut self,
        ui: &mut Ui,
        geometry: &Geometry,
        state: &State,
        view: &View,
        reach: Reach,
        assets: &Assets,
    ) {
        let header = geometry.header;
        let logo = Rect::from_min_size(pos2(header.min.x, header.min.y + 16.0), Vec2::splat(44.0));
        ui.painter().image(
            assets.logo.id(),
            logo,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let actions =
            Rect::from_x_y_ranges(header.x_range(), header.min.y + 18.0..=header.min.y + 58.0);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(actions)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let settings = Quiet {
                    color: theme::rgba(243, 247, 250, 1.0),
                    fill: theme::rgba(21, 35, 46, 0.933),
                    border: theme::rgba(210, 225, 236, 0.5),
                    ..Quiet::new()
                };
                if theme::quiet_button(ui, true, Some("settings"), "Settings", settings).clicked() {
                    self.settings = true;
                }
                if theme::quiet_button(ui, true, Some("archive"), "Mod manager", settings).clicked()
                {
                    self.mods = Some(ModsPage::Manager);
                }
                if theme::quiet_button(ui, true, Some("search"), "Mod browser", settings).clicked()
                {
                    self.mods = Some(ModsPage::Browser);
                }
                if let Some(badge) = &view.updates.badge
                    && update_badge(ui, badge).clicked()
                {
                    self.settings = true;
                }
                let server = state.server_name.clone().or_else(|| state.server.clone());
                if let Some(server) = server {
                    let dot = match (state.connection, reach) {
                        (Connection::Connected, _) | (_, Reach::Online) => Some(theme::OK),
                        (_, Reach::Offline) => Some(theme::BAD),
                        _ => None,
                    };
                    server_chip(ui, geometry.window, assets, &server, dot);
                }
            },
        );
    }

    // ---------- the left column ----------

    /// The wordmark over the city, then how to play or the room, then the
    /// latest release's notes; it scrolls.
    fn story(
        &mut self,
        ui: &mut Ui,
        geometry: &Geometry,
        state: &State,
        view: &View,
        assets: &Assets,
    ) {
        ui.scope_builder(UiBuilder::new().max_rect(geometry.story), |ui| {
            ScrollArea::vertical()
                .id_salt("story")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(geometry.story.width() - 30.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    banner(ui, assets);
                    // The room's panel is at least as tall as the column.
                    let tall = geometry.story.height();
                    self.room_panel(ui, geometry, state, view, assets, tall);
                    if state.room.is_none() {
                        ui.add_space(16.0);
                        release_panel(ui, geometry, assets, self.extras.notes.as_ref());
                    }
                });
        });
        // The column fades in over its top 32 points and out over its
        // bottom 16, as the page's mask does.
        theme::fade(
            ui.painter(),
            geometry.window,
            geometry.story,
            assets,
            32.0,
            16.0,
        );
    }

    /// "How to play" and its five steps, or "Your room": its players, how
    /// your mods differ, the chat and the session log.
    fn room_panel(
        &mut self,
        ui: &mut Ui,
        geometry: &Geometry,
        state: &State,
        view: &View,
        assets: &Assets,
        tall: f32,
    ) {
        panel(
            ui,
            geometry,
            assets,
            Fill::Flat(theme::panel_fill(0.65)),
            tall,
            |ui| {
                ui.add_space(7.0);
                let Some(room) = &state.room else {
                    theme::section_heading(
                        ui,
                        match self.place {
                            Place::Game => "How to play: in the game",
                            Place::Launcher => "How to play",
                        },
                    );
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.y = 11.5;
                    for (words, done) in &view.steps {
                        theme::step(ui, words, *done);
                    }
                    return;
                };
                theme::section_heading(ui, "Your room");
                ui.add_space(15.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    rules_chip(ui, &room.rules);
                    ui.label(
                        theme::text(&room.name, theme::semibold(20.0), theme::TEXT)
                            .extra_letter_spacing(-0.5),
                    );
                });
                ui.add_space(14.0);
                let count = view.players.len();
                for (index, player) in view.players.iter().enumerate() {
                    self.player_row(ui, player, index + 1 == count);
                }
                if !view.differences.is_empty() {
                    ui.add_space(14.0);
                    differences(ui, &view.differences);
                }
                if !state.room_native.is_empty() || state.native_busy.is_some() {
                    ui.add_space(14.0);
                    self.room_native(ui, state);
                }
                ui.add_space(22.0);
                match self.place {
                    Place::Launcher => self.chat(ui, state),
                    Place::Game => {
                        ui.label(theme::text(IN_GAME_NOTE, theme::body(13.0), theme::MUTED));
                    }
                }
                if !state.notices.is_empty() {
                    ui.add_space(22.0);
                    theme::section_heading(ui, "Session log");
                    ui.add_space(8.0);
                    let start = state.notices.len().saturating_sub(NOTICES_SHOWN);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for notice in &state.notices[start..] {
                        ui.label(theme::text(notice, theme::body(11.0), theme::MUTED));
                    }
                }
            },
        );
    }

    /// One player: name and system left, their pills and Remove right.
    fn player_row(&mut self, ui: &mut Ui, player: &view::Player, last: bool) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 61.0), Sense::hover());
        let mut row = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(0.0, 10.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        row.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(2.0);
            ui.label(theme::text(
                &player.name,
                theme::semibold(14.0),
                theme::TEXT,
            ));
            ui.label(theme::text(
                &player.platform,
                theme::body(11.0),
                theme::MUTED,
            ));
        });
        row.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if player.removable {
                let remove = Quiet {
                    color: theme::DANGER_TEXT,
                    font: 11.0,
                    height: 26.0,
                    ..Quiet::new()
                };
                if theme::quiet_button(ui, !self.backend.busy(), None, "Remove", remove).clicked() {
                    self.confirm = Some(Confirm::Kick {
                        id: player.id.clone(),
                        name: player.name.trim_end_matches(" (you)").to_owned(),
                    });
                }
                ui.add_space(6.0);
            }
            for (words, state) in player.badges.iter().rev() {
                theme::pill(ui, words, *state);
            }
        });
        if !last {
            ui.painter().hline(
                rect.x_range(),
                rect.max.y - 0.5,
                Stroke::new(1.0, theme::white(0.094)),
            );
        }
    }

    fn chat(&mut self, ui: &mut Ui, state: &State) {
        theme::section_heading(ui, "Chat");
        ui.add_space(10.0);
        ScrollArea::vertical()
            .id_salt("chat-lines")
            .max_height(150.0)
            .stick_to_bottom(true)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for line in &state.chat {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let color = if line.you { theme::ACCENT } else { theme::TEXT };
                        ui.label(theme::text(&line.from, theme::semibold(13.0), color));
                        ui.label(theme::text(&line.text, theme::body(13.0), theme::TEXT));
                    });
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let send_width = 84.0;
            let width = ui.available_width() - send_width - 10.0;
            let response = ui.add_sized(
                vec2(width, theme::CONTROL),
                theme::text_field(&mut self.chat, "Say something to the room", false)
                    .char_limit(200),
            );
            let entered =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let send = theme::quiet_button(
                ui,
                true,
                Some("send"),
                "Send",
                Quiet::new().width(send_width),
            )
            .clicked();
            if (send || entered) && !self.chat.trim().is_empty() {
                self.backend.act(Action::Chat {
                    text: std::mem::take(&mut self.chat).trim().to_owned(),
                });
                response.request_focus();
            }
        });
    }

    // ---------- the Multiplayer panel ----------

    fn sidebar(
        &mut self,
        ui: &mut Ui,
        geometry: &Geometry,
        state: &State,
        view: &View,
        assets: &Assets,
    ) {
        // The column scrolls as a whole, the panel in it, as the page's
        // sidebar does: 28 above the panel, 12 under it.
        let column = Rect::from_min_max(
            pos2(geometry.sidebar.min.x, geometry.story.min.y),
            pos2(geometry.sidebar.max.x, geometry.story.max.y),
        );
        ui.scope_builder(UiBuilder::new().max_rect(column), |ui| {
            ScrollArea::vertical()
                .id_salt("multiplayer")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(column.width());
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(28.0);
                    let shadow_at = ui.painter().add(Shape::Noop);
                    let panel_at = ui.painter().add(Shape::Noop);
                    let rect = Frame::new()
                        .inner_margin(Margin::same(22))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.panel_content(ui, state, view);
                        })
                        .response
                        .rect;
                    ui.painter()
                        .set(shadow_at, theme::shadow(rect, theme::rgba(7, 16, 18, 0.4)));
                    ui.painter().set(
                        panel_at,
                        theme::frosted(
                            geometry.window,
                            rect,
                            assets,
                            &Fill::Diagonal(
                                theme::rgba(32, 46, 50, 0.93),
                                theme::rgba(13, 23, 28, 0.91),
                            ),
                            theme::rgba(175, 198, 197, 0.196),
                            0.0,
                        ),
                    );
                    ui.add_space(12.0);
                });
        });
    }

    fn panel_content(&mut self, ui: &mut Ui, state: &State, view: &View) {
        let busy = self.backend.busy();
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.set_height(25.0);
            ui.label(
                theme::text("Multiplayer", theme::semibold(15.0), theme::TEXT)
                    .extra_letter_spacing(0.375),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                theme::pill(ui, view.pill.0, view.pill.1);
            });
        });
        ui.add_space(20.0);
        let count = view.rows.len();
        for (index, row) in view.rows.iter().enumerate() {
            theme::row(ui, row.label, &row.value, row.large, index + 1 == count);
        }
        ui.add_space(14.0);
        let form = match view.main.does {
            _ if self.place == Place::Game => None,
            Does::Submit(Form::Connect) => Some(Form::Connect),
            Does::Submit(form) => Some(self.open_form.unwrap_or(form)),
            _ => self
                .open_form
                .filter(|_| state.connection == Connection::Connected),
        };
        let submitted = match form {
            Some(Form::Connect) => self.connect_form(ui, state),
            Some(Form::Create) => self.create_form(ui, state),
            Some(Form::Join) => self.join_form(ui, state),
            None => false,
        };
        let joining = form == Some(Form::Join);
        let (label, icon, enabled) = if joining {
            ("Join room".to_owned(), "users", true)
        } else {
            (view.main.label.clone(), view.main.icon, view.main.enabled())
        };
        let label = if busy && view.main.progress.is_none() {
            "Working…".to_owned()
        } else {
            label
        };
        let clicked =
            theme::primary_button(ui, enabled && !busy, icon, &label, view.main.progress).clicked();
        if (clicked || submitted) && enabled && !busy {
            match (form, &view.main.does) {
                (_, Does::InstallUpdate) => {
                    if let Some(updater) = &self.extras.updater {
                        updater.install_and_restart(ui.ctx());
                    }
                }
                (Some(form), _) => self.submit(form, state),
                (None, Does::Act(action)) => self.backend.act(action.clone()),
                _ => {}
            }
        }
        for (index, button) in view.secondary.iter().enumerate() {
            ui.add_space(if index == 0 { 12.0 } else { 8.0 });
            let open = matches!(button.then, Then::Open(form) if self.open_form == Some(form));
            let label = if open {
                "Back to creating a room"
            } else {
                button.label
            };
            let look = Quiet::new()
                .width(ui.available_width())
                .color(if button.danger {
                    theme::DANGER_TEXT
                } else {
                    theme::TEXT
                });
            if theme::quiet_button(ui, !busy, Some(button.icon), label, look).clicked() {
                match &button.then {
                    Then::Open(form) => {
                        self.open_form = if open { None } else { Some(*form) };
                    }
                    Then::Act(action) => self.backend.act(action.clone()),
                    Then::Copy(text) => {
                        ui.ctx().copy_text(text.clone());
                        self.toast = Some(("Copied.".into(), Instant::now()));
                    }
                    Then::Leave => self.confirm = Some(Confirm::Leave),
                }
            }
        }
        if let Some((words, tone)) = &view.status {
            ui.add_space(14.0);
            theme::hairline(ui, theme::white(0.094));
            ui.add_space(10.0);
            status_line(ui, words, *tone);
        }
        if !view.notes.is_empty() {
            ui.add_space(12.0);
            theme::note_box(
                ui,
                theme::rgba(80, 160, 255, 0.1),
                Some(theme::rgba(80, 160, 255, 0.4)),
                8.0,
                |ui| {
                    ui.label(theme::text(
                        view.notes.join(" "),
                        theme::body(13.0),
                        theme::TEXT,
                    ));
                },
            );
        }
        // The lobby in this window, or back in the game.
        ui.add_space(12.0);
        let (words, other) = match self.place {
            Place::Game => ("Lobby in this window instead", Place::Launcher),
            Place::Launcher => ("Lobby in the game's menu instead", Place::Game),
        };
        let look = Quiet {
            font: 11.0,
            height: 26.0,
            ..Quiet::new()
        }
        .width(ui.available_width());
        if theme::quiet_button(ui, true, None, words, look).clicked() {
            self.place = other;
            self.open_form = None;
        }
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::text(SESSION_NOTE, theme::body(10.0), theme::NOTE));
        });
    }

    /// The fields that go with Connect: the server, in a build without
    /// one of its own; the name; an invite to join at once.
    fn connect_form(&mut self, ui: &mut Ui, state: &State) -> bool {
        let mut submit = false;
        if !state.server_fixed {
            submit |= labelled_field(
                ui,
                "Server",
                None,
                theme::text_field(&mut self.server, "host:port", false),
            );
            ui.add_space(12.0);
        }
        submit |= labelled_field(
            ui,
            "Your name",
            None,
            theme::text_field(&mut self.name, "", false).char_limit(32),
        );
        ui.add_space(12.0);
        submit |= labelled_field(
            ui,
            "Invite",
            Some("(optional)"),
            theme::text_field(&mut self.invite, "K7QM2X", true).char_limit(6),
        );
        self.invite = self.invite.to_uppercase();
        ui.add_space(18.0);
        submit
    }

    fn create_form(&mut self, ui: &mut Ui, state: &State) -> bool {
        let mut submit = labelled_field(
            ui,
            "Room name",
            None,
            theme::text_field(&mut self.room_name, "", false).char_limit(48),
        );
        ui.add_space(12.0);
        let half = (ui.available_width() - 12.0) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.vertical(|ui| {
                ui.set_width(half);
                theme::field_label(ui, "Players", None);
                ui.add_space(6.0);
                theme::select(ui, |ui| {
                    ComboBox::from_id_salt("max-players")
                        .icon(theme::chevron)
                        .width(half)
                        .selected_text(theme::text(
                            format!("{} players", self.max_players),
                            theme::body(13.0),
                            theme::TEXT,
                        ))
                        .show_ui(ui, |ui| {
                            for size in ROOM_SIZES {
                                ui.selectable_value(
                                    &mut self.max_players,
                                    size,
                                    format!("{size} players"),
                                );
                            }
                        });
                });
            });
            ui.vertical(|ui| {
                ui.set_width(half);
                theme::field_label(ui, "Rules", None);
                ui.add_space(6.0);
                if state.rules.is_empty() {
                    return;
                }
                let chosen = self
                    .rules
                    .clone()
                    .filter(|name| state.rules.iter().any(|rules| rules.name == *name))
                    .unwrap_or_else(|| state.rules[0].name.clone());
                let shown = |name: &str| {
                    if state.rules.first().is_some_and(|rules| rules.name == name) {
                        format!("{name} (default)")
                    } else {
                        name.to_owned()
                    }
                };
                let mut picked = chosen.clone();
                theme::select(ui, |ui| {
                    ComboBox::from_id_salt("rules")
                        .icon(theme::chevron)
                        .width(half)
                        .selected_text(theme::text(shown(&picked), theme::body(13.0), theme::TEXT))
                        .show_ui(ui, |ui| {
                            for rules in &state.rules {
                                ui.selectable_value(
                                    &mut picked,
                                    rules.name.clone(),
                                    shown(&rules.name),
                                )
                                .on_hover_text(&rules.description);
                            }
                        });
                });
                self.rules = Some(picked);
            });
        });
        ui.add_space(12.0);
        submit |= labelled_field(
            ui,
            "Password",
            Some("(optional)"),
            theme::text_field(&mut self.create_password, "", false)
                .password(true)
                .char_limit(64),
        );
        ui.add_space(18.0);
        submit
    }

    fn join_form(&mut self, ui: &mut Ui, state: &State) -> bool {
        let mut submit = labelled_field(
            ui,
            "Invite",
            None,
            theme::text_field(&mut self.join_invite, "K7QM2X", true).char_limit(6),
        );
        self.join_invite = self.join_invite.to_uppercase();
        if state.servers.len() > 1 {
            ui.add_space(12.0);
            let selected = self
                .join_server
                .as_deref()
                .filter(|name| state.servers.iter().any(|server| server.name == *name))
                .unwrap_or("");
            let mut picked = selected.to_owned();
            theme::select(ui, |ui| {
                ComboBox::from_id_salt("join-region")
                    .icon(theme::chevron)
                    .selected_text(theme::text(
                        if picked.is_empty() {
                            "Find automatically across regions"
                        } else {
                            picked.as_str()
                        },
                        theme::body(13.0),
                        theme::TEXT,
                    ))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut picked,
                            String::new(),
                            "Find automatically across regions",
                        );
                        for server in &state.servers {
                            let label = if server.reachable {
                                server.name.clone()
                            } else {
                                format!("{} (unavailable)", server.name)
                            };
                            ui.selectable_value(&mut picked, server.name.clone(), label);
                        }
                    });
            });
            self.join_server = (!picked.is_empty()).then_some(picked);
        } else {
            self.join_server = None;
        }
        ui.add_space(12.0);
        submit |= labelled_field(
            ui,
            "Room password",
            Some("(if it has one)"),
            theme::text_field(&mut self.join_password, "", false)
                .password(true)
                .char_limit(64),
        );
        ui.add_space(18.0);
        submit
    }

    /// Sends a form.
    fn submit(&mut self, form: Form, state: &State) {
        match form {
            Form::Connect => {
                // With its own server, the invite goes where the server
                // would: the launcher connects to its own and joins.
                let invite = self.invite.trim();
                let server = match (state.server_fixed, invite.is_empty()) {
                    (true, _) => invite.to_owned(),
                    (false, true) => self.server.trim().to_owned(),
                    (false, false) => format!("{} {invite}", self.server.trim()),
                };
                self.backend.act(Action::Connect {
                    server,
                    name: self.name.clone(),
                });
            }
            Form::Create => {
                let room = self.room_name.trim();
                self.backend.act(Action::Create {
                    room: if room.is_empty() {
                        "TPF3-MP room".to_owned()
                    } else {
                        room.to_owned()
                    },
                    max_players: self.max_players,
                    password: non_empty(&self.create_password),
                    rules: self.rules.clone(),
                    start_save: None,
                    listing: None,
                    competitive: false,
                });
            }
            Form::Join => {
                self.backend.act(Action::Join {
                    invite: self.join_invite.trim().to_uppercase(),
                    server: self.join_server.clone().filter(|name| {
                        state.servers.len() > 1
                            && state.servers.iter().any(|server| server.name == *name)
                    }),
                    password: non_empty(&self.join_password),
                });
                self.join_server = None;
                self.open_form = None;
            }
        }
    }

    // ---------- along the bottom ----------

    /// Your game: where it is, whether the mod is in, and its folder.
    fn files(&mut self, ui: &mut Ui, geometry: &Geometry, state: &State, assets: &Assets) {
        let bar = geometry.files;
        ui.painter().add(theme::frosted(
            geometry.window,
            bar,
            assets,
            &Fill::Flat(theme::panel_fill(0.75)),
            theme::white(0.094),
            0.0,
        ));
        let installed_mod = self.installed_mod();
        let painter = ui.painter();
        let middle = bar.center().y;
        painter.text(
            pos2(bar.min.x + 28.0, middle),
            egui::Align2::LEFT_CENTER,
            "YOUR GAME",
            theme::semibold(11.0),
            theme::TEXT,
        );
        assets.icon(
            painter,
            "folder",
            Rect::from_center_size(pos2(bar.min.x + 137.0, middle), Vec2::splat(20.0)),
            theme::MUTED,
        );
        let label = match &state.installed {
            Some(installed) => format!("GAME FOLDER · STEAM BUILD {}", installed.build),
            None => "GAME FOLDER".to_owned(),
        };
        let path = state
            .installed
            .as_ref()
            .map_or("Transport Fever 3 was not found in Steam.", |installed| {
                installed.dir.as_str()
            });
        let text_left = bar.min.x + 159.0;
        let button = Rect::from_min_size(
            pos2(bar.max.x - 20.0 - 118.0, middle - 20.0),
            vec2(118.0, 40.0),
        );
        let pill_text = match &installed_mod {
            Some(version) => format!("TPF3-MP mod {version}"),
            None => "TPF3-MP mod not installed".to_owned(),
        };
        let pill_galley = painter.layout_no_wrap(pill_text.clone(), theme::body(10.0), theme::TEXT);
        let pill_width = pill_galley.size().x + 14.0;
        let pill_rect = Rect::from_min_size(
            pos2(button.min.x - 29.0 - pill_width, middle - 12.5),
            vec2(pill_width, 25.0),
        );
        let text_width = (pill_rect.min.x - 12.0 - text_left).max(40.0);
        let label_galley = painter.layout_no_wrap(label, theme::body(9.0), theme::MUTED);
        painter.galley(
            pos2(text_left, bar.min.y + 17.0),
            label_galley,
            theme::MUTED,
        );
        let path_galley =
            painter.layout(path.to_owned(), theme::body(11.0), theme::TEXT, text_width);
        painter.galley(pos2(text_left, bar.min.y + 35.0), path_galley, theme::TEXT);
        let pill_state = if installed_mod.is_some() {
            Pill::Ready
        } else {
            Pill::Update
        };
        let pill = ui.put(pill_rect, |ui: &mut Ui| {
            theme::pill(ui, &pill_text, pill_state)
        });
        if installed_mod.is_none() {
            pill.on_hover_text(if cfg!(windows) {
                "Use Settings → Repair installation, then activate TPF3-MP in the game's Mod Hub."
            } else {
                "Run ./install.sh from the TPF3-MP folder, then activate TPF3-MP in the game's Mod Hub."
            });
        }
        let folder = state
            .installed
            .as_ref()
            .map(|installed| installed.dir.clone());
        let look = Quiet::new().color(theme::MUTED).width(118.0);
        let open = ui.put(button, |ui: &mut Ui| {
            theme::quiet_button(ui, folder.is_some(), Some("folder"), "Open folder", look)
        });
        if open.clicked()
            && let Some(folder) = folder
        {
            open_folder(&folder);
        }
    }

    /// "Unofficial launcher", the version, protocol and commit, left; the
    /// support code and the log session, right.
    fn footer(&mut self, ui: &mut Ui, geometry: &Geometry, state: &State) {
        let footer = geometry.footer;
        let painter = ui.painter();
        let mut job = egui::text::LayoutJob::default();
        let format = |color| egui::TextFormat::simple(theme::body(9.0), color);
        job.append("Unofficial launcher", 0.0, format(theme::FOOTER));
        job.append(&footer_build(), 8.0, format(theme::FOOTER));
        let galley = painter.layout_job(job);
        painter.galley(
            pos2(footer.min.x, footer.min.y + 5.0),
            galley,
            theme::FOOTER,
        );
        // Right to left: the support code, then the log session.
        let codes: Vec<(&str, &str, &str, &str)> = [
            state.support_id.as_deref().map(|code| {
                (
                    code,
                    "Support code",
                    "Copy the support code",
                    "Quote this to the server's operator when something goes wrong",
                )
            }),
            state.log_session.as_deref().map(|code| {
                (
                    code,
                    "Log session",
                    "Copy the log session",
                    "Every line of this launcher's logs, the game's too, goes to the server under this code",
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        if codes.is_empty() {
            return;
        }
        let row = Rect::from_min_max(
            pos2(footer.min.x, footer.min.y),
            pos2(footer.max.x + 0.0, footer.min.y + 18.0),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(row)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (index, (code, name, copy, hover)) in codes.into_iter().enumerate() {
                    if index > 0 {
                        ui.add_space(10.0);
                    }
                    if theme::quiet_button(ui, true, None, "Copy", Quiet::new().small())
                        .on_hover_text(copy)
                        .clicked()
                    {
                        ui.ctx().copy_text(code.to_owned());
                        self.toast = Some(("Copied.".into(), Instant::now()));
                    }
                    ui.label(
                        theme::text(code, theme::mono(9.0), theme::TEXT).extra_letter_spacing(1.08),
                    )
                    .on_hover_text(hover);
                    ui.label(theme::text(name, theme::body(9.0), theme::FOOTER));
                }
            },
        );
    }

    // ---------- dialogs ----------

    fn settings_dialog(&mut self, ctx: &egui::Context, state: &State, view: &View) {
        if !self.settings {
            return;
        }
        let height = ctx.content_rect().height() - 48.0;
        let modal = Modal::new(Id::new("settings"))
            .frame(dialog_frame())
            .backdrop_color(theme::rgba(4, 16, 20, 0.66))
            .show(ctx, |ui| {
                ui.set_width(524.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut close = false;
                ui.horizontal(|ui| {
                    ui.label(
                        theme::text("Settings", theme::semibold(26.0), theme::TEXT)
                            .extra_letter_spacing(-0.6),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close |= theme::icon_button(ui, "close", "Close settings").clicked();
                    });
                });
                ui.add_space(24.0);
                ScrollArea::vertical()
                    .id_salt("settings-body")
                    .max_height(height - 170.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.settings_body(ui, state, view);
                    });
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    close |= theme::quiet_button(ui, true, None, "Done", Quiet::new().width(100.0))
                        .clicked();
                });
                close
            });
        if modal.inner || modal.should_close() {
            self.settings = false;
        }
    }

    /// The mods dialogs, from the buttons beside Settings: the mod manager
    /// (the native mods installed) and the mod browser (those the server
    /// offers, and the proposed ones), with tabs between them. Each is a
    /// table of mods beside the details of the one picked.
    fn mods_dialog(&mut self, ctx: &egui::Context, state: &State) {
        let Some(page) = self.mods else {
            return;
        };
        let height = ctx.content_rect().height() - 48.0;
        let modal = Modal::new(Id::new("mods"))
            .frame(dialog_frame())
            .backdrop_color(theme::rgba(4, 16, 20, 0.66))
            .show(ctx, |ui| {
                ui.set_width(TABLE_WIDTH + PANES_GAP + DETAIL_WIDTH);
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut close = false;
                ui.horizontal(|ui| {
                    let title = match page {
                        ModsPage::Manager => "Mod manager",
                        ModsPage::Browser => "Mod browser",
                    };
                    ui.label(
                        theme::text(title, theme::semibold(26.0), theme::TEXT)
                            .extra_letter_spacing(-0.6),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close |= theme::icon_button(ui, "close", "Close mods").clicked();
                    });
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let installed = installed_rows(state).len();
                    let tabs = [
                        (
                            ModsPage::Manager,
                            "archive",
                            format!("Installed ({installed})"),
                        ),
                        (ModsPage::Browser, "search", "Browse".to_owned()),
                    ];
                    for (tab, icon, words) in tabs {
                        let look = if tab == page {
                            Quiet {
                                color: theme::TEXT,
                                fill: theme::white(0.08),
                                border: theme::white(0.45),
                                ..Quiet::new()
                            }
                        } else {
                            Quiet::new().color(theme::MUTED)
                        };
                        if theme::quiet_button(ui, true, Some(icon), &words, look).clicked() {
                            self.mods = Some(tab);
                        }
                    }
                });
                ui.add_space(16.0);
                let body = (height - 240.0).max(220.0);
                match page {
                    ModsPage::Manager => self.mod_manager(ui, state, body),
                    ModsPage::Browser => self.mod_browser(ui, state, body),
                }
                ui.add_space(16.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    close |= theme::quiet_button(ui, true, None, "Done", Quiet::new().width(100.0))
                        .clicked();
                });
                close
            });
        if modal.inner || modal.should_close() {
            self.mods = None;
        }
    }

    fn settings_body(&mut self, ui: &mut Ui, state: &State, view: &View) {
        if cfg!(windows) && crate::update::Install::of_running().is_ok() {
            group(ui, |ui| {
                ui.label(theme::text(
                    "Installation",
                    theme::semibold(14.0),
                    theme::TEXT,
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for (label, uninstall) in [("Repair installation", false), ("Uninstall…", true)]
                    {
                        if theme::quiet_button(ui, state.room.is_none(), None, label, Quiet::new())
                            .clicked()
                        {
                            match crate::installation::open_maintenance(uninstall) {
                                Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                                Err(error) => tracing::error!(%error, "cannot open setup"),
                            }
                        }
                    }
                });
                ui.label(theme::text("Close the game before changing the installation. Your saves and settings are kept.", theme::body(12.0), theme::MUTED));
            });
            ui.add_space(14.0);
        }
        if state.server_fixed || state.server_default.is_some() {
            self.server_settings(ui, state);
            ui.add_space(14.0);
        }
        group(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                icon(ui, "download", 19.0, theme::TEXT);
                ui.label(theme::text(
                    "Launcher and mod updates",
                    theme::semibold(14.0),
                    theme::TEXT,
                ));
            });
            ui.add_space(8.0);
            ui.label(theme::text(
                &view.updates.copy,
                theme::body(12.0),
                theme::MUTED,
            ));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let can_check = self.extras.updater.as_ref().is_some_and(|updater| {
                    !matches!(
                        updater.state(),
                        UpdateState::Off(_) | UpdateState::Installing { .. }
                    )
                });
                if theme::quiet_button(
                    ui,
                    can_check,
                    Some("refresh"),
                    "Check for updates",
                    Quiet::new(),
                )
                .clicked()
                    && let Some(updater) = &self.extras.updater
                {
                    updater.check();
                }
                let installable = view.updates.installable && state.room.is_none();
                let install = theme::primary_small(
                    ui,
                    installable,
                    Some("download"),
                    "Restart and update",
                    false,
                );
                let install = if state.room.is_some() {
                    install.on_hover_text("Leave the room first.")
                } else {
                    install
                };
                if install.clicked()
                    && let Some(updater) = &self.extras.updater
                {
                    updater.install_and_restart(ui.ctx());
                }
            });
        });
        if let Some(on) = state.diagnostics {
            ui.add_space(14.0);
            group(ui, |ui| {
                ui.label(theme::text(
                    "Send diagnostics",
                    theme::body(14.0),
                    theme::TEXT,
                ));
                ui.add_space(4.0);
                let mut sending = on;
                theme::select(ui, |ui| {
                    ComboBox::from_id_salt("diagnostics")
                        .icon(theme::chevron)
                        .width(ui.available_width())
                        .selected_text(theme::text(
                            if on { "On" } else { "Off" },
                            theme::body(13.0),
                            theme::TEXT,
                        ))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut sending, true, "On");
                            ui.selectable_value(&mut sending, false, "Off");
                        });
                });
                if sending != on {
                    self.backend.act(Action::Diagnostics { on: sending });
                }
                ui.add_space(8.0);
                ui.label(theme::text(
                    "The launcher's log, with paths and codes removed, goes to the server so its \
                     operator can see what went wrong from your support code.",
                    theme::body(12.0),
                    theme::MUTED,
                ));
            });
        }
        ui.add_space(14.0);
        group(ui, |ui| {
            ui.label(theme::text(
                "About this launcher",
                theme::semibold(14.0),
                theme::TEXT,
            ));
            ui.add_space(12.0);
            let server = state.server_name.clone().or_else(|| state.server.clone());
            let server = match (&server, &state.server_version) {
                (Some(server), Some(version)) => format!("{server} · {version}"),
                (Some(server), None) => server.clone(),
                (None, _) => "—".to_owned(),
            };
            let game = state.installed.as_ref().map_or_else(
                || "Not found".to_owned(),
                |installed| format!("Steam build {}", installed.build),
            );
            let platform = if cfg!(windows) {
                "Windows"
            } else if cfg!(target_os = "macos") {
                "macOS"
            } else {
                "Linux"
            };
            for (name, value) in [
                (
                    "Multiplayer mod",
                    format!("TPF3-MP {}", env!("CARGO_PKG_VERSION")),
                ),
                (
                    "Protocol",
                    format!(
                        "{} · game link {}",
                        tpf3mp_proto::PROTOCOL_VERSION,
                        tpf3mp_agent::about::Build::this().bridge
                    ),
                ),
                (
                    "Build",
                    format!(
                        "{} · {}",
                        tpf3mp_agent::about::COMMIT,
                        tpf3mp_agent::about::BUILT
                    ),
                ),
                ("Server", server),
                ("Game version", game),
                ("Platform", platform.to_owned()),
            ] {
                ui.horizontal(|ui| {
                    ui.set_height(30.0);
                    ui.label(theme::text(name, theme::body(12.0), theme::MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(theme::text(value, theme::body(12.0), theme::TEXT));
                    });
                });
            }
            ui.add_space(12.0);
            theme::hairline(ui, theme::white(0.094));
            ui.add_space(12.0);
            ui.label(theme::text(
                "Unofficial launcher. Not made or endorsed by Urban Games.\nDesign from \
                 tearded's TPF2 Multiplayer Launcher (MIT). City image © Urban Games.",
                theme::body(12.0),
                theme::MUTED,
            ));
        });
    }

    /// The server setting (D12, as amended): one-server builds may enter a
    /// server address. A multi-region release shows its trusted routes and
    /// can return a pinned route to automatic regional play.
    fn server_settings(&mut self, ui: &mut Ui, state: &State) {
        group(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                icon(ui, "link", 19.0, theme::TEXT);
                ui.label(theme::text("Server", theme::semibold(14.0), theme::TEXT));
            });
            ui.add_space(8.0);
            ui.label(theme::text(
                server_setting_line(state),
                theme::body(12.0),
                theme::MUTED,
            ));
            ui.add_space(14.0);
            let regional = state.regional || state.servers.len() > 1;
            let setting = ServerSetting::of(&self.server_setting, state);
            if !regional {
                let hint = state.server_default.as_deref().unwrap_or("host:port");
                let entered = labelled_field(
                    ui,
                    "Server address",
                    Some("host:port"),
                    theme::text_field(&mut self.server_setting, hint, false).char_limit(128),
                );
                if let Some(problem) = &setting.problem {
                    ui.add_space(6.0);
                    status_line(ui, problem, Tone::Error);
                }
                ui.add_space(14.0);
                let mut chosen = None;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let apply =
                        theme::primary_small(ui, setting.can_apply, None, "Use this server", false);
                    if apply.clicked() || (entered && setting.can_apply) {
                        chosen = Some(self.server_setting.trim().to_owned());
                    }
                    let reset = theme::quiet_button(
                        ui,
                        setting.can_reset,
                        Some("refresh"),
                        "Reset to default",
                        Quiet::new(),
                    );
                    if reset.clicked() {
                        chosen = Some(String::new());
                    }
                });
                if let Some(server) = chosen {
                    self.backend.act(Action::SetServer { server });
                }
            } else if setting.can_reset {
                let reset = theme::quiet_button(
                    ui,
                    true,
                    Some("refresh"),
                    "Reset to default",
                    Quiet::new(),
                );
                if reset.clicked() {
                    self.backend.act(Action::SetServer {
                        server: String::new(),
                    });
                }
            }
            ui.add_space(10.0);
            let note = if state.room.is_some() {
                "Leave the room to change the server."
            } else if regional && state.servers.is_empty() {
                "This release routes only to its trusted servers. Reset to default returns to \
                 automatic regional play."
            } else if regional {
                "Rooms go to the closest listed server. Invites are checked across these trusted \
                 regions; choose one on the join form if a code is ambiguous."
            } else {
                "Changing the server disconnects you and connects to the new one. Invites join \
                 rooms on your server only: to play with friends on another server, all of you \
                 set the same one here."
            };
            ui.label(theme::text(note, theme::body(12.0), theme::MUTED));
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm.clone() else {
            return;
        };
        let (title, message, yes) = match &confirm {
            Confirm::Kick { name, .. } => (
                format!("Remove {name} from the room?"),
                "They cannot come back to this room.",
                "Remove them",
            ),
            Confirm::Leave => (
                "Leave the room?".to_owned(),
                "You can come back with its invite while it is open.",
                "Leave",
            ),
            Confirm::Quit => (
                "Quit TPF3-MP?".to_owned(),
                "You are in a room. Quitting takes you out of the game the others are playing.",
                "Quit",
            ),
        };
        let modal = Modal::new(Id::new("confirm"))
            .frame(dialog_frame())
            .backdrop_color(theme::rgba(4, 16, 20, 0.66))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                let mut answer = None;
                ui.horizontal(|ui| {
                    ui.label(
                        theme::text(&title, theme::semibold(22.0), theme::TEXT)
                            .extra_letter_spacing(-0.5),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::icon_button(ui, "close", "Cancel").clicked() {
                            answer = Some(false);
                        }
                    });
                });
                ui.add_space(12.0);
                ui.label(theme::text(message, theme::body(13.0), theme::MUTED));
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::primary_small(ui, true, None, yes, true).clicked() {
                        answer = Some(true);
                    }
                    if theme::quiet_button(ui, true, None, "Cancel", Quiet::new()).clicked() {
                        answer = Some(false);
                    }
                });
                answer
            });
        let answer = modal
            .inner
            .or_else(|| modal.should_close().then_some(false));
        match answer {
            Some(true) => {
                self.confirm = None;
                match confirm {
                    Confirm::Kick { id, .. } => self.backend.act(Action::Kick { player: id }),
                    Confirm::Leave => self.backend.act(Action::Leave),
                    Confirm::Quit => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    /// Requires a deliberate choice before native plugin code can be fetched
    /// or enabled. The agent binds this answer to the exact displayed request.
    fn plugin_consent_dialog(&mut self, ctx: &egui::Context, state: &State) {
        let Some(prompt) = state.native_consent.clone() else {
            return;
        };
        let modal = Modal::new(Id::new("native-plugin-consent"))
            .frame(dialog_frame())
            .backdrop_color(theme::rgba(4, 16, 20, 0.72))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(theme::text(
                    "Review native plugin code",
                    theme::semibold(22.0),
                    theme::TEXT,
                ));
                ui.add_space(10.0);
                ui.label(theme::text(
                    "These packages run executable code inside Transport Fever 3 with your account's rights on this PC. Approval is required before this action can fetch, install, enable, or roll back native plugin code.",
                    theme::body(13.0),
                    theme::DIFFERS_TEXT,
                ));
                ui.add_space(6.0);
                ui.label(theme::text(
                    format!("Requested action: {}", prompt.action),
                    theme::semibold(13.0),
                    theme::TEXT,
                ));
                ui.add_space(14.0);
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for package in &prompt.packages {
                            ui.separator();
                            ui.label(theme::text(
                                format!("{} {}", package.name, package.version),
                                theme::semibold(15.0),
                                theme::TEXT,
                            ));
                            ui.label(theme::text(
                                format!("Package: {}", package.id),
                                theme::body(12.0),
                                theme::MUTED,
                            ));
                            let source = if package.source == "project" {
                                "TPF3-MP project index".to_owned()
                            } else {
                                format!("Authenticated server {}", package.source)
                            };
                            ui.label(theme::text(
                                format!("Validated source: {source}"),
                                theme::body(12.0),
                                theme::TEXT,
                            ));
                            let authors = if package.authors.is_empty() {
                                "Not listed".to_owned()
                            } else {
                                package.authors.join(", ")
                            };
                            ui.label(theme::text(
                                format!("Author(s): {authors}"),
                                theme::body(12.0),
                                theme::TEXT,
                            ));
                            ui.label(theme::text(
                                "Full package identity SHA-256:",
                                theme::body(12.0),
                                theme::MUTED,
                            ));
                            // Break the digest into groups so its complete
                            // value can wrap inside the dialog at narrow
                            // window sizes.
                            let digest = package
                                .digest
                                .as_bytes()
                                .chunks(8)
                                .map(|part| String::from_utf8_lossy(part).into_owned())
                                .collect::<Vec<_>>()
                                .join(" ");
                            ui.monospace(digest);
                            if package.approved {
                                ui.label(theme::text(
                                    "Already approved for this exact source and package identity.",
                                    theme::body(11.0),
                                    theme::MUTED,
                                ));
                            }
                        }
                    });
                ui.add_space(16.0);
                let mut answer = None;
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if theme::primary_small(
                        ui,
                        true,
                        None,
                        "Approve and continue",
                        true,
                    )
                    .clicked()
                    {
                        answer = Some(true);
                    }
                    if theme::quiet_button(ui, true, None, "Decline", Quiet::new()).clicked() {
                        answer = Some(false);
                    }
                });
                answer
            });
        let answer = modal
            .inner
            .or_else(|| modal.should_close().then_some(false));
        if let Some(approved) = answer {
            self.backend.act(Action::NativeConsentDecision {
                request_id: prompt.request_id,
                approved,
            });
        }
    }

    /// Closing the window during a game asks first.
    fn guard_quit(&mut self, ctx: &egui::Context, state: &State) {
        let close = ctx.input(|input| input.viewport().close_requested());
        if close && state.room.is_some() && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Quit);
        }
    }

    /// A message for a moment, low in the middle: an error, "Copied.".
    fn draw_toast(&mut self, ctx: &egui::Context, window: Rect) {
        let Some((words, since)) = &self.toast else {
            return;
        };
        if since.elapsed() >= TOAST {
            self.toast = None;
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("toast"),
        ));
        let ink = theme::rgba(27, 41, 36, 1.0);
        let galley = painter.layout(words.clone(), theme::body(14.0), ink, 560.0);
        let size = galley.size() + vec2(40.0, 24.0);
        let rect = Rect::from_min_size(
            pos2(
                window.center().x - size.x / 2.0,
                window.max.y - 23.0 - size.y,
            ),
            size,
        );
        painter.add(theme::shadow(rect, theme::rgba(0, 0, 0, 0.3)));
        painter.rect_filled(rect, 3.0, theme::rgba(236, 238, 232, 0.97));
        painter.galley(rect.min + vec2(20.0, 12.0), galley, ink);
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    /// The TPF3-MP version whose mod the installer put in the game, looked
    /// at every few seconds, so running the installer shows here.
    fn installed_mod(&mut self) -> Option<String> {
        if let Some(shown) = &self.extras.shown.installed_mod {
            return shown.clone();
        }
        let stale = self
            .installed_mod
            .as_ref()
            .is_none_or(|(when, _)| when.elapsed() >= INSTALL_CHECK);
        if stale {
            let installed = tpf3mp_agent::launcher::setup::data_dir()
                .ok()
                .and_then(|dir| crate::installed::installed_mod(&dir));
            self.installed_mod = Some((Instant::now(), installed));
        }
        self.installed_mod.as_ref()?.1.clone()
    }
}

impl<B: Backend> eframe::App for LauncherApp<B> {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

/// The wordmark over the city, the line under it, and whose image it is.
fn banner(ui: &mut Ui, assets: &Assets) {
    ui.add_space(76.0);
    ui.horizontal(|ui| {
        ui.add_space(28.0);
        ui.add(
            egui::Image::from_texture(egui::load::SizedTexture::new(
                assets.wordmark.id(),
                vec2(360.0, 153.0),
            ))
            .alt_text("Transport Fever 3 Multiplayer"),
        );
    });
    ui.add_space(30.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        Rect::from_min_size(pos2(rect.min.x + 28.0, rect.center().y), vec2(33.0, 2.0)),
        0.0,
        theme::ACCENT,
    );
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "Mod by ",
        0.0,
        egui::TextFormat::simple(theme::body(12.0), theme::CAPTION),
    );
    job.append(
        "the TPF3-MP team",
        0.0,
        egui::TextFormat::simple(theme::semibold(12.0), Color32::WHITE),
    );
    let galley = painter.layout_job(job);
    painter.galley(
        pos2(rect.min.x + 74.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::CAPTION,
    );
    ui.add_space(48.0);
    ui.horizontal(|ui| {
        ui.add_space(28.0);
        ui.label(
            theme::text(
                "Image © Urban Games",
                theme::mono(9.0),
                theme::SCENE_CAPTION,
            )
            .extra_letter_spacing(0.54),
        );
    });
    ui.add_space(34.0);
}

/// The latest release's notes, under "How to play".
fn release_panel(ui: &mut Ui, geometry: &Geometry, assets: &Assets, notes: Option<&ReleaseNotes>) {
    let notes = notes.map_or(Notes::Checking, ReleaseNotes::get);
    panel(
        ui,
        geometry,
        assets,
        Fill::Flat(theme::panel_fill(0.65)),
        0.0,
        |ui| {
            ui.add_space(7.0);
            theme::section_heading(ui, "Release notes");
            ui.add_space(16.0);
            let headline = match &notes {
                Notes::Checking => "Checking release…".to_owned(),
                Notes::Unavailable => "Release notes unavailable".to_owned(),
                Notes::Release { version, .. } => format!("Version {version}"),
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                rules_chip(ui, "Stable");
                ui.label(
                    theme::text(headline, theme::body(20.0), theme::TEXT)
                        .extra_letter_spacing(-0.5),
                );
            });
            ui.add_space(14.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            match notes {
                Notes::Checking => {
                    ui.label(theme::text(
                        "Fetching the latest release.",
                        theme::body(13.0),
                        theme::MUTED,
                    ));
                }
                Notes::Unavailable => {
                    ui.label(theme::text(
                        "Could not load the releases from GitHub. You can still play.",
                        theme::body(13.0),
                        theme::MUTED,
                    ));
                }
                Notes::Release { blocks, .. } if blocks.is_empty() => {
                    ui.label(theme::text(
                        "No notes for this release.",
                        theme::body(13.0),
                        theme::MUTED,
                    ));
                }
                Notes::Release { blocks, .. } => {
                    for block in blocks {
                        match block {
                            Block::Heading(words) => {
                                ui.add_space(4.0);
                                ui.label(theme::text(words, theme::semibold(13.0), theme::TEXT));
                            }
                            Block::Item(words) => {
                                ui.label(theme::text(
                                    format!("•  {words}"),
                                    theme::body(13.0),
                                    theme::MUTED,
                                ));
                            }
                            Block::Paragraph(words) => {
                                ui.label(theme::text(words, theme::body(13.0), theme::MUTED));
                            }
                        }
                    }
                }
            }
        },
    );
}

/// A frosted panel of the left column, as wide as it, its padding the
/// page's; its size is known after its content, so it is put under it.
fn panel(
    ui: &mut Ui,
    geometry: &Geometry,
    assets: &Assets,
    fill: Fill,
    min_height: f32,
    add: impl FnOnce(&mut Ui),
) {
    let under = ui.painter().add(Shape::Noop);
    let rect = Frame::new()
        .inner_margin(Margin {
            left: 27,
            right: 28,
            top: 28,
            bottom: 28,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height((min_height - 56.0).max(0.0));
            add(ui);
        })
        .response
        .rect;
    ui.painter().set(
        under,
        theme::frosted(
            geometry.window,
            rect,
            assets,
            &fill,
            theme::white(0.094),
            3.0,
        ),
    );
}

/// The small outlined tag before the room's name: its rules.
fn rules_chip(ui: &mut Ui, rules: &str) -> egui::Response {
    let words = rules.to_uppercase();
    let galley = ui
        .painter()
        .layout_no_wrap(words.clone(), theme::body(8.0), theme::MUTED);
    let size = galley.size() + vec2(10.0 + 0.64 * words.len() as f32, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, theme::white(0.094)),
        StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        RichText::new(&words).text(),
        theme::body(8.0),
        theme::MUTED,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &words));
    response
}

impl<B: Backend> LauncherApp<B> {
    /// The room's native mods in its lobby, whether this game runs each,
    /// and what the launcher is doing to get them.
    fn room_native(&mut self, ui: &mut Ui, state: &State) {
        ui.label(theme::text(
            "The room's native mods",
            theme::semibold(12.0),
            theme::TEXT,
        ));
        ui.add_space(6.0);
        for row in &state.room_native {
            let (mark, color) = match row.have {
                ModHave::Yes => ("ready", theme::TEXT),
                ModHave::OtherVersion => ("other version or settings", theme::DIFFERS_TEXT),
                ModHave::No => ("missing", theme::DIFFERS_TEXT),
            };
            ui.label(theme::text(
                format!("{} {} · {mark}", row.name, row.version),
                theme::body(12.0),
                color,
            ));
            ui.add_space(3.0);
        }
        match &state.native_busy {
            Some(busy) => {
                ui.add_space(4.0);
                ui.label(theme::text(busy, theme::body(12.0), theme::MUTED));
            }
            None if state.room_native.iter().any(|row| row.have != ModHave::Yes) => {
                ui.add_space(6.0);
                if theme::quiet_button(
                    ui,
                    state.native.available,
                    Some("download"),
                    "Get the room's native mods",
                    Quiet::new(),
                )
                .clicked()
                {
                    self.backend.act(Action::NativeMatchRoom);
                }
            }
            None => {}
        }
    }

    /// What the launcher is doing with native mods, or what went wrong.
    fn mods_status(ui: &mut Ui, state: &State) {
        if let Some(busy) = &state.native_busy {
            ui.label(theme::text(busy, theme::body(12.0), theme::TEXT));
            ui.add_space(10.0);
        }
        if let Some(problem) = &state.native.problem {
            ui.label(theme::text(problem, theme::body(12.0), theme::DIFFERS_TEXT));
            ui.add_space(10.0);
        }
    }

    /// The mod manager: the native mods installed, in the order the game
    /// loads them, `height` high.
    fn mod_manager(&mut self, ui: &mut Ui, state: &State, height: f32) {
        let installed = installed_rows(state);
        Self::mods_status(ui, state);
        if installed.is_empty() {
            mod_card(ui, |ui| {
                ui.label(theme::text(
                    "No native mods installed yet",
                    theme::semibold(14.0),
                    theme::TEXT,
                ));
                ui.add_space(6.0);
                ui.label(theme::text(
                    "Native mods change the game itself, such as Big Maps. Find one in the mod \
                     browser, or join a room that uses some: the launcher gets them for you and \
                     starts the game again with them.",
                    theme::body(12.0),
                    theme::MUTED,
                ));
                ui.add_space(12.0);
                if theme::quiet_button(
                    ui,
                    true,
                    Some("search"),
                    "Open the mod browser",
                    Quiet::new(),
                )
                .clicked()
                {
                    self.mods = Some(ModsPage::Browser);
                }
            });
            return;
        }
        let on = installed.iter().filter(|row| row.enabled).count();
        let updates = installed.iter().filter(|row| has_update(row)).count();
        let mut summary = format!("{} installed · {on} on", installed.len());
        match updates {
            0 => {}
            1 => summary.push_str(" · 1 update"),
            n => summary.push_str(&format!(" · {n} updates")),
        }
        summary.push_str(" · changes take effect the next time the launcher starts the game");
        ui.label(theme::text(summary, theme::body(12.0), theme::MUTED));
        ui.add_space(12.0);
        self.mods_table(ui, state, &installed, ModsPage::Manager, height - 30.0);
    }

    /// The mod browser: the native mods the server offers for this game,
    /// to search, and when the player asks, the proposed ones; `height`
    /// high.
    fn mod_browser(&mut self, ui: &mut Ui, state: &State, height: f32) {
        let native = &state.native;
        let idle = state.native_busy.is_none();
        let controls = vec2(TABLE_WIDTH + PANES_GAP + DETAIL_WIDTH, 62.0);
        ui.allocate_ui_with_layout(controls, Layout::left_to_right(Align::Max), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.vertical(|ui| {
                ui.set_width(TABLE_WIDTH - 8.0);
                labelled_field(
                    ui,
                    "Search",
                    None,
                    theme::text_field(
                        &mut self.mod_search,
                        "A mod's name, author or description",
                        false,
                    ),
                );
            });
            if theme::quiet_button(ui, idle, Some("refresh"), "Check the index", Quiet::new())
                .clicked()
            {
                self.backend.act(Action::NativeRefresh);
            }
            if native.server.is_some() {
                let words = if native.proposed_shown {
                    "Hide proposed"
                } else {
                    "Show proposed"
                };
                if theme::quiet_button(ui, idle, None, words, Quiet::new()).clicked() {
                    self.backend.act(Action::NativeShowProposed {
                        on: !native.proposed_shown,
                    });
                }
            }
        });
        ui.add_space(8.0);
        let mut line = match &native.server {
            Some(server) => format!("The mods {server} offers for this game."),
            None => "The mods offered for this game.".to_owned(),
        };
        if native.proposed_shown {
            line.push_str(
                " Proposed mods are open pull requests to the server's list of mods: nobody has \
                 reviewed them yet. Install one only if you trust its authors.",
            );
        }
        ui.label(theme::text(
            line,
            theme::body(11.0),
            if native.proposed_shown {
                theme::DIFFERS_TEXT
            } else {
                theme::MUTED
            },
        ));
        ui.add_space(12.0);
        Self::mods_status(ui, state);
        if native.packages.is_empty() {
            ui.label(theme::text(
                if !native.available {
                    "Your server offers no native mods yet."
                } else if native.serial.is_some() {
                    "The index lists no native mod for this game yet."
                } else {
                    "Check the index to see the native mods for this game."
                },
                theme::body(12.0),
                theme::MUTED,
            ));
            return;
        }
        let search = self.mod_search.trim().to_lowercase();
        let shown: Vec<&NativeRow> = native
            .packages
            .iter()
            .filter(|row| row.listed)
            .filter(|row| {
                search.is_empty()
                    || [&row.name, &row.description, &row.id]
                        .into_iter()
                        .chain(&row.authors)
                        .any(|text| text.to_lowercase().contains(&search))
            })
            .collect();
        if shown.is_empty() {
            ui.label(theme::text(
                format!("No mod matches \u{201c}{}\u{201d}.", self.mod_search.trim()),
                theme::body(12.0),
                theme::MUTED,
            ));
            return;
        }
        self.mods_table(ui, state, &shown, ModsPage::Browser, height - 80.0);
    }

    /// `rows` as a table, `height` high, beside the details of the one
    /// picked (the first, until the player picks another).
    fn mods_table(
        &mut self,
        ui: &mut Ui,
        state: &State,
        rows: &[&NativeRow],
        page: ModsPage,
        height: f32,
    ) {
        let picked = self
            .selected_mod
            .clone()
            .filter(|id| rows.iter().any(|row| &row.id == id))
            .or_else(|| rows.first().map(|row| row.id.clone()));
        self.selected_mod.clone_from(&picked);
        let height = height.max(180.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = PANES_GAP;
            ui.vertical(|ui| {
                ui.set_width(TABLE_WIDTH);
                table_header(ui, page);
                ScrollArea::vertical()
                    .id_salt(("mods-table", page))
                    .max_height(height - HEADER_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for row in rows {
                            let chosen = picked.as_deref() == Some(row.id.as_str());
                            if self.table_row(ui, row, page, chosen).clicked() {
                                self.selected_mod = Some(row.id.clone());
                            }
                        }
                    });
            });
            ui.vertical(|ui| {
                ui.set_width(DETAIL_WIDTH);
                ScrollArea::vertical()
                    .id_salt(("mods-detail", page))
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if let Some(row) = rows
                            .iter()
                            .find(|row| picked.as_deref() == Some(row.id.as_str()))
                        {
                            self.mod_details(ui, state, row, page);
                        }
                    });
            });
        });
    }

    /// One mod's line in the table: its preview, name and authors, and the
    /// page's columns. Clicked, it is picked.
    fn table_row(
        &mut self,
        ui: &mut Ui,
        row: &NativeRow,
        page: ModsPage,
        chosen: bool,
    ) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(vec2(TABLE_WIDTH, ROW_HEIGHT), Sense::click());
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::SelectableLabel, true, chosen, &row.name)
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter_at(rect);
        let fill = if chosen {
            theme::white(0.075)
        } else if response.hovered() {
            theme::white(0.035)
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect.shrink2(vec2(0.0, 2.0)), 6.0, fill);
        if chosen {
            painter.rect_stroke(
                rect.shrink2(vec2(0.5, 2.5)),
                6.0,
                Stroke::new(1.0, theme::white(0.16)),
                StrokeKind::Inside,
            );
        }
        let thumb = Rect::from_center_size(
            pos2(rect.min.x + 8.0 + THUMB.x / 2.0, rect.center().y),
            THUMB,
        );
        self.preview(ui.ctx(), &painter, thumb, row);
        let mut x = thumb.max.x + 12.0;
        let name_width =
            TABLE_WIDTH - (x - rect.min.x) - columns(page).iter().map(|c| c.1).sum::<f32>();
        let name = one_line(
            ui,
            &row.name,
            theme::semibold(13.0),
            theme::TEXT,
            name_width - 10.0,
        );
        let (under, under_color) = if row.proposed.is_some() {
            ("Proposed · not reviewed".to_owned(), theme::DIFFERS_TEXT)
        } else if page == ModsPage::Browser && row.installed.is_none() && row.unsupported.is_some()
        {
            ("Can't be installed here".to_owned(), theme::DIFFERS_TEXT)
        } else if row.code {
            (
                format!("{} · runs code", by(&row.authors).unwrap_or_default()),
                theme::DIFFERS_TEXT,
            )
        } else {
            let mut under = by(&row.authors).unwrap_or_default();
            if page == ModsPage::Browser && row.installed.is_some() {
                if !under.is_empty() {
                    under.push_str(" · ");
                }
                under.push_str(if has_update(row) {
                    "update"
                } else {
                    "installed"
                });
            }
            (under, theme::MUTED)
        };
        let under = one_line(
            ui,
            &under,
            theme::body(11.0),
            under_color,
            name_width - 10.0,
        );
        let top = rect.center().y - (name.size().y + 2.0 + under.size().y) / 2.0;
        painter.galley(pos2(x, top), name.clone(), theme::TEXT);
        painter.galley(pos2(x, top + name.size().y + 2.0), under, under_color);
        x += name_width;
        for ((value, color), (_, width)) in cells(row, page).into_iter().zip(columns(page)) {
            let galley = one_line(ui, &value, theme::body(12.0), color, width - 8.0);
            painter.galley(
                pos2(x, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
            x += width;
        }
        painter.hline(
            rect.x_range(),
            rect.max.y - 0.5,
            Stroke::new(1.0, theme::white(0.06)),
        );
        response
    }

    /// The picked mod: its preview, large, what it is, and what can be
    /// done with it.
    fn mod_details(&mut self, ui: &mut Ui, state: &State, row: &NativeRow, page: ModsPage) {
        let idle = state.native_busy.is_none();
        let (image, _) =
            ui.allocate_exact_size(vec2(DETAIL_WIDTH, DETAIL_WIDTH / 2.0), Sense::hover());
        let painter = ui.painter().clone();
        self.preview(ui.ctx(), &painter, image, row);
        ui.add_space(12.0);
        ui.label(theme::text(&row.name, theme::semibold(18.0), theme::TEXT));
        if let Some(authors) = by(&row.authors) {
            ui.add_space(2.0);
            ui.label(theme::text(authors, theme::body(12.0), theme::MUTED));
        }
        let mut pills: Vec<(String, Pill)> = Vec::new();
        if row.installed.is_some() {
            pills.push(if row.enabled {
                ("On".into(), Pill::Ready)
            } else {
                ("Off".into(), Pill::Unknown)
            });
        }
        if has_update(row)
            && let Some(available) = &row.available
        {
            pills.push((format!("{available} available"), Pill::Update));
        }
        if row.proposed.is_some() {
            pills.push(("Proposed".into(), Pill::Update));
        }
        if row.code {
            pills.push(("Runs code".into(), Pill::Update));
        }
        if !row.listed {
            pills.push(("No longer offered".into(), Pill::Unknown));
        }
        if !pills.is_empty() {
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (words, pill) in &pills {
                    theme::pill(ui, words, *pill);
                }
            });
        }
        if !row.description.is_empty() {
            ui.add_space(10.0);
            ui.label(theme::text(
                &row.description,
                theme::body(12.5),
                theme::TEXT,
            ));
        }
        if row.code {
            ui.add_space(8.0);
            ui.label(theme::text(
                "Runs its own code in the game, with your account's rights on this PC. Install \
                 it only from a server you trust.",
                theme::body(12.0),
                theme::DIFFERS_TEXT,
            ));
        }
        if let Some(why) = row.unsupported.as_ref().filter(|_| row.installed.is_none()) {
            ui.add_space(8.0);
            ui.label(theme::text(
                format!("Can't be installed: {why}."),
                theme::body(12.0),
                theme::DIFFERS_TEXT,
            ));
        }
        if let Some(clash) = &row.clash {
            ui.add_space(8.0);
            ui.label(theme::text(
                format!("Can't be switched on: {clash}. Switch that mod off first."),
                theme::body(12.0),
                theme::DIFFERS_TEXT,
            ));
        }
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let id = row.id.clone();
            match page {
                ModsPage::Manager => {
                    if has_update(row)
                        && theme::quiet_button(ui, idle, Some("download"), "Update", Quiet::new())
                            .clicked()
                    {
                        self.backend.act(Action::NativeInstall { id: id.clone() });
                    }
                    let words = if row.enabled {
                        "Switch off"
                    } else {
                        "Switch on"
                    };
                    let allowed = idle && (row.enabled || row.clash.is_none());
                    if theme::quiet_button(ui, allowed, None, words, Quiet::new()).clicked() {
                        self.backend.act(Action::NativeEnable {
                            id: id.clone(),
                            on: !row.enabled,
                        });
                    }
                    if let Some(previous) = &row.previous
                        && theme::quiet_button(
                            ui,
                            idle,
                            None,
                            &format!("Roll back to {previous}"),
                            Quiet::new(),
                        )
                        .clicked()
                    {
                        self.backend.act(Action::NativeRollback { id: id.clone() });
                    }
                    let uninstall = Quiet::new().color(theme::rgba(242, 168, 160, 1.0));
                    if theme::quiet_button(ui, idle, None, "Uninstall", uninstall).clicked() {
                        self.backend.act(Action::NativeUninstall { id });
                    }
                }
                ModsPage::Browser => {
                    let words = match (&row.installed, &row.available) {
                        (None, Some(_)) => Some("Install"),
                        (Some(_), Some(_)) if has_update(row) => Some("Update"),
                        _ => None,
                    };
                    let installable = idle && row.unsupported.is_none();
                    if let Some(words) = words
                        && theme::quiet_button(
                            ui,
                            installable,
                            Some("download"),
                            words,
                            Quiet::new(),
                        )
                        .clicked()
                    {
                        self.backend.act(if row.proposed.is_some() {
                            Action::NativeInstallProposed { id }
                        } else {
                            Action::NativeInstall { id }
                        });
                    } else if row.installed.is_some()
                        && theme::quiet_button(ui, true, Some("archive"), "Manage", Quiet::new())
                            .clicked()
                    {
                        self.mods = Some(ModsPage::Manager);
                    }
                }
            }
        });
        if let Some(proposal) = &row.proposed {
            ui.add_space(8.0);
            ui.hyperlink_to(
                theme::text(
                    format!(
                        "Proposed in pull request #{}: not reviewed",
                        proposal.number
                    ),
                    theme::body(12.0),
                    theme::DIFFERS_TEXT,
                ),
                &proposal.url,
            );
        }
        if let Some(homepage) = &row.homepage {
            ui.add_space(8.0);
            ui.hyperlink_to(
                theme::text("Project page \u{2197}", theme::body(12.0), theme::ACCENT),
                homepage,
            );
        }
        ui.add_space(12.0);
        let facts = facts(row, page);
        let last = facts.len().saturating_sub(1);
        for (at, (name, value)) in facts.iter().enumerate() {
            theme::row(ui, name, value, false, at == last);
        }
    }

    /// `row`'s preview in `rect`, cropped to fill it; its initials while
    /// it loads, or when it has none.
    fn preview(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: Rect,
        row: &NativeRow,
    ) {
        let texture = row
            .preview
            .as_deref()
            .and_then(|url| self.previews.get(ctx, url));
        match texture {
            Some(texture) => {
                let [w, h] = texture.size();
                let (wanted, has) = (rect.width() / rect.height(), w as f32 / h.max(1) as f32);
                let uv = if has > wanted {
                    let cut = (1.0 - wanted / has) / 2.0;
                    Rect::from_min_max(pos2(cut, 0.0), pos2(1.0 - cut, 1.0))
                } else {
                    let cut = (1.0 - has / wanted) / 2.0;
                    Rect::from_min_max(pos2(0.0, cut), pos2(1.0, 1.0 - cut))
                };
                painter.image(texture.id(), rect, uv, Color32::WHITE);
            }
            None => {
                let hue = row.id.bytes().fold(7u32, |hash, b| {
                    hash.wrapping_mul(31).wrapping_add(u32::from(b))
                });
                let fill = egui::ecolor::Hsva::new((hue % 360) as f32 / 360.0, 0.28, 0.32, 1.0);
                painter.rect_filled(rect, 4.0, Color32::from(fill));
                let initials: String = row
                    .name
                    .split_whitespace()
                    .filter_map(|word| word.chars().next())
                    .take(2)
                    .collect::<String>()
                    .to_uppercase();
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    initials,
                    theme::semibold((rect.height() * 0.38).clamp(11.0, 44.0)),
                    theme::white(0.85),
                );
            }
        }
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, theme::white(0.1)),
            StrokeKind::Inside,
        );
    }
}

/// Which mods dialog is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ModsPage {
    /// The native mods installed.
    Manager,
    /// The native mods the server offers.
    Browser,
}

/// The mods dialog's table, its details beside it, and the gap between.
const TABLE_WIDTH: f32 = 540.0;
const DETAIL_WIDTH: f32 = 300.0;
const PANES_GAP: f32 = 20.0;
const ROW_HEIGHT: f32 = 54.0;
const HEADER_HEIGHT: f32 = 26.0;
/// A preview in the table, 16:9.
const THUMB: Vec2 = vec2(64.0, 36.0);

/// The native mods installed, in the order the game loads those enabled.
fn installed_rows(state: &State) -> Vec<&NativeRow> {
    state
        .native
        .packages
        .iter()
        .filter(|row| row.installed.is_some() && row.proposed.is_none())
        .collect()
}

/// Whether the index has a newer version of an installed mod.
fn has_update(row: &NativeRow) -> bool {
    row.installed.is_some()
        && row
            .available
            .as_ref()
            .is_some_and(|available| row.installed.as_ref() != Some(available))
}

/// The table's columns after the mod's own, and their widths.
fn columns(page: ModsPage) -> [(&'static str, f32); 4] {
    match page {
        ModsPage::Manager => [
            ("Version", 72.0),
            ("Size", 70.0),
            ("Load order", 76.0),
            ("Status", 86.0),
        ],
        ModsPage::Browser => [
            ("Version", 72.0),
            ("Size", 70.0),
            ("Downloads", 80.0),
            ("Updated", 82.0),
        ],
    }
}

/// `row`'s values in the page's columns, each with its colour.
fn cells(row: &NativeRow, page: ModsPage) -> [(String, Color32); 4] {
    let dash = || ("\u{2014}".to_owned(), theme::MUTED);
    let size = |bytes: Option<u64>| bytes.map_or_else(dash, |b| (size_text(b), theme::TEXT));
    match page {
        ModsPage::Manager => [
            (row.installed.clone().unwrap_or_default(), theme::TEXT),
            size(row.installed_size),
            row.load_order
                .map_or_else(dash, |at| (format!("#{at}"), theme::TEXT)),
            if has_update(row) {
                (
                    if row.enabled {
                        "On · update"
                    } else {
                        "Off · update"
                    }
                    .to_owned(),
                    theme::ACCENT,
                )
            } else if row.enabled {
                ("On".to_owned(), theme::OK)
            } else if row.clash.is_some() {
                ("Off · clashes".to_owned(), theme::DIFFERS_TEXT)
            } else {
                ("Off".to_owned(), theme::MUTED)
            },
        ],
        ModsPage::Browser => [
            (
                row.available
                    .clone()
                    .or_else(|| row.installed.clone())
                    .unwrap_or_default(),
                theme::TEXT,
            ),
            size(row.available_size.or(row.installed_size)),
            row.downloads
                .map_or_else(dash, |n| (count_text(n), theme::TEXT)),
            row.updated
                .as_deref()
                .map_or_else(dash, |day| (day_text(day), theme::TEXT)),
        ],
    }
}

/// What the details say of `row`, name and value.
fn facts(row: &NativeRow, page: ModsPage) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    match page {
        ModsPage::Manager => {
            if let Some(installed) = &row.installed {
                facts.push(("Version", installed.clone()));
            }
            if let Some(size) = row.installed_size {
                facts.push(("Size", size_text(size)));
            }
            facts.push((
                "Load order",
                row.load_order
                    .map_or_else(|| "Not loaded: off".to_owned(), |at| format!("#{at}")),
            ));
        }
        ModsPage::Browser => {
            if let Some(version) = row.available.as_ref().or(row.installed.as_ref()) {
                facts.push(("Version", version.clone()));
            }
            if let Some(size) = row.available_size.or(row.installed_size) {
                facts.push(("Download", size_text(size)));
            }
        }
    }
    if let Some(downloads) = row.downloads {
        facts.push(("Downloads", count_text(downloads)));
    }
    if let Some(day) = &row.updated {
        facts.push(("Updated", day_text(day)));
    }
    facts.push((
        "In a room",
        if row.simulation {
            "Every player runs it".to_owned()
        } else {
            "Only for you".to_owned()
        },
    ));
    facts
}

/// "by Ann and Bob".
fn by(authors: &[String]) -> Option<String> {
    match authors {
        [] => None,
        [one] => Some(format!("by {one}")),
        [rest @ .., last] => Some(format!("by {} and {last}", rest.join(", "))),
    }
}

/// `bytes` as players read sizes: 812 KB, 4.2 MB.
fn size_text(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.1} GB", b / (KB * KB * KB))
    }
}

/// A count as players read it: 950, 1.3k, 2.4M.
fn count_text(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1}k", count as f64 / 1e3),
        _ => format!("{:.1}M", count as f64 / 1e6),
    }
}

/// `YYYY-MM-DD` as `30 Sep 2026`.
fn day_text(day: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = day.split('-');
    match (
        parts.next(),
        parts.next().and_then(|m| m.parse::<usize>().ok()),
        parts.next(),
    ) {
        (Some(year), Some(month @ 1..=12), Some(d)) => {
            format!("{} {} {year}", d.trim_start_matches('0'), MONTHS[month - 1])
        }
        _ => day.to_owned(),
    }
}

/// `text` on one line, cut with an ellipsis to `width`.
fn one_line(
    ui: &Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

/// The table's column names.
fn table_header(ui: &mut Ui, page: ModsPage) {
    let (rect, _) = ui.allocate_exact_size(vec2(TABLE_WIDTH, HEADER_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    let label = |x: f32, words: &str| {
        painter.text(
            pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            words.to_uppercase(),
            theme::body(9.5),
            theme::MUTED,
        );
    };
    let mut x = rect.min.x + 8.0 + THUMB.x + 12.0;
    label(x, "Mod");
    x = rect.max.x - columns(page).iter().map(|c| c.1).sum::<f32>();
    for (name, width) in columns(page) {
        label(x, name);
        x += width;
    }
    painter.hline(
        rect.x_range(),
        rect.max.y - 0.5,
        Stroke::new(1.0, theme::white(0.094)),
    );
}

/// One card in the mods dialog: a faint frame.
fn mod_card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(theme::rgba(34, 43, 46, 0.35))
        .stroke(Stroke::new(1.0, theme::white(0.094)))
        .corner_radius(8)
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

/// How this player's game differs from the room's.
fn differences(ui: &mut Ui, lines: &[String]) {
    let under = ui.painter().add(Shape::Noop);
    let rect = Frame::new()
        .fill(theme::rgba(217, 179, 106, 0.08))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            for line in lines {
                ui.label(theme::text(line, theme::body(12.0), theme::DIFFERS_TEXT));
            }
        })
        .response
        .rect;
    ui.painter().set(
        under,
        Shape::rect_filled(
            Rect::from_min_size(rect.min, vec2(2.0, rect.height())),
            0.0,
            theme::rgba(217, 179, 106, 0.6),
        ),
    );
}

/// The line under the panel's buttons, with its dot.
fn status_line(ui: &mut Ui, words: &str, tone: Tone) {
    let (dot, color) = match tone {
        Tone::Error => (theme::BAD, theme::DANGER_TEXT),
        Tone::Ready => (theme::OK, theme::MUTED),
        Tone::Plain | Tone::Update => (theme::MUTED, theme::MUTED),
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
        ui.painter()
            .circle_filled(pos2(rect.center().x, rect.min.y + 8.0), 2.0, dot);
        ui.add(egui::Label::new(theme::text(words, theme::body(11.0), color)).wrap());
    });
}

/// A labelled text field; returns whether Enter was pressed in it.
fn labelled_field(ui: &mut Ui, name: &str, note: Option<&str>, edit: egui::TextEdit<'_>) -> bool {
    let label = theme::field_label(ui, name, note);
    ui.add_space(4.0);
    let response = ui.add(edit).labelled_by(label.id);
    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter))
}

/// The green badge of an update ready to install.
fn update_badge(ui: &mut Ui, words: &str) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(words.to_owned(), theme::semibold(12.0), theme::UPDATE_TEXT);
    let size = vec2(galley.size().x + 28.0, 39.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.add(theme::shadow(rect.shrink(4.0), theme::rgba(0, 0, 0, 0.2)));
    painter.rect_filled(rect, 6.0, theme::UPDATE_BG);
    painter.rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, theme::UPDATE_BORDER),
        StrokeKind::Inside,
    );
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::UPDATE_TEXT,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, words));
    response
}

/// The server's name in a frosted chip, with a dot while it is known to
/// answer (green) or not (red).
fn server_chip(ui: &mut Ui, window: Rect, assets: &Assets, name: &str, dot: Option<Color32>) {
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_owned(), theme::body(11.0), theme::CAPTION);
    let dot_room = if dot.is_some() { 13.0 } else { 0.0 };
    let size = vec2(galley.size().x + 20.0 + dot_room, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().add(theme::frosted(
        window,
        rect,
        assets,
        &Fill::Flat(theme::rgba(16, 25, 31, 0.4)),
        theme::white(0.145),
        0.0,
    ));
    let mut x = rect.min.x + 10.0;
    if let Some(color) = dot {
        ui.painter()
            .circle_filled(pos2(x + 3.0, rect.center().y), 3.0, color);
        x += dot_room;
    }
    ui.painter().galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::CAPTION,
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, name));
}

/// What the server setting's field allows, for what the player typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSetting {
    /// Why what was typed is no server, if it is not one.
    pub problem: Option<String>,
    /// "Use this server": a server other than the current, outside a room.
    pub can_apply: bool,
    /// "Reset to default": the default is not the current, outside a room.
    pub can_reset: bool,
}

impl ServerSetting {
    pub fn of(typed: &str, state: &State) -> Self {
        let same = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
        let typed = typed.trim();
        let problem = (!typed.is_empty())
            .then(|| server_address(typed).err())
            .flatten();
        let free = state.room.is_none();
        Self {
            can_apply: !state.regional
                && state.servers.len() <= 1
                && free
                && !typed.is_empty()
                && problem.is_none()
                && !state.server.as_deref().is_some_and(|now| same(now, typed)),
            // On the release's servers the launcher is on its default
            // already, wherever it plays.
            can_reset: free
                && state.servers.is_empty()
                && state.server_default.as_deref().is_some_and(|default| {
                    !state
                        .server
                        .as_deref()
                        .is_some_and(|now| same(now, default))
                }),
            problem,
        }
    }
}

/// The release's servers, with their pings, as "EU · 24 ms (you are
/// here), US · 110 ms"; `None` when the launcher plays on one server.
pub fn servers_line(state: &State) -> Option<String> {
    if state.servers.is_empty() {
        return None;
    }
    let named: Vec<String> = state
        .servers
        .iter()
        .map(|server| {
            let ping = match (server.reachable, server.ping_ms) {
                (false, _) => " · not answering".to_owned(),
                (true, Some(ms)) => format!(" · {ms} ms"),
                (true, None) => String::new(),
            };
            let here = if server.here { " (you are here)" } else { "" };
            format!("{}{ping}{here}", server.name)
        })
        .collect();
    Some(named.join(", "))
}

/// What the server setting says of the server played on.
pub fn server_setting_line(state: &State) -> String {
    if let Some(servers) = servers_line(state) {
        return format!(
            "You play on TPF3-MP's servers: {servers}. Rooms you host go to the closest, and \
             the room list shows the rooms of all of them."
        );
    }
    let Some(server) = &state.server else {
        if state.regional {
            return "You play across TPF3-MP's trusted servers automatically.".to_owned();
        }
        return "No server is set: type one below.".to_owned();
    };
    let default = state
        .server_default
        .as_deref()
        .is_some_and(|default| default.trim().eq_ignore_ascii_case(server.trim()));
    match (&state.server_name, default) {
        (Some(name), true) => format!("You play on {name} ({server}), the default server."),
        (None, true) => format!("You play on {server}, the default server."),
        (Some(name), false) if state.regional => format!(
            "You play on trusted server {name} ({server}). Reset to default for automatic regional play."
        ),
        (None, false) if state.regional => format!(
            "You play on trusted server {server}. Reset to default for automatic regional play."
        ),
        (_, false) => format!("You play on {server}."),
    }
}

/// A group of settings: a faint card.
fn group(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(theme::rgba(34, 43, 46, 0.35))
        .stroke(Stroke::new(1.0, theme::white(0.094)))
        .corner_radius(8)
        .inner_margin(Margin::same(20))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

fn dialog_frame() -> Frame {
    Frame::new()
        .fill(theme::panel_fill(0.96))
        .stroke(Stroke::new(1.0, theme::white(0.094)))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(28))
}

/// An icon on its own, in `color`.
fn icon(ui: &mut Ui, name: &str, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if let Some(assets) = Assets::of(ui.ctx()) {
        assets.icon(ui.painter(), name, rect, color);
    }
}

/// Opens the game's folder in the system's file manager.
fn open_folder(folder: &str) {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if !Path::new(folder).is_dir() {
        return;
    }
    let _ = std::process::Command::new(opener).arg(folder).spawn();
}

/// What the footer says of this build: `v0.1.0 · protocol 13 · 1316710abc`,
/// so a player can read off which launcher they started.
pub fn footer_build() -> String {
    format!(
        "v{} · protocol {} · {}",
        env!("CARGO_PKG_VERSION"),
        tpf3mp_proto::PROTOCOL_VERSION,
        tpf3mp_agent::about::COMMIT
    )
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_footer_names_version_protocol_and_commit() {
        assert_eq!(
            super::footer_build(),
            format!(
                "v{} · protocol {} · {}",
                env!("CARGO_PKG_VERSION"),
                tpf3mp_proto::PROTOCOL_VERSION,
                tpf3mp_agent::about::COMMIT
            )
        );
        assert_ne!(tpf3mp_agent::about::COMMIT, "", "built in");
    }
}
