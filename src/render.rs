//! Drawing a frame.
//!
//! Rendering is a full redraw every frame. A viewer updates rarely, and
//! differential updates are the usual source of "the status line vanishes
//! sometimes" bugs.
//!
//! The order is fixed:
//!
//! ```text
//! draw every content row, padded → MoveTo(0, status_row) → draw status
//! ```
//!
//! Because the bottom row is written last at an absolute position, a content
//! region that miscounts by a row cannot hide it.
//!
//! Note what is *not* here: an erase-display up front. `ESC [ 2 J` deletes
//! graphics along with the text in kitty, taking every already-transmitted
//! image with it, so a heading bitmap would vanish on the second frame. Every
//! row is padded out to the full width instead, which erases exactly as well
//! and leaves the images alone.

use crate::bigtext::{HeadingDecor, Run as HeadingRun};
use crate::graphics::ImageStore;
use crate::ir::{Color, Hit, HitTarget, Line, Link, Rect, Scale, Span, Style};
use crate::screen::Screen;
use crate::theme::Theme;
use crate::width::WidthCalc;
use crate::wrap::slice_spans_columns;
use anyhow::Result;
use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor::MoveTo, queue};
use std::collections::HashSet;
use std::io::Write;

/// Turn autowrap off and on. Writing into the bottom-right cell with DECAWM
/// enabled scrolls the screen, which would push the content up by a row.
const DECAWM_OFF: &str = "\x1b[?7l";
const DECAWM_ON: &str = "\x1b[?7h";
/// DECDHL: top and bottom halves of a double-height line.
const DECDHL_TOP: &str = "\x1b#3";
const DECDHL_BOTTOM: &str = "\x1b#4";
/// DECSWL: restore a line to single width and height.
const DECSWL: &str = "\x1b#5";

/// A styled run to highlight inside a rendered row, in display columns.
#[derive(Debug, Clone, Copy)]
pub struct Highlight {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// Text painted over a row, replacing whatever is underneath. Used by the link
/// picker to stamp a label onto each link.
#[derive(Debug, Clone)]
pub struct Overlay {
    pub line: usize,
    pub col: usize,
    pub text: String,
    pub style: Style,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Decor<'a> {
    /// Highlight every row belonging to this block.
    pub cursor_block: Option<usize>,
    /// Highlight this inclusive range of layout rows.
    pub selection: Option<(usize, usize)>,
    pub cursor_style: Style,
    pub highlights: &'a [Highlight],
    pub overlays: &'a [Overlay],
}

pub struct Frame<'a> {
    pub screen: Screen,
    pub lines: &'a [Line],
    /// Index of the first layout row to draw.
    pub scroll: usize,
    /// Horizontal scroll, in display columns.
    pub hoffset: usize,
    pub links: &'a [Link],
    /// Contents of the bottom row.
    pub bottom: &'a [Span],
    /// Consulted for heading decoration, which is chosen per heading level and
    /// so cannot be baked into the line's spans the way a colour is.
    pub theme: &'a Theme,
    pub decor: Decor<'a>,
    /// Whether DECDHL line attributes may be written at all. Terminals that do
    /// not implement them print `ESC # 3` as a parse error and then draw the
    /// line twice, so the sequences are withheld entirely rather than sent
    /// hopefully.
    pub double_height: bool,
    /// Draw double-height rows as bitmaps instead. For terminals with graphics
    /// but no DECDHL, which is exactly kitty and ghostty.
    pub raster_headings: bool,
}

/// Where each visible link ended up on screen, for mouse hit testing.
#[derive(Debug, Clone, Default)]
pub struct Placement {
    pub hits: Vec<Hit>,
}

impl Placement {
    pub fn target_at(&self, x: u16, y: u16) -> Option<HitTarget> {
        self.hits
            .iter()
            .find(|h| h.rect.contains(x, y))
            .map(|h| h.target)
    }
}

pub struct Renderer {
    pub calc: WidthCalc,
    pub no_color: bool,
    pub truecolor: bool,
    pub hyperlinks: bool,
    pub double_height: bool,
}

impl Renderer {
    pub fn new(calc: WidthCalc) -> Renderer {
        Renderer {
            calc,
            no_color: false,
            truecolor: detect_truecolor(),
            hyperlinks: true,
            double_height: false,
        }
    }

    pub fn draw<W: Write>(
        &self,
        out: &mut W,
        frame: &Frame<'_>,
        images: &mut ImageStore,
    ) -> Result<Placement> {
        let view = frame.screen.viewport();
        let mut placement = Placement::default();

        self.reset(out)?;
        // Autowrap stays off for the whole frame. Every row is padded to the
        // full width, and with autowrap on, writing the last cell of a row
        // leaves the terminal in a pending-wrap state.
        out.write_all(DECAWM_OFF.as_bytes())?;
        images.begin_frame();

        let mut placed: HashSet<usize> = HashSet::new();
        let mut y = 0u16;
        let mut idx = frame.scroll;
        while y < view.rows && idx < frame.lines.len() {
            let line = &frame.lines[idx];
            let rows = if line.scale == Scale::DoubleHeight {
                2
            } else {
                1
            };
            if y + rows > view.rows {
                break;
            }
            if frame.raster_headings && line.scale == Scale::DoubleHeight {
                self.draw_big_line(out, frame, line, y, images)?;
            } else {
                self.draw_line(out, frame, idx, line, y, &mut placement)?;
            }
            self.place_images(out, frame, line, y, images, &mut placed)?;
            y += rows;
            idx += 1;
        }

        // Rows the document does not reach still have last frame's text on
        // them, so they are blanked rather than left alone.
        while y < view.rows {
            self.blank_row(out, frame, y)?;
            y += 1;
        }

        // The status row is written last, at an absolute position, so it
        // survives any miscount above.
        queue!(out, MoveTo(0, frame.screen.status_row()))?;
        if frame.double_height {
            out.write_all(DECSWL.as_bytes())?;
        }
        let bottom = slice_spans_columns(frame.bottom, 0, frame.screen.cols as usize, &self.calc);
        self.write_spans(out, &bottom, None)?;
        let used: usize = bottom.iter().map(|s| self.calc.str(&s.text)).sum();
        // The status line extends its own background across the row; an empty
        // bottom row is simply blanked.
        match bottom.last().map(|s| s.style) {
            Some(style) => self.set_style(out, style)?,
            None => self.reset(out)?,
        }
        self.pad(out, used, frame.screen.cols as usize)?;
        self.reset(out)?;
        out.write_all(DECAWM_ON.as_bytes())?;
        // Placements that did not come back this frame are retired last, so an
        // image that is still on screen never spends a moment unreferenced.
        images.end_frame(out)?;
        out.flush()?;
        Ok(placement)
    }

    /// Write spaces out to `width` columns.
    fn pad<W: Write>(&self, out: &mut W, used: usize, width: usize) -> Result<()> {
        let pad = width.saturating_sub(used);
        if pad > 0 {
            out.write_all(" ".repeat(pad).as_bytes())?;
        }
        Ok(())
    }

    /// Blank one row of the viewport.
    fn blank_row<W: Write>(&self, out: &mut W, frame: &Frame<'_>, y: u16) -> Result<()> {
        queue!(out, MoveTo(0, y))?;
        if frame.double_height {
            // A row that used to be double-height keeps the attribute until it
            // is told otherwise.
            out.write_all(DECSWL.as_bytes())?;
        }
        self.reset(out)?;
        self.pad(out, 0, frame.screen.cols as usize)
    }

    /// Emit an SGR reset, unless colour is off entirely.
    fn reset<W: Write>(&self, out: &mut W) -> Result<()> {
        if !self.no_color {
            queue!(out, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    /// Write one laid-out row plus a newline, with no cursor positioning.
    ///
    /// Used when stdout is a pipe rather than a terminal, where the document is
    /// printed in full instead of being paged.
    pub fn write_line<W: Write>(&self, out: &mut W, line: &Line) -> Result<()> {
        self.write_spans(out, &line.spans, None)?;
        self.reset(out)?;
        out.write_all(b"\n")?;
        Ok(())
    }

    /// Draw a heading as a bitmap over the two rows the layout reserved.
    ///
    /// Falls back to writing the text normally if rasterizing declines, so a
    /// missing font or an unrenderable string still leaves a readable heading.
    fn draw_big_line<W: Write>(
        &self,
        out: &mut W,
        frame: &Frame<'_>,
        line: &Line,
        y: u16,
        images: &mut ImageStore,
    ) -> Result<()> {
        let text = line.text();
        let trimmed = text.trim_end();
        // The layout halved the column budget for this row, so the bitmap gets
        // twice the columns the text would occupy at normal size.
        let cols = ((self.calc.str(trimmed) * 2) as u16).min(frame.screen.cols);
        // The heading's own colour, which spans without one of their own take,
        // and which the decoration is derived from.
        let color = line
            .spans
            .iter()
            .find_map(|s| s.style.fg)
            .and_then(rgb_of)
            .unwrap_or((200, 200, 200));
        let runs = heading_runs(line, trimmed.len(), color);

        let decor = heading_decor(frame.theme, line.heading, color);

        for row in 0..2 {
            self.blank_row(out, frame, y + row)?;
        }
        queue!(out, MoveTo(0, y))?;
        if images.place_text(out, &runs, cols, 2, decor)? {
            return Ok(());
        }
        let mut placement = Placement::default();
        self.draw_line(out, frame, 0, line, y, &mut placement)
    }

    /// Put any image starting on, or continuing through, this row on screen.
    ///
    /// A hit's `rect.y` is the row's index within the image, so an image whose
    /// top has already scrolled past is placed cropped rather than dropped.
    fn place_images<W: Write>(
        &self,
        out: &mut W,
        frame: &Frame<'_>,
        line: &Line,
        y: u16,
        images: &mut ImageStore,
        placed: &mut HashSet<usize>,
    ) -> Result<()> {
        let rows = frame.screen.viewport().rows;
        // Horizontal scrolling would need the image cropped on the left too;
        // until then, no-wrap mode falls back to the reserved blank cells.
        if frame.hoffset > 0 {
            return Ok(());
        }
        for hit in &line.hits {
            let HitTarget::Image(id) = hit.target else {
                continue;
            };
            if !placed.insert(id.0) {
                continue;
            }
            let visible = (hit.rect.h - hit.rect.y).min(rows.saturating_sub(y));
            queue!(out, MoveTo(hit.rect.x, y))?;
            images.place(out, id.0, hit.rect.w, hit.rect.h, hit.rect.y, visible)?;
        }
        Ok(())
    }

    fn draw_line<W: Write>(
        &self,
        out: &mut W,
        frame: &Frame<'_>,
        idx: usize,
        line: &Line,
        y: u16,
        placement: &mut Placement,
    ) -> Result<()> {
        let double = line.scale == Scale::DoubleHeight;
        // A double-height row shows half as many characters per column, and the
        // horizontal offset has to be converted the same way: it is counted in
        // display columns, and each of these characters occupies two of them.
        // Slicing by the raw offset scrolls the heading twice as fast as the
        // body under it.
        let (cols, hoffset) = if double {
            ((frame.screen.cols / 2) as usize, frame.hoffset / 2)
        } else {
            (frame.screen.cols as usize, frame.hoffset)
        };

        let decorated = self.decorate(frame, idx, line);
        let visible = slice_spans_columns(&decorated, hoffset, cols, &self.calc);

        for (i, prefix) in [DECDHL_TOP, DECDHL_BOTTOM].iter().enumerate() {
            if i == 1 && !double {
                break;
            }
            queue!(out, MoveTo(0, y + i as u16))?;
            if frame.double_height {
                // DECSWL undoes a previous DECDHL on this row; the terminal
                // keeps line attributes across a clear.
                out.write_all(if double { prefix } else { DECSWL }.as_bytes())?;
            }
            self.write_spans(out, &visible, Some(frame.links))?;
            self.reset(out)?;
            // Pad to the full width instead of clearing the screen up front.
            // ESC[2J deletes graphics along with the text in kitty, which took
            // every already-transmitted image with it.
            let used: usize = visible.iter().map(|s| self.calc.str(&s.text)).sum();
            self.pad(out, used, cols)?;
        }

        for hit in &line.hits {
            let x = (hit.rect.x as usize).saturating_sub(frame.hoffset);
            if hit.rect.x as usize + hit.rect.w as usize <= frame.hoffset {
                continue;
            }
            placement.hits.push(Hit {
                rect: Rect {
                    x: x as u16,
                    y,
                    w: hit.rect.w,
                    h: hit.rect.h,
                },
                target: hit.target,
            });
        }
        Ok(())
    }

    /// Apply the block cursor, selection, and search highlights.
    fn decorate(&self, frame: &Frame<'_>, idx: usize, line: &Line) -> Vec<Span> {
        let in_cursor = frame.decor.cursor_block == Some(line.block);
        let in_selection = frame
            .decor
            .selection
            .is_some_and(|(a, b)| idx >= a.min(b) && idx <= a.max(b));

        let mut spans = line.spans.clone();
        if in_cursor || in_selection {
            for span in &mut spans {
                span.style = span.style.patch(frame.decor.cursor_style);
            }
        }
        for hl in frame.decor.highlights.iter().filter(|h| h.line == idx) {
            spans = apply_range(&spans, hl.start, hl.end, hl.style, &self.calc);
        }
        for overlay in frame.decor.overlays.iter().filter(|o| o.line == idx) {
            spans = stamp(&spans, overlay, &self.calc);
        }
        spans
    }

    fn write_spans<W: Write>(
        &self,
        out: &mut W,
        spans: &[Span],
        links: Option<&[Link]>,
    ) -> Result<()> {
        let mut open_link = false;
        for span in spans {
            if let (Some(links), true) = (links, self.hyperlinks) {
                let url = span.link.and_then(|id| links.get(id.0)).map(|l| &l.url);
                match (url, open_link) {
                    (Some(url), _) => {
                        if open_link {
                            out.write_all(b"\x1b]8;;\x1b\\")?;
                        }
                        // OSC 8 hyperlinks let the terminal handle clicks, so
                        // mdroll never has to capture the mouse and native text
                        // selection keeps working.
                        write!(out, "\x1b]8;;{url}\x1b\\")?;
                        open_link = true;
                    }
                    (None, true) => {
                        out.write_all(b"\x1b]8;;\x1b\\")?;
                        open_link = false;
                    }
                    (None, false) => {}
                }
            }
            self.set_style(out, span.style)?;
            out.write_all(span.text.as_bytes())?;
        }
        if open_link {
            out.write_all(b"\x1b]8;;\x1b\\")?;
        }
        Ok(())
    }

    fn set_style<W: Write>(&self, out: &mut W, style: Style) -> Result<()> {
        if self.no_color {
            return Ok(());
        }
        // Reset first: SetAttribute(Reset) clears colors too, so colors have to
        // be written after the attributes.
        queue!(out, SetAttribute(Attribute::Reset))?;
        if style.bold {
            queue!(out, SetAttribute(Attribute::Bold))?;
        }
        if style.dim {
            queue!(out, SetAttribute(Attribute::Dim))?;
        }
        if style.italic {
            queue!(out, SetAttribute(Attribute::Italic))?;
        }
        if style.underline {
            queue!(out, SetAttribute(Attribute::Underlined))?;
        }
        if style.strikethrough {
            queue!(out, SetAttribute(Attribute::CrossedOut))?;
        }
        if style.reverse {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        }
        if let Some(fg) = style.fg {
            queue!(out, SetForegroundColor(self.adapt(fg)))?;
        }
        if let Some(bg) = style.bg {
            queue!(out, SetBackgroundColor(self.adapt(bg)))?;
        }
        Ok(())
    }

    /// Truecolor is assumed; 256-color terminals get a nearest-color downgrade.
    fn adapt(&self, color: Color) -> Color {
        match color {
            Color::Rgb { r, g, b } if !self.truecolor => Color::AnsiValue(rgb_to_ansi256(r, g, b)),
            other => other,
        }
    }
}

/// Restyle the display-column range `[start, end)` of a row.
fn apply_range(
    spans: &[Span],
    start: usize,
    end: usize,
    style: Style,
    calc: &WidthCalc,
) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut col = 0usize;
    for span in spans {
        let mut current: Option<(bool, String)> = None;
        for c in span.text.chars() {
            let w = calc.ch(c);
            let inside = col >= start && col < end;
            match &mut current {
                Some((flag, text)) if *flag == inside => text.push(c),
                Some(_) => {
                    let (flag, text) = current.take().unwrap();
                    push_run(&mut out, span, text, flag, style);
                    current = Some((inside, c.to_string()));
                }
                None => current = Some((inside, c.to_string())),
            }
            col += w;
        }
        if let Some((flag, text)) = current {
            push_run(&mut out, span, text, flag, style);
        }
    }
    out
}

/// Paint `overlay.text` over the row, starting at a display column.
fn stamp(spans: &[Span], overlay: &Overlay, calc: &WidthCalc) -> Vec<Span> {
    let width = calc.str(&overlay.text);
    let before = slice_spans_columns(spans, 0, overlay.col, calc);
    let used: usize = before.iter().map(|s| calc.str(&s.text)).sum();
    let mut out = before;
    if used < overlay.col {
        out.push(Span::plain(" ".repeat(overlay.col - used)));
    }
    out.push(Span::new(overlay.text.clone(), overlay.style));

    let total: usize = spans.iter().map(|s| calc.str(&s.text)).sum();
    let tail_start = overlay.col + width;
    if tail_start < total {
        out.extend(slice_spans_columns(
            spans,
            tail_start,
            total - tail_start,
            calc,
        ));
    }
    out
}

fn push_run(out: &mut Vec<Span>, span: &Span, text: String, highlighted: bool, style: Style) {
    if text.is_empty() {
        return;
    }
    out.push(Span {
        text,
        style: if highlighted {
            span.style.patch(style)
        } else {
            span.style
        },
        link: span.link,
    });
}

/// Whether the terminal implements DECDHL double-height lines.
///
/// This one cannot be probed cheaply and cannot be guessed from `TERM` alone:
/// kitty advertises itself as a modern terminal and still renders `ESC # 3` as
/// a parse error, which would print every heading twice. The list is therefore
/// an allowlist of terminals known to implement it, and everything else falls
/// back to colour and weight.
pub fn detect_double_height() -> bool {
    // tmux and screen rewrite line attributes and usually mangle them.
    if std::env::var_os("TMUX").is_some() {
        return false;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    if term.starts_with("screen") || term.contains("kitty") {
        return false;
    }
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    std::env::var_os("WEZTERM_PANE").is_some()
        || std::env::var_os("WEZTERM_EXECUTABLE").is_some()
        || program.eq_ignore_ascii_case("wezterm")
        || term.starts_with("xterm")
        || term.starts_with("foot")
}

/// The RGB a colour would be drawn as, for rasterizing. Palette entries have
/// no single true answer, so only explicit RGB is rasterized in colour.
fn rgb_of(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb { r, g, b } => Some((r, g, b)),
        _ => None,
    }
}

/// Dim a color towards black, for a decoration that has none of its own.
///
/// Blending towards the *background* would read better and cannot be done here:
/// the `terminal` theme deliberately has no background color, so there is
/// nothing to blend with. Scaling the channels needs nothing but the color
/// itself, which is why a derived decoration shows up under every theme rather
/// than only under the ones that paint their own background.
fn dimmed(color: (u8, u8, u8)) -> (u8, u8, u8) {
    let scale = |c: u8| ((c as u16 * 55) / 100) as u8;
    (scale(color.0), scale(color.1), scale(color.2))
}

/// A heading's spans as coloured runs, cut off at `len` bytes.
///
/// `len` is the length of the row with its trailing blank removed. Trimming has
/// to happen here rather than on the joined string, because the spans are what
/// is drawn now and a run of trailing spaces would otherwise push the rule that
/// follows the text out past the last letter.
///
/// A span with no colour of its own takes the heading's, so a heading that
/// names no colour at all comes out in one colour as before.
fn heading_runs(line: &Line, len: usize, color: (u8, u8, u8)) -> Vec<HeadingRun<'_>> {
    let mut runs = Vec::new();
    let mut at = 0usize;
    for span in &line.spans {
        if at >= len {
            break;
        }
        let end = (at + span.text.len()).min(len);
        let text = &span.text[..end - at];
        at += span.text.len();
        if text.is_empty() {
            continue;
        }
        runs.push(HeadingRun {
            text,
            color: span.style.fg.and_then(rgb_of).unwrap_or(color),
            // Unlike the foreground there is nothing to fall back to: a
            // heading with no background of its own is drawn on whatever the
            // terminal is showing, and only a span that asks for one gets a
            // block.
            bg: span.style.bg.and_then(rgb_of),
        });
    }
    runs
}

/// What to draw around a heading of this level, in the color it resolves to.
///
/// A row that is not a heading gets nothing. A level the theme says nothing
/// about gets its default, which is a border on the two levels drawn large and
/// no bar anywhere.
fn heading_decor(theme: &Theme, level: Option<u8>, color: (u8, u8, u8)) -> HeadingDecor {
    let Some(level) = level else {
        return HeadingDecor::default();
    };
    let i = (level.clamp(1, 6) - 1) as usize;
    let resolve = |d: Option<crate::theme::Decoration>| {
        d.map(|d| d.color.and_then(rgb_of).unwrap_or_else(|| dimmed(color)))
    };
    HeadingDecor {
        border: resolve(theme.heading_border[i]),
        bar: resolve(theme.heading_bar[i]),
    }
}

pub fn detect_truecolor() -> bool {
    match std::env::var("COLORTERM") {
        Ok(v) => {
            let v = v.to_ascii_lowercase();
            v.contains("truecolor") || v.contains("24bit")
        }
        Err(_) => false,
    }
}

/// Nearest xterm-256 palette entry for an RGB color.
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    // The gray ramp is denser than the cube for near-neutral colors.
    if r.abs_diff(g) < 8 && g.abs_diff(b) < 8 && r.abs_diff(b) < 8 {
        let level = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        if level < 8 {
            return 16;
        }
        if level > 248 {
            return 231;
        }
        return 232 + ((level as u16 - 8) * 24 / 240) as u8;
    }
    let q = |v: u8| -> u16 {
        const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let mut best = 0u16;
        let mut best_delta = u16::MAX;
        for (i, s) in STEPS.iter().enumerate() {
            let delta = v.abs_diff(*s) as u16;
            if delta < best_delta {
                best_delta = delta;
                best = i as u16;
            }
        }
        best
    };
    (16 + 36 * q(r) + 6 * q(g) + q(b)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::LinkId;

    #[test]
    fn a_heading_keeps_the_colour_of_each_span_inside_it() {
        // `# See `config.toml``: the code span has a colour of its own and the
        // words around it do not. Flattening the row to the first colour found
        // lost the code span on exactly the terminals that get a bitmap.
        let heading = Color::Rgb {
            r: 189,
            g: 147,
            b: 249,
        };
        let code = Color::Rgb {
            r: 255,
            g: 121,
            b: 198,
        };
        let mut line = Line::new(
            1,
            0,
            vec![
                Span::new("See ", Style::fg(heading)),
                Span::new("config.toml", Style::fg(code)),
            ],
        );
        line.heading = Some(1);
        let runs = heading_runs(&line, line.text().trim_end().len(), (189, 147, 249));
        assert_eq!(runs.len(), 2, "{runs:?}");
        assert_eq!(runs[0].color, (189, 147, 249));
        assert_eq!(runs[1].color, (255, 121, 198));
    }

    #[test]
    fn a_span_with_no_colour_of_its_own_takes_the_headings() {
        let mut line = Line::new(
            1,
            0,
            vec![
                Span::plain("  "),
                Span::new("Title", Style::fg(Color::Rgb { r: 1, g: 2, b: 3 })),
            ],
        );
        line.heading = Some(2);
        let runs = heading_runs(&line, line.text().trim_end().len(), (9, 9, 9));
        assert_eq!(runs[0].color, (9, 9, 9), "the margin took a colour");
        assert_eq!(runs[1].color, (1, 2, 3));
    }

    #[test]
    fn a_span_carries_its_background_into_the_bitmap_and_the_others_get_none() {
        // A code span in a heading has one — `code = { bg = "#44475a" }` in
        // Dracula — and under DECDHL the terminal paints it. There is nothing
        // to fall back to for the spans around it: a heading has no background
        // of its own, so they are drawn on whatever the terminal is showing.
        let code = Style {
            fg: Some(Color::Rgb {
                r: 255,
                g: 121,
                b: 198,
            }),
            bg: Some(Color::Rgb {
                r: 68,
                g: 71,
                b: 90,
            }),
            ..Style::PLAIN
        };
        let mut line = Line::new(
            1,
            0,
            vec![Span::plain("See "), Span::new("config.toml", code)],
        );
        line.heading = Some(1);
        let runs = heading_runs(&line, line.text().trim_end().len(), (9, 9, 9));
        assert_eq!(runs[0].bg, None, "a plain span was given a background");
        assert_eq!(runs[1].bg, Some((68, 71, 90)));
    }

    #[test]
    fn a_palette_background_is_left_off_the_bitmap_the_way_a_palette_colour_is() {
        // A palette entry has no single true RGB — what 4 looks like is the
        // terminal's business — so rasterizing it would be a guess, and a
        // guessed block behind a code span is more wrong than none.
        let mut line = Line::new(
            1,
            0,
            vec![Span::new(
                "config.toml",
                Style {
                    bg: Some(Color::AnsiValue(4)),
                    ..Style::PLAIN
                },
            )],
        );
        line.heading = Some(1);
        let runs = heading_runs(&line, line.text().trim_end().len(), (9, 9, 9));
        assert_eq!(runs[0].bg, None);
    }

    #[test]
    fn trailing_blank_is_cut_from_the_runs_rather_than_the_joined_string() {
        // The rule under a heading is drawn to where the glyphs end. A run of
        // trailing spaces the layout padded with would push it out past the
        // last letter, so the trim has to reach the spans themselves.
        let mut line = Line::new(1, 0, vec![Span::plain("Title"), Span::plain("      ")]);
        line.heading = Some(1);
        let runs = heading_runs(&line, line.text().trim_end().len(), (9, 9, 9));
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].text, "Title");
    }

    fn strip_ansi(bytes: &[u8]) -> String {
        let text = String::from_utf8_lossy(bytes);
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\x1b' {
                out.push(c);
                continue;
            }
            match chars.peek() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC, terminated by ST.
                    let mut prev = '\0';
                    for c in chars.by_ref() {
                        if c == '\x07' || (prev == '\x1b' && c == '\\') {
                            break;
                        }
                        prev = c;
                    }
                }
                Some('#') => {
                    chars.next();
                    chars.next();
                }
                _ => {
                    chars.next();
                }
            }
        }
        out
    }

    fn frame_bytes(frame: &Frame<'_>) -> Vec<u8> {
        let renderer = Renderer::new(WidthCalc::default());
        let mut buf = Vec::new();
        renderer
            .draw(&mut buf, frame, &mut ImageStore::disabled())
            .unwrap();
        buf
    }

    static TEST_THEME: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(Theme::default);

    fn basic_frame<'a>(lines: &'a [Line], bottom: &'a [Span]) -> Frame<'a> {
        Frame {
            screen: Screen::new(20, 4),
            lines,
            scroll: 0,
            hoffset: 0,
            links: &[],
            bottom,
            theme: &TEST_THEME,
            decor: Decor::default(),
            double_height: true,
            raster_headings: false,
        }
    }

    #[test]
    fn content_and_status_both_reach_the_output() {
        let lines = vec![
            Line::new(1, 0, vec![Span::plain("first")]),
            Line::new(2, 0, vec![Span::plain("second")]),
        ];
        let bottom = vec![Span::plain("status")];
        let text = strip_ansi(&frame_bytes(&basic_frame(&lines, &bottom)));
        assert!(text.contains("first"));
        assert!(text.contains("second"));
        assert!(text.contains("status"));
    }

    #[test]
    fn autowrap_is_disabled_around_the_status_row() {
        let bottom = vec![Span::plain("status")];
        let raw = frame_bytes(&basic_frame(&[], &bottom));
        let text = String::from_utf8_lossy(&raw);
        let off = text.find(DECAWM_OFF).expect("DECAWM disabled");
        let on = text.find(DECAWM_ON).expect("DECAWM restored");
        assert!(off < on, "autowrap must be restored after the status row");
    }

    #[test]
    fn a_double_height_row_scrolls_at_the_same_speed_as_the_rest() {
        // A DECDHL cell is two display columns wide, so a row of them shows
        // half as many characters — and an offset counted in columns has to be
        // halved before it indexes them, or the heading slides twice as fast as
        // the body under it.
        let mut heading = Line::new(1, 0, vec![Span::plain("ABCDEFGHIJ")]);
        heading.scale = Scale::DoubleHeight;
        let body = Line::new(2, 1, vec![Span::plain("0123456789")]);
        let lines = vec![heading, body];

        let mut frame = basic_frame(&lines, &[]);
        frame.hoffset = 4;
        let text = strip_ansi(&frame_bytes(&frame));

        // Four columns of body text is four characters.
        assert!(text.contains("456789"), "body scrolled four columns");
        // Four columns of double-width text is *two* characters.
        assert_eq!(
            text.matches("CDEFGHIJ").count(),
            2,
            "both halves of the heading, each scrolled four columns and not eight"
        );
    }

    #[test]
    fn content_never_spills_onto_the_status_row() {
        // Four screen rows means three viewport rows; ten lines must not push
        // anything onto the bottom row.
        let lines: Vec<Line> = (1..=10)
            .map(|i| Line::new(i, 0, vec![Span::plain(format!("line {i}"))]))
            .collect();
        let bottom = vec![Span::plain("status")];
        let text = strip_ansi(&frame_bytes(&basic_frame(&lines, &bottom)));
        assert!(text.contains("line 3"));
        assert!(!text.contains("line 4"), "drew into the status row");
    }

    #[test]
    fn a_double_height_line_costs_two_rows() {
        let mut big = Line::new(1, 0, vec![Span::plain("BIG")]);
        big.scale = Scale::DoubleHeight;
        let lines = vec![
            big,
            Line::new(2, 0, vec![Span::plain("a")]),
            Line::new(3, 0, vec![Span::plain("b")]),
        ];
        let bottom = vec![];
        let raw = frame_bytes(&basic_frame(&lines, &bottom));
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains(DECDHL_TOP) && text.contains(DECDHL_BOTTOM));
        // Rows: BIG takes 0 and 1, "a" takes 2. "b" does not fit.
        let plain = strip_ansi(&raw);
        assert!(plain.contains('a'));
        assert!(!plain.contains('b'));
    }

    #[test]
    fn horizontal_offset_slices_by_display_column() {
        let lines = vec![Line::new(1, 0, vec![Span::plain("abcdefghij")])];
        let mut frame = basic_frame(&lines, &[]);
        frame.hoffset = 4;
        let text = strip_ansi(&frame_bytes(&frame));
        assert!(text.contains("efghij"));
        assert!(!text.contains("abcd"));
    }

    #[test]
    fn links_are_emitted_as_osc8_hyperlinks() {
        let links = vec![Link {
            url: "https://example.com".into(),
            title: String::new(),
        }];
        let lines = vec![Line::new(
            1,
            0,
            vec![Span::plain("docs").with_link(Some(LinkId(0)))],
        )];
        let mut frame = basic_frame(&lines, &[]);
        frame.links = &links;
        let raw = frame_bytes(&frame);
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("\x1b]8;;https://example.com\x1b\\"));
        assert!(text.contains("\x1b]8;;\x1b\\"), "hyperlink must be closed");
    }

    #[test]
    fn hit_rectangles_are_reported_in_screen_coordinates() {
        let mut line = Line::new(1, 0, vec![Span::plain("go")]);
        line.hits = vec![Hit {
            rect: Rect {
                x: 6,
                y: 0,
                w: 2,
                h: 1,
            },
            target: HitTarget::Link(LinkId(0)),
        }];
        let lines = vec![Line::new(1, 0, vec![]), line];
        let frame = basic_frame(&lines, &[]);
        let renderer = Renderer::new(WidthCalc::default());
        let mut buf = Vec::new();
        let placement = renderer
            .draw(&mut buf, &frame, &mut ImageStore::disabled())
            .unwrap();
        let hit = placement.hits[0];
        assert_eq!((hit.rect.x, hit.rect.y), (6, 1));
    }

    #[test]
    fn no_color_mode_emits_no_sgr_sequences() {
        let lines = vec![Line::new(
            1,
            0,
            vec![Span::new(
                "x",
                Style {
                    bold: true,
                    ..Style::PLAIN
                },
            )],
        )];
        let mut renderer = Renderer::new(WidthCalc::default());
        renderer.no_color = true;
        let mut buf = Vec::new();
        renderer
            .draw(
                &mut buf,
                &basic_frame(&lines, &[]),
                &mut ImageStore::disabled(),
            )
            .unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(!text.contains("\x1b[1m"));
    }

    #[test]
    fn rgb_downgrades_to_the_nearest_palette_entry() {
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        assert_eq!(rgb_to_ansi256(0, 255, 0), 46);
    }

    #[test]
    fn a_search_highlight_only_restyles_its_own_columns() {
        let calc = WidthCalc::default();
        let spans = vec![Span::plain("abcdef")];
        let out = apply_range(
            &spans,
            2,
            4,
            Style {
                reverse: true,
                ..Style::PLAIN
            },
            &calc,
        );
        let texts: Vec<&str> = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["ab", "cd", "ef"]);
        assert!(!out[0].style.reverse && out[1].style.reverse && !out[2].style.reverse);
    }
}
