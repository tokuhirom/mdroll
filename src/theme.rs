//! Themes.
//!
//! Two color systems are in play: the UI palette defined here, and syntect's
//! `.tmTheme` files used for code block highlighting. A theme names both.
//!
//! Bundled themes are embedded at build time. Additional themes are loaded from
//! `~/.config/mdroll/themes/*.toml`.

use crate::ir::{Color, Style};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Themes compiled into the binary.
pub const BUNDLED: &[(&str, &str)] = &[
    ("terminal", include_str!("../themes/terminal.toml")),
    ("dracula", include_str!("../themes/dracula.toml")),
    (
        "solarized-dark",
        include_str!("../themes/solarized-dark.toml"),
    ),
    (
        "solarized-light",
        include_str!("../themes/solarized-light.toml"),
    ),
    ("nord", include_str!("../themes/nord.toml")),
    ("gruvbox", include_str!("../themes/gruvbox.toml")),
];

pub const DEFAULT_THEME: &str = "terminal";

/// A partial style as written in TOML: `{ fg = "#8be9fd", underline = true }`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleSpec {
    pub fg: Option<String>,
    pub bg: Option<String>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strikethrough: bool,
    #[serde(default)]
    pub dim: bool,
    #[serde(default)]
    pub reverse: bool,
}

impl StyleSpec {
    fn resolve(&self) -> Result<Style> {
        Ok(Style {
            fg: self.fg.as_deref().map(parse_color).transpose()?,
            bg: self.bg.as_deref().map(parse_color).transpose()?,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            dim: self.dim,
            reverse: self.reverse,
        })
    }
}

/// A heading decoration as written in TOML: `border = true`, `border = false`,
/// or `border = "#6272a4"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DecorSpec {
    /// Draw it, or do not. Drawn without a color of its own, it takes the
    /// heading's, dimmed.
    On(bool),
    Color(String),
}

/// A heading's style, plus the two decorations only a heading has.
///
/// The style fields are [`StyleSpec`]'s, written out again rather than
/// flattened: `deny_unknown_fields` and `serde(flatten)` do not work together,
/// and being told about a misspelled attribute is worth more than the ten lines.
/// Keeping `border` and `bar` here rather than on `StyleSpec` means they are
/// legal only where they mean something — `link = { border = … }` is refused.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadingSpec {
    pub fg: Option<String>,
    pub bg: Option<String>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strikethrough: bool,
    #[serde(default)]
    pub dim: bool,
    #[serde(default)]
    pub reverse: bool,
    /// A rule under the heading, the way GitHub borders `h1` and `h2`.
    pub border: Option<DecorSpec>,
    /// A bar down its left side.
    pub bar: Option<DecorSpec>,
}

impl HeadingSpec {
    fn style(&self) -> StyleSpec {
        StyleSpec {
            fg: self.fg.clone(),
            bg: self.bg.clone(),
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            dim: self.dim,
            reverse: self.reverse,
        }
    }
}

/// A decoration that is drawn, and the color to draw it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoration {
    /// `None` means the heading's own color, dimmed. Derived rather than
    /// written down so a theme that predates the feature — which is every theme
    /// a user already has — still shows it.
    pub color: Option<Color>,
}

fn decoration(spec: Option<&DecorSpec>, current: Option<Decoration>) -> Result<Option<Decoration>> {
    match spec {
        None => Ok(current),
        Some(DecorSpec::On(false)) => Ok(None),
        Some(DecorSpec::On(true)) => Ok(Some(Decoration { color: None })),
        Some(DecorSpec::Color(s)) => Ok(Some(Decoration {
            color: Some(parse_color(s)?),
        })),
    }
}

/// Parse `#rrggbb`, `#rgb`, a named ANSI color, or a 0-255 palette index.
pub fn parse_color(s: &str) -> Result<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let (r, g, b) = match hex.len() {
            3 => {
                let v = u32::from_str_radix(hex, 16).context("bad hex color")?;
                let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
                ((r * 17) as u8, (g * 17) as u8, (b * 17) as u8)
            }
            6 => {
                let v = u32::from_str_radix(hex, 16).context("bad hex color")?;
                (
                    ((v >> 16) & 0xff) as u8,
                    ((v >> 8) & 0xff) as u8,
                    (v & 0xff) as u8,
                )
            }
            _ => bail!("color {s:?} must be #rgb or #rrggbb"),
        };
        return Ok(Color::Rgb { r, g, b });
    }
    if let Ok(n) = s.parse::<u8>() {
        return Ok(Color::AnsiValue(n));
    }
    let color = match s.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "black" => Color::Black,
        "red" => Color::DarkRed,
        "green" => Color::DarkGreen,
        "yellow" => Color::DarkYellow,
        "blue" => Color::DarkBlue,
        "magenta" | "purple" => Color::DarkMagenta,
        "cyan" => Color::DarkCyan,
        "white" | "grey" | "gray" => Color::Grey,
        "brightblack" | "darkgrey" | "darkgray" => Color::DarkGrey,
        "brightred" => Color::Red,
        "brightgreen" => Color::Green,
        "brightyellow" => Color::Yellow,
        "brightblue" => Color::Blue,
        "brightmagenta" => Color::Magenta,
        "brightcyan" => Color::Cyan,
        "brightwhite" => Color::White,
        "reset" | "default" | "none" => Color::Reset,
        _ => bail!("unknown color {s:?}"),
    };
    Ok(color)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: Option<String>,
    foreground: Option<String>,
    background: Option<String>,
    #[serde(default)]
    code: BTreeMap<String, toml::Value>,
    #[serde(default)]
    heading: BTreeMap<String, HeadingSpec>,
    #[serde(default)]
    inline: BTreeMap<String, StyleSpec>,
    #[serde(default)]
    block: BTreeMap<String, StyleSpec>,
    #[serde(default)]
    table: BTreeMap<String, StyleSpec>,
    #[serde(default)]
    alert: BTreeMap<String, StyleSpec>,
    #[serde(default)]
    ui: BTreeMap<String, StyleSpec>,
}

/// The resolved palette handed to the parser and the renderer.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub syntect_theme: String,

    pub headings: [Style; 6],
    /// A rule under each heading level, or `None` where none is drawn. Only the
    /// levels drawn large carry one by default, which is the pair GitHub gives
    /// a bottom border.
    pub heading_border: [Option<Decoration>; 6],
    /// A bar down the left of each heading level. GitHub draws no such thing,
    /// so nothing is on unless a theme asks for it.
    pub heading_bar: [Option<Decoration>; 6],

    pub link: Style,
    pub code: Style,
    pub emph: Style,
    pub strong: Style,
    pub strikethrough: Style,
    pub footnote: Style,

    pub quote_bar: Style,
    pub quote: Style,
    pub rule: Style,
    pub list_marker: Style,
    pub task_done: Style,
    pub task_todo: Style,
    pub code_fence: Style,

    pub table_border: Style,
    pub table_header: Style,

    pub alerts: [Style; 5],

    pub status: Style,
    pub toast: Style,
    pub cursor: Style,
    pub search_match: Style,
    pub search_current: Style,
    pub hint: Style,
    pub dim: Style,
}

impl Default for Theme {
    /// The `terminal` fallback: no background, inherit whatever the terminal
    /// already uses. Named themes paint backgrounds only when selected.
    fn default() -> Theme {
        let bold = Style {
            bold: true,
            ..Style::PLAIN
        };
        Theme {
            name: DEFAULT_THEME.into(),
            foreground: None,
            background: None,
            syntect_theme: "base16-ocean.dark".into(),
            headings: [bold; 6],
            heading_border: [
                Some(Decoration { color: None }),
                Some(Decoration { color: None }),
                None,
                None,
                None,
                None,
            ],
            heading_bar: [None; 6],
            link: Style {
                underline: true,
                ..Style::PLAIN
            },
            code: Style::PLAIN,
            emph: Style {
                italic: true,
                ..Style::PLAIN
            },
            strong: bold,
            strikethrough: Style {
                strikethrough: true,
                ..Style::PLAIN
            },
            footnote: Style::PLAIN,
            quote_bar: Style::PLAIN,
            quote: Style {
                italic: true,
                ..Style::PLAIN
            },
            rule: Style::PLAIN,
            list_marker: Style::PLAIN,
            task_done: Style::PLAIN,
            task_todo: Style::PLAIN,
            code_fence: Style {
                dim: true,
                ..Style::PLAIN
            },
            table_border: Style::PLAIN,
            table_header: bold,
            alerts: [bold; 5],
            status: Style {
                reverse: true,
                ..Style::PLAIN
            },
            toast: Style {
                reverse: true,
                ..Style::PLAIN
            },
            cursor: Style {
                reverse: true,
                ..Style::PLAIN
            },
            search_match: Style {
                reverse: true,
                ..Style::PLAIN
            },
            search_current: Style {
                reverse: true,
                bold: true,
                ..Style::PLAIN
            },
            hint: Style {
                reverse: true,
                bold: true,
                ..Style::PLAIN
            },
            dim: Style {
                dim: true,
                ..Style::PLAIN
            },
        }
    }
}

fn take(map: &BTreeMap<String, StyleSpec>, key: &str, fallback: Style) -> Result<Style> {
    match map.get(key) {
        Some(spec) => Ok(fallback.patch(spec.resolve()?)),
        None => Ok(fallback),
    }
}

impl Theme {
    pub fn parse(text: &str) -> Result<Theme> {
        let file: ThemeFile = toml::from_str(text).context("theme is not valid TOML")?;
        let mut t = Theme::default();
        if let Some(name) = file.name {
            t.name = name;
        }
        t.foreground = file.foreground.as_deref().map(parse_color).transpose()?;
        t.background = file.background.as_deref().map(parse_color).transpose()?;

        if let Some(v) = file.code.get("syntect_theme").and_then(|v| v.as_str()) {
            t.syntect_theme = v.to_string();
        }
        if let Some(v) = file.code.get("fence")
            && let Ok(spec) = v.clone().try_into::<StyleSpec>()
        {
            t.code_fence = t.code_fence.patch(spec.resolve()?);
        }

        for (i, key) in ["h1", "h2", "h3", "h4", "h5", "h6"].iter().enumerate() {
            // Unspecified levels inherit the deepest level that was given, so a
            // theme defining only h1..h3 still styles h4..h6 sensibly.
            let fallback = if i == 0 {
                t.headings[0]
            } else {
                t.headings[i - 1]
            };
            let spec = file.heading.get(*key);
            t.headings[i] = match spec {
                Some(spec) => fallback.patch(spec.style().resolve()?),
                None => fallback,
            };
            // Decoration does not inherit the way style does. Carrying `h2`'s
            // border down would put one under every level of a theme that
            // named only the first two, which is the opposite of what asking
            // for a border on `h2` means.
            t.heading_border[i] =
                decoration(spec.and_then(|s| s.border.as_ref()), t.heading_border[i])?;
            t.heading_bar[i] = decoration(spec.and_then(|s| s.bar.as_ref()), t.heading_bar[i])?;
        }

        t.link = take(&file.inline, "link", t.link)?;
        t.code = take(&file.inline, "code", t.code)?;
        t.emph = take(&file.inline, "emph", t.emph)?;
        t.strong = take(&file.inline, "strong", t.strong)?;
        t.strikethrough = take(&file.inline, "strikethrough", t.strikethrough)?;
        t.footnote = take(&file.inline, "footnote", t.footnote)?;

        t.quote_bar = take(&file.block, "quote_bar", t.quote_bar)?;
        t.quote = take(&file.block, "quote", t.quote)?;
        t.rule = take(&file.block, "rule", t.rule)?;
        t.list_marker = take(&file.block, "list_marker", t.list_marker)?;
        t.task_done = take(&file.block, "task_done", t.task_done)?;
        t.task_todo = take(&file.block, "task_todo", t.task_todo)?;

        t.table_border = take(&file.table, "border", t.table_border)?;
        t.table_header = take(&file.table, "header", t.table_header)?;

        for (i, key) in ["note", "tip", "important", "warning", "caution"]
            .iter()
            .enumerate()
        {
            t.alerts[i] = take(&file.alert, key, t.alerts[i])?;
        }

        t.status = take(&file.ui, "status", t.status)?;
        t.toast = take(&file.ui, "toast", t.toast)?;
        t.cursor = take(&file.ui, "cursor", t.cursor)?;
        t.search_match = take(&file.ui, "search_match", t.search_match)?;
        t.search_current = take(&file.ui, "search_current", t.search_current)?;
        t.hint = take(&file.ui, "hint", t.hint)?;
        t.dim = take(&file.ui, "dim", t.dim)?;

        Ok(t)
    }

    pub fn heading(&self, level: u8) -> Style {
        let idx = (level.clamp(1, 6) - 1) as usize;
        self.headings[idx]
    }

    /// The base style for body text. `None` foreground means "inherit".
    pub fn body(&self) -> Style {
        Style {
            fg: self.foreground,
            bg: self.background,
            ..Style::PLAIN
        }
    }
}

/// Write a color back in a form [`parse_color`] reads.
///
/// The named branch is the inverse of the table in `parse_color`, including its
/// two off-by-a-shade pairs: `white` is `Grey` and `brightwhite` is `White`.
fn color_toml(c: Color) -> String {
    match c {
        Color::Rgb { r, g, b } => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::AnsiValue(n) => n.to_string(),
        Color::Black => "black".into(),
        Color::DarkRed => "red".into(),
        Color::DarkGreen => "green".into(),
        Color::DarkYellow => "yellow".into(),
        Color::DarkBlue => "blue".into(),
        Color::DarkMagenta => "magenta".into(),
        Color::DarkCyan => "cyan".into(),
        Color::Grey => "white".into(),
        Color::DarkGrey => "brightblack".into(),
        Color::Red => "brightred".into(),
        Color::Green => "brightgreen".into(),
        Color::Yellow => "brightyellow".into(),
        Color::Blue => "brightblue".into(),
        Color::Magenta => "brightmagenta".into(),
        Color::Cyan => "brightcyan".into(),
        Color::White => "brightwhite".into(),
        Color::Reset => "reset".into(),
    }
}

/// One style as an inline table. Attributes that are off are left out, since
/// writing `bold = false` would suggest it can turn an attribute off, and
/// [`Style::patch`] only ever adds them.
fn style_toml(s: Style) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = s.fg {
        parts.push(format!("fg = \"{}\"", color_toml(c)));
    }
    if let Some(c) = s.bg {
        parts.push(format!("bg = \"{}\"", color_toml(c)));
    }
    for (on, key) in [
        (s.bold, "bold"),
        (s.italic, "italic"),
        (s.underline, "underline"),
        (s.strikethrough, "strikethrough"),
        (s.dim, "dim"),
        (s.reverse, "reverse"),
    ] {
        if on {
            parts.push(format!("{key} = true"));
        }
    }
    if parts.is_empty() {
        "{}".into()
    } else {
        format!("{{ {} }}", parts.join(", "))
    }
}

/// Write a resolved theme back out as TOML that [`Theme::parse`] reads.
///
/// This is the theme reference. A key list kept by hand in the documentation
/// goes stale the first time a key is added and nobody notices; one generated
/// from the resolved theme cannot, and it doubles as the starting point for a
/// new theme, which otherwise means having the repository checked out to copy
/// `themes/*.toml` from.
///
/// Every key is written, including the ones left at their default, so the
/// output says what there is to set rather than only what this theme chose to.
pub fn dump(theme: &Theme) -> String {
    let mut out = String::new();
    out.push_str(&format!("name = {:?}\n", theme.name));
    for (key, color) in [
        ("foreground", theme.foreground),
        ("background", theme.background),
    ] {
        match color {
            // A commented-out key still tells the reader it exists, and leaving
            // it unset is what the `terminal` theme deliberately does.
            None => out.push_str(&format!("# {key} = \"#rrggbb\"   # unset: inherit\n")),
            Some(c) => out.push_str(&format!("{key} = \"{}\"\n", color_toml(c))),
        }
    }

    out.push_str("\n[code]\n");
    out.push_str(&format!("syntect_theme = {:?}\n", theme.syntect_theme));
    out.push_str(&format!("fence = {}\n", style_toml(theme.code_fence)));

    out.push_str("\n[heading]\n");
    for level in 1..=6u8 {
        let i = (level - 1) as usize;
        let mut parts = vec![style_toml(theme.headings[i])];
        for (key, decor) in [
            ("border", theme.heading_border[i]),
            ("bar", theme.heading_bar[i]),
        ] {
            parts.push(match decor {
                None => format!("{key} = false"),
                Some(Decoration { color: None }) => format!("{key} = true"),
                Some(Decoration { color: Some(c) }) => {
                    format!("{key} = \"{}\"", color_toml(c))
                }
            });
        }
        // The style is already an inline table; the decorations join it rather
        // than sitting beside it, since they are keys of the same heading.
        let style = parts.remove(0);
        let inner = style.trim_matches(['{', '}', ' ']);
        let joined = if inner.is_empty() {
            parts.join(", ")
        } else {
            format!("{inner}, {}", parts.join(", "))
        };
        out.push_str(&format!("h{level} = {{ {joined} }}\n"));
    }

    let sections: [(&str, &[(&str, Style)]); 5] = [
        (
            "inline",
            &[
                ("link", theme.link),
                ("code", theme.code),
                ("emph", theme.emph),
                ("strong", theme.strong),
                ("strikethrough", theme.strikethrough),
                ("footnote", theme.footnote),
            ],
        ),
        (
            "block",
            &[
                ("quote_bar", theme.quote_bar),
                ("quote", theme.quote),
                ("rule", theme.rule),
                ("list_marker", theme.list_marker),
                ("task_done", theme.task_done),
                ("task_todo", theme.task_todo),
            ],
        ),
        (
            "table",
            &[
                ("border", theme.table_border),
                ("header", theme.table_header),
            ],
        ),
        (
            "alert",
            &[
                ("note", theme.alerts[0]),
                ("tip", theme.alerts[1]),
                ("important", theme.alerts[2]),
                ("warning", theme.alerts[3]),
                ("caution", theme.alerts[4]),
            ],
        ),
        (
            "ui",
            &[
                ("status", theme.status),
                ("toast", theme.toast),
                ("cursor", theme.cursor),
                ("search_match", theme.search_match),
                ("search_current", theme.search_current),
                ("hint", theme.hint),
                ("dim", theme.dim),
            ],
        ),
    ];
    for (section, keys) in sections {
        out.push_str(&format!("\n[{section}]\n"));
        for (key, style) in keys {
            out.push_str(&format!("{key} = {}\n", style_toml(*style)));
        }
    }
    out
}

/// Directory holding user themes.
pub fn user_theme_dir() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("mdroll").join("themes"))
}

/// Every theme name available: bundled first, then user themes.
pub fn available_names() -> Vec<String> {
    let mut names: Vec<String> = BUNDLED.iter().map(|(n, _)| n.to_string()).collect();
    if let Some(dir) = user_theme_dir()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && !names.iter().any(|n| n == stem)
            {
                names.push(stem.to_string());
            }
        }
    }
    names
}

/// Whether a `--theme` argument names a file rather than a theme.
///
/// A theme being written lives wherever it is being written, and having to
/// install it into the config directory before it can be looked at makes for a
/// slow loop. Nothing is ambiguous: a theme *name* is a file stem, so it can
/// carry neither a separator nor a `.toml` extension.
fn looks_like_path(name: &str) -> bool {
    name.contains('/')
        || name.contains(std::path::MAIN_SEPARATOR)
        || Path::new(name).extension().is_some_and(|e| e == "toml")
}

/// Load a theme by name, preferring a user theme over a bundled one, or read
/// one straight from a path.
pub fn load(name: &str) -> Result<Theme> {
    if looks_like_path(name) {
        return load_path(Path::new(name));
    }
    if let Some(dir) = user_theme_dir() {
        let path = dir.join(format!("{name}.toml"));
        if path.is_file() {
            return load_path(&path);
        }
    }
    if let Some((_, text)) = BUNDLED.iter().find(|(n, _)| *n == name) {
        return Theme::parse(text).with_context(|| format!("bundled theme {name:?}"));
    }
    bail!(
        "unknown theme {name:?}; available: {}",
        available_names().join(", ")
    )
}

pub fn load_path(path: &Path) -> Result<Theme> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Theme::parse(&text).with_context(|| format!("in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_parse() {
        assert_eq!(
            parse_color("#bd93f9").unwrap(),
            Color::Rgb {
                r: 0xbd,
                g: 0x93,
                b: 0xf9
            }
        );
        assert_eq!(
            parse_color("#f0a").unwrap(),
            Color::Rgb {
                r: 0xff,
                g: 0x00,
                b: 0xaa
            }
        );
    }

    #[test]
    fn named_and_indexed_colors_parse() {
        assert_eq!(parse_color("brightcyan").unwrap(), Color::Cyan);
        assert_eq!(parse_color("42").unwrap(), Color::AnsiValue(42));
    }

    #[test]
    fn bad_colors_are_errors() {
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("chartreuse").is_err());
    }

    #[test]
    fn every_bundled_theme_parses() {
        for (name, text) in BUNDLED {
            let theme = Theme::parse(text).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            assert_eq!(&theme.name, name, "theme file name must match its key");
        }
    }

    #[test]
    fn unspecified_heading_levels_inherit_the_previous_one() {
        let theme = Theme::parse(
            r##"
            name = "t"
            [heading]
            h1 = { fg = "#ff0000" }
            "##,
        )
        .unwrap();
        assert_eq!(theme.heading(1).fg, theme.heading(6).fg);
    }

    #[test]
    fn every_bundled_theme_survives_a_dump_and_a_reparse() {
        // This is what keeps the dump honest as a reference: a key the dumper
        // forgets, or writes in a form the parser does not read, comes back as
        // a theme that differs from the one that was written out.
        for (name, text) in BUNDLED {
            let original = Theme::parse(text).unwrap();
            let round_tripped = Theme::parse(&dump(&original))
                .unwrap_or_else(|e| panic!("{name} did not parse after a dump: {e:#}"));
            assert_eq!(original, round_tripped, "{name} changed across a dump");
        }
    }

    #[test]
    fn a_dump_names_every_key_the_parser_reads() {
        // A theme left entirely at its defaults still documents the whole
        // schema, which is the point of dumping one to start from.
        let dumped = dump(&Theme::default());
        for key in [
            "syntect_theme",
            "fence",
            "h1",
            "h6",
            "link",
            "footnote",
            "quote_bar",
            "rule",
            "task_todo",
            "border",
            "header",
            "note",
            "caution",
            "status",
            "search_current",
            "dim",
        ] {
            assert!(dumped.contains(key), "{key} missing from a dumped theme");
        }
    }

    #[test]
    fn every_color_survives_being_written_and_read_back() {
        // The named table in `parse_color` has two pairs that are easy to get
        // backwards — `white` is Grey and `brightwhite` is White — so every
        // variant is checked rather than a sample.
        let colors = [
            Color::Rgb {
                r: 1,
                g: 34,
                b: 255,
            },
            Color::AnsiValue(129),
            Color::Black,
            Color::DarkRed,
            Color::DarkGreen,
            Color::DarkYellow,
            Color::DarkBlue,
            Color::DarkMagenta,
            Color::DarkCyan,
            Color::Grey,
            Color::DarkGrey,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::White,
            Color::Reset,
        ];
        for c in colors {
            let written = color_toml(c);
            assert_eq!(parse_color(&written).unwrap(), c, "{written:?}");
        }
    }

    #[test]
    fn a_theme_that_says_nothing_borders_the_levels_drawn_large() {
        // The point of deriving rather than enumerating: every theme written
        // before decoration existed still shows it.
        for (name, text) in BUNDLED {
            let theme = Theme::parse(text).unwrap();
            assert_eq!(
                theme.heading_border[0],
                Some(Decoration { color: None }),
                "{name} lost h1's border"
            );
            assert_eq!(theme.heading_border[2], None, "{name} bordered h3");
            assert!(
                theme.heading_bar.iter().all(|b| b.is_none()),
                "{name} grew a bar nobody asked for"
            );
        }
    }

    #[test]
    fn a_decoration_can_be_turned_off_given_a_colour_or_left_to_derive_one() {
        let theme = Theme::parse(
            r##"
            name = "t"
            [heading]
            h1 = { fg = "#ff0000", border = false }
            h2 = { border = "#00ff00" }
            h3 = { bar = true }
            "##,
        )
        .unwrap();
        assert_eq!(theme.heading_border[0], None);
        assert_eq!(
            theme.heading_border[1],
            Some(Decoration {
                color: Some(Color::Rgb { r: 0, g: 255, b: 0 })
            })
        );
        assert_eq!(theme.heading_bar[2], Some(Decoration { color: None }));
    }

    #[test]
    fn decoration_does_not_inherit_the_way_style_does() {
        // h4..h6 inherit h3's colour, and must not inherit its bar: a theme
        // asking for a bar on one level is not asking for one on every level
        // below it.
        let theme = Theme::parse(
            r##"
            name = "t"
            [heading]
            h3 = { fg = "#00ff00", bar = true }
            "##,
        )
        .unwrap();
        assert_eq!(theme.heading(4).fg, theme.heading(3).fg);
        assert_eq!(theme.heading_bar[2], Some(Decoration { color: None }));
        assert_eq!(theme.heading_bar[3], None, "the bar leaked down to h4");
    }

    #[test]
    fn border_and_bar_are_refused_on_a_style_that_is_not_a_heading() {
        let err = Theme::parse(
            r##"
            name = "t"
            [inline]
            link = { fg = "#ff0000", border = true }
            "##,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("border"), "{err:#}");
    }

    #[test]
    fn a_theme_argument_naming_a_file_is_read_from_that_file() {
        let path = std::env::temp_dir().join(format!("mdroll-theme-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "name = \"scratch\"\n[heading]\nh1 = { fg = \"#ff0000\" }\n",
        )
        .unwrap();
        let theme = load(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(theme.name, "scratch");
        assert_eq!(theme.heading(1).fg, Some(Color::Rgb { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn a_bare_name_is_still_looked_up_rather_than_opened() {
        // No bundled name carries a separator or an extension, so the two
        // cases never overlap; a name that is not a theme must say so rather
        // than report a missing file.
        assert!(!looks_like_path("dracula"));
        assert!(looks_like_path("dracula.toml"));
        assert!(looks_like_path("./dracula"));
        let err = format!("{:#}", load("no-such-theme").unwrap_err());
        assert!(err.contains("unknown theme"), "{err}");
    }

    #[test]
    fn terminal_theme_sets_no_background() {
        let theme = load("terminal").unwrap();
        assert!(
            theme.background.is_none(),
            "terminal theme must inherit the terminal palette"
        );
    }
}
