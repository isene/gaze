//! Bookmarks: one per line in `~/.gaze/bookmarks`, the URL, a tab, the
//! title. A plain text file you can edit by hand.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

pub struct Bookmarks {
    list: Vec<Bookmark>,
    path: PathBuf,
}

impl Bookmarks {
    pub fn load(path: PathBuf) -> Bookmarks {
        let list = std::fs::read_to_string(&path).unwrap_or_default().lines()
            .filter_map(|l| {
                let (url, title) = l.split_once('\t').unwrap_or((l, ""));
                let url = url.trim();
                if url.is_empty() { None } else { Some(Bookmark { url: url.to_string(), title: title.trim().to_string() }) }
            })
            .collect();
        Bookmarks { list, path }
    }

    pub fn list(&self) -> &[Bookmark] { &self.list }
    pub fn has(&self, url: &str) -> bool { self.list.iter().any(|b| b.url == url) }

    /// Add a bookmark, or give a known URL its new title. True when new.
    pub fn add(&mut self, url: &str, title: &str) -> Result<bool, String> {
        let url = url.trim();
        if url.is_empty() { return Err("nothing to bookmark".into()); }
        let title = title.trim();
        let new = match self.list.iter_mut().find(|b| b.url == url) {
            Some(b) => { if !title.is_empty() { b.title = title.to_string(); } false }
            None => { self.list.push(Bookmark { url: url.to_string(), title: title.to_string() }); true }
        };
        self.save()?;
        Ok(new)
    }

    pub fn remove(&mut self, url: &str) -> Result<bool, String> {
        let before = self.list.len();
        self.list.retain(|b| b.url != url.trim());
        if self.list.len() == before { return Ok(false); }
        self.save()?;
        Ok(true)
    }

    /// Import the HTML file Firefox writes from Bookmarks → Manage →
    /// Import and Backup → Export Bookmarks to HTML. Returns how many
    /// were new.
    pub fn import_html(&mut self, html: &str) -> Result<usize, String> {
        let mut added = 0;
        for (url, title) in anchors(html) {
            if url.starts_with("place:") || url.starts_with("javascript:") { continue; }
            if !self.has(&url) {
                self.list.push(Bookmark { url, title });
                added += 1;
            }
        }
        self.save()?;
        Ok(added)
    }

    fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
        let text: String = self.list.iter().map(|b| format!("{}\t{}\n", b.url, b.title)).collect();
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

/// Every `<a href="…">text</a>` in a page, in order.
fn anchors(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut at = 0;
    while let Some(i) = lower[at..].find("<a ") {
        let start = at + i;
        let Some(tag_end) = lower[start..].find('>') else { break };
        let tag = &html[start..start + tag_end];
        let Some(h) = lower[start..start + tag_end].find("href=") else { at = start + tag_end; continue };
        let rest = &tag[h + 5..];
        let quote = rest.chars().next().unwrap_or('"');
        let url = if quote == '"' || quote == '\'' {
            rest[1..].split(quote).next().unwrap_or("")
        } else {
            rest.split([' ', '>']).next().unwrap_or("")
        };
        let body_start = start + tag_end + 1;
        let close = lower[body_start..].find("</a>").map(|c| body_start + c).unwrap_or(html.len());
        let title = unescape(&strip_tags(&html[body_start..close]));
        if !url.is_empty() { out.push((unescape(url), title)); }
        at = close;
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(name: &str) -> Bookmarks {
        let dir = std::env::temp_dir().join(format!("gaze-bm-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Bookmarks::load(dir.join("bookmarks"))
    }

    #[test]
    fn a_bookmark_is_kept_once_and_comes_back_from_the_file() {
        let mut b = fresh("keep");
        assert!(b.add("https://isene.org", "Geir").unwrap());
        assert!(!b.add("https://isene.org", "Geir Isene").unwrap());
        let again = Bookmarks::load(b.path.clone());
        assert_eq!(again.list(), &[Bookmark { url: "https://isene.org".into(), title: "Geir Isene".into() }]);
        let mut again = again;
        assert!(again.remove("https://isene.org").unwrap());
        assert!(!again.remove("https://isene.org").unwrap());
        let _ = std::fs::remove_dir_all(b.path.parent().unwrap());
    }

    #[test]
    fn firefox_html_imports_folders_and_all() {
        let mut b = fresh("import");
        let html = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
    <DT><H3 ADD_DATE="1">Toolbar</H3>
    <DL><p>
        <DT><A HREF="https://isene.org/" ADD_DATE="2" ICON="data:image/png;base64,xx">Geir &amp; co</A>
        <DT><A HREF="place:type=6">Recent tags</A>
        <DT><A HREF='https://example.com/a?b=1&amp;c=2'>Example <b>bold</b></A>
    </DL><p>
</DL><p>"#;
        assert_eq!(b.import_html(html).unwrap(), 2);
        assert_eq!(b.list()[0].title, "Geir & co");
        assert_eq!(b.list()[1].url, "https://example.com/a?b=1&c=2");
        assert_eq!(b.list()[1].title, "Example bold");
        assert_eq!(b.import_html(html).unwrap(), 0);
        let _ = std::fs::remove_dir_all(b.path.parent().unwrap());
    }
}
