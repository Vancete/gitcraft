//! Pictures from a repository (an image's diff) or a pull request (a long screenshot) can be of
//! any size, but the GPU only takes textures up to `max_texture_side` (8192) a side, and wgpu
//! treats a bigger one as a fatal error: viewing a 9000 px wide PNG would end the app. Those
//! are shrunk to fit before they become textures; they keep the size they show at.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use egui::ColorImage;
use egui::load::{ImageLoadResult, ImageLoader, ImagePoll, SizeHint};
use egui_extras::loaders::image_loader::ImageCrateLoader;

/// Installs the loader; after `egui_extras::install_image_loaders`, so that it is asked first.
pub fn install(ctx: &egui::Context) {
    ctx.add_image_loader(Arc::new(Fitting::default()));
}

/// egui_extras' loader, with the pictures too big for a texture shrunk.
#[derive(Default)]
struct Fitting {
    inner: ImageCrateLoader,
    /// The shrunk pictures by URI, made once.
    shrunk: Mutex<HashMap<String, Arc<ColorImage>>>,
}

impl Fitting {
    fn shrunk(&self) -> MutexGuard<'_, HashMap<String, Arc<ColorImage>>> {
        self.shrunk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ImageLoader for Fitting {
    fn id(&self) -> &str {
        egui::generate_loader_id!(Fitting)
    }

    fn load(&self, ctx: &egui::Context, uri: &str, size_hint: SizeHint) -> ImageLoadResult {
        // Asked before the SVG loader: the raster decoder would fail on an SVG (the app's own
        // icons) and that error would end the search for a loader before the SVG one is asked.
        // SVGs are drawn at the size asked for, so never too big anyway.
        if is_svg(uri) {
            return Err(egui::load::LoadError::NotSupported);
        }
        match self.inner.load(ctx, uri, size_hint)? {
            ImagePoll::Ready { image } => {
                let max = ctx.input(|i| i.max_texture_side).max(1);
                if fits(&image, max) {
                    return Ok(ImagePoll::Ready { image });
                }
                let image = Arc::clone(
                    self.shrunk()
                        .entry(uri.to_owned())
                        .or_insert_with(|| Arc::new(fit(&image, max))),
                );
                Ok(ImagePoll::Ready { image })
            }
            pending => Ok(pending),
        }
    }

    fn forget(&self, uri: &str) {
        self.inner.forget(uri);
        self.shrunk().remove(uri);
    }

    fn forget_all(&self) {
        self.inner.forget_all();
        self.shrunk().clear();
    }

    fn end_pass(&self, pass_index: u64) {
        self.inner.end_pass(pass_index);
    }

    fn byte_size(&self) -> usize {
        let shrunk: usize = self
            .shrunk()
            .values()
            .map(|image| image.pixels.len() * 4)
            .sum();
        self.inner.byte_size() + shrunk
    }

    fn has_pending(&self) -> bool {
        self.inner.has_pending()
    }
}

/// Whether `uri` names an SVG picture, by its extension.
fn is_svg(uri: &str) -> bool {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    path.len() >= 4
        && path
            .get(path.len() - 4..)
            .is_some_and(|ext| ext.eq_ignore_ascii_case(".svg"))
}

fn fits(image: &ColorImage, max: usize) -> bool {
    image.size[0] <= max && image.size[1] <= max
}

/// `image` scaled down to at most `max` texels a side, keeping its proportions and the size it
/// shows at.
fn fit(image: &ColorImage, max: usize) -> ColorImage {
    let [w, h] = image.size;
    let longest = w.max(h).max(1) as f64;
    let side = |n: usize| ((n as f64 * max as f64 / longest).round() as usize).clamp(1, max);
    let (nw, nh) = (side(w), side(h));
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let pixels = match image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
        Some(rgba) => image::imageops::resize(
            &rgba,
            nw as u32,
            nh as u32,
            image::imageops::FilterType::Triangle,
        )
        .pixels()
        .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
        .collect(),
        None => vec![egui::Color32::TRANSPARENT; nw * nh],
    };
    ColorImage {
        size: [nw, nh],
        source_size: image.source_size,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_too_wide_for_a_texture_is_shrunk_to_fit() {
        let image = ColorImage::new([9000, 3], vec![egui::Color32::RED; 27_000]);
        let small = fit(&image, 8192);
        assert_eq!(small.size, [8192, 3]);
        assert_eq!(small.pixels.len(), 8192 * 3);
        assert_eq!(small.source_size, image.source_size);
        assert!(small.pixels.iter().all(|&c| c == egui::Color32::RED));
        let tall = fit(
            &ColorImage::new([2, 20_000], vec![egui::Color32::BLUE; 40_000]),
            8192,
        );
        assert_eq!(tall.size, [1, 8192]);
    }

    /// Through the loader, as the image views ask for it: what comes back fits.
    /// The app's icons are SVGs: asked first, this loader leaves them to the SVG loader.
    #[test]
    fn icons_still_reach_the_svg_loader() -> Result<(), String> {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        install(&ctx);
        let uri = "bytes://icons/check.svg";
        ctx.include_bytes(uri, include_bytes!("../assets/icons/check.svg").to_vec());
        let started = std::time::Instant::now();
        loop {
            match ctx
                .try_load_image(uri, SizeHint::default())
                .map_err(|e| e.to_string())?
            {
                ImagePoll::Ready { .. } => return Ok(()),
                ImagePoll::Pending { .. } if started.elapsed().as_secs() < 10 => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                ImagePoll::Pending { .. } => return Err("never loaded".to_owned()),
            }
        }
    }

    #[test]
    fn the_loader_hands_out_pictures_that_fit() -> Result<(), String> {
        let ctx = egui::Context::default();
        let max = ctx.input(|i| i.max_texture_side);
        let mut png = Vec::new();
        image::RgbaImage::new(max as u32 + 10, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        let uri = "bytes://wide.png";
        ctx.include_bytes(uri, png);
        let loader = Fitting::default();
        let started = std::time::Instant::now();
        loop {
            match loader
                .load(&ctx, uri, SizeHint::default())
                .map_err(|e| e.to_string())?
            {
                ImagePoll::Ready { image } => {
                    assert!(fits(&image, max), "{:?}", image.size);
                    assert_eq!(image.source_size, egui::vec2(max as f32 + 10.0, 2.0));
                    return Ok(());
                }
                ImagePoll::Pending { .. } if started.elapsed().as_secs() < 10 => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                ImagePoll::Pending { .. } => return Err("never loaded".to_owned()),
            }
        }
    }
}
