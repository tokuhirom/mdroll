//! Syntax highlighting for fenced code blocks.
//!
//! Two colour systems are in play across the project: the UI palette in
//! [`crate::theme`], and syntect's `.tmTheme` files used here. A theme names
//! both. syntect ships Solarized and the base16 family but not Dracula, so
//! `Dracula.tmTheme` is vendored and registered under its own name.
//!
//! Only foreground colours are taken from the syntax theme. Its background
//! would fight the `terminal` theme, which deliberately paints none.

use crate::ir::{Color, Span, Style};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as SynTheme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

const DRACULA: &str = include_str!("../themes/Dracula.tmTheme");

struct Assets {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults();
        if let Ok(dracula) = ThemeSet::load_from_reader(&mut std::io::Cursor::new(DRACULA)) {
            themes.themes.insert("Dracula".to_string(), dracula);
        }
        Assets {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            themes,
        }
    })
}

pub fn theme_names() -> Vec<String> {
    assets().themes.themes.keys().cloned().collect()
}

fn syntax_theme(name: &str) -> Option<&'static SynTheme> {
    let assets = assets();
    assets
        .themes
        .themes
        .get(name)
        .or_else(|| assets.themes.themes.get("base16-ocean.dark"))
}

/// Highlight `code`, returning spans that keep their newlines so layout can
/// split them into rows.
///
/// Returns `None` when the language is unknown, in which case the caller should
/// render the code unstyled rather than guessing.
pub fn highlight(code: &str, lang: &str, theme_name: &str, base: Style) -> Option<Vec<Span>> {
    if lang.is_empty() {
        return None;
    }
    let assets = assets();
    let lang = lang.to_ascii_lowercase();
    let syntax = assets
        .syntaxes
        .find_syntax_by_token(&lang)
        .or_else(|| assets.syntaxes.find_syntax_by_extension(&lang))
        .or_else(|| assets.syntaxes.find_syntax_by_name(&lang))?;
    let theme = syntax_theme(theme_name)?;

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out: Vec<Span> = Vec::new();
    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, &assets.syntaxes).ok()?;
        for (style, text) in ranges {
            if text.is_empty() {
                continue;
            }
            let span = Span::new(
                text.to_string(),
                Style {
                    fg: Some(Color::Rgb {
                        r: style.foreground.r,
                        g: style.foreground.g,
                        b: style.foreground.b,
                    }),
                    ..base
                },
            );
            match out.last_mut() {
                Some(last) if last.style == span.style => last.text.push_str(&span.text),
                _ => out.push(span),
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_is_highlighted_into_several_colours() {
        let spans = highlight(
            "fn main() {\n    let x = 1;\n}\n",
            "rust",
            "base16-ocean.dark",
            Style::PLAIN,
        )
        .expect("rust is a known language");
        let colours: std::collections::HashSet<_> = spans.iter().map(|s| s.style.fg).collect();
        assert!(
            colours.len() > 2,
            "expected several colours, got {colours:?}"
        );
    }

    #[test]
    fn the_highlighted_text_is_the_original_text() {
        let code = "let x = 1;\nlet y = 2;\n";
        let spans = highlight(code, "rust", "base16-ocean.dark", Style::PLAIN).unwrap();
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, code);
    }

    #[test]
    fn newlines_survive_so_layout_can_split_rows() {
        let spans = highlight("a\nb\nc\n", "text", "base16-ocean.dark", Style::PLAIN);
        if let Some(spans) = spans {
            let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(joined.matches('\n').count(), 3);
        }
    }

    #[test]
    fn an_unknown_language_is_declined_rather_than_guessed() {
        assert!(highlight("x", "no-such-language", "base16-ocean.dark", Style::PLAIN).is_none());
        assert!(highlight("x", "", "base16-ocean.dark", Style::PLAIN).is_none());
    }

    #[test]
    fn the_vendored_dracula_theme_is_registered() {
        assert!(theme_names().iter().any(|n| n == "Dracula"));
        let spans = highlight("fn main() {}", "rust", "Dracula", Style::PLAIN).unwrap();
        assert!(!spans.is_empty());
    }

    #[test]
    fn an_unknown_theme_falls_back_rather_than_failing() {
        assert!(highlight("fn main() {}", "rust", "No Such Theme", Style::PLAIN).is_some());
    }

    #[test]
    fn the_base_style_is_preserved_apart_from_colour() {
        let base = Style {
            italic: true,
            ..Style::PLAIN
        };
        let spans = highlight("let x = 1;", "rust", "Dracula", base).unwrap();
        assert!(spans.iter().all(|s| s.style.italic));
    }
}
