//! Display-width measurement.
//!
//! Everything that positions text — wrapping, table columns, horizontal
//! scrolling — measures through [`WidthCalc`] and never through `str::len` or
//! `chars().count()`.
//!
//! The East Asian Ambiguous class is configurable because whether `─` or `→`
//! occupies one column or two is a property of the terminal, not of the text.

use unicode_width::UnicodeWidthChar;

/// Columns a hard tab advances to. Tabs are expanded before measurement, so
/// this is only used by [`expand_tabs`].
pub const TAB_STOP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WidthCalc {
    /// Treat East Asian Ambiguous characters as two columns wide. Defaults to
    /// narrow, matching a terminal that has not been told otherwise.
    pub ambiguous_wide: bool,
}

impl WidthCalc {
    pub fn new(ambiguous_wide: bool) -> WidthCalc {
        WidthCalc { ambiguous_wide }
    }

    /// Columns occupied by a single character. Control characters measure 0
    /// rather than panicking; they are stripped before reaching the screen.
    pub fn ch(&self, c: char) -> usize {
        let w = if self.ambiguous_wide {
            c.width_cjk()
        } else {
            c.width()
        };
        w.unwrap_or(0)
    }

    pub fn str(&self, s: &str) -> usize {
        s.chars().map(|c| self.ch(c)).sum()
    }

    /// Whether the character straddles two columns. Used at slice boundaries,
    /// where a half-visible wide character must be replaced by a space.
    pub fn is_wide(&self, c: char) -> bool {
        self.ch(c) == 2
    }

    /// Truncate `s` so it fits in `limit` columns, returning the text and its
    /// actual width. A wide character that would straddle the limit is dropped.
    pub fn truncate(&self, s: &str, limit: usize) -> (String, usize) {
        let mut out = String::new();
        let mut used = 0;
        for c in s.chars() {
            let w = self.ch(c);
            if used + w > limit {
                break;
            }
            out.push(c);
            used += w;
        }
        (out, used)
    }

    /// Slice `s` to the column window `[start, start + len)`.
    ///
    /// The offset is in **display columns**, never bytes or `char` counts. When
    /// a full-width character straddles either boundary it is replaced by a
    /// single space, so the result is always exactly as wide as the window it
    /// fills. Getting this wrong produces a one-column drift that compounds
    /// across the document.
    pub fn slice_columns(&self, s: &str, start: usize, len: usize) -> String {
        if len == 0 {
            return String::new();
        }
        let end = start.saturating_add(len);
        let mut out = String::new();
        let mut col = 0usize;

        for c in s.chars() {
            if col >= end {
                break;
            }
            let w = self.ch(c);
            let next = col + w;

            if next <= start {
                // Entirely left of the window.
                col = next;
                continue;
            }
            if col < start {
                // Straddles the left edge: show the visible half as a space.
                out.push_str(&" ".repeat(next.min(end) - start));
            } else if next > end {
                // Straddles the right edge.
                out.push_str(&" ".repeat(end - col));
            } else {
                out.push(c);
            }
            col = next;
        }
        out
    }
}

/// Expand hard tabs to the next multiple of [`TAB_STOP`].
///
/// The column count is display width, not characters: a tab after `日` advances
/// from column two, not column one, and counting characters would put every
/// tab stop on a line of CJK text one column further left than the terminal
/// puts it.
///
/// Ambiguous-width characters are measured narrow here, because this runs while
/// the document is parsed and the setting belongs to the viewer. A line that
/// mixes tabs with `─` can therefore be off by one; a line that mixes tabs with
/// kanji, which is the common case, is not.
pub fn expand_tabs(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let calc = WidthCalc::default();
    let mut out = String::with_capacity(s.len());
    let mut col = 0usize;
    for c in s.chars() {
        if c == '\t' {
            let n = TAB_STOP - (col % TAB_STOP);
            out.push_str(&" ".repeat(n));
            col += n;
        } else {
            out.push(c);
            col += calc.ch(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NARROW: WidthCalc = WidthCalc {
        ambiguous_wide: false,
    };
    const WIDE: WidthCalc = WidthCalc {
        ambiguous_wide: true,
    };

    #[test]
    fn ascii_is_one_column() {
        assert_eq!(NARROW.str("hello"), 5);
    }

    #[test]
    fn kana_and_kanji_are_two_columns() {
        assert_eq!(NARROW.str("日本語"), 6);
        assert_eq!(NARROW.str("こんにちは"), 10);
    }

    #[test]
    fn ambiguous_width_follows_the_setting() {
        // U+2500 BOX DRAWINGS LIGHT HORIZONTAL and U+2192 RIGHTWARDS ARROW are
        // both East Asian Ambiguous.
        assert_eq!(NARROW.str("─→"), 2);
        assert_eq!(WIDE.str("─→"), 4);
    }

    #[test]
    fn combining_marks_are_zero_width() {
        assert_eq!(NARROW.str("が"), 2);
        assert_eq!(NARROW.str("か\u{3099}"), 2);
    }

    #[test]
    fn truncate_never_splits_a_wide_character() {
        assert_eq!(NARROW.truncate("日本語", 3), ("日".into(), 2));
        assert_eq!(NARROW.truncate("日本語", 4), ("日本".into(), 4));
    }

    #[test]
    fn slice_columns_on_ascii() {
        assert_eq!(NARROW.slice_columns("abcdef", 2, 3), "cde");
    }

    #[test]
    fn slice_columns_replaces_a_straddling_wide_char_with_a_space() {
        // "日本語" occupies columns 0-1, 2-3, 4-5. Starting at column 1 cuts
        // 日 in half.
        assert_eq!(NARROW.slice_columns("日本語", 1, 4), " 本 ");
    }

    #[test]
    fn slice_columns_result_is_exactly_the_window_width() {
        for start in 0..8 {
            for len in 0..8 {
                let s = NARROW.slice_columns("あiうeお", start, len);
                let full = NARROW.str("あiうeお");
                let expected = len.min(full.saturating_sub(start));
                assert_eq!(NARROW.str(&s), expected, "start={start} len={len}");
            }
        }
    }

    #[test]
    fn slice_columns_past_the_end_is_empty() {
        assert_eq!(NARROW.slice_columns("abc", 10, 5), "");
    }

    #[test]
    fn tabs_expand_to_the_next_stop() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tx"), "ab  x");
        assert_eq!(expand_tabs("abcd\tx"), "abcd    x");
    }

    #[test]
    fn a_tab_stop_is_counted_in_columns_not_characters() {
        // `日` is two columns, so the tab after it advances from column two.
        // Counting characters would emit three spaces and put everything after
        // it one column right of where the terminal draws it.
        assert_eq!(expand_tabs("日\tx"), "日  x");
        assert_eq!(NARROW.str(&expand_tabs("日\tx")), NARROW.str("abcd") + 1);
        // A zero-width mark does not advance the stop either.
        assert_eq!(expand_tabs("か\u{3099}\tx"), "か\u{3099}  x");
    }
}
