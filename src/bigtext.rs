//! Rasterized headings, for terminals that have graphics but not DECDHL.
//!
//! kitty and ghostty are exactly that case: they speak the Kitty graphics
//! protocol fluently and do not implement DECDHL at all. Rather than give up on
//! big headings there, the heading text is rendered to a bitmap at twice the
//! cell height and placed over the two rows the layout already reserved for it.
//!
//! The layout does not need to know: a double-height line occupies two rows and
//! half the columns whichever way it is drawn.

use crate::graphics::CellSize;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use anyhow::{Result, bail};
use std::path::PathBuf;

/// Font files to try when fontconfig is unavailable, in preference order.
const CANDIDATES: &[&str] = &[
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    // macOS
    "/System/Library/Fonts/SFNSDisplay.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial Bold.ttf",
    // Windows
    "C:\\Windows\\Fonts\\segoeuib.ttf",
    "C:\\Windows\\Fonts\\arialbd.ttf",
];

/// Locate a bold font to draw headings with.
///
/// fontconfig knows the answer on Linux and is usually installed; the hardcoded
/// list is the fallback, and if neither turns anything up the caller simply
/// does not offer rasterized headings.
pub fn find_font() -> Option<PathBuf> {
    if let Ok(output) = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif:bold"])
        .output()
        && output.status.success()
    {
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
        if path.is_file() {
            return Some(path);
        }
    }
    CANDIDATES.iter().map(PathBuf::from).find(|p| p.is_file())
}

pub struct Renderer {
    font: FontVec,
}

impl Renderer {
    pub fn load(path: &std::path::Path) -> Result<Renderer> {
        let bytes = std::fs::read(path)?;
        let Ok(font) = FontVec::try_from_vec(bytes) else {
            bail!("{} is not a usable font", path.display());
        };
        Ok(Renderer { font })
    }

    pub fn discover() -> Option<Renderer> {
        Renderer::load(&find_font()?).ok()
    }

    /// Draw `text` into an RGBA bitmap `cols` by `rows` cells in size.
    ///
    /// The background stays transparent so the terminal's own background, and
    /// any image behind it, shows through — the same reason the `terminal`
    /// theme paints no background of its own.
    pub fn render(
        &self,
        text: &str,
        color: (u8, u8, u8),
        cols: u16,
        rows: u16,
        cell: CellSize,
    ) -> Option<Vec<u8>> {
        let width = cols as u32 * cell.w as u32;
        let height = rows as u32 * cell.h as u32;
        if width == 0 || height == 0 || text.trim().is_empty() {
            return None;
        }

        // Leave a little headroom so ascenders and descenders are not clipped.
        let px = (height as f32 * 0.78).max(4.0);
        let scaled = self.font.as_scaled(PxScale::from(px));
        let ascent = scaled.ascent();

        let mut canvas = vec![0u8; (width * height * 4) as usize];
        let mut pen = 0.0f32;
        let baseline = ascent + (height as f32 - scaled.height()) / 2.0;
        let mut previous: Option<ab_glyph::GlyphId> = None;

        for c in text.chars() {
            let id = self.font.glyph_id(c);
            if let Some(prev) = previous {
                pen += scaled.kern(prev, id);
            }
            previous = Some(id);

            let glyph =
                id.with_scale_and_position(PxScale::from(px), ab_glyph::point(pen, baseline));
            if let Some(outline) = self.font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, coverage| {
                    let x = bounds.min.x as i32 + gx as i32;
                    let y = bounds.min.y as i32 + gy as i32;
                    if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
                        return;
                    }
                    let i = ((y as u32 * width + x as u32) * 4) as usize;
                    let alpha = (coverage * 255.0).clamp(0.0, 255.0) as u8;
                    // Glyphs may overlap; keep the strongest coverage.
                    if alpha > canvas[i + 3] {
                        canvas[i] = color.0;
                        canvas[i + 1] = color.1;
                        canvas[i + 2] = color.2;
                        canvas[i + 3] = alpha;
                    }
                });
            }
            pen += scaled.h_advance(id);
            if pen > width as f32 {
                break;
            }
        }

        let image = image::RgbaImage::from_raw(width, height, canvas)?;
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        Some(png)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellSize = CellSize { w: 8, h: 16 };

    fn renderer() -> Option<Renderer> {
        Renderer::discover()
    }

    #[test]
    fn a_font_is_found_on_this_machine_or_rasterizing_is_declined() {
        // Never a hard failure: a machine with no fonts simply does not offer
        // rasterized headings.
        match find_font() {
            Some(path) => assert!(path.is_file()),
            None => assert!(renderer().is_none()),
        }
    }

    #[test]
    fn rendering_produces_a_png_of_the_requested_size() {
        let Some(renderer) = renderer() else {
            return;
        };
        let png = renderer
            .render("Heading", (255, 0, 0), 20, 2, CELL)
            .unwrap();
        assert_eq!(&png[..4], b"\x89PNG");
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.width(), 20 * CELL.w as u32);
        assert_eq!(decoded.height(), 2 * CELL.h as u32);
    }

    #[test]
    fn the_background_stays_transparent() {
        let Some(renderer) = renderer() else {
            return;
        };
        let png = renderer.render("I", (255, 255, 255), 10, 2, CELL).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        // The bottom-right corner is well clear of a single narrow glyph.
        let corner = image.get_pixel(image.width() - 1, image.height() - 1);
        assert_eq!(corner.0[3], 0, "corner should be fully transparent");
    }

    #[test]
    fn glyphs_are_drawn_in_the_requested_colour() {
        let Some(renderer) = renderer() else {
            return;
        };
        let png = renderer.render("HHHH", (10, 200, 30), 12, 2, CELL).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        let painted = image
            .pixels()
            .find(|p| p.0[3] > 200)
            .expect("something was drawn");
        assert_eq!((painted.0[0], painted.0[1], painted.0[2]), (10, 200, 30));
    }

    #[test]
    fn empty_text_renders_nothing() {
        let Some(renderer) = renderer() else {
            return;
        };
        assert!(renderer.render("   ", (0, 0, 0), 10, 2, CELL).is_none());
        assert!(renderer.render("x", (0, 0, 0), 0, 2, CELL).is_none());
    }

    #[test]
    fn text_wider_than_the_canvas_is_clipped_rather_than_overflowing() {
        let Some(renderer) = renderer() else {
            return;
        };
        let png = renderer
            .render(&"wide ".repeat(50), (255, 255, 255), 10, 2, CELL)
            .unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.width(), 10 * CELL.w as u32);
    }
}
