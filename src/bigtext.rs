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

fn fc_match(pattern: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    path.is_file().then_some(path)
}

/// Locate a bold font to draw headings with.
///
/// fontconfig knows the answer on Linux and is usually installed; the hardcoded
/// list is the fallback, and if neither turns anything up the caller simply
/// does not offer rasterized headings.
pub fn find_font() -> Option<PathBuf> {
    fc_match("sans-serif:bold")
        .or_else(|| CANDIDATES.iter().map(PathBuf::from).find(|p| p.is_file()))
}

/// Fonts to draw headings with, most preferred first.
///
/// A Latin bold face has no CJK glyphs, so a heading in Japanese would come out
/// as a row of blanks. The chain ends with whatever fontconfig recommends for
/// Japanese, and glyphs are looked up in order.
pub fn font_chain() -> Vec<PathBuf> {
    let mut fonts: Vec<PathBuf> = Vec::new();
    for candidate in [
        find_font(),
        fc_match("sans-serif:bold:lang=ja"),
        fc_match("sans-serif:lang=ja"),
    ]
    .into_iter()
    .flatten()
    {
        if !fonts.contains(&candidate) {
            fonts.push(candidate);
        }
    }
    fonts
}

pub struct Renderer {
    /// Ordered fallback chain; the first font with a glyph for a character
    /// draws it.
    fonts: Vec<FontVec>,
}

/// What to draw around a heading, in colors the caller has already resolved.
///
/// Decoration is only possible on this path. A DECDHL heading is text the
/// terminal scales for itself and there is nowhere to put a rule that does not
/// cost a row; a bitmap already owns two rows and uses 0.78 of them, so a line
/// under the text and a bar beside it are free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeadingDecor {
    /// A rule under the text, the way GitHub borders `h1` and `h2`.
    pub border: Option<(u8, u8, u8)>,
    /// A bar down the left, in the blank the margin and indent leave.
    pub bar: Option<(u8, u8, u8)>,
}

impl HeadingDecor {
    pub fn is_none(&self) -> bool {
        self.border.is_none() && self.bar.is_none()
    }
}

/// A stretch of a heading drawn in one color.
///
/// A heading is not one color. `# See `config.toml`` has a code span in it, and
/// under DECDHL that span keeps the colour the theme gives it, because the
/// terminal is drawing styled text. Handing the rasterizer a single string and
/// a single colour threw that away on exactly the terminals that get a bitmap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run<'a> {
    pub text: &'a str,
    pub color: (u8, u8, u8),
}

/// Paint a solid rectangle, clipped to the canvas.
///
/// Decoration is drawn over whatever is already there rather than blended with
/// it: the glyphs and the rules do not overlap by construction, and a rule that
/// faded where it crossed a descender would look like a rendering fault.
fn fill(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    x: std::ops::Range<u32>,
    y: std::ops::Range<u32>,
    color: (u8, u8, u8),
) {
    for py in y.start..y.end.min(height) {
        for px in x.start..x.end.min(width) {
            let i = ((py * width + px) * 4) as usize;
            canvas[i] = color.0;
            canvas[i + 1] = color.1;
            canvas[i + 2] = color.2;
            canvas[i + 3] = 255;
        }
    }
}

/// Draw the border and the bar around text occupying `start..end` horizontally.
///
/// Both are sized from the cell rather than from a constant, so they keep their
/// proportions on a terminal with a larger font: a two-pixel rule is a hairline
/// at one cell size and a stripe at another.
fn draw_decor(
    canvas: &mut [u8],
    width: u32,
    height: u32,
    cell: CellSize,
    start: u32,
    end: u32,
    decor: HeadingDecor,
) {
    let cell_h = cell.h.max(1) as u32;
    let cell_w = cell.w.max(1) as u32;

    if let Some(color) = decor.border {
        // The text is 0.78 of the canvas and vertically centred, so the bottom
        // tenth is clear of descenders and needs no measuring to avoid them.
        let thickness = (cell_h / 8).max(1);
        let gap = (cell_h / 8).max(1);
        let bottom = height.saturating_sub(gap);
        let top = bottom.saturating_sub(thickness);
        if end > start {
            fill(canvas, width, height, start..end, top..bottom, color);
        }
    }

    if let Some(color) = decor.bar {
        let bar_w = (cell_w / 3).max(2);
        let gap = (cell_w / 2).max(2);
        // The bar goes in the blank in front of the text. Where there is no
        // room for it — no margin, no indent — it is dropped rather than drawn
        // over the first letter.
        if let Some(right) = start.checked_sub(gap)
            && let Some(left) = right.checked_sub(bar_w)
        {
            fill(canvas, width, height, left..right, 0..height, color);
        }
    }
}

fn load_font(path: &std::path::Path) -> Result<FontVec> {
    let bytes = std::fs::read(path)?;
    // A .ttc is a collection; index 0 is the right face for every fontconfig
    // recommendation seen so far.
    let font = if path.extension().is_some_and(|e| e == "ttc") {
        FontVec::try_from_vec_and_index(bytes, 0)
    } else {
        FontVec::try_from_vec(bytes)
    };
    match font {
        Ok(font) => Ok(font),
        Err(_) => bail!("{} is not a usable font", path.display()),
    }
}

impl Renderer {
    pub fn load(path: &std::path::Path) -> Result<Renderer> {
        Ok(Renderer {
            fonts: vec![load_font(path)?],
        })
    }

    pub fn discover() -> Option<Renderer> {
        let fonts: Vec<FontVec> = font_chain()
            .iter()
            .filter_map(|p| load_font(p).ok())
            .collect();
        (!fonts.is_empty()).then_some(Renderer { fonts })
    }

    /// The first font in the chain with a glyph for `c`.
    fn pick(&self, c: char) -> (&FontVec, ab_glyph::GlyphId) {
        for font in &self.fonts {
            let id = font.glyph_id(c);
            if id.0 != 0 {
                return (font, id);
            }
        }
        (&self.fonts[0], self.fonts[0].glyph_id(c))
    }

    /// Draw `runs` into an RGBA bitmap `cols` by `rows` cells in size.
    ///
    /// The background stays transparent so the terminal's own background, and
    /// any image behind it, shows through — the same reason the `terminal`
    /// theme paints no background of its own.
    pub fn render(
        &self,
        runs: &[Run<'_>],
        cols: u16,
        rows: u16,
        cell: CellSize,
        decor: HeadingDecor,
    ) -> Option<Vec<u8>> {
        let width = cols as u32 * cell.w as u32;
        let height = rows as u32 * cell.h as u32;
        if width == 0 || height == 0 || runs.iter().all(|r| r.text.trim().is_empty()) {
            return None;
        }

        // Leave a little headroom so ascenders and descenders are not clipped.
        let px = (height as f32 * 0.78).max(4.0);
        let primary = self.fonts[0].as_scaled(PxScale::from(px));
        let ascent = primary.ascent();

        let mut canvas = vec![0u8; (width * height * 4) as usize];
        let mut pen = 0.0f32;
        // One baseline for the whole run, taken from the primary font, so a
        // fallback glyph does not sit at a different height from its neighbours.
        let baseline = ascent + (height as f32 - primary.height()) / 2.0;
        let mut previous: Option<ab_glyph::GlyphId> = None;
        // Where the text itself starts and ends, ignoring the blank the margin
        // and any indent put in front of it. Decoration lines up with the
        // glyphs, not with the left edge of the screen.
        let mut text_start: Option<f32> = None;
        let mut text_end = 0.0f32;

        'runs: for run in runs {
            let color = run.color;
            for c in run.text.chars() {
                if !c.is_whitespace() {
                    text_start.get_or_insert(pen);
                }
                let (font, id) = self.pick(c);
                let scaled = font.as_scaled(PxScale::from(px));
                if let Some(prev) = previous {
                    pen += scaled.kern(prev, id);
                }
                previous = Some(id);

                let glyph =
                    id.with_scale_and_position(PxScale::from(px), ab_glyph::point(pen, baseline));
                if let Some(outline) = font.outline_glyph(glyph) {
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
                if !c.is_whitespace() {
                    text_end = pen;
                }
                if pen > width as f32 {
                    break 'runs;
                }
            }
        }

        if !decor.is_none() {
            let start = text_start.unwrap_or(0.0).max(0.0) as u32;
            let end = (text_end.min(width as f32) as u32).max(start);
            draw_decor(&mut canvas, width, height, cell, start, end, decor);
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
            .render(
                &[Run {
                    text: "Heading",
                    color: (255, 0, 0),
                }],
                20,
                2,
                CELL,
                HeadingDecor::default(),
            )
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
        let png = renderer
            .render(
                &[Run {
                    text: "I",
                    color: (255, 255, 255),
                }],
                10,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
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
        let png = renderer
            .render(
                &[Run {
                    text: "HHHH",
                    color: (10, 200, 30),
                }],
                12,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        let painted = image
            .pixels()
            .find(|p| p.0[3] > 200)
            .expect("something was drawn");
        assert_eq!((painted.0[0], painted.0[1], painted.0[2]), (10, 200, 30));
    }

    #[test]
    fn cjk_text_finds_a_glyph_through_the_fallback_chain() {
        let Some(renderer) = renderer() else {
            return;
        };
        // A Latin bold face has no kanji; if the chain is not consulted the
        // bitmap comes out blank.
        let png = renderer
            .render(
                &[Run {
                    text: "日本語の見出し",
                    color: (255, 255, 255),
                }],
                30,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        let painted = image.pixels().filter(|p| p.0[3] > 128).count();
        assert!(
            painted > 100,
            "only {painted} pixels drawn — glyphs missing"
        );
    }

    #[test]
    fn a_mixed_script_heading_draws_both_scripts() {
        let Some(renderer) = renderer() else {
            return;
        };
        let latin = renderer
            .render(
                &[Run {
                    text: "Heading",
                    color: (255, 255, 255),
                }],
                30,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        let mixed = renderer
            .render(
                &[Run {
                    text: "Heading 見出し",
                    color: (255, 255, 255),
                }],
                30,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        let count = |png: &[u8]| {
            image::load_from_memory(png)
                .unwrap()
                .to_rgba8()
                .pixels()
                .filter(|p| p.0[3] > 128)
                .count()
        };
        assert!(count(&mixed) > count(&latin), "the kanji added nothing");
    }

    fn decoded(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).unwrap().to_rgba8()
    }

    /// Pixels of exactly this colour, as (x, y).
    fn pixels_of(image: &image::RgbaImage, color: [u8; 4]) -> Vec<(u32, u32)> {
        image
            .enumerate_pixels()
            .filter(|(_, _, p)| p.0 == color)
            .map(|(x, y, _)| (x, y))
            .collect()
    }

    /// Leftmost pixel the glyphs themselves reach.
    ///
    /// Measured rather than worked out: a bitmap heading is laid out with the
    /// font's own advances, not on the terminal's cell grid, so where the text
    /// starts is not `margin * cell.w` and cannot be predicted from the string.
    fn text_left(renderer: &Renderer, text: &str) -> u32 {
        let png = renderer
            .render(
                &[Run {
                    text,
                    color: (255, 255, 255),
                }],
                40,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        decoded(&png)
            .enumerate_pixels()
            .filter(|(_, _, p)| p.0[3] > 128)
            .map(|(x, _, _)| x)
            .min()
            .expect("something was drawn")
    }

    fn border_pixels(renderer: &Renderer, text: &str) -> Vec<(u32, u32)> {
        let png = renderer
            .render(
                &[Run {
                    text,
                    color: (255, 255, 255),
                }],
                40,
                2,
                CELL,
                HeadingDecor {
                    border: Some((255, 0, 0)),
                    bar: None,
                },
            )
            .unwrap();
        pixels_of(&decoded(&png), [255, 0, 0, 255])
    }

    #[test]
    fn a_border_sits_below_the_text_and_starts_where_the_text_does() {
        let Some(renderer) = renderer() else {
            return;
        };
        // A rule that ran the full width of the row would start out in the
        // margin and read as a scrollbar rather than as part of the heading.
        // Indenting the text must carry the border right with it.
        let flush = border_pixels(&renderer, "Heading");
        let indented = border_pixels(&renderer, "        Heading");
        assert!(!flush.is_empty() && !indented.is_empty(), "no border drawn");

        let left = |v: &[(u32, u32)]| v.iter().map(|(x, _)| *x).min().unwrap();
        assert!(
            left(&indented) > left(&flush),
            "border did not follow the indent: {} vs {}",
            left(&indented),
            left(&flush)
        );
        assert!(
            left(&indented) <= text_left(&renderer, "        Heading"),
            "border starts right of the first letter"
        );

        let image_height = 2 * CELL.h as u32;
        let top = indented.iter().map(|(_, y)| *y).min().unwrap();
        assert!(
            top > image_height / 2,
            "border at row {top} is not below the text"
        );
    }

    #[test]
    fn a_bar_stops_short_of_the_first_letter() {
        let Some(renderer) = renderer() else {
            return;
        };
        let text = "        Heading";
        let png = renderer
            .render(
                &[Run {
                    text,
                    color: (255, 255, 255),
                }],
                40,
                2,
                CELL,
                HeadingDecor {
                    border: None,
                    bar: Some((0, 255, 0)),
                },
            )
            .unwrap();
        let green = pixels_of(&decoded(&png), [0, 255, 0, 255]);
        assert!(!green.is_empty(), "no bar drawn");
        let right = green.iter().map(|(x, _)| *x).max().unwrap();
        assert!(
            right < text_left(&renderer, text),
            "bar reached {right}px and the text starts at {}px",
            text_left(&renderer, text)
        );
    }

    #[test]
    fn a_heading_with_nothing_in_front_of_it_gets_no_bar_rather_than_a_smudged_letter() {
        let Some(renderer) = renderer() else {
            return;
        };
        // margin = 0 leaves nowhere to put the bar. Dropping it is the only
        // option that does not draw over the first glyph.
        let png = renderer
            .render(
                &[Run {
                    text: "Heading",
                    color: (255, 255, 255),
                }],
                40,
                2,
                CELL,
                HeadingDecor {
                    border: None,
                    bar: Some((0, 255, 0)),
                },
            )
            .unwrap();
        let image = decoded(&png);
        assert!(
            !image.pixels().any(|p| p.0 == [0, 255, 0, 255]),
            "a bar was drawn where there was no room for one"
        );
    }

    #[test]
    fn decoration_leaves_the_background_transparent() {
        let Some(renderer) = renderer() else {
            return;
        };
        let png = renderer
            .render(
                &[Run {
                    text: "  Hi",
                    color: (255, 255, 255),
                }],
                40,
                2,
                CELL,
                HeadingDecor {
                    border: Some((255, 0, 0)),
                    bar: Some((0, 255, 0)),
                },
            )
            .unwrap();
        let image = decoded(&png);
        // Well right of four characters at double width, and above the border.
        let corner = image.get_pixel(image.width() - 1, 0);
        assert_eq!(corner.0[3], 0, "decoration filled the whole canvas");
    }

    #[test]
    fn each_run_is_drawn_in_its_own_colour() {
        let Some(renderer) = renderer() else {
            return;
        };
        // `# See `config.toml`` is the case: the code span keeps the colour the
        // theme gives it under DECDHL, and used to lose it under kitty because
        // the whole heading was flattened to the first colour found.
        let png = renderer
            .render(
                &[
                    Run {
                        text: "See ",
                        color: (255, 0, 0),
                    },
                    Run {
                        text: "config",
                        color: (0, 255, 0),
                    },
                ],
                40,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        let image = decoded(&png);
        let red = pixels_of(&image, [255, 0, 0, 255]);
        let green = pixels_of(&image, [0, 255, 0, 255]);
        assert!(!red.is_empty(), "the first run lost its colour");
        assert!(!green.is_empty(), "the second run lost its colour");
        // And they are drawn where their run sits, not interleaved.
        let red_right = red.iter().map(|(x, _)| *x).max().unwrap();
        let green_left = green.iter().map(|(x, _)| *x).min().unwrap();
        assert!(
            red_right < green_left,
            "runs overlap: red ends at {red_right}, green starts at {green_left}"
        );
    }

    #[test]
    fn a_run_wide_enough_to_fill_the_canvas_stops_the_ones_after_it() {
        let Some(renderer) = renderer() else {
            return;
        };
        // Clipping has to end the whole heading, not just the run it happened
        // in, or the tail would be drawn back at the left of the next run.
        let png = renderer
            .render(
                &[
                    Run {
                        text: &"wide ".repeat(50),
                        color: (255, 255, 255),
                    },
                    Run {
                        text: "tail",
                        color: (0, 255, 0),
                    },
                ],
                10,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        assert!(
            pixels_of(&decoded(&png), [0, 255, 0, 255]).is_empty(),
            "a run past the right edge was drawn anyway"
        );
    }

    #[test]
    fn empty_text_renders_nothing() {
        let Some(renderer) = renderer() else {
            return;
        };
        assert!(
            renderer
                .render(
                    &[Run {
                        text: "   ",
                        color: (0, 0, 0)
                    }],
                    10,
                    2,
                    CELL,
                    HeadingDecor::default()
                )
                .is_none()
        );
        assert!(
            renderer
                .render(
                    &[Run {
                        text: "x",
                        color: (0, 0, 0)
                    }],
                    0,
                    2,
                    CELL,
                    HeadingDecor::default()
                )
                .is_none()
        );
    }

    #[test]
    fn text_wider_than_the_canvas_is_clipped_rather_than_overflowing() {
        let Some(renderer) = renderer() else {
            return;
        };
        let png = renderer
            .render(
                &[Run {
                    text: &"wide ".repeat(50),
                    color: (255, 255, 255),
                }],
                10,
                2,
                CELL,
                HeadingDecor::default(),
            )
            .unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.width(), 10 * CELL.w as u32);
    }
}
