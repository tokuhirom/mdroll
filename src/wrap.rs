//! Line breaking: UAX #14 plus kinsoku.
//!
//! Whitespace-based wrapping has nowhere to break text that contains no
//! spaces, and even a correct UAX #14 implementation will happily start a line
//! with `。` unless kinsoku rules are layered on top.

use crate::ir::Span;
use crate::width::WidthCalc;
use unicode_linebreak::linebreaks;

/// 行頭禁則 — characters that may never begin a line.
const NO_LINE_START: &str = concat!(
    // Punctuation that clings to the preceding character.
    "。、．，，。、",
    // Closing brackets.
    "）］｝〕〉》」』】〙〗〟’”｠»)]}〞",
    // Prolonged sound mark and iteration marks.
    "ーヽヾゝゞ々〻",
    // Small kana.
    "ぁぃぅぇぉっゃゅょゎゕゖァィゥェォッャュョヮヵヶ",
    // Sentence-ending and separating marks.
    "!?‼⁇⁈⁉!?、:;・…‥—–〜～",
    // Combining-ish marks that must trail.
    "゛゜"
);

/// 行末禁則 — characters that may never end a line.
const NO_LINE_END: &str = "（［｛〔〈《「『【〘〖〝‘“｟«([{";

pub fn forbidden_at_line_start(c: char) -> bool {
    NO_LINE_START.contains(c)
}

pub fn forbidden_at_line_end(c: char) -> bool {
    NO_LINE_END.contains(c)
}

/// Byte offsets at which a line may begin, in ascending order.
///
/// Offset `0` and `text.len()` are excluded — they are not breaks *within* the
/// text. UAX #14 supplies the candidates; kinsoku removes the ones that would
/// strand a clinging character.
pub fn break_points(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (offset, kind) in linebreaks(text) {
        if offset == 0 || offset >= text.len() {
            continue;
        }
        // A mandatory break is a hard line ending; callers split on those
        // first, so anything reaching here is an ordinary opportunity.
        let _ = kind;
        if kinsoku_allows(text, offset) {
            out.push(offset);
        }
    }
    out
}

/// Whether a break immediately before byte `offset` satisfies kinsoku.
fn kinsoku_allows(text: &str, offset: usize) -> bool {
    if !text.is_char_boundary(offset) {
        return false;
    }
    if let Some(next) = text[offset..].chars().next()
        && forbidden_at_line_start(next)
    {
        return false;
    }
    if let Some(prev) = text[..offset].chars().next_back()
        && forbidden_at_line_end(prev)
    {
        return false;
    }
    true
}

/// Break `text` into runs that each fit within `limit` columns.
///
/// Returns byte ranges as `(start, end)` pairs covering the whole string. When
/// no legal break point fits, the line is broken mid-run rather than
/// overflowing — a document with a 200-column URL still has to render.
pub fn wrap_ranges(text: &str, limit: usize, calc: &WidthCalc) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return vec![(0, 0)];
    }
    let limit = limit.max(1);
    if calc.str(text) <= limit {
        return vec![(0, text.len())];
    }

    let breaks = break_points(text);
    let mut out = Vec::new();
    let mut start = 0usize;

    while start < text.len() {
        // Widest prefix of text[start..] that fits.
        let mut fit_end = start;
        let mut used = 0usize;
        for (i, c) in text[start..].char_indices() {
            let w = calc.ch(c);
            if used + w > limit {
                break;
            }
            used += w;
            fit_end = start + i + c.len_utf8();
        }

        if fit_end >= text.len() {
            out.push((start, text.len()));
            break;
        }

        // Prefer the last legal break at or before the fitting point.
        let end = match breaks.iter().rev().find(|&&b| b > start && b <= fit_end) {
            Some(&b) => b,
            // Nothing UAX #14 offers fits, so the line is cut where it stops
            // fitting — but that cut is still a line beginning, and kinsoku
            // applies to it as much as to a chosen one.
            None if fit_end > start => retreat_to_kinsoku(text, start, fit_end),
            // A single character wider than the limit: emit it anyway.
            None => start + text[start..].chars().next().map_or(1, char::len_utf8),
        };
        out.push((start, end));
        start = end;
    }

    if out.is_empty() {
        out.push((0, text.len()));
    }
    out
}

/// Pull a forced break back until it lands somewhere kinsoku permits.
///
/// This is 追い出し: the offending character is pushed down to the next line by
/// shortening this one. Reached only when UAX #14 offers no break that fits —
/// twelve columns of `a` followed by `。` has no opportunity anywhere, and
/// cutting at the fitting point would open the next line with the `。`.
///
/// If every position back to `start` offends, the original point is used. A
/// line that breaks badly is bad; a line that cannot break at all is a hang.
fn retreat_to_kinsoku(text: &str, start: usize, fit_end: usize) -> usize {
    let mut end = fit_end;
    while end > start {
        if kinsoku_allows(text, end) {
            return end;
        }
        end -= text[start..end]
            .chars()
            .next_back()
            .map_or(1, char::len_utf8);
    }
    fit_end
}

/// Chop `text` into runs of at most `limit` columns, ignoring word boundaries.
///
/// Code is not prose: reflowing it at spaces would change what it means. When a
/// code line is too wide to fit, it is cut at the column limit instead, so
/// nothing is lost off the right edge.
pub fn hard_wrap_ranges(text: &str, limit: usize, calc: &WidthCalc) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return vec![(0, 0)];
    }
    let limit = limit.max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut used = 0usize;
    for (i, c) in text.char_indices() {
        let w = calc.ch(c);
        if used + w > limit && i > start {
            out.push((start, i));
            start = i;
            used = 0;
        }
        used += w;
    }
    out.push((start, text.len()));
    out
}

/// The concatenated text of `spans`.
pub fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Slice a span run by byte offsets into its concatenated text.
pub fn slice_spans(spans: &[Span], start: usize, end: usize) -> Vec<Span> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    for span in spans {
        let len = span.text.len();
        let (s, e) = (pos, pos + len);
        pos = e;
        if e <= start || s >= end {
            continue;
        }
        let from = start.saturating_sub(s);
        let to = (end - s).min(len);
        if from >= to {
            continue;
        }
        out.push(Span {
            text: span.text[from..to].to_string(),
            style: span.style,
            link: span.link,
        });
    }
    out
}

/// Slice a styled row to the display-column window `[start, start + len)`.
///
/// This is what horizontal scrolling is built on. The offset is in display
/// columns, never bytes: a full-width character straddling either edge becomes
/// a single space so the row stays exactly as wide as the window.
pub fn slice_spans_columns(
    spans: &[Span],
    start: usize,
    len: usize,
    calc: &WidthCalc,
) -> Vec<Span> {
    if len == 0 {
        return Vec::new();
    }
    let end = start.saturating_add(len);
    let mut out: Vec<Span> = Vec::new();
    let mut col = 0usize;

    for span in spans {
        if col >= end {
            break;
        }
        let mut text = String::new();
        for c in span.text.chars() {
            if col >= end {
                break;
            }
            let w = calc.ch(c);
            let next = col + w;
            if next <= start {
                col = next;
                continue;
            }
            if col < start {
                text.push_str(&" ".repeat(next.min(end) - start));
            } else if next > end {
                text.push_str(&" ".repeat(end - col));
            } else {
                text.push(c);
            }
            col = next;
        }
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.style == span.style && last.link == span.link => {
                last.text.push_str(&text);
            }
            _ => out.push(Span {
                text,
                style: span.style,
                link: span.link,
            }),
        }
    }
    out
}

/// Drop trailing spaces from a laid-out row. Safe because the row is about to
/// be drawn, and a break opportunity after a space leaves that space dangling.
pub fn trim_trailing(mut spans: Vec<Span>) -> Vec<Span> {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end_matches(' ');
        if trimmed.len() == last.text.len() {
            break;
        }
        last.text.truncate(trimmed.len());
        if last.text.is_empty() {
            spans.pop();
        } else {
            break;
        }
    }
    spans
}

/// Reflow a run of spans to `limit` columns, preserving styling and links.
pub fn wrap_spans(spans: &[Span], limit: usize, calc: &WidthCalc) -> Vec<Vec<Span>> {
    let text = spans_text(spans);
    if text.is_empty() {
        return vec![Vec::new()];
    }
    wrap_ranges(&text, limit, calc)
        .into_iter()
        .map(|(s, e)| trim_trailing(slice_spans(spans, s, e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Style;

    const CALC: WidthCalc = WidthCalc {
        ambiguous_wide: false,
    };

    fn wrap(text: &str, limit: usize) -> Vec<String> {
        wrap_ranges(text, limit, &CALC)
            .into_iter()
            .map(|(s, e)| text[s..e].trim_end().to_string())
            .collect()
    }

    #[test]
    fn short_text_is_one_line() {
        assert_eq!(wrap("hello", 20), vec!["hello"]);
    }

    #[test]
    fn latin_breaks_at_spaces() {
        assert_eq!(
            wrap("the quick brown fox", 10),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn japanese_breaks_between_characters() {
        // No whitespace at all: a whitespace-based wrapper would emit one
        // overlong line.
        let lines = wrap("吾輩は猫である。名前はまだ無い。", 12);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(CALC.str(line) <= 12, "{line:?} is too wide");
        }
    }

    #[test]
    fn no_line_begins_with_a_forbidden_character() {
        let text = "これはテストです。次の文が続きます。終わり。";
        for limit in 4..30 {
            for line in wrap(text, limit) {
                if let Some(c) = line.chars().next() {
                    assert!(
                        !forbidden_at_line_start(c),
                        "limit={limit} line={line:?} starts with {c:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn no_line_ends_with_an_opening_bracket() {
        let text = "説明は（ここに書いてあります）ので確認してください。";
        for limit in 4..30 {
            for line in wrap(text, limit) {
                if let Some(c) = line.chars().last() {
                    assert!(
                        !forbidden_at_line_end(c),
                        "limit={limit} line={line:?} ends with {c:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn kinsoku_pulls_the_period_up_rather_than_stranding_it() {
        // Column 8 fits "日本語です" exactly at 10... force the boundary so the
        // trailing "。" would land alone on the next line.
        let lines = wrap("日本語。", 6);
        assert_eq!(lines, vec!["日本", "語。"]);
    }

    #[test]
    fn kinsoku_holds_even_when_the_break_has_to_be_forced() {
        // Twelve columns of `a` and then `。`: UAX #14 offers no opportunity
        // anywhere in the run, so the break is forced at the fitting point —
        // which is exactly where the `。` would be stranded.
        assert_eq!(
            wrap("aaaaaaaaaaaa。つづく", 12),
            vec!["aaaaaaaaaaa", "a。つづく"]
        );
        assert_eq!(
            wrap("bbbbbbbbbbbb）あと", 12),
            vec!["bbbbbbbbbbb", "b）あと"]
        );
    }

    #[test]
    fn a_forced_break_never_strands_a_clinging_character() {
        // The property, over every width the text can be drawn at.
        for text in [
            "aaaaaaaaaaaa。つづく",
            "xxxxxxxxxxxx」おわり",
            "説明は（ここに書いてあります）ので確認してください。",
            "aaaaaaaaaaaaaaaaaaaaaaaa、",
        ] {
            // From four columns up: a clinging character is two columns wide
            // and needs at least one more beside it, so below that there is no
            // arrangement that satisfies the rule and the forced break stands.
            for limit in 4..30 {
                for line in wrap(text, limit) {
                    if let Some(c) = line.chars().next() {
                        assert!(
                            !forbidden_at_line_start(c),
                            "limit={limit} line={line:?} starts with {c:?}"
                        );
                    }
                    if let Some(c) = line.chars().last() {
                        assert!(
                            !forbidden_at_line_end(c),
                            "limit={limit} line={line:?} ends with {c:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_overlong_unbreakable_token_is_split_rather_than_overflowing() {
        let lines = wrap("aaaaaaaaaaaaaaaaaaaa", 5);
        assert!(lines.iter().all(|l| l.len() <= 5));
        assert_eq!(lines.concat(), "aaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn wrapping_never_loses_or_duplicates_text() {
        let text = "Rust の unicode-linebreak は UAX #14 を実装しています。これは重要です。";
        for limit in 2..40 {
            let joined: String = wrap_ranges(text, limit, &CALC)
                .into_iter()
                .map(|(s, e)| &text[s..e])
                .collect();
            assert_eq!(joined, text, "limit={limit}");
        }
    }

    #[test]
    fn every_wrapped_line_fits_the_limit() {
        let text = "混在した text と日本語が入っている段落です。URL は https://example.com/very/long/path など。";
        for limit in 8..40 {
            for line in wrap(text, limit) {
                assert!(CALC.str(&line) <= limit, "limit={limit} line={line:?}");
            }
        }
    }

    #[test]
    fn slice_spans_respects_span_boundaries() {
        let spans = vec![
            Span::plain("hello "),
            Span::new(
                "world",
                Style {
                    bold: true,
                    ..Style::PLAIN
                },
            ),
        ];
        let got = slice_spans(&spans, 3, 8);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text, "lo ");
        assert_eq!(got[1].text, "wo");
        assert!(got[1].style.bold);
    }

    #[test]
    fn wrap_spans_keeps_styles_across_the_break() {
        let spans = vec![
            Span::plain("plain text "),
            Span::new(
                "bold text here",
                Style {
                    bold: true,
                    ..Style::PLAIN
                },
            ),
        ];
        let rows = wrap_spans(&spans, 12, &CALC);
        assert!(rows.len() > 1);
        let bold_chars: usize = rows
            .iter()
            .flatten()
            .filter(|s| s.style.bold)
            .map(|s| s.text.chars().count())
            .sum();
        assert_eq!(bold_chars, "bold text here".chars().count() - 1); // one space trimmed
    }

    #[test]
    fn empty_input_yields_one_empty_row() {
        assert_eq!(wrap_spans(&[], 10, &CALC).len(), 1);
    }
}
