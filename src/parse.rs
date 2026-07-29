//! Markdown → [`Document`].
//!
//! comrak does the parsing, with the GitHub Flavored Markdown extensions turned
//! on. This module flattens its AST into the flat [`Block`] list that layout
//! consumes, carrying each node's `sourcepos` through as a `source_range` so a
//! yank can slice the original file rather than reconstruct it.

use crate::html::{Element as HtmlElement, Node as HtmlNode};
use crate::ir::*;
use crate::theme::Theme;
use crate::width::expand_tabs;
use comrak::nodes::{AlertType, AstNode, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};
use std::ops::Range;

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
        html_styles: Vec::new(),
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
    /// Open inline HTML tags inside the current paragraph. comrak hands each
    /// tag over as its own node, so the styling they imply has to be tracked
    /// across siblings.
    html_styles: Vec<(String, Style)>,
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
                // A paragraph that held nothing but HTML tags has nothing left
                // to show once they are interpreted.
                if spans.iter().all(|s| s.text.trim().is_empty()) {
                    return;
                }
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
                // GitHub renders this HTML, so showing the tags is the wrong
                // answer. Anything the subset cannot express falls back to
                // source, which is still better than nothing.
                let nodes = crate::html::parse(&html.literal);
                let before = self.doc.blocks.len();
                self.html_nodes(&nodes, &range, None);
                if self.doc.blocks.len() == before {
                    let literal = html.literal.trim_end_matches('\n');
                    if !literal.trim().is_empty() {
                        let mut block = Block::new(
                            BlockKind::Code {
                                lang: Some("html".into()),
                            },
                            range,
                        );
                        block.spans = vec![Span::new(literal.to_string(), self.theme.dim)];
                        self.push(block);
                    }
                }
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

    // ---- HTML ------------------------------------------------------------

    /// Render a run of HTML nodes, grouping consecutive inline content into
    /// paragraphs and recursing into block elements.
    fn html_nodes(&mut self, nodes: &[HtmlNode], range: &Range<usize>, align: Option<Align>) {
        let mut inline: Vec<Span> = Vec::new();
        for node in nodes {
            match node {
                HtmlNode::Element(element) if is_html_block(&element.tag) => {
                    self.flush_html_inline(&mut inline, range, align);
                    self.html_block(element, range, align);
                }
                other => self.html_inline(other, self.theme.body(), None, &mut inline),
            }
        }
        self.flush_html_inline(&mut inline, range, align);
    }

    fn flush_html_inline(
        &mut self,
        inline: &mut Vec<Span>,
        range: &Range<usize>,
        align: Option<Align>,
    ) {
        let spans = trim_html_spans(std::mem::take(inline));
        if spans.is_empty() {
            return;
        }
        let mut block = Block::new(BlockKind::Para, range.clone());
        block.spans = merge_adjacent(spans);
        block.align = align;
        self.push(block);
    }

    fn html_block(
        &mut self,
        element: &HtmlElement,
        range: &Range<usize>,
        inherited: Option<Align>,
    ) {
        let align = html_align(element).or(inherited);
        match element.tag.as_str() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = element.tag[1..].parse::<u8>().unwrap_or(1);
                let mut spans = Vec::new();
                for child in &element.children {
                    self.html_inline(child, self.theme.heading(level), None, &mut spans);
                }
                let spans = trim_html_spans(spans);
                if spans.is_empty() {
                    return;
                }
                let mut block = Block::new(BlockKind::Heading(level), range.clone());
                block.spans = merge_adjacent(spans);
                block.align = align;
                self.push(block);
            }

            "hr" => {
                let mut block = Block::new(BlockKind::Rule, range.clone());
                block.spans = vec![Span::new("─", self.theme.rule)];
                self.push(block);
            }

            "pre" => {
                let literal = element.text();
                let literal = literal.trim_matches('\n');
                if literal.trim().is_empty() {
                    return;
                }
                let mut block = Block::new(BlockKind::Code { lang: None }, range.clone());
                block.spans = vec![Span::new(literal.to_string(), self.theme.body())];
                self.push(block);
            }

            "blockquote" => {
                self.gutter.push(Span::new("│ ", self.theme.quote_bar));
                self.html_nodes(&element.children, range, align);
                self.gutter.pop();
            }

            "ul" | "ol" => {
                let ordered = element.tag == "ol";
                let mut ordinal: usize = element
                    .attr("start")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                let outer_tight = self.tight;
                self.tight = true;
                for child in element_children(element, "li") {
                    let marker = if ordered {
                        let text = format!("{ordinal}. ");
                        ordinal += 1;
                        Span::new(text, self.theme.list_marker)
                    } else {
                        Span::new("• ", self.theme.list_marker)
                    };
                    self.pending_prefix = Some(vec![marker]);
                    self.html_nodes(&child.children, range, align);
                    self.pending_prefix = None;
                }
                self.tight = outer_tight;
                self.pending_blank = true;
            }

            "li" => self.html_nodes(&element.children, range, align),

            "table" => self.html_table(element, range),

            // A `<details>` cannot fold in a viewer, so the summary becomes a
            // heading-ish line and the contents follow it.
            "details" => {
                if let Some(summary) = element_children(element, "summary").next() {
                    let mut spans = Vec::new();
                    for child in &summary.children {
                        self.html_inline(child, self.theme.strong, None, &mut spans);
                    }
                    let spans = trim_html_spans(spans);
                    if !spans.is_empty() {
                        let mut block = Block::new(BlockKind::Para, range.clone());
                        block.spans = merge_adjacent(spans);
                        self.push(block);
                    }
                }
                let rest: Vec<HtmlNode> = element
                    .children
                    .iter()
                    .filter(|n| !matches!(n, HtmlNode::Element(e) if e.tag == "summary"))
                    .cloned()
                    .collect();
                self.html_nodes(&rest, range, align);
            }

            // A generic container: a picture on its own becomes a figure,
            // anything else is just its contents.
            _ => match sole_image(&element.children) {
                Some(img) => self.html_image_block(img, range, align),
                None => self.html_nodes(&element.children, range, align),
            },
        }
    }

    fn html_image_block(&mut self, img: &HtmlElement, range: &Range<usize>, align: Option<Align>) {
        let url = img.attr("src").unwrap_or_default().to_string();
        let alt = img.attr("alt").unwrap_or_default().to_string();
        let id = ImageId(self.doc.images.len());
        self.doc.images.push(Image {
            url: url.clone(),
            alt: alt.clone(),
            size: None,
        });
        let mut block = Block::new(BlockKind::Image(id), range.clone());
        block.spans = vec![Span::new(
            if alt.is_empty() { url } else { alt },
            self.theme.dim,
        )];
        block.align = align;
        self.push(block);
    }

    fn html_table(&mut self, element: &HtmlElement, range: &Range<usize>) {
        let mut table = Table::default();
        let mut rows: Vec<&HtmlElement> = Vec::new();
        collect_rows(element, &mut rows);

        for row in rows {
            let mut header = false;
            let cells: Vec<Vec<Span>> = row
                .children
                .iter()
                .filter_map(|n| match n {
                    HtmlNode::Element(e) if e.tag == "td" || e.tag == "th" => Some(e),
                    _ => None,
                })
                .map(|cell| {
                    if cell.tag == "th" {
                        header = true;
                    }
                    let style = if cell.tag == "th" {
                        self.theme.table_header
                    } else {
                        self.theme.body()
                    };
                    let mut spans = Vec::new();
                    for child in &cell.children {
                        self.html_inline(child, style, None, &mut spans);
                    }
                    merge_adjacent(trim_html_spans(spans))
                })
                .collect();
            if cells.is_empty() {
                continue;
            }
            if header && table.head.is_empty() {
                table.head = cells;
            } else {
                table.rows.push(cells);
            }
        }
        let columns = table
            .head
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if columns == 0 {
            return;
        }
        table.align = vec![Align::Left; columns];
        let mut block = Block::new(BlockKind::Table, range.clone());
        block.table = Some(table);
        self.push(block);
    }

    fn html_inline(
        &mut self,
        node: &HtmlNode,
        style: Style,
        link: Option<LinkId>,
        out: &mut Vec<Span>,
    ) {
        let element = match node {
            HtmlNode::Text(text) => {
                let mut text = collapse_whitespace(text);
                // Two adjacent runs of whitespace across an element boundary
                // are still one space.
                if text.starts_with(' ')
                    && out.last().is_some_and(|s| s.text.ends_with([' ', '\n']))
                {
                    text.remove(0);
                }
                if !text.is_empty() {
                    out.push(Span::new(text, style).with_link(link));
                }
                return;
            }
            HtmlNode::Element(element) => element,
        };

        let theme = self.theme;
        let children = |me: &mut Self, style: Style, link, out: &mut Vec<Span>| {
            for child in &element.children {
                me.html_inline(child, style, link, out);
            }
        };

        match element.tag.as_str() {
            "br" => {
                // Six consecutive <br>s are a spacing hack that costs six rows
                // in a terminal; one blank line carries the same meaning.
                if !out.iter().rev().take(2).all(|s| s.text == "\n") {
                    out.push(Span::new("\n", style));
                }
            }
            "a" => {
                let Some(href) = element.attr("href") else {
                    return children(self, style, link, out);
                };
                let id = LinkId(self.doc.links.len());
                self.doc.links.push(Link {
                    url: href.to_string(),
                    title: String::new(),
                });
                let before = out.len();
                children(self, style.patch(theme.link), Some(id), out);
                if out.len() == before {
                    out.push(
                        Span::new(href.to_string(), style.patch(theme.link)).with_link(Some(id)),
                    );
                }
                // Source indentation inside the anchor is not part of the link
                // text, and underlining it looks like a mistake.
                unlink_edge_whitespace(out, before, style);
            }
            "img" => {
                let url = element.attr("src").unwrap_or_default().to_string();
                let alt = element.attr("alt").unwrap_or_default().to_string();
                self.doc.images.push(Image {
                    url: url.clone(),
                    alt: alt.clone(),
                    size: None,
                });
                let label = if alt.is_empty() { url } else { alt };
                if !label.is_empty() {
                    out.push(
                        Span::new(format!("[{label}]"), style.patch(theme.dim)).with_link(link),
                    );
                }
            }
            // A <picture> is a set of alternatives; the <img> is the one every
            // browser falls back to.
            "picture" => match element_children(element, "img").next() {
                Some(img) => self.html_inline(&HtmlNode::Element(img.clone()), style, link, out),
                None => children(self, style, link, out),
            },
            "strong" | "b" => children(self, style.patch(theme.strong), link, out),
            "em" | "i" | "cite" | "var" => children(self, style.patch(theme.emph), link, out),
            "code" | "kbd" | "samp" | "tt" => children(self, style.patch(theme.code), link, out),
            "del" | "s" | "strike" => children(self, style.patch(theme.strikethrough), link, out),
            "ins" | "u" => children(
                self,
                style.patch(Style {
                    underline: true,
                    ..Style::PLAIN
                }),
                link,
                out,
            ),
            "mark" => children(
                self,
                style.patch(Style {
                    reverse: true,
                    ..Style::PLAIN
                }),
                link,
                out,
            ),
            "small" | "sub" | "sup" => children(self, style.patch(theme.dim), link, out),
            _ => children(self, style, link, out),
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
        // Tags left open by one paragraph must not bleed into the next.
        self.html_styles.clear();
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
            NodeValue::Text(text) => {
                let style = style.patch(self.html_style());
                out.push(Span::new(text.to_string(), style).with_link(link));
            }
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
            NodeValue::HtmlInline(html) => self.html_tag(&html, style, link, out),
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

    /// The combined effect of the inline HTML tags currently open.
    fn html_style(&self) -> Style {
        self.html_styles
            .iter()
            .fold(Style::PLAIN, |acc, (_, style)| acc.patch(*style))
    }

    /// One inline HTML tag, as comrak hands it over.
    ///
    /// Showing `<sub>` as literal text is never what the author meant. Tags
    /// with a terminal equivalent become styling; `<br>` and `<img>` become
    /// content; everything else is dropped.
    fn html_tag(&mut self, raw: &str, style: Style, link: Option<LinkId>, out: &mut Vec<Span>) {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("</") {
            let name = rest.trim_end_matches('>').trim().to_ascii_lowercase();
            if let Some(at) = self.html_styles.iter().rposition(|(tag, _)| *tag == name) {
                self.html_styles.truncate(at);
            }
            return;
        }
        let nodes = crate::html::parse(trimmed);
        let Some(HtmlNode::Element(element)) = nodes.first() else {
            return;
        };
        match element.tag.as_str() {
            "br" => out.push(Span::new("\n", style)),
            "img" => {
                let url = element.attr("src").unwrap_or_default().to_string();
                let alt = element.attr("alt").unwrap_or_default().to_string();
                self.doc.images.push(Image {
                    url: url.clone(),
                    alt: alt.clone(),
                    size: None,
                });
                let label = if alt.is_empty() { url } else { alt };
                if !label.is_empty() {
                    out.push(
                        Span::new(format!("[{label}]"), style.patch(self.theme.dim))
                            .with_link(link),
                    );
                }
            }
            // Self-closing or void: nothing to keep open.
            tag if trimmed.ends_with("/>") || crate::html::is_void(tag) => {}
            tag => {
                if let Some(style) = self.html_tag_style(tag) {
                    self.html_styles.push((tag.to_string(), style));
                }
            }
        }
    }

    fn html_tag_style(&self, tag: &str) -> Option<Style> {
        let theme = self.theme;
        Some(match tag {
            "strong" | "b" => theme.strong,
            "em" | "i" | "cite" | "var" => theme.emph,
            "code" | "kbd" | "samp" | "tt" => theme.code,
            "del" | "s" | "strike" => theme.strikethrough,
            "small" | "sub" | "sup" => theme.dim,
            "ins" | "u" => Style {
                underline: true,
                ..Style::PLAIN
            },
            "mark" => Style {
                reverse: true,
                ..Style::PLAIN
            },
            // Not a styling tag, but still tracked so its close tag pairs up.
            _ => Style::PLAIN,
        })
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

/// Move whitespace at the edges of a link's text back outside the link.
fn unlink_edge_whitespace(out: &mut [Span], from: usize, outer: Style) {
    let blank = |span: &Span| span.text.chars().all(char::is_whitespace);
    let tail = &mut out[from..];
    if let Some(first) = tail.first_mut()
        && blank(first)
    {
        first.style = outer;
        first.link = None;
    }
    if let Some(last) = tail.last_mut()
        && blank(last)
    {
        last.style = outer;
        last.link = None;
    }
}

/// Tags that start a new block rather than flowing inline.
fn is_html_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "li"
            | "blockquote"
            | "pre"
            | "hr"
            | "table"
            | "details"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "aside"
            | "nav"
            | "figure"
            | "figcaption"
            | "center"
            | "dl"
            | "dt"
            | "dd"
    )
}

fn html_align(element: &HtmlElement) -> Option<Align> {
    let named = element.attr("align").map(str::to_ascii_lowercase);
    let styled = element.attr("style").map(str::to_ascii_lowercase);
    let from_style = styled.and_then(|s| {
        let at = s.find("text-align")?;
        let value = s[at..]
            .split(':')
            .nth(1)?
            .trim()
            .trim_end_matches(';')
            .to_string();
        Some(value)
    });
    match named.or(from_style)?.as_str() {
        "center" | "centre" => Some(Align::Center),
        "right" | "end" => Some(Align::Right),
        "left" | "start" => Some(Align::Left),
        _ => None,
    }
}

fn element_children<'a>(
    element: &'a HtmlElement,
    tag: &'a str,
) -> impl Iterator<Item = &'a HtmlElement> {
    element.children.iter().filter_map(move |n| match n {
        HtmlNode::Element(e) if e.tag == tag => Some(e),
        _ => None,
    })
}

fn collect_rows<'a>(element: &'a HtmlElement, out: &mut Vec<&'a HtmlElement>) {
    for child in &element.children {
        if let HtmlNode::Element(e) = child {
            match e.tag.as_str() {
                "tr" => out.push(e),
                "thead" | "tbody" | "tfoot" => collect_rows(e, out),
                _ => {}
            }
        }
    }
}

/// The single image a container holds, if that is all it holds.
///
/// A logo wrapped in a link inside a centred paragraph is still just a
/// picture, and should be drawn as one.
fn sole_image(nodes: &[HtmlNode]) -> Option<&HtmlElement> {
    let mut found = None;
    for node in nodes {
        match node {
            HtmlNode::Text(text) if text.trim().is_empty() => {}
            HtmlNode::Text(_) => return None,
            HtmlNode::Element(e) => match e.tag.as_str() {
                "img" if found.is_none() => found = Some(e),
                "br" | "source" => {}
                "a" | "p" | "div" | "picture" | "span" | "figure" => {
                    let inner = sole_image(&e.children)?;
                    if found.is_some() {
                        return None;
                    }
                    found = Some(inner);
                }
                _ => return None,
            },
        }
    }
    found
}

/// Collapse runs of whitespace, the way HTML does. Source indentation inside a
/// `<p>` is layout, not content.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        // Leading whitespace is kept as a single space: the newline after
        // `</strong>` is what separates it from the next word. Spaces left
        // stranded at the edges of a block are trimmed later.
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if space {
        out.push(' ');
    }
    out
}

/// Drop leading and trailing blank spans, including the line breaks `<br>`
/// leaves at the edges of a block.
fn trim_html_spans(mut spans: Vec<Span>) -> Vec<Span> {
    while spans
        .first()
        .is_some_and(|s| s.text.trim_matches([' ', '\n']).is_empty())
    {
        spans.remove(0);
    }
    while spans
        .last()
        .is_some_and(|s| s.text.trim_matches([' ', '\n']).is_empty())
    {
        spans.pop();
    }
    if let Some(first) = spans.first_mut() {
        let trimmed = first.text.trim_start_matches([' ', '\n']).to_string();
        first.text = trimmed;
    }
    if let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end_matches([' ', '\n']).to_string();
        last.text = trimmed;
    }
    spans.retain(|s| !s.text.is_empty());
    spans
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
    fn an_html_paragraph_becomes_a_paragraph() {
        let d = doc("<p>Hello <b>world</b>.</p>\n");
        assert_eq!(d.blocks.len(), 1);
        assert_eq!(d.blocks[0].kind, BlockKind::Para);
        assert_eq!(d.blocks[0].text(), "Hello world.");
        assert!(d.blocks[0].spans.iter().any(|s| s.style.bold));
    }

    #[test]
    fn html_alignment_is_carried_onto_the_block() {
        let d = doc("<p align=\"center\">middle</p>\n");
        assert_eq!(d.blocks[0].align, Some(Align::Center));
        let d = doc("<p style=\"text-align: right\">end</p>\n");
        assert_eq!(d.blocks[0].align, Some(Align::Right));
    }

    #[test]
    fn alignment_is_inherited_by_nested_blocks() {
        let d = doc("<div align=\"center\"><h2>Title</h2><p>body</p></div>\n");
        assert!(d.blocks.iter().all(|b| b.align == Some(Align::Center)));
    }

    #[test]
    fn html_headings_keep_their_level() {
        let d = doc("<h3>Deep</h3>\n");
        assert_eq!(d.blocks[0].kind, BlockKind::Heading(3));
        assert_eq!(d.blocks[0].text(), "Deep");
    }

    #[test]
    fn an_html_link_is_registered_like_a_markdown_one() {
        let d = doc("<p>see <a href=\"https://example.com\">docs</a></p>\n");
        assert_eq!(d.links[0].url, "https://example.com");
        assert!(
            d.blocks[0]
                .spans
                .iter()
                .any(|s| s.link == Some(LinkId(0)) && s.text == "docs")
        );
    }

    #[test]
    fn a_lone_html_image_becomes_a_figure() {
        let d = doc("<p align=\"center\"><img src=\"logo.png\" alt=\"Logo\"></p>\n");
        assert_eq!(d.blocks[0].kind, BlockKind::Image(ImageId(0)));
        assert_eq!(d.images[0].url, "logo.png");
        assert_eq!(d.blocks[0].align, Some(Align::Center));
    }

    #[test]
    fn a_logo_wrapped_in_a_link_and_a_picture_is_still_a_figure() {
        let d = doc(
            "<p align=\"center\"><a href=\"https://example.com\"><picture>\
             <source srcset=\"dark.svg\"><img alt=\"Dewy\" src=\"light.svg\"></picture></a></p>\n",
        );
        assert_eq!(d.blocks[0].kind, BlockKind::Image(ImageId(0)));
        assert_eq!(d.images[0].url, "light.svg");
    }

    #[test]
    fn a_badge_row_keeps_each_badge_separate() {
        let d = doc(
            "<p>\n  <a href=\"https://a.example\"><img alt=\"Build\" src=\"b.svg\"></a>\n\
             \x20 <a href=\"https://c.example\"><img alt=\"Release\" src=\"d.svg\"></a>\n</p>\n",
        );
        assert_eq!(d.blocks[0].text(), "[Build] [Release]");
        assert_eq!(d.links.len(), 2);
    }

    #[test]
    fn whitespace_inside_a_link_is_not_part_of_the_link() {
        let d = doc("<p>\n  <a href=\"https://a.example\">\n    <b>text</b>\n  </a>\n</p>\n");
        for span in &d.blocks[0].spans {
            if span.text.trim().is_empty() {
                assert!(span.link.is_none(), "{span:?}");
                assert!(!span.style.underline, "{span:?}");
            }
        }
    }

    #[test]
    fn an_html_table_becomes_a_table() {
        let d = doc("<table><tr><th>Name</th><th>Type</th></tr>\
             <tr><td>alpha</td><td>string</td></tr></table>\n");
        let table = d.blocks[0].table.as_ref().unwrap();
        assert_eq!(table.head.len(), 2);
        assert_eq!(table.rows.len(), 1);
    }

    #[test]
    fn an_html_list_gets_markers() {
        let d = doc("<ul><li>one</li><li>two</li></ul>\n");
        assert_eq!(d.blocks.len(), 2);
        assert!(d.blocks[0].prefix.iter().any(|s| s.text.contains('•')));
        let d = doc("<ol start=\"3\"><li>three</li></ol>\n");
        assert!(d.blocks[0].prefix.iter().any(|s| s.text.starts_with('3')));
    }

    #[test]
    fn details_shows_its_summary_and_its_contents() {
        let d = doc("<details><summary>More</summary><p>Hidden</p></details>\n");
        let text: String = d.blocks.iter().map(|b| b.text()).collect();
        assert!(text.contains("More") && text.contains("Hidden"));
    }

    #[test]
    fn a_run_of_br_tags_collapses_to_one_blank_line() {
        let d = doc("<p>a<br><br><br><br>b</p>\n");
        assert_eq!(d.blocks[0].text(), "a\n\nb");
    }

    #[test]
    fn html_whitespace_collapses_but_word_gaps_survive() {
        let d = doc("<p>\n  <b>Dewy</b> enables\n  deployment.\n</p>\n");
        assert_eq!(d.blocks[0].text(), "Dewy enables deployment.");
    }

    #[test]
    fn inline_html_tags_style_the_text_rather_than_showing_themselves() {
        let d = doc("Press <kbd>C</kbd> and H<sub>2</sub>O.\n");
        let text = d.blocks[0].text();
        assert_eq!(text, "Press C and H2O.");
        assert!(!text.contains('<'), "tags must not be printed");
    }

    #[test]
    fn an_inline_br_breaks_the_line() {
        let d = doc("one<br>two\n");
        assert_eq!(d.blocks[0].text(), "one\ntwo");
    }

    #[test]
    fn an_unclosed_inline_tag_does_not_leak_into_the_next_paragraph() {
        let d = doc("<b>bold\n\nplain text\n");
        assert!(d.blocks[1].spans.iter().all(|s| !s.style.bold));
    }

    #[test]
    fn an_html_block_that_carries_no_content_falls_back_to_source() {
        // Nothing renderable came out, so showing the markup beats showing
        // nothing at all.
        let d = doc("<div>\n</div>\n");
        assert!(matches!(d.blocks[0].kind, BlockKind::Code { .. }));
    }

    #[test]
    fn a_paragraph_of_nothing_but_tags_disappears() {
        let d = doc("<custom-element data-x=\"1\"></custom-element>\n");
        assert!(d.blocks.is_empty(), "{:#?}", d.blocks);
    }

    #[test]
    fn an_empty_document_has_no_blocks() {
        assert!(doc("").blocks.is_empty());
    }
}
