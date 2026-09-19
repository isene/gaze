//! Saved logins, kept in one encrypted file: `~/.gaze/passwords`.
//!
//! The file is the JSON list of logins, sealed with ChaCha20-Poly1305
//! under a key that Argon2id derives from the master password. The key
//! lives in memory only while the store is unlocked.

use argon2::Argon2;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAGIC: &[u8] = b"GAZE1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Login {
    pub origin: String,
    pub username: String,
    pub password: String,
    /// Seconds since the epoch of the last fill; the most recent fills first.
    #[serde(default)]
    pub used: u64,
}

#[derive(Debug, PartialEq)]
pub enum Change { New, Updated, Same }

pub struct Store {
    path: PathBuf,
    key: Option<[u8; 32]>,
    salt: [u8; SALT_LEN],
    logins: Vec<Login>,
}

impl Store {
    pub fn new(path: PathBuf) -> Store {
        Store { path, key: None, salt: [0; SALT_LEN], logins: Vec::new() }
    }

    pub fn exists(&self) -> bool { self.path.exists() }
    pub fn unlocked(&self) -> bool { self.key.is_some() }
    pub fn logins(&self) -> &[Login] { &self.logins }

    /// Open the store with the master password, or start a new one when
    /// there is no file yet. Returns how many logins it holds.
    pub fn unlock(&mut self, master: &str) -> Result<usize, String> {
        let Ok(raw) = std::fs::read(&self.path) else {
            let mut salt = [0u8; SALT_LEN];
            OsRng.fill_bytes(&mut salt);
            self.salt = salt;
            self.key = Some(derive(master, &salt)?);
            self.logins.clear();
            self.save()?;
            return Ok(0);
        };
        if raw.len() < MAGIC.len() + SALT_LEN + NONCE_LEN || &raw[..MAGIC.len()] != MAGIC {
            return Err("not a gaze password file".into());
        }
        let salt: [u8; SALT_LEN] = raw[MAGIC.len()..MAGIC.len() + SALT_LEN].try_into().unwrap();
        let nonce = &raw[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
        let sealed = &raw[MAGIC.len() + SALT_LEN + NONCE_LEN..];
        let key = derive(master, &salt)?;
        let plain = ChaCha20Poly1305::new(Key::from_slice(&key))
            .decrypt(Nonce::from_slice(nonce), sealed)
            .map_err(|_| "wrong master password".to_string())?;
        self.logins = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
        self.salt = salt;
        self.key = Some(key);
        Ok(self.logins.len())
    }

    /// Seal the store under a new master password, with a fresh salt.
    pub fn change_master(&mut self, master: &str) -> Result<(), String> {
        if self.key.is_none() { return Err("passwords are locked".into()); }
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let key = derive(master, &salt)?;
        let (old_salt, old_key) = (self.salt, self.key);
        self.salt = salt;
        self.key = Some(key);
        if let Err(e) = self.save() {
            self.salt = old_salt;
            self.key = old_key;
            return Err(e);
        }
        Ok(())
    }

    pub fn lock(&mut self) {
        self.key = None;
        self.logins.clear();
    }

    /// The logins for the site of `uri`, the last used first.
    pub fn for_site(&self, uri: &str) -> Vec<&Login> {
        let site = site_key(uri);
        let mut out: Vec<&Login> = self.logins.iter().filter(|l| site_key(&l.origin) == site).collect();
        out.sort_by(|a, b| b.used.cmp(&a.used).then(a.username.cmp(&b.username)));
        out
    }

    /// Keep a login the page just sent. A known username on the site gets
    /// its password replaced; the same password again changes nothing.
    pub fn remember(&mut self, login: Login) -> Result<Change, String> {
        if self.key.is_none() { return Err("passwords are locked".into()); }
        let site = site_key(&login.origin);
        let change = match self.logins.iter_mut()
            .find(|l| site_key(&l.origin) == site && l.username == login.username)
        {
            Some(l) if l.password == login.password => return Ok(Change::Same),
            Some(l) => { l.password = login.password; l.used = now(); Change::Updated }
            None => { self.logins.push(Login { used: now(), ..login }); Change::New }
        };
        self.save()?;
        Ok(change)
    }

    /// Note that a login was filled, so it comes first next time.
    pub fn touch(&mut self, origin: &str, username: &str) {
        let site = site_key(origin);
        if let Some(l) = self.logins.iter_mut().find(|l| site_key(&l.origin) == site && l.username == username) {
            l.used = now();
            let _ = self.save();
        }
    }

    pub fn remove(&mut self, uri: &str, username: &str) -> Result<bool, String> {
        let site = site_key(uri);
        let before = self.logins.len();
        self.logins.retain(|l| !(site_key(&l.origin) == site && l.username == username));
        if self.logins.len() == before { return Ok(false); }
        self.save()?;
        Ok(true)
    }

    /// Import the CSV Firefox writes from about:logins → Export. Returns
    /// (added, updated).
    pub fn import_csv(&mut self, text: &str) -> Result<(usize, usize), String> {
        if self.key.is_none() { return Err("passwords are locked".into()); }
        let rows = parse_csv(text);
        let Some(header) = rows.first() else { return Err("empty file".into()) };
        let col = |name: &str| header.iter().position(|h| h.trim().eq_ignore_ascii_case(name));
        let (Some(u), Some(us), Some(pw)) = (col("url"), col("username"), col("password")) else {
            return Err("no url, username and password columns; export from Firefox's about:logins".into());
        };
        let used = col("timeLastUsed");
        let (mut added, mut updated) = (0, 0);
        for row in &rows[1..] {
            let (Some(origin), Some(username), Some(password)) = (row.get(u), row.get(us), row.get(pw)) else { continue };
            if origin.is_empty() || password.is_empty() { continue; }
            let when = used.and_then(|i| row.get(i)).and_then(|s| s.parse::<u64>().ok()).map(|ms| ms / 1000).unwrap_or(0);
            let site = site_key(origin);
            match self.logins.iter_mut().find(|l| site_key(&l.origin) == site && l.username == *username) {
                Some(l) => { if l.password != *password { l.password = password.clone(); updated += 1; } }
                None => {
                    self.logins.push(Login { origin: origin.clone(), username: username.clone(), password: password.clone(), used: when });
                    added += 1;
                }
            }
        }
        self.save()?;
        Ok((added, updated))
    }

    fn save(&self) -> Result<(), String> {
        let Some(key) = self.key else { return Err("passwords are locked".into()) };
        let mut nonce = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);
        let plain = serde_json::to_vec(&self.logins).map_err(|e| e.to_string())?;
        let sealed = ChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt(Nonce::from_slice(&nonce), plain.as_ref())
            .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.salt);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
        let tmp = self.path.with_extension("tmp");
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600)
                .open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&out).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

fn derive(master: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    Argon2::default().hash_password_into(master.as_bytes(), salt, &mut key).map_err(|e| e.to_string())?;
    Ok(key)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `scheme://host` of a URI, lower case, without a leading `www.`, so a
/// login saved on www.example.com fills on example.com too.
pub fn site_key(uri: &str) -> String {
    let (scheme, rest) = uri.trim().split_once("://").unwrap_or(("https", uri.trim()));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase())
}

/// A small CSV reader: commas, double quotes, doubled quotes inside a
/// quoted field, and newlines inside quotes.
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') => {
                if chars.peek() == Some(&'"') { field.push('"'); chars.next(); } else { quoted = false; }
            }
            (true, _) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (false, _) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("gaze-pw-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Store::new(dir.join("passwords"))
    }

    #[test]
    fn a_login_survives_lock_and_unlock_and_refuses_the_wrong_master() {
        let mut s = fresh("roundtrip");
        assert_eq!(s.unlock("hunter2").unwrap(), 0);
        let change = s.remember(Login { origin: "https://www.example.com".into(), username: "geir".into(), password: "pw".into(), used: 0 }).unwrap();
        assert_eq!(change, Change::New);
        s.lock();
        assert!(!s.unlocked());
        assert_eq!(s.unlock("nope").unwrap_err(), "wrong master password");
        assert_eq!(s.unlock("hunter2").unwrap(), 1);
        let found = s.for_site("https://example.com/login?next=/");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].password, "pw");
        let _ = std::fs::remove_dir_all(s.path.parent().unwrap());
    }

    #[test]
    fn the_same_password_again_is_no_change_and_a_new_one_updates() {
        let mut s = fresh("update");
        s.unlock("").unwrap();
        let l = |pw: &str| Login { origin: "https://a.no".into(), username: "u".into(), password: pw.into(), used: 0 };
        assert_eq!(s.remember(l("one")).unwrap(), Change::New);
        assert_eq!(s.remember(l("one")).unwrap(), Change::Same);
        assert_eq!(s.remember(l("two")).unwrap(), Change::Updated);
        assert_eq!(s.logins().len(), 1);
        assert!(s.remove("https://a.no/x", "u").unwrap());
        assert!(s.logins().is_empty());
        let _ = std::fs::remove_dir_all(s.path.parent().unwrap());
    }

    #[test]
    fn firefox_csv_imports_and_the_last_used_comes_first() {
        let mut s = fresh("csv");
        s.unlock("m").unwrap();
        let csv = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
                   \"https://accounts.example.com\",\"old\",\"p1\",,\"https://accounts.example.com\",\"{1}\",\"1600000000000\",\"1600000000000\",\"1600000000000\"\n\
                   \"https://accounts.example.com\",\"new\",\"p,2\"\"q\",,\"https://accounts.example.com\",\"{2}\",\"1700000000000\",\"1700000000000\",\"1700000000000\"\n";
        assert_eq!(s.import_csv(csv).unwrap(), (2, 0));
        let found = s.for_site("https://accounts.example.com/signin");
        assert_eq!(found[0].username, "new");
        assert_eq!(found[0].password, "p,2\"q");
        assert_eq!(s.import_csv(csv).unwrap(), (0, 0));
        let _ = std::fs::remove_dir_all(s.path.parent().unwrap());
    }

    #[test]
    fn the_master_password_can_be_changed() {
        let mut s = fresh("master");
        s.unlock("old").unwrap();
        s.remember(Login { origin: "https://a.no".into(), username: "u".into(), password: "p".into(), used: 0 }).unwrap();
        s.change_master("new").unwrap();
        s.lock();
        assert!(s.unlock("old").is_err());
        assert_eq!(s.unlock("new").unwrap(), 1);
        let _ = std::fs::remove_dir_all(s.path.parent().unwrap());
    }

    #[test]
    fn a_site_key_drops_www_port_and_path() {
        assert_eq!(site_key("https://www.Example.com:443/a?b#c"), "https://example.com");
        assert_eq!(site_key("http://user@host.no/"), "http://host.no");
        assert_eq!(site_key("example.org"), "https://example.org");
    }
}
