//! YAML front matter, as far as it needs to be understood.
//!
//! GitHub draws front matter as a table rather than as the source, which is
//! what makes an ADR readable: the status and the date are the first things you
//! want, and a wall of `key: value` is not a good way to read them.
//!
//! Only the shapes that appear at the top of a document are parsed — scalars,
//! sequences, and comments. Anything else returns [`None`] and the caller falls
//! back to showing the source, the same bargain the HTML subset makes. Front
//! matter here is only ever displayed, so the cost of not understanding
//! something is that you see it as written.

/// A front matter key and its value, already flattened for display.
pub type Entry = (String, String);

/// Parse the subset, or [`None`] if anything in it is not understood.
///
/// The delimiters may or may not be part of `text` depending on who is asking,
/// so they are skipped rather than required.
pub fn parse(text: &str) -> Option<Vec<Entry>> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let trimmed = line.trim_end();
        // Delimiters, blank lines, and whole-line comments carry nothing.
        // Trailing comments are left alone: `title: C# in 2026` is a value, not
        // a value and a comment, and there is no way to tell from here.
        if trimmed.is_empty() || trimmed == "---" || trimmed.trim_start().starts_with('#') {
            continue;
        }
        // Indentation at this point means a nested mapping, a continuation, or
        // a sequence under a key that was already consumed below. None of them
        // are understood here.
        if line.starts_with(' ') || line.starts_with('\t') {
            return None;
        }

        let (key, rest) = trimmed.split_once(':')?;
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        let rest = rest.trim();

        // A block scalar's body is arbitrary text that would have to be folded
        // or kept verbatim; showing the source is the better answer.
        if rest.starts_with('|') || rest.starts_with('>') {
            return None;
        }
        // Anchors, aliases, and tags mean the document refers to itself, which
        // is more than a table can show.
        if rest.starts_with('&') || rest.starts_with('*') || rest.starts_with('!') {
            return None;
        }

        let value = if rest.is_empty() {
            // Either an empty value or a block sequence on the lines below.
            let mut items = Vec::new();
            while let Some(next) = lines.peek() {
                let item = next.trim_start();
                if !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                let Some(item) = item.strip_prefix("- ") else {
                    // Indented and not a sequence entry: a nested mapping.
                    return None;
                };
                items.push(unquote(item.trim()));
                lines.next();
            }
            items.join(", ")
        } else if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            // A flow sequence. Splitting on commas is wrong for a quoted value
            // containing one, which is rare enough at the top of a document to
            // be worth the simplicity.
            inner
                .split(',')
                .map(|item| unquote(item.trim()))
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        } else if rest.starts_with('{') {
            // A flow mapping is a nested structure by another spelling.
            return None;
        } else {
            unquote(rest)
        };

        entries.push((key.to_string(), value));
    }

    (!entries.is_empty()).then_some(entries)
}

/// Strip one matching pair of quotes, leaving anything else as written.
fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(text: &str) -> Vec<(String, String)> {
        parse(text).expect("understood")
    }

    #[test]
    fn scalars_keep_their_order() {
        let got = parsed("---\ntitle: Use PostgreSQL\nstatus: accepted\n---\n");
        assert_eq!(
            got,
            [
                ("title".into(), "Use PostgreSQL".into()),
                ("status".into(), "accepted".into()),
            ]
        );
    }

    #[test]
    fn quotes_come_off() {
        let got = parsed("title: \"Use PostgreSQL\"\nauthor: 'Ada'\n");
        assert_eq!(got[0].1, "Use PostgreSQL");
        assert_eq!(got[1].1, "Ada");
    }

    #[test]
    fn a_colon_in_the_value_is_part_of_the_value() {
        let got = parsed("link: https://example.com/a\n");
        assert_eq!(got[0].1, "https://example.com/a");
    }

    #[test]
    fn both_spellings_of_a_sequence_flatten_the_same_way() {
        let flow = parsed("tags: [db, storage]\n");
        let block = parsed("tags:\n  - db\n  - storage\n");
        assert_eq!(flow[0].1, "db, storage");
        assert_eq!(block, flow);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let got = parsed("# a note\n\nstatus: draft\n");
        assert_eq!(got, [("status".into(), "draft".into())]);
    }

    #[test]
    fn a_trailing_hash_stays_in_the_value() {
        // Stripping it would eat the value of `title: C# in 2026`.
        let got = parsed("title: C# in 2026\n");
        assert_eq!(got[0].1, "C# in 2026");
    }

    #[test]
    fn an_empty_value_is_kept_as_an_empty_cell() {
        let got = parsed("deciders:\nstatus: draft\n");
        assert_eq!(got[0], ("deciders".into(), String::new()));
        assert_eq!(got.len(), 2, "the next key is not swallowed");
    }

    #[test]
    fn what_is_not_understood_is_declined_rather_than_guessed() {
        // Nested mapping.
        assert!(parse("owner:\n  name: Ada\n").is_none());
        // Block scalars.
        assert!(parse("body: |\n  two\n  lines\n").is_none());
        assert!(parse("body: >\n  folded\n").is_none());
        // Flow mapping.
        assert!(parse("owner: {name: Ada}\n").is_none());
        // Anchors and aliases.
        assert!(parse("base: &anchor value\n").is_none());
        // Not a mapping at all.
        assert!(parse("just a line\n").is_none());
        // Nothing in it.
        assert!(parse("---\n---\n").is_none());
    }
}
