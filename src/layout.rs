//! `Vec<Block>` → `Vec<Line>`.
//!
//! [`layout`] is a pure function of the document, the viewport, and the mode.
//! Toggling wrap, toggling source view, and resizing the terminal are all
//! handled by discarding the result and calling it again. There is no
//! incremental state and therefore nothing to invalidate.

use crate::graphics::{self, CellSize};
use crate::ir::*;
use crate::mermaid;
use crate::screen::Viewport;
use crate::theme::Theme;
use crate::width::WidthCalc;
use crate::wrap::{hard_wrap_ranges, slice_spans, spans_text, trim_trailing, wrap_ranges};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Reflow to the viewport width. When false, lines keep their natural
    /// length and the renderer scrolls horizontally.
    pub wrap: bool,
    /// Show the raw Markdown instead of the rendered form.
    pub source: bool,
    /// Cap on the reflow width. `0` means the full viewport.
    pub max_width: usize,
    pub calc: WidthCalc,
    /// Emit headings as DECDHL double-height lines.
    pub double_height: bool,
    /// Reserve rows for inline images. When false, or when the terminal has no
    /// graphics support, image blocks render as their alt text.
    pub images: bool,
    /// Pixels per character cell, used to turn an image's pixel size into rows.
    pub cell: CellSize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            wrap: true,
            source: false,
            max_width: 0,
            calc: WidthCalc::default(),
            double_height: false,
            images: false,
            cell: CellSize::default(),
        }
    }
}

pub fn layout(doc: &Document, view: Viewport, opts: &Options, theme: &Theme) -> Vec<Line> {
    let mut ctx = Ctx {
        doc,
        theme,
        opts,
        view,
        total: total_width(view, opts),
        lines: Vec::new(),
    };
    if opts.source {
        ctx.source_view();
    } else {
        for (i, block) in doc.blocks.iter().enumerate() {
            ctx.block(i, block);
        }
    }
    // A document that renders to nothing still needs a row to scroll onto.
    if ctx.lines.is_empty() {
        ctx.lines.push(Line::new(1, 0, Vec::new()));
    }
    ctx.lines
}

fn total_width(view: Viewport, opts: &Options) -> usize {
    let cols = view.cols.max(1) as usize;
    if opts.max_width > 0 {
        cols.min(opts.max_width)
    } else {
        cols
    }
}

struct Ctx<'a> {
    doc: &'a Document,
    theme: &'a Theme,
    opts: &'a Options,
    view: Viewport,
    total: usize,
    lines: Vec<Line>,
}

impl<'a> Ctx<'a> {
    fn calc(&self) -> &WidthCalc {
        &self.opts.calc
    }

    fn width_of(&self, spans: &[Span]) -> usize {
        spans.iter().map(|s| self.calc().str(&s.text)).sum()
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
    }

    fn blank(&mut self, source_line: usize, block: usize) {
        self.push(Line::new(source_line, block, Vec::new()));
    }

    // ---- rendered view ---------------------------------------------------

    fn block(&mut self, idx: usize, block: &Block) {
        if block.blank_before && !self.lines.is_empty() {
            self.blank(block.source_range.start, idx);
        }
        match &block.kind {
            BlockKind::Rule => self.rule(idx, block),
            BlockKind::Code { .. } => self.code(idx, block),
            BlockKind::Table => self.table(idx, block),
            BlockKind::Image(id) => {
                let id = *id;
                self.image(idx, block, id);
            }
            BlockKind::Heading(level) => {
                let level = *level;
                self.flow(idx, block, self.heading_scale(level));
            }
            _ => self.flow(idx, block, Scale::Normal),
        }
    }

    fn heading_scale(&self, level: u8) -> Scale {
        if self.opts.double_height && level <= 2 {
            Scale::DoubleHeight
        } else {
            Scale::Normal
        }
    }

    /// Columns available to a block's content, after indent, gutter and marker.
    fn content_width(&self, block: &Block, scale: Scale) -> usize {
        let lead = block.indent + self.width_of(&block.gutter) + self.width_of(&block.prefix);
        let total = match scale {
            // A double-height row shows half as many characters.
            Scale::DoubleHeight => self.total / 2,
            Scale::Normal => self.total,
        };
        total.saturating_sub(lead).max(4)
    }

    /// Indent + gutter + (marker on the first row, padding afterwards).
    fn lead_spans(&self, block: &Block, first: bool) -> Vec<Span> {
        let mut out = Vec::new();
        if block.indent > 0 {
            out.push(Span::plain(" ".repeat(block.indent)));
        }
        out.extend(block.gutter.iter().cloned());
        if first {
            out.extend(block.prefix.iter().cloned());
        } else {
            let pad = self.width_of(&block.prefix);
            if pad > 0 {
                out.push(Span::plain(" ".repeat(pad)));
            }
        }
        out
    }

    /// Split a block's spans on hard line breaks. Soft breaks became spaces at
    /// parse time, so anything left is a `<br>` or a code block's own newlines.
    fn logical_lines(&self, spans: &[Span]) -> Vec<Vec<Span>> {
        let text = spans_text(spans);
        if !text.contains('\n') {
            return vec![spans.to_vec()];
        }
        let mut out = Vec::new();
        let mut start = 0usize;
        for (i, c) in text.char_indices() {
            if c == '\n' {
                out.push(slice_spans(spans, start, i));
                start = i + 1;
            }
        }
        out.push(slice_spans(spans, start, text.len()));
        out
    }

    fn flow(&mut self, idx: usize, block: &Block, scale: Scale) {
        let width = self.content_width(block, scale);
        let mut first = true;
        let mut row = 0usize;
        for logical in self.logical_lines(&block.spans) {
            let pieces: Vec<Vec<Span>> = if self.opts.wrap {
                let text = spans_text(&logical);
                wrap_ranges(&text, width, self.calc())
                    .into_iter()
                    .map(|(s, e)| trim_trailing(slice_spans(&logical, s, e)))
                    .collect()
            } else {
                vec![logical]
            };
            for piece in pieces {
                let mut spans = self.lead_spans(block, first);
                spans.extend(piece);
                let mut line = Line::new(self.source_line(block, row), idx, spans);
                line.scale = scale;
                line.hits = self.hits(&line.spans, scale);
                self.push(line);
                first = false;
                row += 1;
            }
        }
    }

    /// Best guess at which original line a rendered row came from. Exact for
    /// code and tables, approximate inside a reflowed paragraph — which is all
    /// that position-preserving mode switches need.
    fn source_line(&self, block: &Block, row: usize) -> usize {
        let start = block.source_range.start;
        let last = block.source_range.end.saturating_sub(1).max(start);
        (start + row).min(last)
    }

    /// Reserve cells for an inline image and hang a hit rectangle off every
    /// row it covers.
    ///
    /// The hit carries the row's index *within* the image, which is what lets
    /// the renderer place a partially scrolled image cropped rather than
    /// dropping it.
    fn image(&mut self, idx: usize, block: &Block, id: ImageId) {
        if !self.image_rows(idx, block, id, true) {
            // No graphics, or a remote or unreadable file: show the alt text.
            self.flow(idx, block, Scale::Normal);
        }
    }

    /// Lay out an image, returning false if it cannot be drawn at all.
    fn image_rows(&mut self, idx: usize, block: &Block, id: ImageId, caption: bool) -> bool {
        let size = self
            .doc
            .images
            .get(id.0)
            .and_then(|i| i.size)
            .filter(|_| self.opts.images);
        let Some(size) = size else {
            return false;
        };

        let width = self.content_width(block, Scale::Normal);
        // An image never takes more than two thirds of the screen, so there is
        // always some text left to orient by.
        let max_rows = ((self.view.rows as usize * 2) / 3).max(3);
        let (cols, rows) = graphics::fit(size, self.opts.cell, width, max_rows);
        let x = self.width_of(&self.lead_spans(block, false)) as u16;
        let source = block.source_range.start;

        for row in 0..rows {
            let mut spans = self.lead_spans(block, row == 0);
            // Reserved cells: the terminal paints the image over them.
            spans.push(Span::plain(" ".repeat(cols as usize)));
            let mut line = Line::new(source, idx, spans);
            line.hits = vec![Hit {
                rect: Rect {
                    x,
                    y: row,
                    w: cols,
                    h: rows,
                },
                target: HitTarget::Image(id),
            }];
            self.push(line);
        }

        let alt = self
            .doc
            .images
            .get(id.0)
            .map(|i| i.alt.clone())
            .unwrap_or_default();
        if caption && !alt.trim().is_empty() {
            let mut spans = self.lead_spans(block, false);
            let (text, _) = self.calc().truncate(&alt, width);
            spans.push(Span::new(text, self.theme.dim));
            self.push(Line::new(source, idx, spans));
        }
        true
    }

    fn rule(&mut self, idx: usize, block: &Block) {
        let width = self.content_width(block, Scale::Normal);
        let mut spans = self.lead_spans(block, true);
        spans.push(Span::new("─".repeat(width), self.theme.rule));
        self.push(Line::new(block.source_range.start, idx, spans));
    }

    /// Render a ```mermaid block as a diagram, or `false` if it cannot be.
    ///
    /// A diagram is drawn at its intrinsic size; if that does not fit the
    /// available columns there is nothing sensible to reflow, so the block
    /// falls back to being shown as source.
    fn mermaid(&mut self, idx: usize, block: &Block) -> bool {
        let BlockKind::Code { lang: Some(lang) } = &block.kind else {
            return false;
        };
        if !lang.eq_ignore_ascii_case("mermaid") {
            return false;
        }
        let Some(rows) = mermaid::render(&block.text(), self.calc()) else {
            return false;
        };
        let width = self.content_width(block, Scale::Normal);
        if rows.iter().any(|r| self.calc().str(r) > width) {
            return false;
        }

        let source = block.source_range.start;
        for (i, row) in rows.iter().enumerate() {
            let mut spans = self.lead_spans(block, i == 0);
            spans.extend(self.diagram_spans(row));
            self.push(Line::new(source, idx, spans));
        }
        true
    }

    /// Split a diagram row so the rules are drawn in the border colour and the
    /// labels in the body colour.
    fn diagram_spans(&self, row: &str) -> Vec<Span> {
        let mut out: Vec<Span> = Vec::new();
        for c in row.chars() {
            let style = if is_box_drawing(c) {
                self.theme.table_border
            } else {
                self.theme.body()
            };
            match out.last_mut() {
                Some(last) if last.style == style => last.text.push(c),
                _ => out.push(Span::new(c.to_string(), style)),
            }
        }
        out
    }

    fn code(&mut self, idx: usize, block: &Block) {
        // A mermaid block rendered through mmdc displays as its picture but
        // stays a code block, so `yc` still yanks the diagram source.
        if let Some(id) = block.image
            && self.image_rows(idx, block, id, false)
        {
            return;
        }
        if self.mermaid(idx, block) {
            return;
        }
        // Fenced blocks have their opening fence on the first source line;
        // indented blocks start immediately.
        let fenced = self
            .doc
            .source_lines
            .get(block.source_range.start.saturating_sub(1))
            .is_some_and(|l| {
                let t = l.trim_start();
                t.starts_with("```") || t.starts_with("~~~")
            });
        let offset = usize::from(fenced);
        let rail = "▏ ";
        let width = self
            .content_width(block, Scale::Normal)
            .saturating_sub(self.calc().str(rail))
            .max(4);

        let mut first = true;
        for (i, logical) in self.logical_lines(&block.spans).into_iter().enumerate() {
            let source = block.source_range.start + offset + i;
            let text = spans_text(&logical);
            let pieces = if self.opts.wrap {
                hard_wrap_ranges(&text, width, self.calc())
            } else {
                vec![(0, text.len())]
            };
            for (s, e) in pieces {
                let mut spans = self.lead_spans(block, first);
                spans.push(Span::new(rail, self.theme.code_fence));
                spans.extend(slice_spans(&logical, s, e));
                self.push(Line::new(source, idx, spans));
                first = false;
            }
        }
    }

    fn table(&mut self, idx: usize, block: &Block) {
        let Some(table) = block.table.as_ref() else {
            return;
        };
        let border = self.theme.table_border;
        let ncols = table
            .head
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if ncols == 0 {
            return;
        }

        let widths = self.column_widths(block, table, ncols);
        let rule = |left: &str, mid: &str, right: &str, widths: &[usize]| {
            let mut s = String::from(left);
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    s.push_str(mid);
                }
                s.push_str(&"─".repeat(w + 2));
            }
            s.push_str(right);
            s
        };

        let start = block.source_range.start;
        let mut source = start;
        let emit_rule = |ctx: &mut Self, text: String, source: usize| {
            let mut spans = ctx.lead_spans(block, ctx.lines.is_empty());
            spans.push(Span::new(text, border));
            ctx.push(Line::new(source, idx, spans));
        };

        emit_rule(self, rule("┌", "┬", "┐", &widths), source);
        if !table.head.is_empty() {
            self.table_row(idx, block, &table.head, &widths, &table.align, source);
            source += 1;
            emit_rule(self, rule("├", "┼", "┤", &widths), source);
            source += 1;
        }
        for row in &table.rows {
            self.table_row(idx, block, row, &widths, &table.align, source);
            source += 1;
        }
        emit_rule(
            self,
            rule("└", "┴", "┘", &widths),
            source.min(block.source_range.end.saturating_sub(1)),
        );
    }

    /// Natural column widths, shrunk proportionally if the table would
    /// overflow. Widths are display columns, so a CJK cell reserves the space
    /// it actually occupies.
    fn column_widths(&self, block: &Block, table: &Table, ncols: usize) -> Vec<usize> {
        let mut widths = vec![0usize; ncols];
        let mut consider = |row: &Vec<Vec<Span>>| {
            for (i, cell) in row.iter().enumerate() {
                if i < ncols {
                    widths[i] = widths[i].max(self.width_of(cell));
                }
            }
        };
        consider(&table.head);
        for row in &table.rows {
            consider(row);
        }
        for w in &mut widths {
            *w = (*w).max(1);
        }

        if !self.opts.wrap {
            return widths;
        }
        // "│ " + cell + " " per column, plus the closing "│".
        let chrome = 3 * ncols + 1;
        let lead = block.indent + self.width_of(&block.gutter) + self.width_of(&block.prefix);
        let budget = self.total.saturating_sub(lead + chrome);
        let mut total: usize = widths.iter().sum();
        while total > budget.max(ncols) {
            let Some(widest) = widths
                .iter()
                .enumerate()
                .filter(|(_, w)| **w > 1)
                .max_by_key(|(i, w)| (**w, usize::MAX - *i))
                .map(|(i, _)| i)
            else {
                break;
            };
            widths[widest] -= 1;
            total -= 1;
        }
        widths
    }

    fn table_row(
        &mut self,
        idx: usize,
        block: &Block,
        row: &[Vec<Span>],
        widths: &[usize],
        align: &[Align],
        source: usize,
    ) {
        let border = self.theme.table_border;
        // Wrap every cell first; the row is as tall as its tallest cell.
        let wrapped: Vec<Vec<Vec<Span>>> = widths
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let cell = row.get(i).cloned().unwrap_or_default();
                let text = spans_text(&cell);
                if text.is_empty() {
                    return vec![Vec::new()];
                }
                wrap_ranges(&text, *w, self.calc())
                    .into_iter()
                    .map(|(s, e)| trim_trailing(slice_spans(&cell, s, e)))
                    .collect()
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);

        for line_idx in 0..height {
            let mut spans = self.lead_spans(block, false);
            for (col, width) in widths.iter().enumerate() {
                spans.push(Span::new("│ ", border));
                let empty = Vec::new();
                let cell = wrapped[col].get(line_idx).unwrap_or(&empty);
                let used = self.width_of(cell);
                let pad = width.saturating_sub(used);
                let (before, after) = match align.get(col).copied().unwrap_or_default() {
                    Align::Left => (0, pad),
                    Align::Right => (pad, 0),
                    Align::Center => (pad / 2, pad - pad / 2),
                };
                if before > 0 {
                    spans.push(Span::plain(" ".repeat(before)));
                }
                spans.extend(cell.iter().cloned());
                spans.push(Span::plain(" ".repeat(after + 1)));
            }
            spans.push(Span::new("│", border));
            let mut line = Line::new(source, idx, spans);
            line.hits = self.hits(&line.spans, Scale::Normal);
            self.push(line);
        }
    }

    /// Column rectangles for the linked runs in a row.
    ///
    /// `rect.y` is relative to the line's own top; the renderer translates it
    /// to screen coordinates when it draws.
    fn hits(&self, spans: &[Span], scale: Scale) -> Vec<Hit> {
        let mut hits: Vec<Hit> = Vec::new();
        let mut x = 0usize;
        let factor = match scale {
            Scale::DoubleHeight => 2,
            Scale::Normal => 1,
        };
        for span in spans {
            let w = self.calc().str(&span.text) * factor;
            if let Some(link) = span.link {
                let target = HitTarget::Link(link);
                match hits.last_mut() {
                    Some(last)
                        if last.target == target && (last.rect.x + last.rect.w) as usize == x =>
                    {
                        last.rect.w = last.rect.w.saturating_add(w as u16);
                    }
                    _ => hits.push(Hit {
                        rect: Rect {
                            x: x as u16,
                            y: 0,
                            w: w as u16,
                            h: scale.rows(),
                        },
                        target,
                    }),
                }
            }
            x += w;
        }
        hits
    }

    // ---- source view -----------------------------------------------------

    fn source_view(&mut self) {
        let owner = self.line_owners();
        let mut in_fence = false;
        for (i, raw) in self.doc.source_lines.iter().enumerate() {
            let source = i + 1;
            let block = owner.get(i).copied().unwrap_or(0);
            let trimmed = raw.trim_start();
            let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
            let style = if in_fence || is_fence {
                self.theme.code
            } else {
                self.source_style(trimmed)
            };
            if is_fence {
                in_fence = !in_fence;
            }

            let spans = vec![Span::new(raw.clone(), style)];
            if self.opts.wrap {
                let width = self.total.max(4);
                for (s, e) in wrap_ranges(raw, width, self.calc()) {
                    self.push(Line::new(source, block, slice_spans(&spans, s, e)));
                }
            } else {
                self.push(Line::new(source, block, spans));
            }
        }
    }

    fn source_style(&self, trimmed: &str) -> Style {
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count().min(6) as u8;
            self.theme.heading(level.max(1))
        } else if trimmed.starts_with('>') {
            self.theme.quote
        } else if trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("+ ")
        {
            self.theme.list_marker
        } else if trimmed.starts_with("---") || trimmed.starts_with("***") {
            self.theme.rule
        } else {
            self.theme.body()
        }
    }

    /// Which block owns each source line, so the block cursor keeps working in
    /// source view.
    fn line_owners(&self) -> Vec<usize> {
        let mut owner = vec![0usize; self.doc.source_lines.len()];
        for (i, block) in self.doc.blocks.iter().enumerate() {
            for line in block.source_range.clone() {
                if line >= 1 && line - 1 < owner.len() {
                    owner[line - 1] = i;
                }
            }
        }
        owner
    }
}

/// Box-drawing, block, and arrow characters, which diagrams paint in the
/// border colour rather than the body colour.
fn is_box_drawing(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{257f}' | '\u{2580}'..='\u{259f}' | '▲' | '▶' | '▼' | '◀' | '◇')
}

/// The layout row that best matches a source line. Used to preserve reading
/// position across a mode switch.
pub fn row_for_source_line(lines: &[Line], source_line: usize) -> usize {
    let mut best = 0usize;
    let mut best_delta = usize::MAX;
    for (i, line) in lines.iter().enumerate() {
        let delta = line.source_line.abs_diff(source_line);
        if delta < best_delta {
            best_delta = delta;
            best = i;
            if delta == 0 {
                break;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn render(src: &str, cols: u16, opts: Options) -> Vec<String> {
        let theme = Theme::default();
        let doc = parse(src, &theme);
        layout(&doc, Viewport::new(cols, 24), &opts, &theme)
            .iter()
            .map(|l| l.text().trim_end().to_string())
            .collect()
    }

    fn wrapped(src: &str, cols: u16) -> Vec<String> {
        render(src, cols, Options::default())
    }

    #[test]
    fn a_paragraph_reflows_to_the_viewport() {
        let out = wrapped("alpha beta gamma delta epsilon", 12);
        assert!(out.len() > 1);
        for line in &out {
            assert!(line.chars().count() <= 12, "{line:?}");
        }
    }

    #[test]
    fn no_wrap_keeps_one_row_per_logical_line() {
        let out = render(
            "alpha beta gamma delta epsilon",
            12,
            Options {
                wrap: false,
                ..Options::default()
            },
        );
        assert_eq!(out, vec!["alpha beta gamma delta epsilon"]);
    }

    #[test]
    fn max_width_caps_the_reflow_width() {
        let out = render(
            "alpha beta gamma delta epsilon zeta eta",
            200,
            Options {
                max_width: 20,
                ..Options::default()
            },
        );
        assert!(out.iter().all(|l| l.chars().count() <= 20));
        assert!(out.len() > 1);
    }

    #[test]
    fn blocks_are_separated_by_a_blank_line_but_never_lead_with_one() {
        let out = wrapped("# Title\n\nBody text.\n", 40);
        assert_eq!(out, vec!["Title", "", "Body text."]);
    }

    #[test]
    fn list_markers_appear_once_and_continuations_are_padded() {
        let out = wrapped("- alpha beta gamma delta\n", 12);
        assert!(out[0].starts_with("• "));
        assert!(out[1].starts_with("  "), "{:?}", out[1]);
        assert!(!out[1].contains('•'));
    }

    #[test]
    fn quote_bars_repeat_on_every_wrapped_row() {
        let out = wrapped("> alpha beta gamma delta epsilon\n", 14);
        assert!(out.len() > 1);
        assert!(out.iter().all(|l| l.starts_with("│ ")), "{out:?}");
    }

    #[test]
    fn code_is_chopped_at_the_column_limit_rather_than_reflowed() {
        let out = wrapped("```\nlet a = 1; let b = 2; let c = 3;\n```\n", 20);
        assert!(out.len() > 1);
        // No word-boundary reflow: the pieces concatenate back to the original.
        let joined: String = out.iter().map(|l| l.trim_start_matches("▏ ")).collect();
        assert_eq!(joined, "let a = 1; let b = 2; let c = 3;");
    }

    #[test]
    fn code_keeps_its_full_length_when_not_wrapping() {
        let long = "x".repeat(60);
        let out = render(
            &format!("```\n{long}\n```\n"),
            20,
            Options {
                wrap: false,
                ..Options::default()
            },
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].ends_with(&long));
    }

    #[test]
    fn code_lines_map_back_to_their_own_source_lines() {
        let theme = Theme::default();
        let doc = parse("```rust\nlet a = 1;\nlet b = 2;\n```\n", &theme);
        let lines = layout(&doc, Viewport::new(40, 24), &Options::default(), &theme);
        assert_eq!(lines[0].source_line, 2);
        assert_eq!(lines[1].source_line, 3);
    }

    #[test]
    fn a_thematic_break_fills_the_width() {
        let out = wrapped("---\n", 10);
        assert_eq!(out, vec!["──────────"]);
    }

    #[test]
    fn tables_are_drawn_with_borders() {
        let out = wrapped("| a | b |\n| --- | --- |\n| 1 | 2 |\n", 40);
        assert!(out[0].starts_with('┌'), "{out:?}");
        assert!(out.iter().any(|l| l.starts_with('├')));
        assert!(out.last().unwrap().starts_with('└'));
    }

    #[test]
    fn table_columns_are_measured_in_display_columns() {
        let out = wrapped("| key | value |\n| --- | --- |\n| a | 日本語 |\n", 40);
        // Every row of the table must be exactly the same display width, or
        // the right-hand border will not line up.
        let calc = WidthCalc::default();
        let widths: Vec<usize> = out.iter().map(|l| calc.str(l)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged table: {widths:?}\n{out:#?}"
        );
    }

    #[test]
    fn a_wide_table_is_shrunk_to_fit_when_wrapping() {
        let src =
            "| a | b |\n| --- | --- |\n| a very long cell indeed | another very long cell |\n";
        let out = wrapped(src, 30);
        let calc = WidthCalc::default();
        assert!(out.iter().all(|l| calc.str(l) <= 30), "{out:#?}");
    }

    #[test]
    fn right_aligned_cells_are_padded_on_the_left() {
        let out = wrapped("| n |\n| ---: |\n| 7 |\n", 40);
        let row = out.iter().find(|l| l.contains('7')).unwrap();
        assert!(row.contains("  7 │") || row.contains(" 7 │"), "{row:?}");
    }

    #[test]
    fn source_view_shows_the_markup_verbatim() {
        let src = "# Title\n\n- item\n";
        let out = render(
            src,
            40,
            Options {
                source: true,
                wrap: false,
                ..Options::default()
            },
        );
        assert_eq!(out, vec!["# Title", "", "- item"]);
    }

    #[test]
    fn source_view_rows_map_one_to_one_onto_source_lines_when_not_wrapping() {
        let theme = Theme::default();
        let src = "# Title\n\nbody\n";
        let doc = parse(src, &theme);
        let lines = layout(
            &doc,
            Viewport::new(40, 24),
            &Options {
                source: true,
                wrap: false,
                ..Options::default()
            },
            &theme,
        );
        assert_eq!(
            lines.iter().map(|l| l.source_line).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn double_height_headings_halve_the_usable_width() {
        let theme = Theme::default();
        let doc = parse("# alpha beta gamma delta\n", &theme);
        let lines = layout(
            &doc,
            Viewport::new(24, 24),
            &Options {
                double_height: true,
                ..Options::default()
            },
            &theme,
        );
        assert!(lines.iter().all(|l| l.scale == Scale::DoubleHeight));
        let calc = WidthCalc::default();
        assert!(
            lines.iter().all(|l| calc.str(&l.text()) <= 12),
            "{lines:#?}"
        );
    }

    #[test]
    fn links_produce_hit_rectangles_covering_their_label() {
        let theme = Theme::default();
        let doc = parse("see [docs](https://example.com) now\n", &theme);
        let lines = layout(&doc, Viewport::new(60, 24), &Options::default(), &theme);
        let hit = lines[0].hits.first().expect("a link hit");
        assert_eq!(hit.target, HitTarget::Link(LinkId(0)));
        assert_eq!(hit.rect.x, 4);
        assert_eq!(hit.rect.w, 4);
    }

    #[test]
    fn every_rendered_row_fits_the_viewport_when_wrapping() {
        let src = include_str!("../tests/fixtures/kitchen-sink.md");
        let calc = WidthCalc::default();
        for cols in [20u16, 40, 60, 80, 100] {
            for line in render(src, cols, Options::default()) {
                assert!(
                    calc.str(&line) <= cols as usize,
                    "cols={cols} overflowed: {line:?}"
                );
            }
        }
    }

    fn image_doc(size: Option<(u32, u32)>) -> (Document, Theme) {
        let theme = Theme::default();
        let mut doc = parse("![a picture](pic.png)\n", &theme);
        doc.images[0].size = size;
        (doc, theme)
    }

    #[test]
    fn an_image_reserves_rows_and_tags_every_one_of_them() {
        let (doc, theme) = image_doc(Some((400, 200)));
        let opts = Options {
            images: true,
            cell: CellSize { w: 10, h: 20 },
            ..Options::default()
        };
        let lines = layout(&doc, Viewport::new(80, 24), &opts, &theme);
        // 400x200px over 10x20px cells is 40x10 cells, within both budgets.
        let tagged: Vec<&Line> = lines.iter().filter(|l| !l.hits.is_empty()).collect();
        assert_eq!(tagged.len(), 10);
        for (i, line) in tagged.iter().enumerate() {
            let hit = line.hits[0];
            assert_eq!(hit.target, HitTarget::Image(ImageId(0)));
            assert_eq!(
                hit.rect.y, i as u16,
                "each row knows its place in the image"
            );
            assert_eq!(hit.rect.h, 10);
            assert_eq!(hit.rect.w, 40);
        }
    }

    #[test]
    fn an_image_is_followed_by_its_alt_text_as_a_caption() {
        let (doc, theme) = image_doc(Some((400, 200)));
        let opts = Options {
            images: true,
            cell: CellSize { w: 10, h: 20 },
            ..Options::default()
        };
        let lines = layout(&doc, Viewport::new(80, 24), &opts, &theme);
        assert!(lines.last().unwrap().text().contains("a picture"));
    }

    #[test]
    fn an_image_is_capped_so_text_stays_on_screen() {
        let (doc, theme) = image_doc(Some((400, 4000)));
        let opts = Options {
            images: true,
            cell: CellSize { w: 10, h: 20 },
            ..Options::default()
        };
        let lines = layout(&doc, Viewport::new(80, 24), &opts, &theme);
        let rows = lines.iter().filter(|l| !l.hits.is_empty()).count();
        assert!(rows <= 16, "a tall image took {rows} of 24 rows");
    }

    #[test]
    fn an_unmeasurable_image_falls_back_to_alt_text() {
        let (doc, theme) = image_doc(None);
        let opts = Options {
            images: true,
            ..Options::default()
        };
        let lines = layout(&doc, Viewport::new(80, 24), &opts, &theme);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].hits.is_empty());
        assert_eq!(lines[0].text(), "a picture");
    }

    #[test]
    fn turning_images_off_falls_back_to_alt_text() {
        let (doc, theme) = image_doc(Some((400, 200)));
        let lines = layout(&doc, Viewport::new(80, 24), &Options::default(), &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "a picture");
    }

    #[test]
    fn a_mermaid_block_is_drawn_as_a_diagram() {
        let out = wrapped("```mermaid\nflowchart TD\n  A[one] --> B[two]\n```\n", 60);
        let text = out.join("\n");
        assert!(text.contains('┌') && text.contains('▼'), "{text}");
        assert!(
            !text.contains("flowchart"),
            "the source should not be shown"
        );
    }

    #[test]
    fn an_unsupported_mermaid_diagram_falls_back_to_source() {
        let out = wrapped("```mermaid\npie title Pets\n  \"Dogs\" : 42\n```\n", 60);
        assert!(out.iter().any(|l| l.contains("pie title Pets")), "{out:#?}");
    }

    #[test]
    fn a_diagram_too_wide_for_the_viewport_falls_back_to_source() {
        let src = "```mermaid\nflowchart LR\n  A[a really quite long label] --> B[another long one]\n```\n";
        let out = wrapped(src, 30);
        assert!(out.iter().any(|l| l.contains("flowchart LR")), "{out:#?}");
    }

    #[test]
    fn row_for_source_line_finds_the_nearest_row() {
        let lines = vec![
            Line::new(1, 0, Vec::new()),
            Line::new(5, 0, Vec::new()),
            Line::new(9, 0, Vec::new()),
        ];
        assert_eq!(row_for_source_line(&lines, 5), 1);
        assert_eq!(row_for_source_line(&lines, 6), 1);
        assert_eq!(row_for_source_line(&lines, 100), 2);
    }
}
