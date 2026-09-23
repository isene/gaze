//! Ad and tracker blocking through WebKit's content filter, built from a
//! hosts list: every domain on the list is blocked, in everything a page
//! pulls in. The list lives in `~/.gaze/adblock/hosts`; WebKit keeps the
//! compiled filter next to it, so the list compiles once.
//!
//! What is never blocked is the page you asked for. A hosts list holds
//! click trackers, and a password reset arrives through one: Discord
//! sends you to `click.discord.com`, which is on the list. Blocking that
//! leaves you looking at nothing with no idea why, so the rules say
//! which kinds of thing they cover and a page is not one of them.

/// Steven Black's unified hosts list: ads and trackers, public domain.
pub const SOURCE: &str = "https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts";

/// What the compiled filter is filed under. The name changes with the
/// shape of the rules, so a filter compiled by an older gaze is left
/// where it lies and a new one is built.
pub const FILTER: &str = "ads-page-safe";

/// Everything a page pulls in, which is everything but the page itself.
const KINDS: &str = r#"["image","style-sheet","script","font","raw","svg-document","media","popup"]"#;

/// WebKit content-filter rules (Safari's JSON shape) that block each
/// domain of a hosts file and its subdomains. Returns the JSON and how
/// many domains it covers.
pub fn rules_from_hosts(text: &str) -> (String, usize) {
    let mut seen = std::collections::HashSet::new();
    let mut rules = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut parts = line.split_whitespace();
        let (Some(addr), Some(host)) = (parts.next(), parts.next()) else { continue };
        if addr != "0.0.0.0" && addr != "127.0.0.1" { continue; }
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if !host.contains('.') || host.starts_with("localhost") || host.ends_with(".local")
            || host.ends_with(".localdomain") || host == "broadcasthost" { continue; }
        if !host.chars().any(|c| c.is_ascii_alphabetic()) { continue; }
        if !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') { continue; }
        if !seen.insert(host.clone()) { continue; }
        let escaped = host.replace('.', "\\\\.");
        rules.push(format!(
            "{{\"trigger\":{{\"url-filter\":\"^[^:]+://+([^:/]+\\\\.)?{}[:/]\",\"resource-type\":{}}},\"action\":{{\"type\":\"block\"}}}}",
            escaped, KINDS));
    }
    let n = rules.len();
    (format!("[{}]", rules.join(",")), n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_lines_become_block_rules_without_the_local_names() {
        let hosts = "# header\n127.0.0.1 localhost\n0.0.0.0 0.0.0.0\n0.0.0.0 ads.example.com # comment\n\
                     0.0.0.0 Ads.Example.com\n0.0.0.0 tracker.net\n::1 ip6-localhost\n0.0.0.0 broadcasthost\n";
        let (json, n) = rules_from_hosts(hosts);
        assert_eq!(n, 2);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rules = parsed.as_array().unwrap();
        assert_eq!(rules.len(), 2);
        let filter = rules[0]["trigger"]["url-filter"].as_str().unwrap();
        assert_eq!(filter, r"^[^:]+://+([^:/]+\.)?ads\.example\.com[:/]");
        assert_eq!(rules[0]["action"]["type"], "block");
        let kinds = rules[0]["trigger"]["resource-type"].as_array().unwrap();
        assert!(kinds.iter().all(|k| k != "document"), "the page you asked for is never blocked");
        assert!(kinds.iter().any(|k| k == "script"), "everything it pulls in still is");
    }
}
