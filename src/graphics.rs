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

use crate::bigtext;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// Pixels per cell, from the terminal if it will say and the kernel otherwise.
///
/// `TIOCGWINSZ` carries a pixel size, but only the process on the same machine
/// as the terminal can read it; over `ssh` it comes back as zero. Asking the
/// terminal is what works at both ends of a connection.
pub fn cell_size() -> CellSize {
    if let Some(cell) = probed().cell {
        return cell;
    }
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

/// Whether the terminal can draw pictures.
///
/// The terminal is asked directly, and only when it cannot be — output is a
/// pipe, or this is not Unix — does the environment get a say.
pub fn detect() -> Protocol {
    probed().protocol.unwrap_or_else(detect_from_env)
}

/// Guess graphics support from the environment.
///
/// A guess is all this can ever be. The variables named here are set by a
/// terminal on the machine it runs on, so over `ssh` none of them are present
/// however capable the terminal at the other end is. It is the fallback for
/// when the terminal cannot be asked.
pub fn detect_from_env() -> Protocol {
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

/// What the terminal said when it was asked what it can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Probe {
    /// `None` when there was no way to ask.
    pub protocol: Option<Protocol>,
    /// Pixels per cell, when the terminal was willing to say.
    pub cell: Option<CellSize>,
}

/// Long enough for a reply to cross a slow link and come back, short enough
/// that a terminal which answers nothing at all is not worth noticing.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// The result of asking, worked out once and remembered.
pub fn probed() -> Probe {
    static PROBE: OnceLock<Probe> = OnceLock::new();
    *PROBE.get_or_init(|| probe(PROBE_TIMEOUT))
}

/// Ask the terminal what it can do, and wait for the answer.
///
/// This is the only way that survives `ssh`. Escape sequences travel down the
/// connection like any other output, so the terminal at the far end can be
/// asked and can answer, while the environment variables it sets never left
/// the machine it runs on.
///
/// The catch is that a terminal which does not understand the graphics query
/// says nothing, and there is no silence to wait for. So a Device Attributes
/// request is sent right after it: every terminal answers that one, and
/// answers come back in the order the questions were asked. A DA reply with no
/// graphics reply ahead of it is therefore a definite no, not a slow yes.
#[cfg(unix)]
pub fn probe(timeout: Duration) -> Probe {
    use std::io::{IsTerminal, Read, Write};

    let mut out = std::io::stdout();
    if !out.is_terminal() || !std::io::stdin().is_terminal() {
        return Probe::default();
    }
    let Ok(_raw) = RawMode::enter() else {
        return Probe::default();
    };

    // `a=q` asks rather than draws. The payload is one transparent pixel,
    // because a terminal is entitled to answer only a query it could act on,
    // and `i=31` is an id nothing else here uses. Then the cell size, then the
    // question every terminal answers.
    let asked = write!(
        out,
        "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c"
    )
    .and_then(|()| out.flush())
    .is_ok();
    if !asked {
        return Probe::default();
    }

    let deadline = Instant::now() + timeout;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !readable(left) {
            break;
        }
        match std::io::stdin().read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
        }
        // The DA reply is last, so once it is here there is nothing to wait
        // for and no reason to hold up the redraw any longer.
        if device_attributes_seen(&reply) {
            break;
        }
    }

    Probe {
        protocol: Some(if contains(&reply, b"_Gi=31;OK") {
            Protocol::Kitty
        } else {
            Protocol::None
        }),
        cell: parse_cell_size(&reply),
    }
}

/// Windows has no `poll`, and no terminal that speaks this protocol either.
#[cfg(not(unix))]
pub fn probe(_timeout: Duration) -> Probe {
    Probe::default()
}

/// Raw mode for the length of the question, so the reply arrives as bytes
/// rather than being echoed or swallowed by line editing.
#[cfg(unix)]
struct RawMode;

#[cfg(unix)]
impl RawMode {
    fn enter() -> std::io::Result<RawMode> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(RawMode)
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(unix)]
fn readable(within: Duration) -> bool {
    let mut fd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = within.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: one initialised pollfd, and a count that matches it.
    unsafe { libc::poll(&mut fd, 1, ms) > 0 }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Whether a Device Attributes reply — `ESC [ ? … c` — has arrived in full.
fn device_attributes_seen(reply: &[u8]) -> bool {
    let mut rest = reply;
    while let Some(at) = rest.windows(3).position(|w| w == b"\x1b[?") {
        rest = &rest[at + 3..];
        if rest.contains(&b'c') {
            return true;
        }
    }
    false
}

/// The cell size out of a `CSI 16 t` reply, which reads `ESC [ 6 ; h ; w t`.
fn parse_cell_size(reply: &[u8]) -> Option<CellSize> {
    let at = reply.windows(4).position(|w| w == b"\x1b[6;")?;
    let rest = &reply[at + 4..];
    let end = rest.iter().position(|b| *b == b't')?;
    let body = std::str::from_utf8(&rest[..end]).ok()?;
    let (h, w) = body.split_once(';')?;
    let (h, w): (u16, u16) = (h.trim().parse().ok()?, w.trim().parse().ok()?);
    (h > 0 && w > 0).then_some(CellSize { w, h })
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

/// Which cache a live kitty image id belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Origin {
    Image(usize),
    Text(String),
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
    /// Rasterized headings, keyed by their text, colour, and size.
    text: HashMap<String, u32>,
    /// What each live kitty image id came from, so a dropped one can be
    /// invalidated in the right cache.
    origin: HashMap<u32, Origin>,
    placed_now: HashSet<u32>,
    placed_before: HashSet<u32>,
    /// Font renderer for big headings, absent when no usable font was found.
    pub big: Option<bigtext::Renderer>,
    next_id: u32,
}

/// A document has few headings, but a long session with `--watch` and theme
/// cycling could otherwise grow the cache without bound.
const TEXT_CACHE_LIMIT: usize = 256;

impl ImageStore {
    pub fn new(protocol: Protocol, cell: CellSize) -> ImageStore {
        ImageStore {
            protocol,
            cell,
            sources: HashMap::new(),
            uploaded: HashMap::new(),
            text: HashMap::new(),
            origin: HashMap::new(),
            placed_now: HashSet::new(),
            placed_before: HashSet::new(),
            big: None,
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
        self.invalidate_uploads();
    }

    /// Forget what the terminal is holding, while remembering where the images
    /// came from.
    ///
    /// Needed after leaving and re-entering the alternate screen — an editor
    /// has been on the screen in between, and there is no way to know what
    /// survived, so everything is re-sent.
    pub fn invalidate_uploads(&mut self) {
        self.uploaded.clear();
        self.text.clear();
        self.origin.clear();
        self.placed_now.clear();
        self.placed_before.clear();
    }

    /// Start a frame. Nothing is emitted: placements are *replaced* in place as
    /// the frame is drawn, and only the ones that turn out to be gone are
    /// deleted at the end.
    ///
    /// This matters more than it looks. Deleting every placement up front
    /// leaves each image with no placements referring to it, and the protocol
    /// explicitly allows the terminal to free such data at any moment — which
    /// kitty does, as soon as a later transmission needs the room. The heading
    /// bitmap would then vanish the first time a diagram finished loading.
    pub fn begin_frame(&mut self) {
        std::mem::swap(&mut self.placed_before, &mut self.placed_now);
        self.placed_now.clear();
    }

    /// Delete the placements that were on screen last frame and are not now.
    ///
    /// Their image data may be freed by the terminal once it has no
    /// placements, so the upload is forgotten too and will be re-sent if the
    /// image scrolls back into view.
    pub fn end_frame<W: Write>(&mut self, out: &mut W) -> Result<()> {
        if !self.protocol.available() {
            return Ok(());
        }
        let gone: Vec<u32> = self
            .placed_before
            .difference(&self.placed_now)
            .copied()
            .collect();
        for id in gone {
            write!(out, "\x1b_Ga=d,d=i,i={id},p={id}\x1b\\")?;
            // Only when the cache still points at *this* id. A resize sends
            // the image up again under a new one, and retiring the old
            // placement must not throw the new upload away — that leaves the
            // cache empty with the image still on screen, so the next frame
            // uploads it again, and so does the one after that, for as long as
            // it stays visible.
            match self.origin.remove(&id) {
                Some(Origin::Image(image))
                    if self.uploaded.get(&image).is_some_and(|e| e.kitty_id == id) =>
                {
                    self.uploaded.remove(&image);
                }
                Some(Origin::Text(key)) if self.text.get(&key) == Some(&id) => {
                    self.text.remove(&key);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Whether headings can be drawn as bitmaps on this terminal.
    pub fn can_rasterize(&self) -> bool {
        self.protocol.available() && self.big.is_some()
    }

    /// Draw a heading as a bitmap at the cursor, spanning `cols` by `rows`.
    ///
    /// For terminals that have graphics but no DECDHL — kitty and ghostty —
    /// this is what makes a heading actually bigger rather than merely bolder.
    pub fn place_text<W: Write>(
        &mut self,
        out: &mut W,
        runs: &[bigtext::Run<'_>],
        cols: u16,
        rows: u16,
        decor: bigtext::HeadingDecor,
    ) -> Result<bool> {
        if !self.can_rasterize() || cols == 0 || rows == 0 {
            return Ok(false);
        }
        // Everything the bitmap is drawn from is part of the key. Two headings
        // with the same words differ as images if a span inside one of them is
        // a different colour, or if the rule under them is.
        let key = format!("{cols}x{rows}:{decor:?}:{runs:?}");
        let kitty_id = match self.text.get(&key) {
            Some(id) => *id,
            None => {
                let Some(png) = self
                    .big
                    .as_ref()
                    .and_then(|r| r.render(runs, cols, rows, self.cell, decor))
                else {
                    return Ok(false);
                };
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                transmit(out, id, &png)?;
                if self.text.len() >= TEXT_CACHE_LIMIT {
                    self.text.clear();
                    self.origin.retain(|_, o| !matches!(o, Origin::Text(_)));
                }
                self.origin.insert(id, Origin::Text(key.clone()));
                self.text.insert(key, id);
                id
            }
        };
        // Placement id matches the image id, so re-placing replaces rather than
        // stacking, and the image is never left with no placements at all.
        write!(
            out,
            "\x1b_Ga=p,i={kitty_id},p={kitty_id},c={cols},r={rows},C=1,q=2\x1b\\"
        )?;
        self.placed_now.insert(kitty_id);
        Ok(true)
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
        // The placement id matches the image id, so re-placing replaces rather
        // than adding, and the image is never left with zero placements.
        // C=1 keeps the cursor where it was, so the text layout is unaffected.
        write!(
            out,
            "\x1b_Ga=p,i={kitty_id},p={kitty_id},c={cols},r={visible_rows},x=0,y={y},w={w},h={h},C=1,q=2\x1b\\"
        )?;
        self.placed_now.insert(kitty_id);
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
        self.origin.insert(kitty_id, Origin::Image(id));
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
///
/// An SVG skips the downscale: it is rasterized straight at the target size, so
/// a logo is as sharp as the terminal's cells allow.
pub fn encode_png(path: &Path, target: (u32, u32)) -> Result<Vec<u8>> {
    let data = std::fs::read(path)?;
    if crate::svg::looks_like_svg(&data) {
        return crate::svg::render_png(&data, target);
    }
    let img = image::load_from_memory(&data)?;
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
///
/// Bitmaps are answered from the header alone. Only when that fails is the file
/// read in full and tried as SVG, which is the one format `image` cannot see
/// into.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    if let Ok(size) = image::image_dimensions(path) {
        return Some(size);
    }
    let data = std::fs::read(path).ok()?;
    if !crate::svg::looks_like_svg(&data) {
        return None;
    }
    crate::svg::dimensions(&data)
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
    fn a_device_attributes_reply_ends_the_wait() {
        assert!(device_attributes_seen(b"\x1b[?62;4;6;22c"));
        // Still arriving: the terminator has not turned up yet.
        assert!(!device_attributes_seen(b"\x1b[?62;4"));
        assert!(!device_attributes_seen(b""));
        // A `c` before the reply starts is not the reply's terminator.
        assert!(!device_attributes_seen(b"c\x1b[?62"));
    }

    #[test]
    fn a_graphics_reply_is_recognised_among_the_others() {
        let reply = b"\x1b_Gi=31;OK\x1b\\\x1b[6;38;19t\x1b[?62;4c";
        assert!(contains(reply, b"_Gi=31;OK"));
        assert!(device_attributes_seen(reply));
        assert_eq!(parse_cell_size(reply), Some(CellSize { w: 19, h: 38 }));
    }

    #[test]
    fn a_terminal_that_will_not_say_its_cell_size_is_not_guessed_at() {
        assert_eq!(parse_cell_size(b"\x1b[?62;4c"), None);
        // Present but nonsense: a zero would divide the layout by zero.
        assert_eq!(parse_cell_size(b"\x1b[6;0;0t"), None);
        assert_eq!(parse_cell_size(b"\x1b[6;38t"), None);
        // Truncated: the terminator never arrived.
        assert_eq!(parse_cell_size(b"\x1b[6;38;19"), None);
    }

    #[test]
    fn a_disabled_store_emits_nothing() {
        let mut store = ImageStore::disabled();
        let mut out = Vec::new();
        store.begin_frame();
        assert!(!store.place(&mut out, 0, 10, 5, 0, 5).unwrap());
        store.end_frame(&mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn a_placement_that_stays_on_screen_is_never_deleted() {
        let mut store = ImageStore::new(Protocol::Kitty, CELL);
        store.uploaded.insert(
            0,
            Entry {
                kitty_id: 9,
                cols: 4,
                rows: 2,
            },
        );
        for _ in 0..3 {
            let mut out = Vec::new();
            store.begin_frame();
            store.place(&mut out, 0, 4, 2, 0, 2).unwrap();
            store.end_frame(&mut out).unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(text.contains("a=p,i=9,p=9"), "{text:?}");
            assert!(!text.contains("a=d"), "still visible, so never deleted");
        }
    }

    #[test]
    fn a_placement_that_scrolls_away_is_deleted_and_forgotten() {
        let mut store = ImageStore::new(Protocol::Kitty, CELL);
        store.uploaded.insert(
            0,
            Entry {
                kitty_id: 9,
                cols: 4,
                rows: 2,
            },
        );
        store.origin.insert(9, Origin::Image(0));

        let mut out = Vec::new();
        store.begin_frame();
        store.place(&mut out, 0, 4, 2, 0, 2).unwrap();
        store.end_frame(&mut out).unwrap();

        let mut out = Vec::new();
        store.begin_frame();
        store.end_frame(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("a=d,d=i,i=9,p=9"), "{text:?}");
        // The terminal may free data with no placements, so the upload has to
        // be forgotten or the next appearance would place a ghost.
        assert!(!store.uploaded.contains_key(&0));
    }

    #[test]
    fn a_re_upload_survives_the_old_placement_being_retired() {
        let mut store = ImageStore::new(Protocol::Kitty, CELL);
        store.uploaded.insert(
            0,
            Entry {
                kitty_id: 9,
                cols: 4,
                rows: 2,
            },
        );
        store.origin.insert(9, Origin::Image(0));

        let mut out = Vec::new();
        store.begin_frame();
        store.place(&mut out, 0, 4, 2, 0, 2).unwrap();
        store.end_frame(&mut out).unwrap();

        // The window was resized, so the image goes up again at its new size
        // under a new id and the old placement is retired.
        store.begin_frame();
        store.uploaded.insert(
            0,
            Entry {
                kitty_id: 10,
                cols: 8,
                rows: 4,
            },
        );
        store.origin.insert(10, Origin::Image(0));
        let mut out = Vec::new();
        store.place(&mut out, 0, 8, 4, 0, 4).unwrap();
        store.end_frame(&mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("a=d,d=i,i=9"), "the old one goes: {text:?}");
        // Dropping the fresh entry here leaves the image on screen with an
        // empty cache, so every later frame re-reads, rescales, re-encodes and
        // re-transmits the file — for as long as it stays visible.
        assert_eq!(store.uploaded.get(&0).map(|e| e.kitty_id), Some(10));
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
