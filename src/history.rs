//! Pages you have been to, for the open prompt's completion. One line per
//! visit is appended to `~/.gaze/history`: time, URL, a tab, the title.
//! The file is folded into one entry per URL when it is read, and
//! rewritten when it has grown past ten thousand lines.

use std::collections::HashMap;
use std::path::PathBuf;

const KEEP: usize = 5000;
const REWRITE_AT: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub last: u64,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub title: String,
    pub bookmark: bool,
}

pub struct History {
    visits: Vec<Visit>,
    path: PathBuf,
}

impl History {
    pub fn load(path: PathBuf) -> History {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut lines = 0;
        let mut by_url: HashMap<String, Visit> = HashMap::new();
        for line in text.lines() {
            lines += 1;
            let mut parts = line.splitn(3, '\t');
            let (Some(when), Some(url), title) = (parts.next(), parts.next(), parts.next().unwrap_or("")) else { continue };
            let when: u64 = when.parse().unwrap_or(0);
            match by_url.get_mut(url) {
                Some(v) => {
                    v.count += 1;
                    if when >= v.last { v.last = when; if !title.is_empty() { v.title = title.to_string(); } }
                }
                None => { by_url.insert(url.to_string(), Visit { url: url.to_string(), title: title.to_string(), last: when, count: 1 }); }
            }
        }
        let mut visits: Vec<Visit> = by_url.into_values().collect();
        visits.sort_by(|a, b| b.last.cmp(&a.last));
        visits.truncate(KEEP);
        let h = History { visits, path };
        if lines > REWRITE_AT { let _ = h.rewrite(); }
        h
    }

    /// Note a visit: one line appended to the file.
    pub fn record(&mut self, url: &str, title: &str) {
        if url.is_empty() || url.starts_with("about:") || url.starts_with("gaze:") || url.starts_with("data:") { return; }
        let now = now();
        match self.visits.iter().position(|v| v.url == url) {
            Some(i) => {
                let mut v = self.visits.remove(i);
                v.last = now;
                v.count += 1;
                if !title.is_empty() { v.title = title.to_string(); }
                self.visits.insert(0, v);
            }
            None => {
                self.visits.insert(0, Visit { url: url.to_string(), title: title.to_string(), last: now, count: 1 });
                self.visits.truncate(KEEP);
            }
        }
        use std::io::Write;
        if let Some(dir) = self.path.parent() { let _ = std::fs::create_dir_all(dir); }
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(&self.path) {
            let _ = writeln!(f, "{}\t{}\t{}", now, url, title.replace(['\t', '\n'], " "));
        }
    }

    /// A page's title often arrives after the visit was noted.
    pub fn retitle(&mut self, url: &str, title: &str) {
        if title.is_empty() { return; }
        if let Some(v) = self.visits.iter_mut().find(|v| v.url == url) {
            if v.title != title { v.title = title.to_string(); }
        }
    }

    /// What to offer for `query`: every word must appear in the URL or
    /// the title. Bookmarks come first, then the most visited and the
    /// most recent. An empty query gives the latest pages.
    pub fn matches(&self, query: &str, bookmarks: &[(String, String)], limit: usize) -> Vec<Candidate> {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        let hit = |url: &str, title: &str| {
            let (u, t) = (url.to_lowercase(), title.to_lowercase());
            words.iter().all(|w| u.contains(w.as_str()) || t.contains(w.as_str()))
        };
        let mut out: Vec<(i64, Candidate)> = Vec::new();
        for (url, title) in bookmarks {
            if !hit(url, title) { continue; }
            let count = self.visits.iter().find(|v| v.url == *url).map(|v| v.count).unwrap_or(0) as i64;
            out.push((1_000_000 + count, Candidate { url: url.clone(), title: title.clone(), bookmark: true }));
        }
        for (rank, v) in self.visits.iter().enumerate() {
            if bookmarks.iter().any(|(u, _)| *u == v.url) || !hit(&v.url, &v.title) { continue; }
            // With nothing typed, the latest pages first; with words, the
            // most visited first.
            let score = if words.is_empty() { -(rank as i64) } else { v.count as i64 * 100 - rank as i64 };
            out.push((score, Candidate { url: v.url.clone(), title: v.title.clone(), bookmark: false }));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out.into_iter().take(limit).map(|(_, c)| c).collect()
    }

    fn rewrite(&self) -> Result<(), String> {
        let mut text = String::new();
        for v in self.visits.iter().rev() {
            for _ in 0..v.count.min(50) {
                text.push_str(&format!("{}\t{}\t{}\n", v.last, v.url, v.title));
            }
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(name: &str) -> History {
        let dir = std::env::temp_dir().join(format!("gaze-hist-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        History::load(dir.join("history"))
    }

    #[test]
    fn visits_fold_by_url_and_come_back_from_the_file() {
        let mut h = fresh("fold");
        h.record("https://isene.org/", "Geir");
        h.record("https://example.com/", "Example");
        h.record("https://isene.org/", "Geir Isene");
        let again = History::load(h.path.clone());
        assert_eq!(again.visits.len(), 2);
        let g = again.visits.iter().find(|v| v.url == "https://isene.org/").unwrap();
        assert_eq!((g.count, g.title.as_str()), (2, "Geir Isene"));
        let _ = std::fs::remove_dir_all(h.path.parent().unwrap());
    }

    #[test]
    fn every_word_must_match_and_bookmarks_lead() {
        let mut h = fresh("match");
        h.record("https://isene.org/blog", "Blog posts");
        h.record("https://isene.org/blog", "Blog posts");
        h.record("https://example.com/free-will", "Free will essay");
        let marks = vec![("https://plato.stanford.edu/free-will".to_string(), "Free Will (Stanford)".to_string())];
        let got = h.matches("free will", &marks, 10);
        assert_eq!(got.len(), 2);
        assert!(got[0].bookmark);
        assert_eq!(got[1].url, "https://example.com/free-will");
        assert_eq!(h.matches("blog isene", &marks, 10).len(), 1);
        assert_eq!(h.matches("blog nothing", &marks, 10).len(), 0);
        let latest = h.matches("", &[], 10);
        assert_eq!(latest[0].url, "https://example.com/free-will", "an empty query lists the latest first");
        let _ = std::fs::remove_dir_all(h.path.parent().unwrap());
    }

    #[test]
    fn internal_pages_are_not_kept() {
        let mut h = fresh("skip");
        h.record("gaze://help", "gaze");
        h.record("about:blank", "");
        assert!(h.visits.is_empty());
        let _ = std::fs::remove_dir_all(h.path.parent().unwrap());
    }
}
