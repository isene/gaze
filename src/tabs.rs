//! Tabs and tab groups, and the session file that brings them back.
//!
//! Groups work like Firefox's: a named, coloured run of tabs that can be
//! folded away. Tabs of one group always sit next to each other.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Group colours, in the order new groups get them.
pub const COLORS: [(&str, &str); 9] = [
    ("blue", "#4a90e2"), ("red", "#e05252"), ("yellow", "#e0b93a"),
    ("green", "#4caf50"), ("pink", "#e06fb0"), ("purple", "#9b6fe0"),
    ("orange", "#f0883e"), ("cyan", "#3ac7c7"), ("gray", "#9aa0a6"),
];

/// A colour name from the list, or a `#rrggbb` value, as `#rrggbb`.
pub fn color_hex(name: &str) -> String {
    if is_hex(name) { return name.to_ascii_lowercase(); }
    COLORS.iter().find(|(n, _)| *n == name).map(|(_, hex)| hex.to_string()).unwrap_or_else(|| "#9aa0a6".into())
}

fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Group {
    pub id: u64,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Tab {
    pub id: u64,
    pub uri: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub group: Option<u64>,
    /// The tab this one was opened from as a popup; closing returns there.
    #[serde(default)]
    pub opener: Option<u64>,
    /// Restored from the session but not loaded yet.
    #[serde(skip)]
    pub pending: bool,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub groups: Vec<Group>,
    pub active: usize,
    #[serde(skip)]
    next_id: u64,
}

impl Tabs {
    pub fn load(path: &Path) -> Tabs {
        let mut t: Tabs = std::fs::read_to_string(path).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        t.next_id = t.tabs.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        for tab in &mut t.tabs { tab.pending = true; }
        if t.active >= t.tabs.len() { t.active = t.tabs.len().saturating_sub(1); }
        t.prune_groups();
        t
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn len(&self) -> usize { self.tabs.len() }
    pub fn is_empty(&self) -> bool { self.tabs.is_empty() }
    pub fn current(&self) -> Option<&Tab> { self.tabs.get(self.active) }
    pub fn current_mut(&mut self) -> Option<&mut Tab> { self.tabs.get_mut(self.active) }
    pub fn index_of(&self, id: u64) -> Option<usize> { self.tabs.iter().position(|t| t.id == id) }
    pub fn group_by_id(&self, id: u64) -> Option<&Group> { self.groups.iter().find(|g| g.id == id) }
    pub fn group_by_name(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name.eq_ignore_ascii_case(name))
    }
    pub fn tabs_in(&self, gid: u64) -> Vec<usize> {
        self.tabs.iter().enumerate().filter(|(_, t)| t.group == Some(gid)).map(|(i, _)| i).collect()
    }

    /// Open a tab after the current one. It joins the current tab's group,
    /// the way a link opened from a grouped tab does in Firefox.
    pub fn open(&mut self, uri: &str, background: bool) -> u64 {
        self.open_from(uri, background, None)
    }

    /// Open a tab that knows its opener: a popup the page asked for.
    pub fn open_from(&mut self, uri: &str, background: bool, opener: Option<u64>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let group = self.current().and_then(|t| t.group);
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        self.tabs.insert(at, Tab { id, uri: uri.to_string(), title: String::new(), group, opener, pending: false });
        if !background || self.tabs.len() == 1 { self.active = at; }
        id
    }

    /// Close the tab at `idx`. The tab to its right becomes current, or the
    /// last one when it was the last.
    pub fn close(&mut self, idx: usize) -> Option<Tab> {
        if idx >= self.tabs.len() { return None; }
        let was_current = idx == self.active;
        let tab = self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.active = 0;
        } else if idx < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1).min(self.tabs.len() - 1);
        }
        // A popup that closes hands the focus back to the page that opened it.
        if was_current {
            if let Some(back) = tab.opener.and_then(|o| self.index_of(o)) { self.active = back; }
        }
        Some(tab)
    }

    /// The group called `name`, made when missing. A colour given here
    /// (a name from the list or `#rrggbb`) is applied; an empty one leaves
    /// the group's colour alone, or picks the first unused for a new group.
    pub fn ensure_group(&mut self, name: &str, color: &str) -> u64 {
        let name = name.trim();
        if let Some(g) = self.groups.iter_mut().find(|g| g.name.eq_ignore_ascii_case(name)) {
            if is_hex(color) || COLORS.iter().any(|(n, _)| *n == color) { g.color = color.to_string(); }
            return g.id;
        }
        let id = self.groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
        let used: Vec<&str> = self.groups.iter().map(|g| g.color.as_str()).collect();
        let color = if is_hex(color) || COLORS.iter().any(|(n, _)| *n == color) { color.to_string() } else {
            COLORS.iter().map(|(n, _)| *n).find(|n| !used.contains(n))
                .unwrap_or(COLORS[self.groups.len() % COLORS.len()].0).to_string()
        };
        self.groups.push(Group { id, name: name.to_string(), color, collapsed: false });
        id
    }

    /// Drop a group that has no tabs. False when it has some, or is unknown.
    pub fn delete_group(&mut self, name: &str) -> Result<(), String> {
        let Some(g) = self.group_by_name(name) else { return Err(format!("No group called {}", name.trim())) };
        let (id, n) = (g.id, self.tabs_in(g.id).len());
        if n > 0 { return Err(format!("{} still has {} tabs; :group-close closes them", name.trim(), n)); }
        self.groups.retain(|g| g.id != id);
        Ok(())
    }

    pub fn empty_groups(&self) -> Vec<&Group> {
        self.groups.iter().filter(|g| self.tabs_in(g.id).is_empty()).collect()
    }

    /// Put the tab at `idx` in the named group, creating the group when it
    /// is new. The tab moves next to the group's other tabs.
    pub fn set_group(&mut self, idx: usize, name: &str) -> Option<u64> {
        if idx >= self.tabs.len() || name.trim().is_empty() { return None; }
        let gid = self.ensure_group(name, "");
        if self.tabs[idx].group == Some(gid) { return Some(gid); }
        let was_active = self.tabs[self.active].id;
        let mut tab = self.tabs.remove(idx);
        tab.group = Some(gid);
        let at = match self.tabs.iter().rposition(|t| t.group == Some(gid)) {
            Some(last) => last + 1,
            None => idx.min(self.tabs.len()),
        };
        self.tabs.insert(at, tab);
        self.active = self.index_of(was_active).unwrap_or(0);
        Some(gid)
    }

    /// Take the tab at `idx` out of its group. It lands right after the
    /// group so the group stays in one piece.
    pub fn ungroup(&mut self, idx: usize) {
        let Some(gid) = self.tabs.get(idx).and_then(|t| t.group) else { return };
        let was_active = self.tabs[self.active].id;
        let mut tab = self.tabs.remove(idx);
        tab.group = None;
        let at = self.tabs.iter().rposition(|t| t.group == Some(gid)).map(|l| l + 1).unwrap_or(idx);
        self.tabs.insert(at, tab);
        self.active = self.index_of(was_active).unwrap_or(0);
    }

    pub fn rename_group(&mut self, gid: u64, name: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) { g.name = name.trim().to_string(); }
    }

    pub fn recolor_group(&mut self, gid: u64, color: &str) -> bool {
        if !is_hex(color) && !COLORS.iter().any(|(n, _)| *n == color) { return false; }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) { g.color = color.to_string(); }
        true
    }

    pub fn collapse(&mut self, gid: u64, on: bool) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == gid) { g.collapsed = on; }
    }

    pub fn collapse_all(&mut self, on: bool) {
        for g in &mut self.groups { g.collapsed = on; }
    }

    /// A tab shows in the bar unless its group is folded. The current tab
    /// always shows.
    pub fn visible(&self, idx: usize) -> bool {
        if idx == self.active { return true; }
        match self.tabs.get(idx).and_then(|t| t.group).and_then(|g| self.group_by_id(g)) {
            Some(g) => !g.collapsed,
            None => true,
        }
    }

    pub fn visible_indices(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.visible(i)).collect()
    }

    /// The next visible tab in direction `dir` (1 or -1), wrapping round.
    pub fn neighbour(&self, dir: i32) -> usize {
        let vis = self.visible_indices();
        if vis.len() < 2 { return self.active; }
        let pos = vis.iter().position(|&i| i == self.active).unwrap_or(0) as i32;
        let n = vis.len() as i32;
        vis[((pos + dir) % n + n) as usize % vis.len()]
    }

    pub fn move_tab(&mut self, idx: usize, delta: i32) {
        let n = self.tabs.len() as i32;
        if n < 2 || idx as i32 >= n { return; }
        let to = ((idx as i32 + delta) % n + n) % n;
        let mut tab = self.tabs.remove(idx);
        let to = to as usize;
        // Landing next to its own group keeps the tab in it, so a group's
        // first tab can move to the front. Landing between two tabs of
        // another group joins that one; anywhere else leaves the group.
        let before = if to > 0 { self.tabs.get(to - 1).and_then(|t| t.group) } else { None };
        let after = self.tabs.get(to).and_then(|t| t.group);
        let own = tab.group;
        tab.group = if own.is_some() && (before == own || after == own) { own }
            else if before.is_some() && before == after { before }
            else { None };
        self.tabs.insert(to, tab);
        if self.active == idx { self.active = to; }
        else if idx < self.active && to >= self.active { self.active -= 1; }
        else if idx > self.active && to <= self.active { self.active += 1; }
    }

    /// A tab that points at a group the file no longer has loses it.
    fn prune_groups(&mut self) {
        for t in &mut self.tabs {
            if let Some(g) = t.group { if !self.groups.iter().any(|x| x.id == g) { t.group = None; } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Tabs {
        let mut t = Tabs::default();
        t.next_id = 1;
        t.open("a", false);
        t.open("b", false);
        t.open("c", false);
        t
    }

    #[test]
    fn a_new_tab_opens_after_the_current_one_and_joins_its_group() {
        let mut t = three();
        t.active = 0;
        t.set_group(0, "work");
        t.open("d", false);
        let order: Vec<&str> = t.tabs.iter().map(|x| x.uri.as_str()).collect();
        assert_eq!(order, ["a", "d", "b", "c"]);
        assert_eq!(t.active, 1);
        assert_eq!(t.tabs[1].group, t.tabs[0].group);
        t.open("e", true);
        assert_eq!(t.active, 1, "a background tab leaves the current one alone");
    }

    #[test]
    fn grouping_keeps_a_group_in_one_run() {
        let mut t = three();
        t.set_group(0, "news");
        t.set_group(2, "news");
        let order: Vec<&str> = t.tabs.iter().map(|x| x.uri.as_str()).collect();
        assert_eq!(order, ["a", "c", "b"]);
        assert_eq!(t.groups.len(), 1);
        assert_eq!(t.tabs[0].group, t.tabs[1].group);
        assert_eq!(t.tabs[2].group, None);
        t.ungroup(0);
        let order: Vec<&str> = t.tabs.iter().map(|x| x.uri.as_str()).collect();
        assert_eq!(order, ["c", "a", "b"]);
        t.ungroup(0);
        assert_eq!(t.groups.len(), 1, "an empty group stays until deleted");
        assert!(t.delete_group("news").is_ok());
        assert!(t.groups.is_empty());
    }

    #[test]
    fn a_group_can_be_made_ahead_with_a_hex_colour() {
        let mut t = three();
        let id = t.ensure_group("Dualog", "#5faf87");
        assert_eq!(t.empty_groups().len(), 1);
        assert_eq!(color_hex(&t.groups[0].color), "#5faf87");
        assert_eq!(t.delete_group("Dualog"), Ok(()));
        t.ensure_group("Dualog", "");
        t.set_group(0, "dualog");
        assert_eq!(t.delete_group("Dualog").unwrap_err(), "Dualog still has 1 tabs; :group-close closes them");
        assert!(t.recolor_group(t.tabs[0].group.unwrap(), "#D78700"));
        assert!(!t.recolor_group(t.tabs[0].group.unwrap(), "#12345"));
        let _ = id;
    }

    #[test]
    fn a_folded_group_hides_all_but_the_current_tab() {
        let mut t = three();
        t.set_group(0, "x");
        t.set_group(1, "x");
        t.active = 2;
        let gid = t.groups[0].id;
        t.collapse(gid, true);
        assert_eq!(t.visible_indices(), [2]);
        assert_eq!(t.neighbour(1), 2, "nothing else to go to");
        t.active = 0;
        assert_eq!(t.visible_indices(), [0, 2]);
        assert_eq!(t.neighbour(1), 2);
        assert_eq!(t.neighbour(-1), 2);
    }

    #[test]
    fn closing_moves_right_then_left_at_the_end() {
        let mut t = three();
        t.active = 1;
        t.close(1);
        assert_eq!(t.tabs[t.active].uri, "c");
        t.close(1);
        assert_eq!(t.tabs[t.active].uri, "a");
        t.close(0);
        assert!(t.is_empty());
        assert_eq!(t.active, 0);
    }

    #[test]
    fn a_popup_returns_to_its_opener_when_it_closes() {
        let mut t = three();
        t.active = 0;
        let opener = t.tabs[0].id;
        t.open_from("popup", false, Some(opener));
        assert_eq!(t.active, 1);
        t.close(1);
        assert_eq!(t.tabs[t.active].uri, "a", "back to the opener, not to the next tab");
    }

    #[test]
    fn closing_before_the_current_tab_keeps_it_current() {
        let mut t = three();
        t.active = 2;
        t.close(0);
        assert_eq!(t.tabs[t.active].uri, "c");
    }

    #[test]
    fn new_groups_take_unused_colours() {
        let mut t = three();
        t.set_group(0, "one");
        t.set_group(1, "two");
        assert_eq!(t.groups[0].color, "blue");
        assert_eq!(t.groups[1].color, "red");
        assert!(t.recolor_group(t.groups[0].id, "green"));
        assert!(!t.recolor_group(t.groups[0].id, "mauve"));
    }

    #[test]
    fn a_session_round_trips_with_ids_kept_apart() {
        let mut t = three();
        t.set_group(1, "keep");
        t.active = 1;
        let dir = std::env::temp_dir().join(format!("gaze-test-{}", std::process::id()));
        let path = dir.join("session.json");
        t.save(&path).unwrap();
        let back = Tabs::load(&path);
        assert_eq!(back.tabs.iter().map(|x| (x.id, x.uri.clone(), x.group)).collect::<Vec<_>>(),
                   t.tabs.iter().map(|x| (x.id, x.uri.clone(), x.group)).collect::<Vec<_>>());
        assert_eq!(back.groups, t.groups);
        assert_eq!(back.active, 1);
        assert!(back.tabs.iter().all(|x| x.pending));
        let mut back = back;
        let id = back.open("z", false);
        assert!(t.tabs.iter().all(|x| x.id != id), "a restored session never reuses an id");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn moving_a_tab_wraps_and_follows_the_group_it_lands_in() {
        let mut t = three();
        t.set_group(1, "g");
        t.set_group(2, "g");
        t.active = 0;
        t.move_tab(0, 1);
        let order: Vec<&str> = t.tabs.iter().map(|x| x.uri.as_str()).collect();
        assert_eq!(order, ["b", "a", "c"]);
        assert_eq!(t.active, 1);
        assert!(t.tabs[1].group.is_some(), "landing between grouped tabs joins the group");
        t.move_tab(1, -1);
        assert_eq!(t.tabs[0].uri, "a");
        assert!(t.tabs[0].group.is_some(), "moving to the front of its own group keeps the group");
        t.move_tab(0, 2);
        assert_eq!(t.tabs[2].uri, "a");
        assert!(t.tabs[2].group.is_some(), "the end of its own group too");
    }

    #[test]
    fn the_first_group_can_have_its_tabs_reordered() {
        let mut t = three();
        t.set_group(0, "g");
        t.set_group(1, "g");
        t.move_tab(1, -1);
        let order: Vec<(&str, bool)> = t.tabs.iter().map(|x| (x.uri.as_str(), x.group.is_some())).collect();
        assert_eq!(order, [("b", true), ("a", true), ("c", false)]);
    }
}
