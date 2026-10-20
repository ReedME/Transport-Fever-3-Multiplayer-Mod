//! The launcher's look: tearded's TPF2 Multiplayer Launcher (MIT), as
//! silver2127 ported it to Transport Fever 3 as a web page, drawn here with
//! egui to the pixel (D20). Its colours, sizes and spacing are the page's
//! computed styles; its images and icons are the page's own
//! (`images/ATTRIBUTION.md`).
//!
//! - the city across the whole window, with frosted panels over it: under
//!   each panel the city is drawn again from a blurred copy, as the page's
//!   `backdrop-filter` does;
//! - the game's wordmark, and the TF3 MP logo in the corner;
//! - one light main button with an arrow, quieter outlined ones under it,
//!   and small status pills;
//! - your game in a bar along the bottom.
//!
//! Text is set in the system's own fonts, as the page is: Segoe UI and
//! Consolas on Windows, and the nearest there are elsewhere. A font file
//! egui could not read is never handed to it.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use eframe::egui::{
    self, Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Id, Mesh, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, TextStyle,
    TextureHandle, TextureOptions, Ui, Vec2, Visuals, WidgetInfo, WidgetText, WidgetType, pos2,
    vec2,
};

// The page's colours (`styles.css`).
/// `--bg`: behind everything.
pub const BG: Color32 = Color32::from_rgb(16, 20, 21);
/// `--text`.
pub const TEXT: Color32 = Color32::from_rgb(244, 245, 239);
/// `--muted`.
pub const MUTED: Color32 = Color32::from_rgb(156, 169, 173);
/// A row's name, beside its value.
pub const LABEL: Color32 = Color32::from_rgb(166, 181, 185);
/// `--accent`: the main button, a step done.
pub const ACCENT: Color32 = Color32::from_rgb(209, 215, 218);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(238, 241, 242);
/// `--ink`: on the main button.
pub const INK: Color32 = Color32::from_rgb(23, 26, 23);
/// The line under the game's name, and the server chip.
pub const CAPTION: Color32 = Color32::from_rgb(216, 225, 224);
/// The footer's text.
pub const FOOTER: Color32 = Color32::from_rgb(213, 222, 223);
/// "Image © Urban Games".
pub const SCENE_CAPTION: Color32 = Color32::from_rgb(192, 205, 208);
/// The note under the panel's buttons.
pub const NOTE: Color32 = Color32::from_rgb(149, 168, 173);
/// The dot of a server that answers, and of a line that went well.
pub const OK: Color32 = Color32::from_rgb(127, 199, 154);
/// The dot of a server that does not answer, and of an error.
pub const BAD: Color32 = Color32::from_rgb(224, 115, 107);
/// The words of a button that removes or leaves.
pub const DANGER_TEXT: Color32 = Color32::from_rgb(241, 179, 173);
/// A primary button that removes or leaves.
pub const DANGER: Color32 = Color32::from_rgb(240, 167, 157);
/// The launcher-update badge.
pub const UPDATE_BG: Color32 = Color32::from_rgb(209, 239, 201);
pub const UPDATE_BORDER: Color32 = Color32::from_rgb(183, 223, 175);
pub const UPDATE_TEXT: Color32 = Color32::from_rgb(24, 53, 31);
/// The words of a note on how this game differs from the room's.
pub const DIFFERS_TEXT: Color32 = Color32::from_rgb(232, 220, 194);

/// A colour at this opacity (0 to 1).
pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let a = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// White at this opacity: the page's hairlines and borders.
pub fn white(a: f32) -> Color32 {
    rgba(255, 255, 255, a)
}

/// A panel over the city: `rgba(21, 27, 29, a)`.
pub fn panel_fill(a: f32) -> Color32 {
    rgba(21, 27, 29, a)
}

/// The border of a quiet button.
pub fn quiet_border() -> Color32 {
    rgba(244, 245, 239, 0.24)
}

/// A text field's fill and border.
pub fn field_fill() -> Color32 {
    rgba(10, 18, 22, 0.72)
}
pub fn field_border() -> Color32 {
    white(0.15)
}

/// The corners of buttons and fields.
pub const RADIUS: f32 = 5.0;
/// The height of a text field and of a quiet button.
pub const CONTROL: f32 = 40.0;
/// The height of the main button.
pub const PRIMARY: f32 = 56.0;

/// The page's fonts: Segoe UI, its semibold, Consolas.
pub const SEMIBOLD: &str = "semibold";

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Sets the look on the whole window, and loads the images and icons.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(14.0, 9.0);
        // Rows are as tall as what is in them; controls set their own
        // heights.
        style.spacing.interact_size.y = 16.0;
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;
        style.text_styles = [
            (TextStyle::Small, body(11.0)),
            (TextStyle::Body, body(13.0)),
            (TextStyle::Button, semibold(12.0)),
            (TextStyle::Heading, semibold(15.0)),
            (TextStyle::Monospace, mono(13.0)),
        ]
        .into();
    });
    let assets = Arc::new(Assets::load(ctx));
    ctx.data_mut(|data| data.insert_temp(Id::new(ASSETS), assets));
}

fn visuals() -> Visuals {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = panel_fill(0.96);
    visuals.window_stroke = Stroke::new(1.0, white(0.094));
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.extreme_bg_color = field_fill();
    visuals.text_edit_bg_color = Some(field_fill());
    visuals.faint_bg_color = rgba(34, 43, 46, 1.0);
    visuals.hyperlink_color = ACCENT;
    visuals.error_fg_color = DANGER_TEXT;
    visuals.weak_text_color = Some(MUTED);
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = rgba(209, 215, 218, 0.3);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.text_cursor.stroke = Stroke::new(1.5, TEXT);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let radius = CornerRadius::same(RADIUS as u8);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    widgets.noninteractive.weak_bg_fill = Color32::TRANSPARENT;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, white(0.094));
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.noninteractive.corner_radius = radius;
    for state in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.bg_fill = field_fill();
        state.weak_bg_fill = field_fill();
        state.bg_stroke = Stroke::new(1.0, field_border());
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    widgets.hovered.bg_stroke = Stroke::new(1.0, white(0.3));
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    widgets.open.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.popup_shadow = egui::Shadow::NONE;
    visuals.window_shadow = egui::Shadow {
        offset: [0, 22],
        blur: 60,
        spread: 0,
        color: rgba(7, 16, 18, 0.4),
    };
    visuals
}

// ---------- fonts ----------

/// A font file on this system.
struct SystemFont {
    path: PathBuf,
}

/// Puts the system's fonts first in each family, egui's after them for
/// what they lack.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), defaults.clone());
    for (role, candidates, family) in [
        ("tpf3mp-body", body_fonts(), FontFamily::Proportional),
        (
            "tpf3mp-semibold",
            semibold_fonts(),
            FontFamily::Name(SEMIBOLD.into()),
        ),
        ("tpf3mp-mono", mono_fonts(), FontFamily::Monospace),
    ] {
        if let Some(font) = first_readable(&candidates) {
            fonts.font_data.insert(role.to_owned(), font.into());
            if let Some(list) = fonts.families.get_mut(&family) {
                list.insert(0, role.to_owned());
            }
        }
    }
    // Semibold falls back to the body's font before egui's.
    if fonts.font_data.contains_key("tpf3mp-body")
        && let Some(list) = fonts.families.get_mut(&FontFamily::Name(SEMIBOLD.into()))
        && !list.iter().any(|name| name == "tpf3mp-body")
    {
        let at = usize::from(list.first().is_some_and(|name| name == "tpf3mp-semibold"));
        list.insert(at, "tpf3mp-body".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// The first of `candidates` that is there and that egui can read: egui
/// stops the program on a font it cannot parse, so each is parsed here
/// first, as egui would.
fn first_readable(candidates: &[SystemFont]) -> Option<FontData> {
    candidates.iter().find_map(|font| {
        let bytes = std::fs::read(&font.path).ok()?;
        skrifa::FontRef::from_index(&bytes, 0).ok()?;
        Some(FontData::from_owned(bytes))
    })
}

fn fonts_in(files: &[&str]) -> Vec<SystemFont> {
    let dirs = font_dirs();
    files
        .iter()
        .flat_map(|file| {
            dirs.iter().map(move |dir| SystemFont {
                path: dir.join(file),
            })
        })
        .collect()
}

#[cfg(windows)]
fn font_dirs() -> Vec<PathBuf> {
    let windows = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    vec![windows.join("Fonts")]
}

#[cfg(target_os = "macos")]
fn font_dirs() -> Vec<PathBuf> {
    [
        "/System/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
        "/Library/Fonts",
    ]
    .map(PathBuf::from)
    .to_vec()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn font_dirs() -> Vec<PathBuf> {
    // Debian and Ubuntu, Fedora, Arch.
    [
        "/usr/share/fonts/truetype/dejavu",
        "/usr/share/fonts/dejavu-sans-fonts",
        "/usr/share/fonts/dejavu-sans-mono-fonts",
        "/usr/share/fonts/TTF",
        "/usr/share/fonts/dejavu",
        "/usr/share/fonts/truetype/noto",
        "/usr/share/fonts/noto",
        "/usr/share/fonts/google-noto",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/liberation-sans",
        "/usr/share/fonts/liberation-mono",
        "/usr/share/fonts/liberation",
    ]
    .map(PathBuf::from)
    .to_vec()
}

fn body_fonts() -> Vec<SystemFont> {
    fonts_in(if cfg!(windows) {
        &["segoeui.ttf", "arial.ttf"]
    } else if cfg!(target_os = "macos") {
        &["SFNS.ttf", "Arial.ttf"]
    } else {
        &[
            "NotoSans-Regular.ttf",
            "DejaVuSans.ttf",
            "LiberationSans-Regular.ttf",
        ]
    })
}

fn semibold_fonts() -> Vec<SystemFont> {
    fonts_in(if cfg!(windows) {
        &["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"]
    } else if cfg!(target_os = "macos") {
        &["SFNS.ttf", "Arial Bold.ttf"]
    } else {
        &[
            "NotoSans-SemiBold.ttf",
            "NotoSans-Bold.ttf",
            "DejaVuSans-Bold.ttf",
            "LiberationSans-Bold.ttf",
        ]
    })
}

fn mono_fonts() -> Vec<SystemFont> {
    fonts_in(if cfg!(windows) {
        &["consola.ttf"]
    } else if cfg!(target_os = "macos") {
        &["SFNSMono.ttf", "Menlo.ttc"]
    } else {
        &["DejaVuSansMono.ttf", "LiberationMono-Regular.ttf"]
    })
}

// ---------- images and icons ----------

const ASSETS: &str = "tpf3mp-assets";

/// The page's images, and its icons drawn from their SVG.
pub struct Assets {
    /// The city, and the same blurred, for under the panels.
    pub scene: TextureHandle,
    pub scene_blurred: TextureHandle,
    pub scene_size: Vec2,
    pub wordmark: TextureHandle,
    pub logo: TextureHandle,
    icons: HashMap<&'static str, TextureHandle>,
}

/// The page's icons (`index.html`'s symbols), 24 by 24, stroked as its
/// style sheet strokes them.
const ICONS: &[(&str, &str)] = &[
    ("arrow", r#"<path d="M5 12h14m-6-6 6 6-6 6"/>"#),
    (
        "download",
        r#"<path d="M12 3v12m-5-5 5 5 5-5M5 16v4h14v-4"/>"#,
    ),
    ("play", r#"<path d="m8 5 11 7-11 7Z"/>"#),
    (
        "settings",
        r#"<path d="M4 7h16M4 17h16"/><rect x="7" y="4" width="4" height="6" rx="1"/><rect x="14" y="14" width="4" height="6" rx="1"/>"#,
    ),
    (
        "refresh",
        r#"<path d="M20 10a8 8 0 0 0-14-5L3 8m0-5v5h5M4 14a8 8 0 0 0 14 5l3-3m0 5v-5h-5"/>"#,
    ),
    ("folder", r#"<path d="M3 7V5h7l2 3h9v11H3Z"/>"#),
    (
        "archive",
        r#"<path d="M4 8v12h16V8M9 12h6"/><rect x="3" y="3" width="18" height="5" rx="1"/>"#,
    ),
    (
        "search",
        r#"<circle cx="10.5" cy="10.5" r="6"/><path d="m15 15 5 5"/>"#,
    ),
    ("close", r#"<path d="m6 6 12 12M6 18 18 6"/>"#),
    ("check", r#"<path d="m5 12 5 5 9-10"/>"#),
    (
        "link",
        r#"<path d="M10 14a4 4 0 0 0 6 0l3-3a4 4 0 0 0-6-6l-1 1m2 4a4 4 0 0 0-6 0l-3 3a4 4 0 0 0 6 6l1-1"/>"#,
    ),
    (
        "users",
        r#"<circle cx="9" cy="8" r="3.5"/><path d="M3 20a6 6 0 0 1 12 0M16 4.5a3.5 3.5 0 0 1 0 7M18 14a6 6 0 0 1 3 6"/>"#,
    ),
    ("send", r#"<path d="M4 12 20 4l-4 16-4-7Zm8 1 8-9"/>"#),
    // A step done: the page's check on the light disc.
    ("step-done", r#"<path d="m6 12 4 4 8-9" stroke-width="3"/>"#),
];

/// Icons are rasterised at this many pixels a side, and shrunk to fit.
const ICON_PIXELS: u32 = 48;

impl Assets {
    fn load(ctx: &egui::Context) -> Self {
        let scene = decode(include_bytes!("../images/transport-fever-2.jpg"));
        let scene_size = vec2(scene.size[0] as f32, scene.size[1] as f32);
        let blurred = blurred(&scene);
        let icons = ICONS
            .iter()
            .filter_map(|(name, body)| {
                let image = rasterise(body)?;
                Some((
                    *name,
                    ctx.load_texture(format!("icon-{name}"), image, TextureOptions::LINEAR),
                ))
            })
            .collect();
        Self {
            scene: ctx.load_texture("scene", scene, TextureOptions::LINEAR),
            scene_blurred: ctx.load_texture("scene-blurred", blurred, TextureOptions::LINEAR),
            scene_size,
            wordmark: ctx.load_texture(
                "wordmark",
                decode(include_bytes!("../images/wordmark.png")),
                TextureOptions::LINEAR,
            ),
            logo: ctx.load_texture(
                "logo",
                decode(include_bytes!("../images/logo.png")),
                TextureOptions::LINEAR,
            ),
            icons,
        }
    }

    /// The images and icons, once [`apply`] has loaded them.
    pub fn of(ctx: &egui::Context) -> Option<Arc<Self>> {
        ctx.data(|data| data.get_temp::<Arc<Self>>(Id::new(ASSETS)))
    }

    /// Draws the icon `name` in `rect`, in `color`.
    pub fn icon(&self, painter: &Painter, name: &str, rect: Rect, color: Color32) {
        if let Some(texture) = self.icons.get(name) {
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                color,
            );
        }
    }
}

/// An image built into the launcher. The images are ours and checked by
/// the tests: one that does not decode is a broken build.
fn decode(bytes: &[u8]) -> ColorImage {
    let image = image::load_from_memory(bytes)
        .expect("the launcher's built-in images decode")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, image.as_raw())
}

/// The city blurred and a little greyed, as `backdrop-filter: blur(22px)
/// saturate(0.75)` shows it under a panel, at a quarter of its size.
fn blurred(scene: &ColorImage) -> ColorImage {
    let [width, height] = scene.size;
    #[allow(clippy::cast_possible_truncation)]
    let full = image::RgbaImage::from_raw(
        width as u32,
        height as u32,
        scene
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect(),
    )
    .unwrap_or_default();
    let small = image::imageops::resize(
        &full,
        (width / 4).max(1) as u32,
        (height / 4).max(1) as u32,
        image::imageops::FilterType::Triangle,
    );
    let mut soft = image::imageops::blur(&small, 9.0);
    for pixel in soft.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let grey = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        let mix = |c: u8| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let value = (f32::from(c) * 0.75 + grey * 0.25)
                .round()
                .clamp(0.0, 255.0) as u8;
            value
        };
        pixel.0 = [mix(r), mix(g), mix(b), a];
    }
    let size = [soft.width() as usize, soft.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, soft.as_raw())
}

/// One of [`ICONS`], in white on transparency, for tinting when drawn.
fn rasterise(body: &str) -> Option<ColorImage> {
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(ICON_PIXELS, ICON_PIXELS)?;
    #[allow(clippy::cast_precision_loss)]
    let scale = ICON_PIXELS as f32 / 24.0;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let size = [ICON_PIXELS as usize, ICON_PIXELS as usize];
    // tiny-skia's pixels are premultiplied.
    Some(ColorImage::from_rgba_premultiplied(size, pixmap.data()))
}

// ---------- the scene and its panels ----------

/// Where the city lies in `window`: covering it, centred across and at
/// 43% down, as `background: center 43% / cover` places it.
fn scene_rect(window: Rect, image: Vec2) -> Rect {
    let scale = (window.width() / image.x).max(window.height() / image.y);
    let size = image * scale;
    let min = pos2(
        window.min.x + (window.width() - size.x) * 0.5,
        window.min.y + (window.height() - size.y) * 0.43,
    );
    Rect::from_min_size(min, size)
}

/// The city across `window`, darkened toward the left as the page's
/// `.scene-image:after` does.
pub fn scene(painter: &Painter, window: Rect, assets: &Assets) {
    painter.rect_filled(window, 0.0, rgba(37, 55, 53, 1.0));
    let placed = scene_rect(window, assets.scene_size);
    painter.image(
        assets.scene.id(),
        placed,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    painter.add(shade_mesh(window, window));
}

/// The shade over the city, within `clip`: `linear-gradient(90deg,
/// #0613214d, #0613210d 65%, #06132133)` across `window`.
fn shade_mesh(window: Rect, clip: Rect) -> Shape {
    let stops = [
        (0.0, rgba(6, 19, 33, 0.30)),
        (0.65, rgba(6, 19, 33, 0.05)),
        (1.0, rgba(6, 19, 33, 0.20)),
    ];
    let mut mesh = Mesh::default();
    for pair in stops.windows(2) {
        let (from, left) = pair[0];
        let (to, right) = pair[1];
        let x0 = window.min.x + window.width() * from;
        let x1 = window.min.x + window.width() * to;
        let band =
            Rect::from_min_max(pos2(x0, window.min.y), pos2(x1, window.max.y)).intersect(clip);
        if band.width() <= 0.0 || band.height() <= 0.0 {
            continue;
        }
        let at = |x: f32| {
            let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
            mix(left, right, t)
        };
        let base = u32::try_from(mesh.vertices.len()).unwrap_or(0);
        mesh.colored_vertex(band.left_top(), at(band.min.x));
        mesh.colored_vertex(band.right_top(), at(band.max.x));
        mesh.colored_vertex(band.left_bottom(), at(band.min.x));
        mesh.colored_vertex(band.right_bottom(), at(band.max.x));
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
    }
    Shape::mesh(mesh)
}

fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let value = (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        value
    };
    Color32::from_rgba_premultiplied(
        lerp(from.r(), to.r()),
        lerp(from.g(), to.g()),
        lerp(from.b(), to.b()),
        lerp(from.a(), to.a()),
    )
}

/// What a frosted panel is filled with.
pub enum Fill {
    Flat(Color32),
    /// `linear-gradient(135deg, from, to)`.
    Diagonal(Color32, Color32),
}

/// A frosted panel at `rect`: the city blurred under it, then its fill and
/// border, as the page's panels with `backdrop-filter` look. A shape, so a
/// panel whose size is known only after its content can be put under it.
pub fn frosted(
    window: Rect,
    rect: Rect,
    assets: &Assets,
    fill: &Fill,
    border: Color32,
    radius: f32,
) -> Shape {
    let placed = scene_rect(window, assets.scene_size);
    let uv = Rect::from_min_max(
        pos2(
            (rect.min.x - placed.min.x) / placed.width(),
            (rect.min.y - placed.min.y) / placed.height(),
        ),
        pos2(
            (rect.max.x - placed.min.x) / placed.width(),
            (rect.max.y - placed.min.y) / placed.height(),
        ),
    );
    let mut shapes = vec![Shape::image(
        assets.scene_blurred.id(),
        rect,
        uv,
        Color32::WHITE,
    )];
    shapes.push(shade_mesh(window, rect));
    match fill {
        Fill::Flat(color) => shapes.push(Shape::rect_filled(rect, radius, *color)),
        Fill::Diagonal(from, to) => {
            let mut mesh = Mesh::default();
            // 135deg: from the top left to the bottom right.
            mesh.colored_vertex(rect.left_top(), *from);
            mesh.colored_vertex(rect.right_top(), mix(*from, *to, 0.5));
            mesh.colored_vertex(rect.left_bottom(), mix(*from, *to, 0.5));
            mesh.colored_vertex(rect.right_bottom(), *to);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(1, 3, 2);
            shapes.push(Shape::mesh(mesh));
        }
    }
    shapes.push(Shape::rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    ));
    Shape::Vec(shapes)
}

/// The city again over the top `top` and bottom `bottom` points of
/// `rect`, from opaque at its edge to clear: the page's scrolling column
/// fades its content out there (`mask-image`).
pub fn fade(painter: &Painter, window: Rect, rect: Rect, assets: &Assets, top: f32, bottom: f32) {
    let placed = scene_rect(window, assets.scene_size);
    let uv = |p: Pos2| {
        pos2(
            (p.x - placed.min.x) / placed.width(),
            (p.y - placed.min.y) / placed.height(),
        )
    };
    for (band, edge_at_top) in [
        (
            Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + top)),
            true,
        ),
        (
            Rect::from_min_max(pos2(rect.min.x, rect.max.y - bottom), rect.max),
            false,
        ),
    ] {
        let (upper, lower) = if edge_at_top {
            (Color32::WHITE, Color32::TRANSPARENT)
        } else {
            (Color32::TRANSPARENT, Color32::WHITE)
        };
        let mut mesh = Mesh::with_texture(assets.scene.id());
        for (corner, color) in [
            (band.left_top(), upper),
            (band.right_top(), upper),
            (band.left_bottom(), lower),
            (band.right_bottom(), lower),
        ] {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: corner,
                uv: uv(corner),
                color,
            });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(1, 3, 2);
        painter.add(Shape::mesh(mesh));
        // The shade over the city fades in with it.
        if let Shape::Mesh(shade) = shade_mesh(window, band) {
            let mut shade = std::sync::Arc::unwrap_or_clone(shade);
            for vertex in &mut shade.vertices {
                let t = ((vertex.pos.y - band.min.y) / band.height()).clamp(0.0, 1.0);
                let keep = if edge_at_top { 1.0 - t } else { t };
                vertex.color = vertex.color.gamma_multiply(keep);
            }
            painter.add(Shape::mesh(shade));
        }
    }
}

/// The page blurred behind a dialog: the city blurred over it, letting a
/// little of the page show through.
pub fn blur_behind(painter: &Painter, window: Rect, assets: &Assets) {
    let placed = scene_rect(window, assets.scene_size);
    let uv = Rect::from_min_max(
        pos2(
            (window.min.x - placed.min.x) / placed.width(),
            (window.min.y - placed.min.y) / placed.height(),
        ),
        pos2(
            (window.max.x - placed.min.x) / placed.width(),
            (window.max.y - placed.min.y) / placed.height(),
        ),
    );
    painter.image(
        assets.scene_blurred.id(),
        window,
        uv,
        Color32::from_white_alpha(215),
    );
}

/// A soft shadow under a panel: `box-shadow: 0 22px 60px`.
pub fn shadow(rect: Rect, color: Color32) -> Shape {
    let shadow = egui::Shadow {
        offset: [0, 22],
        blur: 60,
        spread: 0,
        color,
    };
    Shape::from(shadow.as_shape(rect, CornerRadius::ZERO))
}

// ---------- text ----------

/// Text in `font` and `color`, spaced out by `spacing` points a letter.
pub fn text(words: impl Into<String>, font: FontId, color: Color32) -> RichText {
    RichText::new(words).font(font).color(color)
}

/// A panel's heading: small capitals, spaced (`h2`).
pub fn section_heading(ui: &mut Ui, words: &str) -> Response {
    ui.label(text(words.to_uppercase(), semibold(11.0), TEXT).extra_letter_spacing(0.88))
}

/// A field's name over it: small capitals, and an optional note beside.
pub fn field_label(ui: &mut Ui, words: &str, note: Option<&str>) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label =
            ui.label(text(words.to_uppercase(), body(9.5), MUTED).extra_letter_spacing(0.76));
        if let Some(note) = note {
            ui.label(text(note, body(9.0), MUTED));
        }
        label
    })
    .inner
}

// ---------- controls ----------

/// How a status pill is coloured (`.status-pill[data-state]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Unknown,
    Ready,
    Update,
}

/// A small outlined label: the panel's state, a player's.
pub fn pill(ui: &mut Ui, words: &str, state: Pill) -> Response {
    let (color, border) = match state {
        Pill::Unknown => (rgba(193, 203, 202, 1.0), white(0.125)),
        Pill::Ready => (rgba(189, 201, 194, 1.0), rgba(189, 201, 194, 0.25)),
        Pill::Update => (ACCENT, rgba(209, 215, 218, 0.2)),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(words.to_owned(), body(10.0), color);
    let size = galley.size() + vec2(14.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 0.0, white(0.024));
        ui.painter()
            .rect_stroke(rect, 0.0, Stroke::new(1.0, border), StrokeKind::Inside);
        ui.painter()
            .galley(rect.center() - galley.size() * 0.5, galley, color);
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, words));
    response
}

/// How a quiet button looks.
#[derive(Debug, Clone, Copy)]
pub struct Quiet {
    pub color: Color32,
    pub fill: Color32,
    pub border: Color32,
    pub font: f32,
    pub height: f32,
    /// Its width, or `None` to fit its words.
    pub width: Option<f32>,
}

impl Quiet {
    /// The page's `.quiet-button`.
    pub fn new() -> Self {
        Self {
            color: TEXT,
            fill: panel_fill(0.88),
            border: quiet_border(),
            font: 12.0,
            height: CONTROL,
            width: None,
        }
    }

    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn small(mut self) -> Self {
        self.font = 9.0;
        self.height = 18.0;
        self
    }
}

impl Default for Quiet {
    fn default() -> Self {
        Self::new()
    }
}

/// A quiet, outlined button, with an icon before its words.
pub fn quiet_button(
    ui: &mut Ui,
    enabled: bool,
    icon: Option<&str>,
    words: &str,
    look: Quiet,
) -> Response {
    let font = semibold(look.font);
    let galley = ui
        .painter()
        .layout_no_wrap(words.to_owned(), font, look.color);
    let icon_size = if look.height >= 30.0 { 15.0 } else { 0.0 };
    let pad = if look.height >= 30.0 { 14.0 } else { 8.0 };
    let icon_room = if icon.is_some() && icon_size > 0.0 {
        icon_size + 8.0
    } else {
        0.0
    };
    let natural = galley.size().x + icon_room + pad * 2.0;
    let width = look.width.unwrap_or(natural);
    let (rect, response) = ui.allocate_exact_size(vec2(width, look.height), sense(enabled));
    if ui.is_rect_visible(rect) {
        let hovered = enabled && response.hovered();
        let fill = if hovered {
            white(0.06).blend(look.fill)
        } else {
            look.fill
        };
        let painter = ui.painter();
        painter.rect_filled(rect, RADIUS, fill);
        let border = if hovered { white(0.4) } else { look.border };
        painter.rect_stroke(rect, RADIUS, Stroke::new(1.0, border), StrokeKind::Inside);
        let color = if enabled {
            look.color
        } else {
            look.color.gamma_multiply(0.5)
        };
        let content = galley.size().x + icon_room;
        let mut x = rect.center().x - content * 0.5;
        if let (Some(icon), Some(assets)) = (icon, Assets::of(ui.ctx()))
            && icon_size > 0.0
        {
            let icon_rect = Rect::from_center_size(
                pos2(x + icon_size * 0.5, rect.center().y),
                Vec2::splat(icon_size),
            );
            assets.icon(painter, icon, icon_rect, color);
            x += icon_room;
        }
        painter.galley(
            pos2(x, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, words));
    response
}

/// A button that cannot be pressed now only knows it is hovered.
fn sense(enabled: bool) -> Sense {
    if enabled {
        Sense::click()
    } else {
        Sense::hover()
    }
}

/// A square quiet button with an icon alone, named `name` for screen
/// readers (the dialogs' close button).
pub fn icon_button(ui: &mut Ui, icon: &str, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CONTROL), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if response.hovered() {
            white(0.06).blend(panel_fill(0.88))
        } else {
            panel_fill(0.88)
        };
        painter.rect_filled(rect, RADIUS, fill);
        painter.rect_stroke(
            rect,
            RADIUS,
            Stroke::new(1.0, quiet_border()),
            StrokeKind::Inside,
        );
        if let Some(assets) = Assets::of(ui.ctx()) {
            assets.icon(
                painter,
                icon,
                Rect::from_center_size(rect.center(), Vec2::splat(18.0)),
                TEXT,
            );
        }
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
    response
}

/// The main button: light, full width, 56 high, an icon before its words
/// and an arrow at its end (`#main-action`). `progress`, from 0 to 1,
/// fills it while something downloads.
pub fn primary_button(
    ui: &mut Ui,
    enabled: bool,
    icon: &str,
    words: &str,
    progress: Option<f32>,
) -> Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, PRIMARY), sense(enabled));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let fill = if hovered { ACCENT_HOVER } else { ACCENT };
        // Pressed, it is a step on; unavailable, it fades, as the page's.
        let fill = if enabled || progress.is_some() {
            fill
        } else {
            rgba(209, 215, 218, 0.45)
        };
        painter.add(shadow(rect.shrink(8.0), rgba(209, 215, 218, 0.05)));
        painter.rect_filled(rect, RADIUS, fill);
        if let Some(done) = progress {
            let filled = Rect::from_min_size(
                rect.min,
                vec2(rect.width() * done.clamp(0.0, 1.0), rect.height()),
            );
            painter.rect_filled(filled, RADIUS, ACCENT_HOVER);
        }
        let ink = if enabled || progress.is_some() {
            INK
        } else {
            rgba(23, 26, 23, 0.7)
        };
        if let Some(assets) = Assets::of(ui.ctx()) {
            let left = Rect::from_center_size(
                pos2(rect.min.x + 18.0 + 9.5, rect.center().y),
                Vec2::splat(19.0),
            );
            assets.icon(painter, icon, left, ink);
            // The arrow at the end, hidden while something is under way.
            if progress.is_none() {
                let right = Rect::from_center_size(
                    pos2(rect.max.x - 18.0 - 8.5, rect.center().y),
                    Vec2::splat(19.0),
                );
                assets.icon(painter, "arrow", right, ink);
            }
        }
        painter.text(
            pos2(rect.min.x + 18.0 + 19.0 + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            words,
            semibold(16.0),
            ink,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, words));
    response
}

/// A primary button sized to its words, as in dialogs.
pub fn primary_small(
    ui: &mut Ui,
    enabled: bool,
    icon: Option<&str>,
    words: &str,
    danger: bool,
) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(words.to_owned(), semibold(12.0), INK);
    let icon_room = if icon.is_some() { 24.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_room + 28.0, CONTROL);
    let (rect, response) = ui.allocate_exact_size(size, sense(enabled));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let base = if danger { DANGER } else { ACCENT };
        let fill = match (enabled, response.hovered()) {
            (false, _) => base.gamma_multiply(0.55),
            (true, true) => {
                if danger {
                    rgba(246, 194, 186, 1.0)
                } else {
                    ACCENT_HOVER
                }
            }
            (true, false) => base,
        };
        painter.rect_filled(rect, RADIUS, fill);
        let ink = if danger { rgba(42, 15, 11, 1.0) } else { INK };
        let ink = if enabled {
            ink
        } else {
            ink.gamma_multiply(0.7)
        };
        let mut x = rect.min.x + 14.0;
        if let (Some(icon), Some(assets)) = (icon, Assets::of(ui.ctx())) {
            assets.icon(
                painter,
                icon,
                Rect::from_center_size(pos2(x + 8.0, rect.center().y), Vec2::splat(16.0)),
                ink,
            );
            x += icon_room;
        }
        painter.galley(
            pos2(x, rect.center().y - galley.size().y * 0.5),
            galley,
            ink,
        );
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, words));
    response
}

/// A text field as the page draws them: 40 high, dark, a hairline border.
/// `mono` sets it in Consolas, spaced and in capitals, as the invite is.
pub fn text_field<'t>(buffer: &'t mut String, hint: &str, mono_caps: bool) -> egui::TextEdit<'t> {
    let font = if mono_caps { mono(13.0) } else { body(13.0) };
    let hint = if mono_caps {
        RichText::new(hint)
            .font(mono(13.0))
            .color(rgba(156, 169, 173, 0.55))
            .extra_letter_spacing(2.34)
    } else {
        RichText::new(hint)
            .font(body(13.0))
            .color(rgba(156, 169, 173, 0.55))
    };
    egui::TextEdit::singleline(buffer)
        .font(font)
        .hint_text(WidgetText::from(hint))
        .margin(egui::Margin::symmetric(11, 8))
        .min_size(vec2(0.0, CONTROL))
        .vertical_align(egui::Align::Center)
        .desired_width(f32::INFINITY)
}

/// A drop-down as tall as a text field.
pub fn select<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = CONTROL;
        ui.spacing_mut().button_padding = vec2(11.0, 8.0);
        add(ui)
    })
    .inner
}

/// The chevron at the end of a drop-down, as the page's selects have.
pub fn chevron(ui: &Ui, rect: Rect, _visuals: &egui::style::WidgetVisuals, _open: bool) {
    let center = rect.center();
    let stroke = Stroke::new(1.4, TEXT);
    let half = 4.0;
    ui.painter().line(
        vec![
            pos2(center.x - half, center.y - half * 0.5),
            pos2(center.x, center.y + half * 0.5),
            pos2(center.x + half, center.y - half * 0.5),
        ],
        stroke,
    );
}

/// A step of the five, with its disc: hollow until done, then light with
/// a check (`.steps li`).
pub fn step(ui: &mut Ui, words: &str, done: bool) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (disc, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        let painter = ui.painter();
        if done {
            painter.circle_filled(disc.center(), 9.0, ACCENT);
            if let Some(assets) = Assets::of(ui.ctx()) {
                assets.icon(
                    painter,
                    "step-done",
                    Rect::from_center_size(disc.center(), Vec2::splat(12.0)),
                    BG,
                );
            }
        } else {
            painter.circle_stroke(disc.center(), 8.5, Stroke::new(1.0, white(0.22)));
        }
        ui.label(text(words, body(13.0), if done { TEXT } else { MUTED }))
    })
    .inner
}

/// A row of the panel's list: its name left, its value right, in
/// Consolas; `large` for the room's name.
pub fn row(ui: &mut Ui, name: &str, value: &str, large: bool, last: bool) {
    let height = if large { 48.0 } else { 36.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    painter.text(
        pos2(rect.min.x, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        body(11.0),
        LABEL,
    );
    let (font, color) = if large {
        (mono(21.0), Color32::WHITE)
    } else {
        (mono(13.0), TEXT)
    };
    let galley = painter.layout_no_wrap(value.to_owned(), font, color);
    let at = pos2(
        rect.max.x - galley.size().x,
        rect.center().y - galley.size().y * 0.5,
    );
    painter.galley(at, galley, color);
    if !last {
        painter.hline(rect.x_range(), rect.max.y, Stroke::new(1.0, white(0.047)));
    }
}

/// A hairline across `ui`.
pub fn hairline(ui: &mut Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, color));
}

/// A box in a colour: a note from the server, how mods differ.
pub fn note_box(
    ui: &mut Ui,
    fill: Color32,
    border: Option<Color32>,
    radius: f32,
    add: impl FnOnce(&mut Ui),
) {
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(border.map_or(Stroke::NONE, |color| Stroke::new(1.0, color)))
        .corner_radius(radius)
        .inner_margin(egui::Margin::symmetric(12, 10));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// A point in `rect` at fractions `(x, y)` of it.
pub fn at(rect: Rect, x: f32, y: f32) -> Pos2 {
    pos2(
        rect.min.x + rect.width() * x,
        rect.min.y + rect.height() * y,
    )
}
