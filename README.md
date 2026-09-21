# gaze

<img src="img/gaze.svg" align="right" width="150">

**Looking out onto the web. A keyboard-driven browser in Rust, around WebKitGTK.**

![Rust](https://img.shields.io/badge/language-Rust-orange) ![Unlicense](https://img.shields.io/badge/license-Unlicense-green) ![Platform](https://img.shields.io/badge/platform-Linux-blue) ![Stay Amazing](https://img.shields.io/badge/Stay-Amazing-important)

gaze is a small shell around the WebKit engine, the way qutebrowser is a shell around Chromium. Keys work like vim and qutebrowser, and every key is a command you can rebind. Tabs group, fold and come back with the session the way Firefox does it. Logins live in one encrypted file and fill a login page as it loads. Ads and trackers are blocked from a hosts list. The shell itself does nothing while you read. Part of the [Fe₂O₃ Rust terminal suite](https://github.com/isene/fe2o3).

## Keys

Press `?` inside gaze for the full list, with the keys as they are bound right now.

| Key | Does |
|---|---|
| `o` / `O` | Open a URL or a search here / in a new tab (`go`, `gO` start from the current URL). The prompt offers pages from your history and bookmarks; `Tab` picks one |
| `f` / `F` | Hints: type the letters on a link to follow it / open it in a background tab |
| `H` / `L`, `Ctrl-Left` / `Ctrl-Right` | Back / forward in this tab's history |
| `r` | Reload |
| `j` `k` `h` `l`, `gg`, `G`, `Ctrl-d` / `Ctrl-u`, `Space` | Scroll |
| `/`, `n` / `N` | Find on the page |
| `Ctrl-f` | Only the page: hide the tab bar and the status line; again to bring them back |
| `D` | Dark pages on or off for this site, kept for next time (see Dark pages) |
| `i`, `gi` | Insert mode (type into the page) / focus the first field. A click on a field enters it too; `Tab` moves to the next field; `Esc` leaves |
| `yy`, `pp` / `PP` | Copy the URL; open what the clipboard holds here / in a new tab |
| `t`, `d`, `u` | New tab, close tab, bring back the last closed |
| `Left` / `Right`, `J` / `K`, `Alt-1`…`Alt-9` | Previous / next tab, tab by number |
| `Shift-Left` / `Shift-Right` | Move the tab left / right |
| `zc` / `zo` / `za` | Fold / unfold / toggle the current tab's group; `zM` and `zR` do all |
| `M`, `gb` / `gB` | Bookmark this page; the bookmark list here / in a new tab |
| `gp` | Fill the login form; again for the next saved login of the site |
| `v` | Play this page's video in `mpv` (see Video) |
| `:` | Command line |
| `Q`, `ZZ` | Quit |

`Tab` completes on the command line: command names, then group names after `:group` and colours after `:group-color`. `:bind <keys> <command>` changes a binding from the command line and `:unbind <keys>` drops one. The changes go to `~/.gaze/keys.yml`, which you can also edit by hand. Key names follow qutebrowser: plain characters as they are, else `<Ctrl-d>`, `<Alt-1>`, `<Shift-Left>`, `<Space>`, `<Backspace>`.

## Tab groups

`:group work` puts the current tab in the group *work*, made on the spot when it is new. Each group has a colour and shows as a coloured run in the tab bar. A tab opened from a grouped tab joins the group. `zc` folds the group away, and `J`/`K` skip its tabs until `zo` unfolds it. `:ungroup`, `:group-rename <name>`, `:group-color <colour>`, `:group-close` and `:groups` do what they say. A colour is a name (blue red yellow green pink purple orange cyan gray) or `#rrggbb`.

A group with no tabs stays, dimmed at the end of the bar, until `:group-delete`. Groups come back with the session at the next start, and the config can name groups that exist from the start:

```yaml
groups:
  - {name: Work, color: "#5faf87"}
  - {name: Home, color: orange}
```

## Passwords

Logins live in `~/.gaze/passwords`, one file sealed with ChaCha20-Poly1305 under a key that Argon2id makes from a master password. You choose the master password the first time gaze needs the file. It is asked once per session; `:password-lock` locks again.

- A login page gets filled when it loads, with the login you used last on that site. `gp` cycles through the others.
- After you log in with a new or changed password, gaze asks whether to save it: `y` or `n`.
- A site's HTTP password dialog is answered from the store too, when it is open.
- `:passwords` lists the sites and usernames. Passwords themselves are never shown.
- `:password-import ~/logins.csv` reads the file Firefox writes from `about:logins` → Export Logins. gaze deletes the CSV after a successful import.

## History and bookmarks

`o` on its own lists the pages you were at last. Typing narrows the list to pages whose URL or title holds every word you typed, bookmarks (★) first. `Tab` and `Shift-Tab` put one on the line and `Return` opens it. Visits are kept in `~/.gaze/history`, the last five thousand pages.

`M` bookmarks the page; `gb` shows the list, where `f` and the letters open one. `:bookmark-del` forgets the current page. `:bookmark-import ~/bookmarks.html` reads the file Firefox writes from Manage Bookmarks → Import and Backup → Export Bookmarks to HTML. The list is `~/.gaze/bookmarks`, one URL, a tab and a title per line.

## Ad blocking

On by default; `adblock: false` in the config turns it off. The first start fetches [Steven Black's hosts list](https://github.com/StevenBlack/hosts) to `~/.gaze/adblock/hosts` and compiles it into a WebKit content filter, which takes a few seconds once. Every domain on the list is then blocked for every request. `:adblock-update` fetches the list again.

## Dark pages

On by default; `dark: false` in the config turns it off.

`D` turns dark pages on or off for the site you are on, and remembers it. A site you have set keeps its answer on every later visit. Everything else follows the default. `:dark-default` flips that default, and the sites you set keep theirs.

Every site is asked for its dark style first, the way a dark desktop asks. A site that has one uses its own colours. A site with none is turned around instead. The page is inverted and the hues turned back, then pictures and video are inverted a second time. Text goes light on dark, blue links stay blue, photos keep their colours.

A picture set as an element's background is no `<img>`. CSS cannot ask for one, so gaze looks for them as the page settles. A box big enough to hold a picture is turned back. A small one is an icon, part of the writing, and turns with it. A picture behind the whole page is turned back on its own. What sits on it is then turned once more, so the page stays dark.

Turning a page around is a blunt tool. A dark band on an otherwise light page comes out light, so a site with a dark header looks odd. `D` gets you out of it.

It costs something: about a fifth more work in the page for a big article, since every layer is painted twice.

## Video

A video page goes to `mpv` instead of the browser: a click on a YouTube link, a typed or pasted address, a link opened in a new tab. YouTube's own pages stay in gaze; only the watch pages leave. `v` sends the page you are on to `mpv`, for a video you reached inside YouTube itself. `mpv` plays YouTube through `yt-dlp`, and it costs about half the battery of the same video in a browser.

`video_player` in the config names the program, empty keeps videos in gaze, and `video_urls` lists how a video page's address starts.

## Install

```bash
sudo apt install libwebkitgtk-6.0-dev libgtk-4-dev   # Debian / Ubuntu
cargo install --path .
```

Runtime: WebKitGTK 6.0 and GTK 4. gaze drops `gl` from `GDK_DISABLE` for itself: with GL switched off that way, WebKit crashes on pages with video, while a display with no GL at all is fine. `GAZE_KEEP_GDK_DISABLE=1` leaves the variable alone. It also sets `LP_NUM_THREADS=1` unless you did, since llvmpipe on every core costs several times the CPU for the same page, and where the desktop forces software GL with `LIBGL_ALWAYS_SOFTWARE` it sets `WEBKIT_SKIA_ENABLE_CPU_RENDERING=1`, which saves a third more. With a real GPU neither matters. To make gaze the browser other programs open links in, copy `share/gaze.desktop` to `~/.local/share/applications/` and run `xdg-settings set default-web-browser gaze.desktop`. A second `gaze <url>` opens the URL in the running window.

## Files

- `~/.gaze/config.yml`: home page, search engine (`%s` is the query), download folder, zoom, scroll step, ad blocking, dark pages, text size of the bars (`font_size`, in pixels), standing groups.
- `~/.gaze/keys.yml`: the key bindings you changed.
- `~/.gaze/bookmarks`: the bookmarks, one per line.
- `~/.gaze/dark`: the sites where dark pages differ from the default, one `<site> on` or `<site> off` per line.
- `~/.gaze/history`: one line per visit, folded to one entry per page when read.
- `~/.gaze/session.json`: the open tabs and groups, written when they change and read at start. Only the current tab loads at start; the others load when you go to them.
- `~/.gaze/passwords`: the sealed logins.
- `~/.gaze/adblock`: the hosts list and the compiled filter.
- `~/.gaze/data`, `~/.gaze/cache`: cookies, local storage and the cache, kept by WebKit.
  Cookies are accepted from any site, third parties too: Google's sign-in hands you to YouTube through one, and refusing it ends on YouTube's "oops" page.

## License

Public domain (Unlicense).
