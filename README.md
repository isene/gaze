# gaze

<img src="img/gaze.svg" align="right" width="150">

**Looking out onto the web. A keyboard-driven browser in Rust, around WebKitGTK.**

![Rust](https://img.shields.io/badge/language-Rust-orange) ![Unlicense](https://img.shields.io/badge/license-Unlicense-green) ![Platform](https://img.shields.io/badge/platform-Linux-blue) ![Stay Amazing](https://img.shields.io/badge/Stay-Amazing-important)

gaze is a small shell around the WebKit engine, the way qutebrowser is a shell around Chromium. Keys work like vim and qutebrowser. Tabs can be grouped, named and folded the way Firefox does it. Logins are saved in one encrypted file and filled when a login page loads. The shell itself does nothing while you read. Part of the [Fe₂O₃ Rust terminal suite](https://github.com/isene/fe2o3).

## Keys

Press `?` inside gaze for the full list.

| Key | Does |
|---|---|
| `o` / `O` | Open a URL or a search here / in a new tab (`go`, `gO` start from the current URL) |
| `f` / `F` | Hints: type the letters on a link to follow it / open it in a background tab |
| `H` / `L` | Back / forward |
| `r` | Reload |
| `j` `k` `h` `l`, `gg`, `G`, `Ctrl-d` / `Ctrl-u` | Scroll |
| `/`, `n` / `N` | Find on the page |
| `i`, `gi` | Insert mode (type into the page) / focus the first field. `Esc` leaves |
| `yy`, `pp` / `PP` | Copy the URL; open what the clipboard holds here / in a new tab |
| `t`, `d`, `u` | New tab, close tab, bring back the last closed |
| `J` / `K`, `Alt-1`…`Alt-9` | Next / previous tab, tab by number |
| `zc` / `zo` / `za` | Fold / unfold / toggle the current tab's group; `zM` and `zR` do all |
| `gp` | Fill the login form; again for the next saved login of the site |
| `:` | Command line |

## Tab groups

`:group work` puts the current tab in the group *work*, made on the spot when it is new. Each group has a colour and shows as a coloured run in the tab bar. A tab opened from a grouped tab joins the group. `zc` folds the group away, and `J`/`K` skip its tabs until `zo` unfolds it. `:ungroup`, `:group-rename`, `:group-color`, `:group-close` and `:groups` do what they say. Groups come back with the session at the next start.

## Passwords

Logins live in `~/.gaze/passwords`, one file sealed with ChaCha20-Poly1305 under a key that Argon2id makes from a master password. You choose the master password the first time gaze needs the file. It is asked once per session; `:password-lock` locks again.

- A login page gets filled when it loads, with the login you used last on that site. `gp` cycles through the others.
- After you log in with a new or changed password, gaze asks whether to save it: `y` or `n`.
- `:passwords` lists the sites and usernames. Passwords themselves are never shown.
- `:password-import ~/logins.csv` reads the file Firefox writes from `about:logins` → Export Logins. Delete the CSV afterwards.

## Install

```bash
sudo apt install libwebkitgtk-6.0-dev libgtk-4-dev   # Debian / Ubuntu
cargo install --path .
```

Runtime: WebKitGTK 6.0 and GTK 4. To make gaze the browser other programs open links in, copy `share/gaze.desktop` to `~/.local/share/applications/` and run `xdg-settings set default-web-browser gaze.desktop`. A second `gaze <url>` opens the URL in the running window.

## Files

- `~/.gaze/config.yml`: home page, search engine (`%s` is the query), download folder, zoom, scroll step.
- `~/.gaze/session.json`: the open tabs and groups, written when they change and read at start. Only the current tab loads at start; the others load when you go to them.
- `~/.gaze/passwords`: the sealed logins.
- `~/.gaze/data`, `~/.gaze/cache`: cookies, local storage and the cache, kept by WebKit.

## License

Public domain (Unlicense).
