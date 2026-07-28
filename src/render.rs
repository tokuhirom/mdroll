//! Drawing a frame.
//!
//! Rendering is a full redraw every frame. A viewer updates rarely, and
//! differential updates are the usual source of "the status line vanishes
//! sometimes" bugs.
//!
//! The order is fixed:
//!
//! ```text
//! clear → draw content → MoveTo(0, status_row) → draw status
//! ```
//!
//! Because the bottom row is written last at an absolute position, a content
//! region that miscounts by a row cannot hide it.

use crate::ir::{Color, Hit, HitTarget, Line, Link, Rect, Scale, Span, Style};
use crate::screen::Screen;
use crate::width::WidthCalc;
use crate::wrap::slice_spans_columns;
use anyhow::Result;
use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor::MoveTo, queue};
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
    pub decor: Decor<'a>,
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

    pub fn draw<W: Write>(&self, out: &mut W, frame: &Frame<'_>) -> Result<Placement> {
        let view = frame.screen.viewport();
        let mut placement = Placement::default();

        queue!(out, SetAttribute(Attribute::Reset))?;
        queue!(out, Clear(ClearType::All))?;

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
            self.draw_line(out, frame, idx, line, y, &mut placement)?;
            y += rows;
            idx += 1;
        }

        // The status row is written last, at an absolute position, so it
        // survives any miscount above.
        queue!(out, MoveTo(0, frame.screen.status_row()))?;
        out.write_all(DECAWM_OFF.as_bytes())?;
        out.write_all(DECSWL.as_bytes())?;
        let bottom = slice_spans_columns(frame.bottom, 0, frame.screen.cols as usize, &self.calc);
        self.write_spans(out, &bottom, None)?;
        let used: usize = bottom.iter().map(|s| self.calc.str(&s.text)).sum();
        if let Some(style) = bottom.last().map(|s| s.style) {
            self.set_style(out, style)?;
            let pad = (frame.screen.cols as usize).saturating_sub(used);
            if pad > 0 {
                out.write_all(" ".repeat(pad).as_bytes())?;
            }
        }
        queue!(out, SetAttribute(Attribute::Reset))?;
        out.write_all(DECAWM_ON.as_bytes())?;
        out.flush()?;
        Ok(placement)
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
        // A double-height row shows half as many characters per column.
        let cols = if double {
            (frame.screen.cols / 2) as usize
        } else {
            frame.screen.cols as usize
        };

        let decorated = self.decorate(frame, idx, line);
        let visible = slice_spans_columns(&decorated, frame.hoffset, cols, &self.calc);

        for (i, prefix) in [DECDHL_TOP, DECDHL_BOTTOM].iter().enumerate() {
            if i == 1 && !double {
                break;
            }
            queue!(out, MoveTo(0, y + i as u16))?;
            if double {
                out.write_all(prefix.as_bytes())?;
            } else {
                out.write_all(DECSWL.as_bytes())?;
            }
            self.write_spans(out, &visible, Some(frame.links))?;
            queue!(out, SetAttribute(Attribute::Reset))?;
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
fn apply_range(spans: &[Span], start: usize, end: usize, style: Style, calc: &WidthCalc) -> Vec<Span> {
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
        renderer.draw(&mut buf, frame).unwrap();
        buf
    }

    fn basic_frame<'a>(lines: &'a [Line], bottom: &'a [Span]) -> Frame<'a> {
        Frame {
            screen: Screen::new(20, 4),
            lines,
            scroll: 0,
            hoffset: 0,
            links: &[],
            bottom,
            decor: Decor::default(),
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
        let lines = vec![big, Line::new(2, 0, vec![Span::plain("a")]), Line::new(3, 0, vec![Span::plain("b")])];
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
        let placement = renderer.draw(&mut buf, &frame).unwrap();
        let hit = placement.hits[0];
        assert_eq!((hit.rect.x, hit.rect.y), (6, 1));
    }

    #[test]
    fn no_color_mode_emits_no_sgr_sequences() {
        let lines = vec![Line::new(
            1,
            0,
            vec![Span::new("x", Style { bold: true, ..Style::PLAIN })],
        )];
        let mut renderer = Renderer::new(WidthCalc::default());
        renderer.no_color = true;
        let mut buf = Vec::new();
        renderer.draw(&mut buf, &basic_frame(&lines, &[])).unwrap();
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
