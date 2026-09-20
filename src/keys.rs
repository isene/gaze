//! Key bindings: the defaults, your changes in `~/.gaze/keys.yml`, and
//! the name a key press gets so the two can be compared.
//!
//! Names follow qutebrowser: plain characters as they are, everything
//! else in angle brackets: `<Ctrl-d>`, `<Alt-1>`, `<Shift-Left>`,
//! `<Space>`, `<Backspace>`. A literal `<` is `<lt>`.

use gtk4::gdk;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Every default binding. `:bind` and keys.yml override these.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("j", "scroll-down"), ("k", "scroll-up"), ("h", "scroll-left"), ("l", "scroll-right"),
    ("<Down>", "scroll-down"), ("<Up>", "scroll-up"),
    ("gg", "scroll-top"), ("G", "scroll-bottom"), ("<Home>", "scroll-top"), ("<End>", "scroll-bottom"),
    ("<Ctrl-d>", "scroll-page 0.5"), ("<Ctrl-u>", "scroll-page -0.5"),
    ("<Ctrl-f>", "scroll-page 0.9"), ("<Ctrl-b>", "scroll-page -0.9"),
    ("<Space>", "scroll-page 0.9"), ("<Shift-Space>", "scroll-page -0.9"),
    ("<PgDown>", "scroll-page 0.9"), ("<PgUp>", "scroll-page -0.9"),
    ("H", "back"), ("L", "forward"), ("<Backspace>", "back"),
    ("<Ctrl-Left>", "back"), ("<Ctrl-Right>", "forward"),
    ("r", "reload"), ("R", "reload-force"),
    ("o", "cmd open "), ("O", "cmd tabopen "), ("t", "cmd tabopen "),
    ("go", "cmd open {url}"), ("gO", "cmd tabopen {url}"), (":", "cmd "),
    ("/", "find"), ("n", "find-next"), ("N", "find-prev"),
    ("f", "hint"), ("F", "hint-tab"),
    ("i", "insert"), ("gi", "focus-input"),
    ("yy", "yank url"), ("yt", "yank title"), ("pp", "paste"), ("PP", "paste-tab"),
    ("+", "zoom-in"), ("-", "zoom-out"), ("=", "zoom-reset"),
    ("J", "tab-next"), ("K", "tab-prev"), ("<Right>", "tab-next"), ("<Left>", "tab-prev"),
    ("<Shift-Right>", "tab-move +1"), ("<Shift-Left>", "tab-move -1"),
    ("g0", "tab-first"), ("g$", "tab-last"),
    ("<Alt-1>", "tab 1"), ("<Alt-2>", "tab 2"), ("<Alt-3>", "tab 3"), ("<Alt-4>", "tab 4"),
    ("<Alt-5>", "tab 5"), ("<Alt-6>", "tab 6"), ("<Alt-7>", "tab 7"), ("<Alt-8>", "tab 8"), ("<Alt-9>", "tab 9"),
    ("d", "close"), ("u", "undo"),
    ("zc", "group-fold"), ("zo", "group-unfold"), ("za", "group-toggle"),
    ("zM", "groups-fold"), ("zR", "groups-unfold"),
    ("gp", "fill"),
    ("M", "bookmark-add"), ("gb", "open gaze://bookmarks"), ("gB", "tabopen gaze://bookmarks"),
    ("?", "help"), ("Q", "quit"), ("ZZ", "quit"), ("<Ctrl-q>", "quit"),
];

const HEADER: &str = "# Keys you changed, one per line: <keys>: <command>\n\
# An empty command unbinds the keys. :bind and :unbind write this file.\n\
# Names: plain characters as they are, else <Ctrl-d>, <Alt-1>, <Shift-Left>, <Space>.\n";

pub struct Keymap {
    bound: HashMap<String, String>,
    user: BTreeMap<String, String>,
    path: PathBuf,
}

impl Keymap {
    pub fn load(path: PathBuf) -> Keymap {
        let user: BTreeMap<String, String> = std::fs::read_to_string(&path).ok()
            .and_then(|s| serde_yaml::from_str(&s).map_err(|e| eprintln!("gaze: {}: {}", path.display(), e)).ok())
            .unwrap_or_default();
        let mut map = Keymap { bound: HashMap::new(), user, path };
        map.rebuild();
        map
    }

    fn rebuild(&mut self) {
        self.bound = DEFAULTS.iter().map(|(k, c)| (k.to_string(), c.to_string())).collect();
        for (k, c) in &self.user {
            if c.trim().is_empty() { self.bound.remove(k); } else { self.bound.insert(k.clone(), c.clone()); }
        }
    }

    /// The command bound to `seq`, and whether longer bindings start
    /// with it (so the next key may complete one).
    pub fn lookup(&self, seq: &str) -> (Option<String>, bool) {
        let exact = self.bound.get(seq).cloned();
        let more = self.bound.keys().any(|k| k.len() > seq.len() && k.starts_with(seq));
        (exact, more)
    }

    pub fn bind(&mut self, keys: &str, command: &str) -> Result<(), String> {
        if keys.is_empty() || command.trim().is_empty() { return Err("bind <keys> <command>".into()); }
        self.user.insert(keys.to_string(), command.to_string());
        self.rebuild();
        self.save()
    }

    pub fn unbind(&mut self, keys: &str) -> Result<bool, String> {
        let had = self.bound.contains_key(keys);
        if DEFAULTS.iter().any(|(k, _)| *k == keys) {
            self.user.insert(keys.to_string(), String::new());
        } else {
            self.user.remove(keys);
        }
        self.rebuild();
        self.save()?;
        Ok(had)
    }

    /// The keys bound to a command, by its first word, for the help page.
    pub fn keys_for(&self, command: &str) -> Vec<String> {
        let mut out: Vec<(String, String)> = self.bound.iter()
            .filter(|(_, c)| c.split_whitespace().next() == Some(command))
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.len().cmp(&b.0.len())).then(a.0.cmp(&b.0)));
        out.into_iter().map(|(k, c)| {
            let rest = c.trim_start_matches(command).trim();
            if rest.is_empty() { k } else { format!("{} ({})", k, rest) }
        }).collect()
    }

    fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
        let body = if self.user.is_empty() { "{}\n".to_string() } else { serde_yaml::to_string(&self.user).map_err(|e| e.to_string())? };
        std::fs::write(&self.path, format!("{}{}", HEADER, body)).map_err(|e| e.to_string())
    }
}

/// The name of a key press, or None for a press that binds to nothing
/// (a bare modifier, a dead key).
pub fn key_name(key: gdk::Key, state: gdk::ModifierType) -> Option<String> {
    use gdk::Key;
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let alt = state.contains(gdk::ModifierType::ALT_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    let special = match key {
        Key::Left => "Left", Key::Right => "Right", Key::Up => "Up", Key::Down => "Down",
        Key::Page_Up => "PgUp", Key::Page_Down => "PgDown", Key::Home => "Home", Key::End => "End",
        Key::BackSpace => "Backspace", Key::Return | Key::KP_Enter => "Return", Key::Tab | Key::ISO_Left_Tab => "Tab",
        Key::space => "Space", Key::Delete => "Del", Key::Insert => "Ins", Key::Escape => "Esc",
        Key::F1 => "F1", Key::F2 => "F2", Key::F3 => "F3", Key::F4 => "F4", Key::F5 => "F5", Key::F6 => "F6",
        Key::F7 => "F7", Key::F8 => "F8", Key::F9 => "F9", Key::F10 => "F10", Key::F11 => "F11", Key::F12 => "F12",
        _ => "",
    };
    let mods = format!("{}{}", if ctrl { "Ctrl-" } else { "" }, if alt { "Alt-" } else { "" });
    if !special.is_empty() {
        return Some(format!("<{}{}{}>", mods, if shift { "Shift-" } else { "" }, special));
    }
    let c = key.to_unicode()?;
    if c.is_control() { return None; }
    if ctrl || alt { return Some(format!("<{}{}>", mods, c)); }
    if c == '<' { return Some("<lt>".into()); }
    Some(c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(name: &str) -> Keymap {
        let dir = std::env::temp_dir().join(format!("gaze-keys-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Keymap::load(dir.join("keys.yml"))
    }

    #[test]
    fn a_prefix_waits_and_a_full_sequence_runs() {
        let m = fresh("prefix");
        assert_eq!(m.lookup("g"), (None, true));
        assert_eq!(m.lookup("gg"), (Some("scroll-top".into()), false));
        assert_eq!(m.lookup("gx"), (None, false));
        assert_eq!(m.lookup("<Shift-Left>"), (Some("tab-move -1".into()), false));
    }

    #[test]
    fn a_binding_survives_a_reload_and_an_unbind_hides_a_default() {
        let mut m = fresh("bind");
        m.bind("x", "close").unwrap();
        m.unbind("Q").unwrap();
        let again = Keymap::load(m.path.clone());
        assert_eq!(again.lookup("x").0, Some("close".into()));
        assert_eq!(again.lookup("Q").0, None);
        assert_eq!(again.lookup("ZZ").0, Some("quit".into()));
        assert!(again.keys_for("close").contains(&"x".to_string()));
        let _ = std::fs::remove_dir_all(m.path.parent().unwrap());
    }

    #[test]
    fn key_presses_get_qutebrowser_names() {
        let none = gdk::ModifierType::empty();
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        let shift = gdk::ModifierType::SHIFT_MASK;
        assert_eq!(key_name(gdk::Key::j, none).as_deref(), Some("j"));
        assert_eq!(key_name(gdk::Key::J, shift).as_deref(), Some("J"));
        assert_eq!(key_name(gdk::Key::d, ctrl).as_deref(), Some("<Ctrl-d>"));
        assert_eq!(key_name(gdk::Key::Left, shift).as_deref(), Some("<Shift-Left>"));
        assert_eq!(key_name(gdk::Key::_1, gdk::ModifierType::ALT_MASK).as_deref(), Some("<Alt-1>"));
        assert_eq!(key_name(gdk::Key::space, none).as_deref(), Some("<Space>"));
        assert_eq!(key_name(gdk::Key::less, shift).as_deref(), Some("<lt>"));
        assert_eq!(key_name(gdk::Key::Shift_L, shift), None);
    }
}
