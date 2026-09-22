//! `~/.gaze/config.yml`, and turning what you type into a URI.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Page for a fresh start and `:home`.
    pub home: String,
    /// Search URL; `%s` is what you typed.
    pub search: String,
    /// Where downloads land.
    pub downloads: String,
    /// Page zoom, 1.0 is 100%.
    pub zoom: f64,
    /// Lines the page scrolls for j and k, in pixels.
    pub scroll_step: i32,
    /// Block the domains of Steven Black's hosts list.
    pub adblock: bool,
    /// Ask pages for their dark style, and turn around the ones that
    /// have none.
    pub dark: bool,
    /// Text size of the tab bar, status bar and command line, in pixels.
    pub font_size: u32,
    /// Tab groups that exist from the start, made when missing.
    pub groups: Vec<GroupSpec>,
    /// The program a video page opens in instead of the browser; empty
    /// keeps videos in gaze.
    pub video_player: String,
    /// Beginnings of the URLs that go to the player.
    pub video_urls: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct GroupSpec {
    pub name: String,
    /// A name from the list (blue red yellow green pink purple orange cyan gray) or `#rrggbb`.
    pub color: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            home: "https://duckduckgo.com".into(),
            search: "https://duckduckgo.com/?q=%s".into(),
            downloads: "~/Downloads".into(),
            zoom: 1.0,
            scroll_step: 80,
            adblock: true,
            dark: true,
            font_size: 14,
            groups: Vec::new(),
            video_player: "mpv".into(),
            video_urls: ["https://www.youtube.com/watch", "https://m.youtube.com/watch", "https://youtu.be/", "https://www.youtube.com/shorts/", "https://vimeo.com/"]
                .iter().map(|s| s.to_string()).collect(),
        }
    }
}

const TEMPLATE: &str = "\
# gaze configuration
home: https://duckduckgo.com
search: https://duckduckgo.com/?q=%s
downloads: ~/Downloads
zoom: 1.0
scroll_step: 80
adblock: true
# Dark pages: ask every site for its dark style, and turn around the ones
# that have none. D toggles it.
dark: true
# Text size of the tab bar, status bar and command line, in pixels:
font_size: 14
# A video page opens in this program instead of the browser; empty keeps it in gaze.
video_player: mpv
# The URLs that count as a video page, by how they start:
video_urls:
  - https://www.youtube.com/watch
  - https://m.youtube.com/watch
  - https://youtu.be/
  - https://www.youtube.com/shorts/
  - https://vimeo.com/
# Tab groups that always exist, with a colour name or #rrggbb:
# groups:
#   - {name: Work, color: '#5faf87'}
";

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

pub fn gaze_dir() -> PathBuf {
    home_dir().join(".gaze")
}

pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None if path == "~" => home_dir(),
        None => PathBuf::from(path),
    }
}

pub fn load() -> Config {
    let path = gaze_dir().join("config.yml");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_yaml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("gaze: {}: {}; using defaults", path.display(), e);
            Config::default()
        }),
        Err(_) => {
            let _ = std::fs::create_dir_all(gaze_dir());
            let _ = std::fs::write(&path, TEMPLATE);
            Config::default()
        }
    }
}

/// Sites where dark pages differ from the default, kept in
/// `~/.gaze/dark`, one `<site> on` or `<site> off` per line.
pub struct DarkSites {
    pub sites: std::collections::BTreeMap<String, bool>,
    path: PathBuf,
    /// The line written at the top of the file, saying what it holds.
    note: &'static str,
}

impl DarkSites {
    pub fn load(path: PathBuf) -> DarkSites {
        DarkSites::load_noted(path, "Sites where dark pages differ from the default in config.yml.")
    }

    /// The same list, under another name and another heading. The
    /// microphone list is one of these.
    pub fn load_noted(path: PathBuf, note: &'static str) -> DarkSites {
        let mut sites = std::collections::BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                let mut word = line.split_whitespace();
                if let (Some(site), Some(state)) = (word.next(), word.next()) {
                    sites.insert(site.to_string(), state == "on");
                }
            }
        }
        DarkSites { sites, path, note }
    }

    pub fn get(&self, site: &str) -> Option<bool> { self.sites.get(site).copied() }

    /// True when at least one site asks for dark pages, whatever the
    /// default says. The page script is needed then.
    pub fn any_on(&self) -> bool { self.sites.values().any(|on| *on) }

    pub fn set(&mut self, site: &str, on: bool) {
        self.sites.insert(site.to_string(), on);
        let mut out = format!("# {}\n", self.note);
        for (site, on) in &self.sites {
            out.push_str(site);
            out.push_str(if *on { " on\n" } else { " off\n" });
        }
        let _ = std::fs::write(&self.path, out);
    }
}

/// Write the dark-mode flag back to config.yml, leaving the rest of the
/// file, comments and all, as it stands.
pub fn save_dark(on: bool) {
    let path = gaze_dir().join("config.yml");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let line = format!("dark: {}", on);
    let mut out: Vec<&str> = Vec::new();
    let mut found = false;
    for l in text.lines() {
        if l.starts_with("dark:") { out.push(&line); found = true; } else { out.push(l); }
    }
    if !found { out.push(&line); }
    let _ = std::fs::write(&path, out.join("\n") + "\n");
}

/// What you typed in the open prompt, as a URI: a URL as it is, a host
/// with https in front, a path as a file, anything else as a search.
pub fn to_uri(input: &str, search: &str) -> String {
    let s = input.trim();
    if s.is_empty() { return "about:blank".into(); }
    if s.contains("://") || s.starts_with("about:") || s.starts_with("gaze:") || s.starts_with("data:") || s.starts_with("mailto:") {
        return s.to_string();
    }
    if s.starts_with('/') || s.starts_with("~/") {
        return format!("file://{}", expand(s).display());
    }
    if s == "localhost" || s.starts_with("localhost:") || s.starts_with("localhost/") {
        return format!("http://{}", s);
    }
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let host_only = host.split(':').next().unwrap_or("");
    let last = host_only.rsplit('.').next().unwrap_or("");
    let looks_like_host = !s.contains(' ')
        && host_only.contains('.')
        && !host_only.starts_with('.')
        && !host_only.ends_with('.')
        && last.chars().all(|c| c.is_ascii_alphabetic())
        && last.len() >= 2;
    if looks_like_host { return format!("https://{}", s); }
    search.replace("%s", &form_encode(s))
}

fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const S: &str = "https://ddg.gg/?q=%s";

    #[test]
    fn typed_text_becomes_a_url_a_file_or_a_search() {
        assert_eq!(to_uri("https://isene.org/x", S), "https://isene.org/x");
        assert_eq!(to_uri("isene.org", S), "https://isene.org");
        assert_eq!(to_uri("isene.org/about?x=1", S), "https://isene.org/about?x=1");
        assert_eq!(to_uri("localhost:8080/a", S), "http://localhost:8080/a");
        assert_eq!(to_uri("/tmp/x.html", S), "file:///tmp/x.html");
        assert_eq!(to_uri("free will", S), "https://ddg.gg/?q=free+will");
        assert_eq!(to_uri("what is 2.5", S), "https://ddg.gg/?q=what+is+2.5");
        assert_eq!(to_uri("æøå", S), "https://ddg.gg/?q=%C3%A6%C3%B8%C3%A5");
        assert_eq!(to_uri("v0.3", S), "https://ddg.gg/?q=v0.3");
        assert_eq!(to_uri("", S), "about:blank");
    }
}
