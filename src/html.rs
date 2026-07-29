//! A small, forgiving HTML parser.
//!
//! README files are full of HTML that Markdown has no syntax for — centred
//! logos, badge rows, `<details>` sections, `<sub>` captions. GitHub renders
//! all of it, so dumping the tags as source is the wrong answer.
//!
//! This is deliberately not a spec-conformant parser. It handles the subset
//! that appears in hand-written documents: well-nested tags, quoted or bare
//! attributes, void elements, comments, and the handful of entities people
//! actually type. Anything it cannot make sense of is dropped rather than
//! guessed at.

/// Elements that never have a closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose contents are not markup and must not be parsed as such.
const RAW_TEXT: &[&str] = &["script", "style"];

/// Whether an element never has a closing tag.
pub fn is_void(tag: &str) -> bool {
    VOID.contains(&tag)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Text(String),
    Element(Element),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// All text below this element, with markup removed.
    pub fn text(&self) -> String {
        let mut out = String::new();
        collect_text(&self.children, &mut out);
        out
    }
}

fn collect_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Element(element) => collect_text(&element.children, out),
        }
    }
}

pub fn parse(html: &str) -> Vec<Node> {
    let mut parser = Parser {
        input: html.as_bytes(),
        pos: 0,
        src: html,
    };
    let mut stack: Vec<Element> = Vec::new();
    let mut roots: Vec<Node> = Vec::new();

    while let Some(token) = parser.next_token() {
        match token {
            Token::Text(text) => {
                if text.is_empty() {
                    continue;
                }
                push_node(&mut stack, &mut roots, Node::Text(text));
            }
            Token::Open {
                tag,
                attrs,
                self_closing,
            } => {
                let element = Element {
                    tag: tag.clone(),
                    attrs,
                    children: Vec::new(),
                };
                if self_closing || VOID.contains(&tag.as_str()) {
                    push_node(&mut stack, &mut roots, Node::Element(element));
                } else if RAW_TEXT.contains(&tag.as_str()) {
                    // Consumed and discarded: a stylesheet is not document text.
                    parser.skip_raw_text(&tag);
                } else {
                    stack.push(element);
                }
            }
            Token::Close(tag) => {
                // Close the innermost matching element, discarding anything
                // left open inside it — the usual response to sloppy markup.
                if let Some(at) = stack.iter().rposition(|e| e.tag == tag) {
                    while stack.len() > at {
                        let element = stack.pop().expect("checked by rposition");
                        push_node(&mut stack, &mut roots, Node::Element(element));
                    }
                }
            }
        }
    }
    // Anything still open at the end is closed here.
    while let Some(element) = stack.pop() {
        push_node(&mut stack, &mut roots, Node::Element(element));
    }
    roots
}

fn push_node(stack: &mut [Element], roots: &mut Vec<Node>, node: Node) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => roots.push(node),
    }
}

enum Token {
    Text(String),
    Open {
        tag: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    Close(String),
}

struct Parser<'a> {
    input: &'a [u8],
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos.min(self.src.len())..].starts_with(s)
    }

    fn next_token(&mut self) -> Option<Token> {
        if self.pos >= self.input.len() {
            return None;
        }
        if self.peek() != Some(b'<') {
            return Some(Token::Text(self.read_text()));
        }
        if self.starts_with("<!--") {
            self.skip_to("-->", 3);
            return self.next_token();
        }
        if self.starts_with("<!") || self.starts_with("<?") {
            self.skip_to(">", 1);
            return self.next_token();
        }
        if self.starts_with("</") {
            self.pos += 2;
            let tag = self.read_name();
            self.skip_to(">", 1);
            return Some(Token::Close(tag));
        }
        // A `<` that does not begin a tag is just text.
        let after = self.input.get(self.pos + 1).copied();
        if !after.is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
            return Some(Token::Text("<".to_string()));
        }
        self.pos += 1;
        let tag = self.read_name();
        let attrs = self.read_attrs();
        let self_closing = self.starts_with("/>");
        self.skip_to(">", 1);
        Some(Token::Open {
            tag,
            attrs,
            self_closing,
        })
    }

    fn read_text(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.input.len() && self.input[self.pos] != b'<' {
            self.pos += 1;
        }
        decode_entities(&self.src[start..self.pos])
    }

    fn read_name(&mut self) -> String {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b':')
        {
            self.pos += 1;
        }
        self.src[start..self.pos].to_ascii_lowercase()
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn read_attrs(&mut self) -> Vec<(String, String)> {
        let mut attrs = Vec::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                None | Some(b'>') => break,
                Some(b'/') if self.starts_with("/>") => break,
                _ => {}
            }
            let name = self.read_name();
            if name.is_empty() {
                // Something unparseable; step over it rather than spinning.
                self.pos += 1;
                continue;
            }
            self.skip_whitespace();
            let value = if self.peek() == Some(b'=') {
                self.pos += 1;
                self.skip_whitespace();
                self.read_value()
            } else {
                String::new()
            };
            attrs.push((name, value));
        }
        attrs
    }

    fn read_value(&mut self) -> String {
        let quote = match self.peek() {
            Some(q @ (b'"' | b'\'')) => {
                self.pos += 1;
                Some(q)
            }
            _ => None,
        };
        let start = self.pos;
        match quote {
            Some(q) => {
                while self.peek().is_some_and(|c| c != q) {
                    self.pos += 1;
                }
                let value = &self.src[start..self.pos];
                self.pos += 1; // closing quote
                decode_entities(value)
            }
            None => {
                while self
                    .peek()
                    .is_some_and(|c| !c.is_ascii_whitespace() && c != b'>')
                {
                    self.pos += 1;
                }
                decode_entities(&self.src[start..self.pos])
            }
        }
    }

    /// Advance past the next occurrence of `needle`, or to the end.
    fn skip_to(&mut self, needle: &str, _min: usize) {
        match self.src[self.pos.min(self.src.len())..].find(needle) {
            Some(at) => self.pos += at + needle.len(),
            None => self.pos = self.input.len(),
        }
    }

    fn skip_raw_text(&mut self, tag: &str) {
        let close = format!("</{tag}");
        match self.src[self.pos.min(self.src.len())..]
            .to_ascii_lowercase()
            .find(&close)
        {
            Some(at) => {
                self.pos += at;
                self.skip_to(">", 1);
            }
            None => self.pos = self.input.len(),
        }
    }
}

/// Decode the entities people actually write by hand.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // A dozen *characters*, not a dozen bytes. An entity name is ASCII, so
        // the two agree wherever one really begins — but `&` is far more often
        // an ampersand than the start of anything, and a byte index counted
        // into the text that follows lands inside a character and panics.
        let Some(end) = rest
            .char_indices()
            .take(12)
            .find(|(_, c)| *c == ';')
            .map(|(at, _)| at)
        else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "trade" => Some('™'),
            _ => numeric_entity(name),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(nodes: &[Node]) -> &Element {
        match &nodes[0] {
            Node::Element(e) => e,
            other => panic!("expected an element, got {other:?}"),
        }
    }

    #[test]
    fn a_simple_element_parses() {
        let nodes = parse("<p>hello</p>");
        assert_eq!(nodes.len(), 1);
        let p = element(&nodes);
        assert_eq!(p.tag, "p");
        assert_eq!(p.text(), "hello");
    }

    #[test]
    fn tags_are_lowercased() {
        assert_eq!(element(&parse("<P>x</P>")).tag, "p");
    }

    #[test]
    fn attributes_parse_in_every_spelling() {
        let nodes = parse(r#"<img src="a.png" width=240 alt='A logo' hidden>"#);
        let img = element(&nodes);
        assert_eq!(img.attr("src"), Some("a.png"));
        assert_eq!(img.attr("width"), Some("240"));
        assert_eq!(img.attr("alt"), Some("A logo"));
        assert_eq!(img.attr("hidden"), Some(""));
        assert_eq!(img.attr("missing"), None);
    }

    #[test]
    fn void_elements_do_not_swallow_their_siblings() {
        let nodes = parse("<p>a<br>b</p>");
        let p = element(&nodes);
        assert_eq!(p.children.len(), 3);
        assert_eq!(p.text(), "ab");
    }

    #[test]
    fn self_closing_syntax_is_accepted() {
        let nodes = parse("<p><img src='x.png'/>after</p>");
        let p = element(&nodes);
        assert_eq!(p.children.len(), 2);
    }

    #[test]
    fn nesting_is_preserved() {
        let nodes = parse("<div><p>one</p><p>two</p></div>");
        let div = element(&nodes);
        assert_eq!(div.children.len(), 2);
        assert_eq!(div.text(), "onetwo");
    }

    #[test]
    fn comments_are_dropped() {
        let nodes = parse("<p>a<!-- a note -->b</p>");
        assert_eq!(element(&nodes).text(), "ab");
    }

    #[test]
    fn a_doctype_is_dropped() {
        let nodes = parse("<!DOCTYPE html><p>x</p>");
        assert_eq!(element(&nodes).tag, "p");
    }

    #[test]
    fn script_and_style_contents_never_reach_the_document() {
        let nodes = parse("<div><style>p { color: red }</style><p>text</p></div>");
        assert_eq!(element(&nodes).text(), "text");
        let nodes = parse("<div><script>if (a < b) {}</script>after</div>");
        assert_eq!(element(&nodes).text(), "after");
    }

    #[test]
    fn an_unclosed_tag_is_closed_at_the_end() {
        let nodes = parse("<p>text");
        assert_eq!(element(&nodes).text(), "text");
    }

    #[test]
    fn a_stray_close_tag_is_ignored() {
        let nodes = parse("<p>a</span>b</p>");
        assert_eq!(element(&nodes).text(), "ab");
    }

    #[test]
    fn mismatched_nesting_closes_the_inner_elements() {
        let nodes = parse("<div><p>text</div>");
        let div = element(&nodes);
        assert_eq!(div.tag, "div");
        assert_eq!(div.text(), "text");
    }

    #[test]
    fn a_bare_less_than_is_text() {
        let nodes = parse("<p>a &lt; b and 1<2</p>");
        assert_eq!(element(&nodes).text(), "a < b and 1<2");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(decode_entities("a&amp;b"), "a&b");
        assert_eq!(decode_entities("&lt;tag&gt;"), "<tag>");
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        assert_eq!(decode_entities("&mdash;"), "—");
        assert_eq!(decode_entities("nothing here"), "nothing here");
    }

    #[test]
    fn an_unknown_entity_is_left_alone() {
        assert_eq!(decode_entities("&frobnicate;"), "&frobnicate;");
        assert_eq!(decode_entities("A & B"), "A & B");
    }

    #[test]
    fn an_ampersand_before_a_multi_byte_character_is_left_alone() {
        // The `;` is looked for in the text just after the `&`, and stopping
        // that search at a byte offset lands inside 'の' here.
        assert_eq!(
            decode_entities("QuickCheck & 日本語のテスト"),
            "QuickCheck & 日本語のテスト"
        );
        assert_eq!(decode_entities("&あああああ"), "&あああああ");
        // Still decoded when the entity really is one, whatever follows it.
        assert_eq!(decode_entities("&amp;日本語"), "&日本語");
        assert_eq!(decode_entities("A &amp; B & 日"), "A & B & 日");
    }

    #[test]
    fn the_badge_row_from_a_real_readme_parses() {
        let html = r#"<p align="center">
  <a href="https://example.com/actions">
    <img alt="Build" src="https://img.shields.io/badge/build-passing-green">
  </a>
</p>"#;
        let nodes = parse(html);
        let p = element(&nodes);
        assert_eq!(p.tag, "p");
        assert_eq!(p.attr("align"), Some("center"));
        let link = p
            .children
            .iter()
            .find_map(|n| match n {
                Node::Element(e) if e.tag == "a" => Some(e),
                _ => None,
            })
            .expect("the link survived");
        assert_eq!(link.attr("href"), Some("https://example.com/actions"));
    }

    #[test]
    fn parsing_never_loops_on_malformed_input() {
        for input in [
            "<",
            "<<<",
            "< p >",
            "<p",
            "<p =",
            "</",
            "<!--",
            "<a href=\"",
        ] {
            let _ = parse(input);
        }
    }
}
