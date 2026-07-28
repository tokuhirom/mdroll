//! The intermediate representation that sits between the Markdown AST and the
//! screen.
//!
//! Everything downstream of parsing works on [`Block`]s; everything downstream
//! of layout works on [`Line`]s. Both carry an original-source line number, and
//! that is what makes block yanking and position-preserving mode switches
//! possible.

use std::ops::Range;

pub use crossterm::style::Color;

/// Identifies a link target. Indexes into [`Document::links`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(pub usize);

/// Identifies an image. Indexes into [`Document::images`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImageId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub dim: bool,
    pub reverse: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        fg: None,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
        strikethrough: false,
        dim: false,
        reverse: false,
    };

    /// Overlay `other` on top of `self`. Colors set in `other` win; attributes
    /// are unioned, because emphasis nests (`**bold *and italic* **`).
    pub fn patch(self, other: Style) -> Style {
        Style {
            fg: other.fg.or(self.fg),
            bg: other.bg.or(self.bg),
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            underline: self.underline || other.underline,
            strikethrough: self.strikethrough || other.strikethrough,
            dim: self.dim || other.dim,
            reverse: self.reverse || other.reverse,
        }
    }

    pub fn fg(color: Color) -> Style {
        Style {
            fg: Some(color),
            ..Style::PLAIN
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    pub link: Option<LinkId>,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Span {
        Span {
            text: text.into(),
            style,
            link: None,
        }
    }

    pub fn plain(text: impl Into<String>) -> Span {
        Span::new(text, Style::PLAIN)
    }

    pub fn with_link(mut self, link: Option<LinkId>) -> Span {
        self.link = link;
        self
    }
}

/// Alignment of a table column, as declared by the delimiter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub head: Vec<Vec<Span>>,
    pub rows: Vec<Vec<Vec<Span>>>,
    pub align: Vec<Align>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Heading(u8),
    Para,
    Code { lang: Option<String> },
    Quote,
    List,
    Table,
    Image(ImageId),
    Rule,
}

/// One logical unit of the document.
///
/// `source_range` is a **1-based, end-exclusive** line range into the original
/// file. Yanking a block is a slice of those lines, never a reconstruction from
/// the rendered form.
#[derive(Debug, Clone)]
pub struct Block {
    pub source_range: Range<usize>,
    pub kind: BlockKind,
    pub spans: Vec<Span>,
    /// Drawn at the start of *every* line of the block: blockquote bars, alert
    /// stripes. Nesting appends, so a quote inside a quote gets two bars.
    pub gutter: Vec<Span>,
    /// Drawn once, after the gutter, on the first line only: list bullets,
    /// footnote labels. Continuation lines are padded to the same width.
    pub prefix: Vec<Span>,
    /// Columns of indentation applied to every line, before the gutter.
    pub indent: usize,
    /// Populated only for [`BlockKind::Table`].
    pub table: Option<Table>,
    /// Whether a blank line separates this block from the one before it.
    /// False inside tight lists.
    pub blank_before: bool,
}

impl Block {
    pub fn new(kind: BlockKind, source_range: Range<usize>) -> Block {
        Block {
            source_range,
            kind,
            spans: Vec::new(),
            gutter: Vec::new(),
            prefix: Vec::new(),
            indent: 0,
            table: None,
            blank_before: true,
        }
    }

    pub fn heading_level(&self) -> Option<u8> {
        match self.kind {
            BlockKind::Heading(level) => Some(level),
            _ => None,
        }
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The rendered plain text of the block, including its marker.
    pub fn plain_text(&self) -> String {
        let prefix: String = self.prefix.iter().map(|s| s.text.as_str()).collect();
        format!("{prefix}{}", self.text())
    }
}

#[derive(Debug, Clone)]
pub struct Link {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub url: String,
    pub alt: String,
    /// Pixel dimensions, filled in after parsing by whoever can touch the disk.
    /// Layout is pure, so it can only reserve rows for an image whose size is
    /// already known.
    pub size: Option<(u32, u32)>,
}

/// A parsed document: blocks plus the side tables their spans point into.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
    pub images: Vec<Image>,
    /// The original source, split into lines. Yanks slice this.
    pub source_lines: Vec<String>,
}

impl Document {
    /// The original Markdown for a block, verbatim.
    pub fn source_of(&self, block: &Block) -> String {
        let start = block.source_range.start.saturating_sub(1);
        let end = block.source_range.end.saturating_sub(1).min(self.source_lines.len());
        if start >= end {
            return String::new();
        }
        self.source_lines[start..end].join("\n")
    }
}

/// Vertical scale of a rendered line. `DoubleHeight` costs two physical rows
/// and halves the usable column count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scale {
    #[default]
    Normal,
    DoubleHeight,
}

impl Scale {
    pub fn rows(self) -> u16 {
        match self {
            Scale::Normal => 1,
            Scale::DoubleHeight => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x.saturating_add(self.w) && y >= self.y && y < self.y.saturating_add(self.h)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    Link(LinkId),
    Image(ImageId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub rect: Rect,
    pub target: HitTarget,
}

/// One laid-out row, ready to draw.
#[derive(Debug, Clone)]
pub struct Line {
    /// 1-based line in the original file this row came from.
    pub source_line: usize,
    /// Index into [`Document::blocks`], for the block cursor.
    pub block: usize,
    pub scale: Scale,
    pub spans: Vec<Span>,
    pub hits: Vec<Hit>,
}

impl Line {
    pub fn new(source_line: usize, block: usize, spans: Vec<Span>) -> Line {
        Line {
            source_line,
            block,
            scale: Scale::Normal,
            spans,
            hits: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_patch_unions_attributes_and_prefers_new_colors() {
        let base = Style {
            fg: Some(Color::Red),
            bold: true,
            ..Style::PLAIN
        };
        let over = Style {
            fg: Some(Color::Blue),
            italic: true,
            ..Style::PLAIN
        };
        let merged = base.patch(over);
        assert_eq!(merged.fg, Some(Color::Blue));
        assert!(merged.bold && merged.italic);
    }

    #[test]
    fn style_patch_keeps_old_color_when_new_has_none() {
        let base = Style::fg(Color::Red);
        let merged = base.patch(Style {
            bold: true,
            ..Style::PLAIN
        });
        assert_eq!(merged.fg, Some(Color::Red));
    }

    #[test]
    fn source_of_slices_the_original_lines() {
        let doc = Document {
            source_lines: vec!["a".into(), "b".into(), "c".into()],
            ..Document::default()
        };
        let block = Block::new(BlockKind::Para, 2..4);
        assert_eq!(doc.source_of(&block), "b\nc");
    }

    #[test]
    fn source_of_clamps_past_the_end() {
        let doc = Document {
            source_lines: vec!["a".into()],
            ..Document::default()
        };
        let block = Block::new(BlockKind::Para, 1..9);
        assert_eq!(doc.source_of(&block), "a");
    }
}
