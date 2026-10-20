//! Native mods' preview images in the mod browser and manager: fetched
//! from the address each mod's file names, in the background, once a
//! session, and shown only (docs/NATIVE_MODS.md, "In the launcher").
//!
//! An image is read over HTTPS only, at most [`MAX_PREVIEW`] bytes, and
//! must decode as PNG or JPEG; it is shrunk to [`MAX_SIDE`] pixels at most.
//! One that cannot be had is shown as the mod's initials, as one still
//! loading is.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// Largest preview image read, in bytes.
pub const MAX_PREVIEW: u64 = 4 << 20;
/// Longest side of a preview kept, in pixels.
pub const MAX_SIDE: u32 = 640;

const USER_AGENT: &str = concat!("TPF3-MP-launcher/", env!("CARGO_PKG_VERSION"));

enum Preview {
    /// Being fetched.
    Loading,
    /// Fetched, not yet a texture.
    Decoded(ColorImage),
    Ready(TextureHandle),
    Failed,
}

/// Previews fetched since last looked at: an address and its image, or
/// none when it could not be had.
type Arrived = Arc<Mutex<Vec<(String, Option<ColorImage>)>>>;

/// The previews asked for this session, by address.
#[derive(Default)]
pub struct Previews {
    shown: HashMap<String, Preview>,
    arrived: Arrived,
}

impl Previews {
    /// Holds `image` as the preview at `url`, as if fetched.
    pub fn insert(&mut self, url: &str, image: ColorImage) {
        self.shown.insert(url.to_owned(), Preview::Decoded(image));
    }

    /// The preview at `url`, once fetched; asks for it the first time.
    pub fn get(&mut self, ctx: &egui::Context, url: &str) -> Option<TextureHandle> {
        let arrived: Vec<_> = std::mem::take(
            &mut *self
                .arrived
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (from, image) in arrived {
            self.shown
                .insert(from, image.map_or(Preview::Failed, Preview::Decoded));
        }
        let entry = self.shown.entry(url.to_owned()).or_insert_with(|| {
            fetch(ctx.clone(), url.to_owned(), Arc::clone(&self.arrived));
            Preview::Loading
        });
        if let Preview::Decoded(image) = entry {
            let texture = ctx.load_texture(
                format!("preview {url}"),
                std::mem::take(image),
                TextureOptions::LINEAR,
            );
            *entry = Preview::Ready(texture);
        }
        match entry {
            Preview::Ready(texture) => Some(texture.clone()),
            _ => None,
        }
    }
}

/// Fetches and decodes the preview at `url` on a thread of its own.
fn fetch(ctx: egui::Context, url: String, into: Arrived) {
    std::thread::spawn(move || {
        let image = read(&url)
            .inspect_err(
                |error| tracing::info!(%url, %error, "a native mod's preview is not shown"),
            )
            .ok();
        into.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((url, image));
        ctx.request_repaint();
    });
}

fn read(url: &str) -> Result<ColorImage, String> {
    if !url.starts_with("https://") {
        return Err("not an https address".into());
    }
    let agent = tpf3mp_nativemods::fetch::agent(true, Duration::from_secs(20));
    let bytes = tpf3mp_nativemods::fetch::get(&agent, url, MAX_PREVIEW, USER_AGENT)
        .map_err(|error| error.to_string())?;
    decode(&bytes)
}

/// `bytes` as an image no larger than [`MAX_SIDE`] a side.
pub fn decode(bytes: &[u8]) -> Result<ColorImage, String> {
    let format = image::guess_format(bytes).map_err(|error| error.to_string())?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
        return Err(format!("{format:?} is not PNG or JPEG"));
    }
    let image =
        image::load_from_memory_with_format(bytes, format).map_err(|error| error.to_string())?;
    let image = if image.width() > MAX_SIDE || image.height() > MAX_SIDE {
        image.thumbnail(MAX_SIDE, MAX_SIDE)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_png_or_jpeg_and_shrunk() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(1280, 720, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let shown = decode(&png).unwrap();
        assert_eq!(shown.size, [640, 360]);
        assert!(decode(b"GIF89a not one").is_err());
        assert!(decode(b"").is_err());
        assert!(read("http://example.invalid/a.png").is_err(), "https only");
    }
}
