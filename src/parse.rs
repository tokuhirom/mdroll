//! Markdown → [`Document`].
//!
//! comrak does the parsing, with the GitHub Flavored Markdown extensions turned
//! on. This module flattens its AST into the flat [`Block`] list that layout
//! consumes, carrying each node's `sourcepos` through as a `source_range` so a
//! yank can slice the original file rather than reconstruct it.

use crate::ir::*;
use crate::theme::Theme;
use crate::width::expand_tabs;
use comrak::nodes::{AlertType, AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

/// comrak options: CommonMark plus the GitHub extensions.
pub fn options<'c>() -> Options<'c> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o.extension.alerts = true;
    o.extension.superscript = true;
    o.extension.subscript = true;
    o.extension.underline = true;
    o.extension.spoiler = true;
    o.extension.math_dollars = true;
    o.extension.math_code = true;
    o.extension.multiline_block_quotes = true;
    o.extension.description_lists = true;
    o.extension.front_matter_delimiter = Some("---".into());
    o.parse.tasklist_in_table = true;
    o.parse.smart = false;
    o
}

pub fn parse(source: &str, theme: &Theme) -> Document {
    let arena = Arena::new();
    let opts = options();
    let root = parse_document(&arena, source, &opts);

    let mut builder = Builder {
        theme,
        doc: Document {
            source_lines: source.lines().map(expand_tabs).collect(),
            ..Document::default()
        },
        indent: 0,
        gutter: Vec::new(),
        pending_prefix: None,
        pending_blank: false,
        tight: false,
    };
    builder.blocks(root);
    builder.doc
}

struct Builder<'t> {
    theme: &'t Theme,
    doc: Document,
    indent: usize,
    gutter: Vec<Span>,
    pending_prefix: Option<Vec<Span>>,
    pending_blank: bool,
    tight: bool,
}

/// 1-based, end-exclusive line range of a node.
fn range_of<'a>(node: &'a AstNode<'a>) -> std::ops::Range<usize> {
    let pos = node.data.borrow().sourcepos;
    let start = pos.start.line.max(1);
    let end = pos.end.line.max(start);
    start..end + 1
}

impl<'t> Builder<'t> {
    fn push(&mut self, mut block: Block) {
        block.indent = self.indent;
        block.gutter = self.gutter.clone();
        if let Some(prefix) = self.pending_prefix.take() {
            block.prefix = prefix;
        }
        block.blank_before = self.pending_blank && !self.doc.blocks.is_empty();
        self.pending_blank = !self.tight;
        self.doc.blocks.push(block);
    }

    fn blocks<'a>(&mut self, node: &'a AstNode<'a>) {
        for child in node.children() {
            self.block(child);
        }
    }

    fn block<'a>(&mut self, node: &'a AstNode<'a>) {
        let range = range_of(node);
        let value = node.data.borrow().value.clone();

        match value {
            NodeValue::Document => self.blocks(node),

            NodeValue::FrontMatter(text) => {
                let mut block = Block::new(BlockKind::Code { lang: None }, range);
                block.spans = vec![Span::new(text.trim_end().to_string(), self.theme.dim)];
                self.push(block);
            }

            NodeValue::Paragraph => {
                // A paragraph holding nothing but an image is a figure, not a
                // sentence with a picture in it.
                if let Some(only) = sole_child(node)
                    && let NodeValue::Image(link) = &only.data.borrow().value
                {
                    let alt = collect_text(only);
                    let id = ImageId(self.doc.images.len());
                    self.doc.images.push(Image {
                        url: link.url.clone(),
                        alt: alt.clone(),
                        size: None,
                    });
                    let mut block = Block::new(BlockKind::Image(id), range);
                    block.spans = vec![Span::new(
                        if alt.is_empty() {
                            link.url.clone()
                        } else {
                            alt
                        },
                        self.theme.dim,
                    )];
                    self.push(block);
                    return;
                }
                let spans = self.inlines(node, self.theme.body(), None);
                let mut block = Block::new(BlockKind::Para, range);
                block.spans = spans;
                self.push(block);
            }

            NodeValue::Heading(h) => {
                let style = self.theme.heading(h.level);
                let spans = self.inlines(node, style, None);
                let mut block = Block::new(BlockKind::Heading(h.level), range);
                block.spans = spans;
                self.push(block);
            }

            NodeValue::ThematicBreak => {
                let mut block = Block::new(BlockKind::Rule, range);
                block.spans = vec![Span::new("─", self.theme.rule)];
                self.push(block);
            }

            NodeValue::CodeBlock(code) => {
                let lang = code
                    .info
                    .split_whitespace()
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string);
                let literal = code.literal.trim_end_matches('\n');
                let spans = lang
                    .as_deref()
                    .and_then(|lang| {
                        crate::highlight::highlight(
                            literal,
                            lang,
                            &self.theme.syntect_theme,
                            self.theme.body(),
                        )
                    })
                    .unwrap_or_else(|| vec![Span::new(literal.to_string(), self.theme.body())]);
                let mut block = Block::new(BlockKind::Code { lang }, range);
                block.spans = spans;
                self.push(block);
            }

            NodeValue::HtmlBlock(html) => {
                let mut block = Block::new(
                    BlockKind::Code {
                        lang: Some("html".into()),
                    },
                    range,
                );
                block.spans = vec![Span::new(
                    html.literal.trim_end_matches('\n').to_string(),
                    self.theme.dim,
                )];
                self.push(block);
            }

            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                self.gutter.push(Span::new("│ ", self.theme.quote_bar));
                self.blocks(node);
                self.gutter.pop();
                self.pending_blank = true;
            }

            NodeValue::Alert(alert) => {
                let style = self.alert_style(alert.alert_type);
                let title = alert
                    .title
                    .clone()
                    .unwrap_or_else(|| alert_label(alert.alert_type).to_string());
                self.gutter.push(Span::new("▌ ", style));
                let mut head = Block::new(BlockKind::Para, range.clone());
                head.spans = vec![Span::new(title, style)];
                self.push(head);
                self.pending_blank = false;
                self.blocks(node);
                self.gutter.pop();
                self.pending_blank = true;
            }

            NodeValue::List(list) => {
                let outer_tight = self.tight;
                self.tight = list.tight;
                let mut ordinal = list.start;
                for (i, item) in node.children().enumerate() {
                    if i > 0 {
                        self.pending_blank = !list.tight;
                    }
                    self.pending_prefix =
                        Some(self.item_marker(item, &list.list_type, &mut ordinal));
                    let outer_indent = self.indent;
                    self.item(item);
                    self.indent = outer_indent;
                }
                self.tight = outer_tight;
                self.pending_blank = true;
            }

            NodeValue::Item(_) | NodeValue::TaskItem(_) => self.item(node),

            NodeValue::DescriptionList => self.blocks(node),
            NodeValue::DescriptionItem(_) => self.blocks(node),
            NodeValue::DescriptionTerm => {
                let spans = self.inlines(node, self.theme.strong, None);
                let mut block = Block::new(BlockKind::Para, range);
                block.spans = spans;
                self.push(block);
            }
            NodeValue::DescriptionDetails => {
                self.indent += 2;
                self.blocks(node);
                self.indent -= 2;
            }

            NodeValue::Table(table) => self.table(node, &table.alignments, range),

            NodeValue::FootnoteDefinition(def) => {
                self.pending_prefix = Some(vec![Span::new(
                    format!("[^{}] ", def.name),
                    self.theme.footnote,
                )]);
                self.blocks(node);
            }

            // Inline nodes cannot appear here, and anything left over is
            // rendered as a paragraph rather than silently dropped.
            _ => {
                let spans = self.inlines(node, self.theme.body(), None);
                if !spans.iter().all(|s| s.text.trim().is_empty()) {
                    let mut block = Block::new(BlockKind::Para, range);
                    block.spans = spans;
                    self.push(block);
                }
            }
        }
    }

    fn item<'a>(&mut self, node: &'a AstNode<'a>) {
        let outer = self.indent;
        let mut first = true;
        for child in node.children() {
            if !first {
                // Continuation blocks line up under the item text.
                self.indent = outer + self.pending_indent();
            }
            self.block(child);
            first = false;
        }
        self.indent = outer;
    }

    /// Columns a list marker occupies, used to indent an item's later blocks.
    fn pending_indent(&self) -> usize {
        2
    }

    fn item_marker<'a>(
        &self,
        item: &'a AstNode<'a>,
        list_type: &ListType,
        ordinal: &mut usize,
    ) -> Vec<Span> {
        if let NodeValue::TaskItem(task) = &item.data.borrow().value {
            return match task.symbol {
                Some(_) => vec![Span::new("[x] ", self.theme.task_done)],
                None => vec![Span::new("[ ] ", self.theme.task_todo)],
            };
        }
        match list_type {
            ListType::Ordered => {
                let text = format!("{ordinal}. ");
                *ordinal += 1;
                vec![Span::new(text, self.theme.list_marker)]
            }
            _ => vec![Span::new("• ", self.theme.list_marker)],
        }
    }

    fn alert_style(&self, kind: AlertType) -> Style {
        let idx = match kind {
            AlertType::Note => 0,
            AlertType::Tip => 1,
            AlertType::Important => 2,
            AlertType::Warning => 3,
            AlertType::Caution => 4,
        };
        self.theme.alerts[idx]
    }

    fn table<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        alignments: &[TableAlignment],
        range: std::ops::Range<usize>,
    ) {
        let mut table = Table {
            align: alignments
                .iter()
                .map(|a| match a {
                    TableAlignment::Center => Align::Center,
                    TableAlignment::Right => Align::Right,
                    _ => Align::Left,
                })
                .collect(),
            ..Table::default()
        };

        for row in node.children() {
            let is_header = matches!(row.data.borrow().value, NodeValue::TableRow(true));
            let style = if is_header {
                self.theme.table_header
            } else {
                self.theme.body()
            };
            let cells: Vec<Vec<Span>> = row
                .children()
                .map(|cell| self.inlines(cell, style, None))
                .collect();
            if is_header {
                table.head = cells;
            } else {
                table.rows.push(cells);
            }
        }

        let mut block = Block::new(BlockKind::Table, range);
        block.table = Some(table);
        self.push(block);
    }

    fn inlines<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        style: Style,
        link: Option<LinkId>,
    ) -> Vec<Span> {
        let mut out = Vec::new();
        for child in node.children() {
            self.inline(child, style, link, &mut out);
        }
        merge_adjacent(out)
    }

    fn inline<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        style: Style,
        link: Option<LinkId>,
        out: &mut Vec<Span>,
    ) {
        let value = node.data.borrow().value.clone();
        let theme = self.theme;
        match value {
            NodeValue::Text(text) => out.push(Span::new(text.to_string(), style).with_link(link)),
            NodeValue::Raw(text) => out.push(Span::new(text, style).with_link(link)),
            NodeValue::EscapedTag(text) => {
                out.push(Span::new(text.to_string(), style).with_link(link));
            }
            NodeValue::Code(code) => {
                out.push(Span::new(code.literal.clone(), style.patch(theme.code)).with_link(link));
            }
            NodeValue::Math(math) => {
                out.push(Span::new(math.literal.clone(), style.patch(theme.code)).with_link(link));
            }
            NodeValue::HtmlInline(html) => {
                out.push(Span::new(html.clone(), style.patch(theme.dim)).with_link(link));
            }
            NodeValue::SoftBreak => {
                // A newline between two full-width characters is a wrapping
                // artefact of the source file, not a word space. Turning it
                // into a space puts a visible gap in the middle of a Japanese
                // sentence, which is why CJK text written at 80 columns looks
                // wrong in most viewers.
                let before = out.last().and_then(|s| s.text.chars().next_back());
                let after = node.next_sibling().and_then(first_char);
                let joined =
                    matches!((before, after), (Some(a), Some(b)) if is_wide(a) && is_wide(b));
                if !joined {
                    out.push(Span::new(" ", style).with_link(link));
                }
            }
            NodeValue::LineBreak => out.push(Span::new("\n", style)),
            NodeValue::Emph => self.children_inline(node, style.patch(theme.emph), link, out),
            NodeValue::Strong => self.children_inline(node, style.patch(theme.strong), link, out),
            NodeValue::Strikethrough => {
                self.children_inline(node, style.patch(theme.strikethrough), link, out);
            }
            NodeValue::Underline => {
                let s = style.patch(Style {
                    underline: true,
                    ..Style::PLAIN
                });
                self.children_inline(node, s, link, out);
            }
            NodeValue::SpoileredText => {
                let s = style.patch(Style {
                    reverse: true,
                    ..Style::PLAIN
                });
                self.children_inline(node, s, link, out);
            }
            NodeValue::Superscript => {
                out.push(Span::new("^", style.patch(theme.dim)));
                self.children_inline(node, style, link, out);
            }
            NodeValue::Subscript => {
                out.push(Span::new("_", style.patch(theme.dim)));
                self.children_inline(node, style, link, out);
            }
            NodeValue::Escaped | NodeValue::Subtext => self.children_inline(node, style, link, out),
            NodeValue::Link(target) => {
                let id = LinkId(self.doc.links.len());
                self.doc.links.push(Link {
                    url: target.url.clone(),
                    title: target.title.clone(),
                });
                let label_style = style.patch(theme.link);
                let before = out.len();
                self.children_inline(node, label_style, Some(id), out);
                if out.len() == before {
                    out.push(Span::new(target.url.clone(), label_style).with_link(Some(id)));
                }
            }
            NodeValue::WikiLink(target) => {
                let id = LinkId(self.doc.links.len());
                self.doc.links.push(Link {
                    url: target.url.clone(),
                    title: String::new(),
                });
                self.children_inline(node, style.patch(theme.link), Some(id), out);
            }
            NodeValue::Image(target) => {
                let alt = collect_text(node);
                self.doc.images.push(Image {
                    url: target.url.clone(),
                    alt: alt.clone(),
                    size: None,
                });
                let text = if alt.is_empty() {
                    target.url.clone()
                } else {
                    alt
                };
                out.push(Span::new(format!("[{text}]"), style.patch(theme.dim)));
            }
            NodeValue::FootnoteReference(footnote) => {
                out.push(Span::new(
                    format!("[^{}]", footnote.name),
                    style.patch(theme.footnote),
                ));
            }
            _ => self.children_inline(node, style, link, out),
        }
    }

    fn children_inline<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        style: Style,
        link: Option<LinkId>,
        out: &mut Vec<Span>,
    ) {
        for child in node.children() {
            self.inline(child, style, link, out);
        }
    }
}

fn alert_label(kind: AlertType) -> &'static str {
    match kind {
        AlertType::Note => "NOTE",
        AlertType::Tip => "TIP",
        AlertType::Important => "IMPORTANT",
        AlertType::Warning => "WARNING",
        AlertType::Caution => "CAUTION",
    }
}

/// Whether a character occupies two columns, which for this purpose means
/// "belongs to a script that does not separate words with spaces".
fn is_wide(c: char) -> bool {
    unicode_width::UnicodeWidthChar::width(c) == Some(2)
}

/// The first character any inline node will contribute.
fn first_char<'a>(node: &'a AstNode<'a>) -> Option<char> {
    match &node.data.borrow().value {
        NodeValue::Text(text) => text.chars().next(),
        NodeValue::Code(code) => code.literal.chars().next(),
        _ => node.children().find_map(first_char),
    }
}

fn sole_child<'a>(node: &'a AstNode<'a>) -> Option<&'a AstNode<'a>> {
    let mut it = node.children();
    let first = it.next()?;
    it.next().is_none().then_some(first)
}

fn collect_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut out = String::new();
    fn walk<'a>(node: &'a AstNode<'a>, out: &mut String) {
        match &node.data.borrow().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => {}
        }
        for child in node.children() {
            walk(child, out);
        }
    }
    for child in node.children() {
        walk(child, &mut out);
    }
    out
}

/// Collapse runs of spans that share a style and link. Emphasis nesting
/// produces a lot of one-character spans otherwise, and every downstream pass
/// walks this list.
fn merge_adjacent(spans: Vec<Span>) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.style == span.style && last.link == span.link => {
                last.text.push_str(&span.text);
            }
            _ => out.push(span),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Document {
        parse(src, &Theme::default())
    }

    #[test]
    fn headings_carry_their_level() {
        let d = doc("# One\n\n## Two\n");
        assert_eq!(d.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(d.blocks[1].kind, BlockKind::Heading(2));
        assert_eq!(d.blocks[0].text(), "One");
    }

    #[test]
    fn source_range_points_back_at_the_original_lines() {
        let src = "# Title\n\nA paragraph\nthat spans two lines.\n";
        let d = doc(src);
        assert_eq!(d.source_of(&d.blocks[0]), "# Title");
        assert_eq!(
            d.source_of(&d.blocks[1]),
            "A paragraph\nthat spans two lines."
        );
    }

    #[test]
    fn code_blocks_keep_their_language_and_literal_text() {
        let d = doc("```rust\nfn main() {}\n```\n");
        assert_eq!(
            d.blocks[0].kind,
            BlockKind::Code {
                lang: Some("rust".into())
            }
        );
        assert_eq!(d.blocks[0].text(), "fn main() {}");
    }

    #[test]
    fn emphasis_nests() {
        let d = doc("**bold *and italic* here**\n");
        let spans = &d.blocks[0].spans;
        let nested = spans
            .iter()
            .find(|s| s.text.contains("and italic"))
            .unwrap();
        assert!(nested.style.bold && nested.style.italic);
    }

    #[test]
    fn links_are_registered_and_referenced_by_their_spans() {
        let d = doc("See [the docs](https://example.com/docs).\n");
        assert_eq!(d.links.len(), 1);
        assert_eq!(d.links[0].url, "https://example.com/docs");
        let labelled = d.blocks[0]
            .spans
            .iter()
            .find(|s| s.link == Some(LinkId(0)))
            .unwrap();
        assert_eq!(labelled.text, "the docs");
    }

    #[test]
    fn autolinks_work_without_angle_brackets() {
        let d = doc("Visit https://example.com today.\n");
        assert_eq!(d.links.len(), 1);
        assert_eq!(d.links[0].url, "https://example.com");
    }

    #[test]
    fn strikethrough_is_recognised() {
        let d = doc("~~gone~~\n");
        assert!(d.blocks[0].spans.iter().any(|s| s.style.strikethrough));
    }

    #[test]
    fn task_list_items_get_checkbox_markers() {
        let d = doc("- [x] done\n- [ ] todo\n");
        let markers: Vec<String> = d
            .blocks
            .iter()
            .map(|b| b.prefix.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(markers, vec!["[x] ", "[ ] "]);
    }

    #[test]
    fn ordered_lists_number_from_the_declared_start() {
        let d = doc("3. three\n4. four\n");
        let markers: Vec<String> = d
            .blocks
            .iter()
            .map(|b| b.prefix.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(markers, vec!["3. ", "4. "]);
    }

    #[test]
    fn blockquotes_get_a_gutter_that_nests() {
        let d = doc("> outer\n>\n> > inner\n");
        assert_eq!(d.blocks[0].gutter.len(), 1);
        assert_eq!(d.blocks[1].gutter.len(), 2);
    }

    #[test]
    fn alerts_produce_a_labelled_title_block() {
        let d = doc("> [!WARNING]\n> Be careful.\n");
        assert_eq!(d.blocks[0].text(), "WARNING");
        assert_eq!(d.blocks[1].text(), "Be careful.");
        assert!(!d.blocks[0].gutter.is_empty());
    }

    #[test]
    fn tables_split_head_from_body_and_keep_alignment() {
        let d = doc("| a | b |\n| --- | ---: |\n| 1 | 2 |\n");
        let table = d.blocks[0].table.as_ref().unwrap();
        assert_eq!(table.head.len(), 2);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.align, vec![Align::Left, Align::Right]);
    }

    #[test]
    fn footnotes_become_references_and_a_definition_block() {
        let d = doc("Text[^1].\n\n[^1]: The note.\n");
        assert!(d.blocks[0].text().contains("[^1]"));
        let def = d.blocks.last().unwrap();
        assert!(def.prefix.iter().any(|s| s.text.contains("[^1]")));
    }

    #[test]
    fn a_paragraph_that_is_only_an_image_becomes_an_image_block() {
        let d = doc("![alt text](pic.png)\n");
        assert_eq!(d.blocks[0].kind, BlockKind::Image(ImageId(0)));
        assert_eq!(d.images[0].url, "pic.png");
    }

    #[test]
    fn soft_breaks_become_spaces_so_paragraphs_reflow() {
        let d = doc("one\ntwo\n");
        assert_eq!(d.blocks[0].text(), "one two");
    }

    #[test]
    fn a_soft_break_between_full_width_characters_adds_no_space() {
        let d = doc("日本語の文章が\n続きます。\n");
        assert_eq!(d.blocks[0].text(), "日本語の文章が続きます。");
    }

    #[test]
    fn a_soft_break_beside_latin_still_becomes_a_space() {
        assert_eq!(doc("日本語\ntext\n").blocks[0].text(), "日本語 text");
        assert_eq!(doc("text\n日本語\n").blocks[0].text(), "text 日本語");
    }

    #[test]
    fn a_soft_break_before_emphasis_looks_past_the_markup() {
        // The next node is Emph, not Text, so the lookahead has to descend.
        let d = doc("日本語が\n**続きます**。\n");
        assert_eq!(d.blocks[0].text(), "日本語が続きます。");
    }

    #[test]
    fn tight_lists_have_no_blank_line_between_items() {
        let d = doc("- one\n- two\n");
        assert!(!d.blocks[1].blank_before);
    }

    #[test]
    fn loose_lists_keep_the_blank_line() {
        let d = doc("- one\n\n- two\n");
        assert!(d.blocks[1].blank_before);
    }

    #[test]
    fn adjacent_spans_with_the_same_style_are_merged() {
        let d = doc("plain plain plain\n");
        assert_eq!(d.blocks[0].spans.len(), 1);
    }

    #[test]
    fn an_empty_document_has_no_blocks() {
        assert!(doc("").blocks.is_empty());
    }
}
