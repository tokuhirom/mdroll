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
    ("solarized-dark", include_str!("../themes/solarized-dark.toml")),
    ("solarized-light", include_str!("../themes/solarized-light.toml")),
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
                (((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8)
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
    heading: BTreeMap<String, StyleSpec>,
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
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub foreground: Option<Color>,
    pub background: Option<Color>,
    pub syntect_theme: String,

    pub headings: [Style; 6],

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
            let fallback = if i == 0 { t.headings[0] } else { t.headings[i - 1] };
            t.headings[i] = take(&file.heading, key, fallback)?;
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
        let idx = (level.max(1).min(6) - 1) as usize;
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

/// Load a theme by name, preferring a user theme over a bundled one.
pub fn load(name: &str) -> Result<Theme> {
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
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
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
    fn terminal_theme_sets_no_background() {
        let theme = load("terminal").unwrap();
        assert!(theme.background.is_none(), "terminal theme must inherit the terminal palette");
    }
}
