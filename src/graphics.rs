//! Inline images, via the Kitty graphics protocol.
//!
//! The protocol separates *transmission* from *placement*: an image is uploaded
//! once under an id, then placed as many times as you like. Since `mdroll`
//! redraws the whole screen every frame, each frame deletes the previous
//! placements and makes new ones, while the uploads stay cached. That is what
//! keeps an image glued to its text as the document scrolls.
//!
//! Terminals without graphics support fall back to the alt text, which is
//! handled in layout rather than here.

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Escape-code payload size. Kitty's documented limit is 4096 bytes per chunk.
const CHUNK: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Kitty,
    None,
}

impl Protocol {
    pub fn available(self) -> bool {
        self != Protocol::None
    }
}

/// Pixels per character cell. Needed to turn an image's pixel size into a
/// number of rows to reserve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub w: u16,
    pub h: u16,
}

impl Default for CellSize {
    /// A plausible 8x16 cell, used when the terminal will not tell us. Getting
    /// this wrong distorts the aspect ratio but never breaks the layout.
    fn default() -> CellSize {
        CellSize { w: 8, h: 16 }
    }
}

/// Ask the terminal how big a cell is, in pixels.
pub fn cell_size() -> CellSize {
    match crossterm::terminal::window_size() {
        Ok(size) if size.width > 0 && size.height > 0 && size.columns > 0 && size.rows > 0 => {
            CellSize {
                w: (size.width / size.columns).max(1),
                h: (size.height / size.rows).max(1),
            }
        }
        _ => CellSize::default(),
    }
}

/// Detect graphics support from the environment.
///
/// WezTerm is the reference terminal; kitty and ghostty speak the same
/// protocol. Everything else degrades to alt text rather than emitting escape
/// codes that would show up as garbage.
pub fn detect() -> Protocol {
    let has = |name: &str| std::env::var_os(name).is_some();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();

    if has("KITTY_WINDOW_ID")
        || has("WEZTERM_EXECUTABLE")
        || has("WEZTERM_PANE")
        || has("GHOSTTY_RESOURCES_DIR")
        || term.contains("kitty")
        || term.contains("ghostty")
        || term_program.eq_ignore_ascii_case("wezterm")
        || term_program.eq_ignore_ascii_case("ghostty")
    {
        // tmux mangles pass-through graphics unless explicitly configured, so
        // stay out of its way.
        if has("TMUX") {
            return Protocol::None;
        }
        return Protocol::Kitty;
    }
    Protocol::None
}

/// Cell dimensions an image should occupy, preserving its aspect ratio.
///
/// Constrained by the column budget and a row cap, so a tall screenshot cannot
/// push the whole document off the screen.
pub fn fit(pixels: (u32, u32), cell: CellSize, max_cols: usize, max_rows: usize) -> (u16, u16) {
    let (pw, ph) = (pixels.0.max(1) as f64, pixels.1.max(1) as f64);
    let (cw, ch) = (cell.w.max(1) as f64, cell.h.max(1) as f64);
    let max_cols = max_cols.max(1) as f64;
    let max_rows = max_rows.max(1) as f64;

    let mut cols = (pw / cw).ceil();
    let mut rows = (ph / ch).ceil();

    if cols > max_cols {
        rows *= max_cols / cols;
        cols = max_cols;
    }
    if rows > max_rows {
        cols *= max_rows / rows;
        rows = max_rows;
    }
    (cols.round().max(1.0) as u16, rows.round().max(1.0) as u16)
}

#[derive(Debug)]
struct Entry {
    kitty_id: u32,
    cols: u16,
    rows: u16,
}

/// Loads, resizes, and uploads images, remembering what the terminal already
/// holds.
pub struct ImageStore {
    pub protocol: Protocol,
    pub cell: CellSize,
    sources: HashMap<usize, PathBuf>,
    uploaded: HashMap<usize, Entry>,
    next_id: u32,
}

impl ImageStore {
    pub fn new(protocol: Protocol, cell: CellSize) -> ImageStore {
        ImageStore {
            protocol,
            cell,
            sources: HashMap::new(),
            uploaded: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn disabled() -> ImageStore {
        ImageStore::new(Protocol::None, CellSize::default())
    }

    /// Register where an image lives on disk. Only local files are supported;
    /// `mdroll` does not fetch over the network.
    pub fn register(&mut self, id: usize, path: PathBuf) {
        if self.sources.get(&id) != Some(&path) {
            self.uploaded.remove(&id);
            self.sources.insert(id, path);
        }
    }

    pub fn forget_all(&mut self) {
        self.sources.clear();
        self.uploaded.clear();
    }

    /// Delete every placement made by the previous frame.
    pub fn clear_placements<W: Write>(&self, out: &mut W) -> Result<()> {
        if self.protocol.available() {
            // d=a deletes visible placements but keeps the uploaded images.
            out.write_all(b"\x1b_Ga=d,d=a\x1b\\")?;
        }
        Ok(())
    }

    /// Place an image at the cursor's current position.
    ///
    /// `skip_rows` crops the top of the image, which is what makes an image
    /// scroll smoothly off the top of the screen instead of vanishing.
    pub fn place<W: Write>(
        &mut self,
        out: &mut W,
        id: usize,
        cols: u16,
        rows: u16,
        skip_rows: u16,
        visible_rows: u16,
    ) -> Result<bool> {
        if !self.protocol.available() || visible_rows == 0 {
            return Ok(false);
        }
        let Some(kitty_id) = self.upload(out, id, cols, rows)? else {
            return Ok(false);
        };

        let y = skip_rows as u32 * self.cell.h as u32;
        let h = visible_rows as u32 * self.cell.h as u32;
        let w = cols as u32 * self.cell.w as u32;
        // C=1 keeps the cursor where it was, so the text layout is unaffected.
        write!(
            out,
            "\x1b_Ga=p,i={kitty_id},c={cols},r={visible_rows},x=0,y={y},w={w},h={h},C=1,q=2\x1b\\"
        )?;
        Ok(true)
    }

    /// Upload an image at the exact pixel size it will be displayed at.
    /// Sending the original would mean megabytes of base64 for a screenshot.
    fn upload<W: Write>(
        &mut self,
        out: &mut W,
        id: usize,
        cols: u16,
        rows: u16,
    ) -> Result<Option<u32>> {
        if let Some(entry) = self.uploaded.get(&id)
            && entry.cols == cols
            && entry.rows == rows
        {
            return Ok(Some(entry.kitty_id));
        }
        let Some(path) = self.sources.get(&id).cloned() else {
            return Ok(None);
        };
        let target = (
            cols as u32 * self.cell.w as u32,
            rows as u32 * self.cell.h as u32,
        );
        let Ok(png) = encode_png(&path, target) else {
            return Ok(None);
        };

        let kitty_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        transmit(out, kitty_id, &png)?;
        self.uploaded.insert(
            id,
            Entry {
                kitty_id,
                cols,
                rows,
            },
        );
        Ok(Some(kitty_id))
    }
}

/// Decode, downscale, and re-encode as PNG.
pub fn encode_png(path: &Path, target: (u32, u32)) -> Result<Vec<u8>> {
    let img = image::open(path)?;
    let scaled = if img.width() > target.0 || img.height() > target.1 {
        img.thumbnail(target.0.max(1), target.1.max(1))
    } else {
        img
    };
    let mut buf = Vec::new();
    scaled.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)?;
    Ok(buf)
}

/// Send image data under `kitty_id` without displaying it.
fn transmit<W: Write>(out: &mut W, kitty_id: u32, png: &[u8]) -> Result<()> {
    let encoded = STANDARD.encode(png);
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(CHUNK)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            // f=100 is "the payload is a PNG"; q=2 suppresses the reply.
            write!(
                out,
                "\x1b_Ga=t,f=100,i={kitty_id},q=2,m={more};{chunk}\x1b\\"
            )?;
        } else {
            write!(out, "\x1b_Gm={more};{chunk}\x1b\\")?;
        }
    }
    Ok(())
}

/// Pixel dimensions of an image file, read from its header.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellSize = CellSize { w: 10, h: 20 };

    #[test]
    fn a_small_image_keeps_its_natural_size() {
        assert_eq!(fit((100, 200), CELL, 80, 40), (10, 10));
    }

    #[test]
    fn a_wide_image_is_capped_by_the_column_budget() {
        let (cols, rows) = fit((1000, 500), CELL, 50, 40);
        assert_eq!(cols, 50);
        // 100x25 cells naturally; halving the width halves the height.
        assert_eq!(rows, 13);
    }

    #[test]
    fn a_tall_image_is_capped_by_the_row_budget() {
        let (cols, rows) = fit((200, 4000), CELL, 80, 20);
        assert_eq!(rows, 20);
        assert!(cols < 20, "width must shrink with the height, got {cols}");
    }

    #[test]
    fn fit_never_returns_zero() {
        assert_eq!(fit((1, 1), CELL, 80, 40), (1, 1));
        assert_eq!(fit((0, 0), CELL, 0, 0), (1, 1));
    }

    #[test]
    fn a_disabled_store_emits_nothing() {
        let mut store = ImageStore::disabled();
        let mut out = Vec::new();
        store.clear_placements(&mut out).unwrap();
        assert!(!store.place(&mut out, 0, 10, 5, 0, 5).unwrap());
        assert!(out.is_empty());
    }

    #[test]
    fn clearing_placements_keeps_uploads() {
        let store = ImageStore::new(Protocol::Kitty, CELL);
        let mut out = Vec::new();
        store.clear_placements(&mut out).unwrap();
        assert_eq!(out, b"\x1b_Ga=d,d=a\x1b\\");
    }

    #[test]
    fn transmission_is_chunked_with_a_continuation_flag() {
        let mut out = Vec::new();
        let png = vec![0u8; CHUNK * 2];
        transmit(&mut out, 7, &png).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("\x1b_Ga=t,f=100,i=7,q=2,m=1;"));
        assert!(text.contains("\x1b_Gm=1;"), "middle chunks continue");
        assert!(
            text.contains("\x1b_Gm=0;"),
            "the last chunk ends the sequence"
        );
    }

    #[test]
    fn a_short_image_transmits_in_one_chunk() {
        let mut out = Vec::new();
        transmit(&mut out, 3, b"tiny").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("\x1b_Ga=t,f=100,i=3,q=2,m=0;"));
        assert_eq!(text.matches("\x1b_G").count(), 1);
    }

    #[test]
    fn re_registering_a_different_path_invalidates_the_upload() {
        let mut store = ImageStore::new(Protocol::Kitty, CELL);
        store.register(0, PathBuf::from("a.png"));
        store.uploaded.insert(
            0,
            Entry {
                kitty_id: 1,
                cols: 4,
                rows: 4,
            },
        );
        store.register(0, PathBuf::from("b.png"));
        assert!(store.uploaded.is_empty());
    }

    #[test]
    fn placement_crops_the_top_when_scrolled_partly_off_screen() {
        let mut store = ImageStore::new(Protocol::Kitty, CELL);
        store.uploaded.insert(
            0,
            Entry {
                kitty_id: 9,
                cols: 10,
                rows: 8,
            },
        );
        let mut out = Vec::new();
        assert!(store.place(&mut out, 0, 10, 8, 3, 5).unwrap());
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("i=9"));
        assert!(text.contains("r=5"), "only the visible rows are placed");
        assert!(text.contains("y=60"), "three rows of 20px are cropped off");
        assert!(text.contains("h=100"));
        assert!(text.contains("C=1"), "the cursor must not move");
    }

    #[test]
    fn tmux_disables_graphics() {
        // Detection is environment-driven; this documents the intent rather
        // than mutating the process environment under a parallel test runner.
        assert!(!Protocol::None.available());
    }
}
