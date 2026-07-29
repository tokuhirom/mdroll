//! Rasterizing SVG, through `resvg`.
//!
//! Terminals draw pixels, so a vector image has to become a bitmap somewhere.
//! Doing it here, rather than once at load time, means it can be rendered at
//! exactly the size it will be shown at — which is the whole point of a logo
//! being an SVG in the first place.
//!
//! READMEs are full of them: project logos, and every shields.io badge.

use anyhow::{Context, Result};
use resvg::tiny_skia;
use resvg::usvg;
use std::sync::{Arc, OnceLock};

/// Whether this data looks like SVG.
///
/// Sniffed from the content rather than taken from the file name, because the
/// name is often no help: a badge URL like `img.shields.io/…?style=flat` has no
/// extension at all, and the cache file it lands in inherits that.
pub fn looks_like_svg(data: &[u8]) -> bool {
    // Enough to get past an XML declaration, a doctype, and a comment or two.
    let head = &data[..data.len().min(4096)];
    String::from_utf8_lossy(head).contains("<svg")
}

/// Parse options carrying the system fonts.
///
/// Loading the font database takes long enough to notice, and a badge is mostly
/// text, so it is loaded once and shared.
fn options() -> usvg::Options<'static> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fontdb = FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    usvg::Options {
        fontdb,
        ..usvg::Options::default()
    }
}

fn parse(data: &[u8]) -> Result<usvg::Tree> {
    usvg::Tree::from_data(data, &options()).context("not valid SVG")
}

/// The size an SVG asks to be drawn at, from its `width`/`height` or `viewBox`.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let size = parse(data).ok()?.size();
    Some((
        (size.width().ceil() as u32).max(1),
        (size.height().ceil() as u32).max(1),
    ))
}

/// Rasterize to a PNG that fits `target` pixels, keeping the aspect ratio.
///
/// The background is left transparent, so the terminal's own colours show
/// through the same way they do behind a diagram.
pub fn render_png(data: &[u8], target: (u32, u32)) -> Result<Vec<u8>> {
    let tree = parse(data)?;
    let size = tree.size();
    let scale = (target.0 as f32 / size.width()).min(target.1 as f32 / size.height());
    // A degenerate or absurd scale would mean allocating a pixmap of nonsense
    // dimensions, so clamp before rounding.
    let scale = if scale.is_finite() && scale > 0.0 {
        scale.min(64.0)
    } else {
        1.0
    };
    let w = ((size.width() * scale).round() as u32).clamp(1, 8192);
    let h = ((size.height() * scale).round() as u32).clamp(1, 8192);

    let mut pixmap = tiny_skia::Pixmap::new(w, h).context("SVG has no usable size")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap.encode_png()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIRCLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
        <circle cx="20" cy="10" r="10" fill="#ff0000"/>
    </svg>"##;

    #[test]
    fn an_xml_declaration_does_not_hide_the_svg_tag() {
        let data = format!("<?xml version=\"1.0\"?>\n<!-- a comment -->\n{CIRCLE}");
        assert!(looks_like_svg(data.as_bytes()));
        assert!(!looks_like_svg(b"\x89PNG\r\n\x1a\n"));
        assert!(!looks_like_svg(b""));
    }

    #[test]
    fn the_declared_size_is_what_gets_reported() {
        assert_eq!(dimensions(CIRCLE.as_bytes()), Some((40, 20)));
        assert_eq!(dimensions(b"not svg at all"), None);
    }

    #[test]
    fn a_viewbox_alone_is_enough_to_get_a_size() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 30 60"/>"#;
        assert_eq!(dimensions(svg.as_bytes()), Some((30, 60)));
    }

    #[test]
    fn rasterizing_fills_the_target_without_distorting_the_shape() {
        let png = render_png(CIRCLE.as_bytes(), (80, 80)).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        // Height is the binding constraint at 2:1, so 20 * 4 = 80 wide.
        assert_eq!((img.width(), img.height()), (80, 40));
    }

    #[test]
    fn the_background_stays_transparent() {
        let png = render_png(CIRCLE.as_bytes(), (40, 20)).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(
            img.get_pixel(0, 0).0[3],
            0,
            "the corner is outside the circle"
        );
        assert!(img.get_pixel(20, 10).0[3] > 200, "the middle is inside it");
    }
}
