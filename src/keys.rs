//! Key bindings.
//!
//! Every binding goes through a [`Keymap`], so the `[keys]` section of the
//! config file is the same mechanism the defaults use rather than a special
//! case bolted on beside them. Naming an action in the config replaces *all* of
//! its default bindings, which is what makes it possible to unbind something by
//! giving it an empty list.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ScrollDown,
    ScrollUp,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    Top,
    Bottom,
    ScrollLeft,
    ScrollRight,
    ResetScroll,
    ToggleWrap,
    ToggleSource,
    CycleTheme,
    ToggleBigHeadings,
    ToggleImages,
    CursorNext,
    CursorPrev,
    YankPrefix,
    YankRendered,
    SelectLines,
    LinkPick,
    OpenUnderCursor,
    SearchForward,
    SearchBackward,
    NextMatch,
    PrevMatch,
    Reload,
    Contents,
    Help,
}

/// The default bindings, which are also the ones the README documents.
pub const DEFAULTS: &[(Action, &[&str])] = &[
    (Action::Quit, &["q", "Esc"]),
    (Action::ScrollDown, &["j", "Down"]),
    (Action::ScrollUp, &["k", "Up"]),
    (Action::HalfPageDown, &["d"]),
    (Action::HalfPageUp, &["u"]),
    (Action::PageDown, &["f", "Space", "PgDn"]),
    (Action::PageUp, &["b", "PgUp"]),
    (Action::Top, &["g", "Home"]),
    (Action::Bottom, &["G", "End"]),
    (Action::ScrollLeft, &["h", "Left"]),
    (Action::ScrollRight, &["l", "Right"]),
    (Action::ResetScroll, &["0"]),
    (Action::ToggleWrap, &["w"]),
    (Action::ToggleSource, &["s"]),
    (Action::CycleTheme, &["t"]),
    (Action::ToggleBigHeadings, &["z"]),
    (Action::ToggleImages, &["i"]),
    (Action::CursorNext, &["Tab"]),
    (Action::CursorPrev, &["Shift-Tab"]),
    (Action::YankPrefix, &["y"]),
    (Action::YankRendered, &["Y"]),
    (Action::SelectLines, &["V"]),
    (Action::LinkPick, &["F"]),
    (Action::OpenUnderCursor, &["o", "Enter"]),
    (Action::SearchForward, &["/"]),
    (Action::SearchBackward, &["?"]),
    (Action::NextMatch, &["n"]),
    (Action::PrevMatch, &["N"]),
    (Action::Reload, &["r"]),
    (Action::Contents, &["T"]),
    (Action::Help, &["H"]),
];

impl Action {
    /// The name used in the config file, in snake_case.
    pub fn name(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::ScrollDown => "scroll_down",
            Action::ScrollUp => "scroll_up",
            Action::HalfPageDown => "half_page_down",
            Action::HalfPageUp => "half_page_up",
            Action::PageDown => "page_down",
            Action::PageUp => "page_up",
            Action::Top => "top",
            Action::Bottom => "bottom",
            Action::ScrollLeft => "scroll_left",
            Action::ScrollRight => "scroll_right",
            Action::ResetScroll => "reset_scroll",
            Action::ToggleWrap => "toggle_wrap",
            Action::ToggleSource => "toggle_source",
            Action::CycleTheme => "cycle_theme",
            Action::ToggleBigHeadings => "toggle_big_headings",
            Action::ToggleImages => "toggle_images",
            Action::CursorNext => "cursor_next",
            Action::CursorPrev => "cursor_prev",
            Action::YankPrefix => "yank",
            Action::YankRendered => "yank_rendered",
            Action::SelectLines => "select_lines",
            Action::LinkPick => "link_pick",
            Action::OpenUnderCursor => "open",
            Action::SearchForward => "search_forward",
            Action::SearchBackward => "search_backward",
            Action::NextMatch => "next_match",
            Action::PrevMatch => "prev_match",
            Action::Reload => "reload",
            Action::Contents => "contents",
            Action::Help => "help",
        }
    }

    pub fn from_name(name: &str) -> Option<Action> {
        DEFAULTS
            .iter()
            .map(|(action, _)| *action)
            .find(|a| a.name() == name)
    }
}

/// Parse a key spec such as `q`, `Esc`, `Shift-Tab`, or `Ctrl-d`.
pub fn parse_key(spec: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut modifiers = KeyModifiers::NONE;
    let mut rest = spec;

    loop {
        let Some((head, tail)) = rest.split_once(['-', '+']) else {
            break;
        };
        // A bare "-" or "+" is the key itself, not a modifier separator.
        let modifier = match head.to_ascii_lowercase().as_str() {
            "ctrl" | "c" => KeyModifiers::CONTROL,
            "alt" | "meta" | "a" | "m" => KeyModifiers::ALT,
            "shift" | "s" => KeyModifiers::SHIFT,
            _ => break,
        };
        modifiers |= modifier;
        rest = tail;
    }

    let code = match rest.to_ascii_lowercase().as_str() {
        "esc" | "escape" => KeyCode::Esc,
        "enter" | "return" | "cr" => KeyCode::Enter,
        "space" => KeyCode::Char(' '),
        // Shift-Tab arrives as its own key code, so the modifier is folded in.
        "tab" if modifiers.contains(KeyModifiers::SHIFT) => {
            return Some((KeyCode::BackTab, KeyModifiers::NONE));
        }
        "tab" => KeyCode::Tab,
        "backtab" => return Some((KeyCode::BackTab, KeyModifiers::NONE)),
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        _ => {
            let mut chars = rest.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
    };
    Some((code, modifiers))
}

/// Normalise an event for lookup.
///
/// A capital letter arrives as `Char('G')` with SHIFT set on some terminals and
/// without it on others, so the shift flag is dropped for characters and the
/// case of the character carries the distinction instead.
fn normalise(code: KeyCode, mut modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    // BackTab already *is* shift-tab, but terminals disagree on whether they
    // also set the flag.
    if matches!(code, KeyCode::Char(_) | KeyCode::BackTab) {
        modifiers.remove(KeyModifiers::SHIFT);
    }
    modifiers.remove(KeyModifiers::NONE);
    (code, modifiers)
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: Vec<((KeyCode, KeyModifiers), Action)>,
}

impl Keymap {
    /// Build the map from the defaults, with any configured overrides applied.
    ///
    /// Returns the map plus a list of complaints about specs that could not be
    /// understood, so a typo in the config is reported rather than silently
    /// leaving a key unbound.
    pub fn new(overrides: &BTreeMap<String, Vec<String>>) -> (Keymap, Vec<String>) {
        let mut map = Keymap::default();
        let mut problems = Vec::new();

        for (name, _) in overrides.iter() {
            if Action::from_name(name).is_none() {
                problems.push(format!("unknown action {name:?} in [keys]"));
            }
        }

        for (action, defaults) in DEFAULTS {
            let specs: Vec<String> = match overrides.get(action.name()) {
                Some(custom) => custom.clone(),
                None => defaults.iter().map(|s| s.to_string()).collect(),
            };
            for spec in specs {
                match parse_key(&spec) {
                    Some((code, modifiers)) => {
                        map.bindings.push((normalise(code, modifiers), *action));
                    }
                    None => problems.push(format!("cannot parse key {spec:?}")),
                }
            }
        }
        (map, problems)
    }

    pub fn lookup(&self, key: &KeyEvent) -> Option<Action> {
        let wanted = normalise(key.code, key.modifiers);
        self.bindings
            .iter()
            .find(|(binding, _)| *binding == wanted)
            .map(|(_, action)| *action)
    }

    /// Every key currently bound to an action, for the help screen.
    pub fn keys_for(&self, action: Action) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|((code, modifiers), _)| describe(*code, *modifiers))
            .collect()
    }
}

fn describe(code: KeyCode, modifiers: KeyModifiers) -> String {
    let mut out = String::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        out.push_str("Ctrl-");
    }
    if modifiers.contains(KeyModifiers::ALT) {
        out.push_str("Alt-");
    }
    match code {
        KeyCode::Char(' ') => out.push_str("Space"),
        KeyCode::Char(c) => out.push(c),
        KeyCode::BackTab => out.push_str("Shift-Tab"),
        other => out.push_str(&format!("{other:?}")),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn defaults() -> Keymap {
        let (map, problems) = Keymap::new(&BTreeMap::new());
        assert!(problems.is_empty(), "{problems:?}");
        map
    }

    #[test]
    fn every_default_spec_parses() {
        for (action, specs) in DEFAULTS {
            for spec in *specs {
                assert!(parse_key(spec).is_some(), "{}: {spec:?}", action.name());
            }
        }
    }

    #[test]
    fn every_action_has_a_unique_name() {
        let mut names: Vec<&str> = DEFAULTS.iter().map(|(a, _)| a.name()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate action name");
    }

    #[test]
    fn plain_characters_look_themselves_up() {
        let map = defaults();
        assert_eq!(
            map.lookup(&key(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Action::ScrollDown)
        );
        assert_eq!(
            map.lookup(&key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn a_capital_matches_whether_or_not_shift_is_reported() {
        let map = defaults();
        assert_eq!(
            map.lookup(&key(KeyCode::Char('G'), KeyModifiers::NONE)),
            Some(Action::Bottom)
        );
        assert_eq!(
            map.lookup(&key(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Action::Bottom)
        );
    }

    #[test]
    fn a_capital_is_not_confused_with_its_lowercase() {
        let map = defaults();
        assert_eq!(
            map.lookup(&key(KeyCode::Char('g'), KeyModifiers::NONE)),
            Some(Action::Top)
        );
    }

    #[test]
    fn named_keys_parse() {
        assert_eq!(parse_key("Esc"), Some((KeyCode::Esc, KeyModifiers::NONE)));
        assert_eq!(
            parse_key("Space"),
            Some((KeyCode::Char(' '), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("PgDn"),
            Some((KeyCode::PageDown, KeyModifiers::NONE))
        );
    }

    #[test]
    fn shift_tab_is_its_own_key_code() {
        assert_eq!(
            parse_key("Shift-Tab"),
            Some((KeyCode::BackTab, KeyModifiers::NONE))
        );
        let map = defaults();
        // Terminals disagree about whether BackTab also carries the flag.
        assert_eq!(
            map.lookup(&key(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::CursorPrev)
        );
        assert_eq!(
            map.lookup(&key(KeyCode::BackTab, KeyModifiers::NONE)),
            Some(Action::CursorPrev)
        );
    }

    #[test]
    fn modifiers_parse_in_either_spelling() {
        assert_eq!(
            parse_key("Ctrl-d"),
            Some((KeyCode::Char('d'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("ctrl+d"),
            Some((KeyCode::Char('d'), KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn a_bare_hyphen_is_a_key_not_a_separator() {
        assert_eq!(
            parse_key("-"),
            Some((KeyCode::Char('-'), KeyModifiers::NONE))
        );
    }

    #[test]
    fn an_override_replaces_the_default_bindings() {
        let mut overrides = BTreeMap::new();
        overrides.insert("quit".to_string(), vec!["x".to_string()]);
        let (map, problems) = Keymap::new(&overrides);
        assert!(problems.is_empty());
        assert_eq!(
            map.lookup(&key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(Action::Quit)
        );
        assert_eq!(
            map.lookup(&key(KeyCode::Char('q'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn an_empty_list_unbinds_an_action() {
        let mut overrides = BTreeMap::new();
        overrides.insert("quit".to_string(), Vec::new());
        let (map, _) = Keymap::new(&overrides);
        assert_eq!(
            map.lookup(&key(KeyCode::Char('q'), KeyModifiers::NONE)),
            None
        );
        assert!(map.keys_for(Action::Quit).is_empty());
    }

    #[test]
    fn a_typo_in_the_config_is_reported() {
        let mut overrides = BTreeMap::new();
        overrides.insert("qiut".to_string(), vec!["x".to_string()]);
        overrides.insert("reload".to_string(), vec!["NoSuchKey".to_string()]);
        let (_, problems) = Keymap::new(&overrides);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("qiut")));
        assert!(problems.iter().any(|p| p.contains("NoSuchKey")));
    }

    #[test]
    fn keys_for_describes_the_bindings() {
        let map = defaults();
        assert_eq!(
            map.keys_for(Action::PageDown),
            vec!["f", "Space", "PageDown"]
        );
    }
}
