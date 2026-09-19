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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            home: "https://duckduckgo.com".into(),
            search: "https://duckduckgo.com/?q=%s".into(),
            downloads: "~/Downloads".into(),
            zoom: 1.0,
            scroll_step: 80,
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
