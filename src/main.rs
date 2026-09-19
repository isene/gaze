//! gaze: looking out onto the web. A keyboard-driven browser around
//! WebKitGTK, with tab groups and saved logins.

mod adblock;
mod bookmarks;
mod config;
mod history;
mod js;
mod keys;
mod passwords;
mod tabs;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use javascriptcore6 as jsc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use webkit6::prelude::*;
use webkit6::{
    CookiePersistentStorage, Credential, CredentialPersistence, Download, FindOptions, LoadEvent,
    NavigationPolicyDecision, NetworkSession, PolicyDecisionType, ResponsePolicyDecision, Settings,
    URISchemeRequest, UserContentFilter, UserContentFilterStore, UserContentInjectedFrames,
    UserContentManager, UserScript, UserScriptInjectionTime, WebContext, WebView,
};

use passwords::{Change, Login, Store};
use tabs::Tabs;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Normal, Insert, Hint, Command, Prompt }

/// What the command line at the bottom is asking for.
#[derive(Clone, Debug)]
enum Ask { Command, Find, GroupName, Master(Then) }

/// What to do once the passwords are unlocked.
#[derive(Clone, Debug)]
enum Then { Fill, Save(Login), Import(String), List, Remove(String) }

#[derive(Clone, Debug)]
enum Prompt { SaveLogin(Login) }

struct Ui {
    window: gtk::ApplicationWindow,
    tabbar: gtk::Label,
    stack: gtk::Stack,
    status: gtk::Label,
    right: gtk::Label,
    entry: gtk::Entry,
    completion: gtk::Label,
}

struct App {
    ui: Ui,
    cfg: config::Config,
    tabs: Tabs,
    views: HashMap<u64, WebView>,
    session: NetworkSession,
    settings: Settings,
    mode: Mode,
    /// Keys typed so far of a two-key command (g, z, y, p, Z).
    keys: String,
    ask: Ask,
    prompt: Option<Prompt>,
    message: String,
    hover: String,
    store: Store,
    /// Which of a site's logins was filled last, per tab.
    fill_at: HashMap<u64, usize>,
    /// Closed tabs, newest last: uri and group name.
    closed: Vec<(String, Option<String>)>,
    find: String,
    session_path: PathBuf,
    save_pending: bool,
    keymap: keys::Keymap,
    marks: bookmarks::Bookmarks,
    hist: history::History,
    /// What the open prompt offers right now, and which one Tab picked.
    candidates: Vec<history::Candidate>,
    selected: Option<usize>,
    /// True while gaze itself writes the command line, so the write does
    /// not count as typing.
    setting_text: bool,
    /// The compiled ad blocker, once it is ready.
    filter: Option<UserContentFilter>,
}

type Shared = Rc<RefCell<App>>;

thread_local! {
    static APP: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&Shared) -> R) -> Option<R> {
    APP.with(|a| a.borrow().as_ref().map(f))
}

fn main() {
    let app = gtk::Application::builder()
        .application_id("org.isene.gaze")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_command_line(|app, cmdline| {
        let args: Vec<String> = cmdline.arguments().iter().skip(1)
            .map(|a| a.to_string_lossy().into_owned()).collect();
        app.activate();
        for arg in args {
            with_app(|s| {
                let uri = config::to_uri(&arg, &s.borrow().cfg.search);
                open_tab(s, &uri, false);
            });
        }
        with_app(|s| {
            let home = s.borrow().cfg.home.clone();
            if s.borrow().tabs.is_empty() { open_tab(s, &home, false); }
        });
        0.into()
    });
    app.connect_activate(|app| {
        match with_app(|s| s.borrow().ui.window.clone()) {
            Some(w) => w.present(),
            None => {
                let shared = build(app);
                APP.with(|a| *a.borrow_mut() = Some(shared));
            }
        }
    });
    app.run();
}

// ---------------------------------------------------------------- setup

fn build(app: &gtk::Application) -> Shared {
    let cfg = config::load();
    let dir = config::gaze_dir();
    let data = dir.join("data");
    let cache = dir.join("cache");
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);

    let session = NetworkSession::new(data.to_str(), cache.to_str());
    if let Some(cm) = session.cookie_manager() {
        cm.set_persistent_storage(&data.join("cookies.sqlite").to_string_lossy(), CookiePersistentStorage::Sqlite);
    }
    let downloads = config::expand(&cfg.downloads);
    session.connect_download_started(move |_, download| on_download(download, downloads.clone()));

    let settings = Settings::new();
    settings.set_enable_developer_extras(true);
    settings.set_enable_smooth_scrolling(false);
    settings.set_javascript_can_open_windows_automatically(false);

    if let Some(ctx) = WebContext::default() {
        ctx.register_uri_scheme("gaze", serve_internal);
    }
    style();

    let window = gtk::ApplicationWindow::builder()
        .application(app).title("gaze").default_width(1280).default_height(860).build();
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let tabbar = gtk::Label::new(None);
    tabbar.set_xalign(0.0);
    tabbar.set_use_markup(true);
    tabbar.set_ellipsize(gtk::pango::EllipsizeMode::End);
    tabbar.add_css_class("tabbar");
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::None);
    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bottom.add_css_class("statusbar");
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_use_markup(true);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let right = gtk::Label::new(None);
    right.set_xalign(1.0);
    right.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    right.set_max_width_chars(80);
    bottom.append(&status);
    bottom.append(&right);
    let entry = gtk::Entry::new();
    entry.add_css_class("cmdline");
    entry.set_visible(false);
    let completion = gtk::Label::new(None);
    completion.set_xalign(0.0);
    completion.set_use_markup(true);
    completion.set_ellipsize(gtk::pango::EllipsizeMode::End);
    completion.add_css_class("completion");
    completion.set_visible(false);
    vbox.append(&tabbar);
    vbox.append(&stack);
    vbox.append(&completion);
    vbox.append(&bottom);
    vbox.append(&entry);
    window.set_child(Some(&vbox));

    let session_path = dir.join("session.json");
    let mut tabs = Tabs::load(&session_path);
    for g in &cfg.groups { tabs.ensure_group(&g.name, &g.color); }
    let hist = history::History::load(dir.join("history"));
    let store = Store::new(dir.join("passwords"));
    let keymap = keys::Keymap::load(dir.join("keys.yml"));
    let marks = bookmarks::Bookmarks::load(dir.join("bookmarks"));
    let shared: Shared = Rc::new(RefCell::new(App {
        ui: Ui { window: window.clone(), tabbar, stack, status, right, entry: entry.clone(), completion },
        cfg, tabs, views: HashMap::new(), session, settings,
        mode: Mode::Normal, keys: String::new(), ask: Ask::Command, prompt: None,
        message: String::new(), hover: String::new(), store, fill_at: HashMap::new(),
        closed: Vec::new(), find: String::new(), session_path, save_pending: false,
        keymap, marks, hist, candidates: Vec::new(), selected: None, setting_text: false, filter: None,
    }));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let s = shared.clone();
        keys.connect_key_pressed(move |_, key, _, state| on_key(&s, key, state));
    }
    window.add_controller(keys);
    {
        let s = shared.clone();
        entry.connect_activate(move |_| entry_done(&s));
    }
    {
        let s = shared.clone();
        entry.connect_changed(move |_| on_entry_changed(&s));
    }
    {
        let s = shared.clone();
        window.connect_close_request(move |_| { save_session(&s); glib::Propagation::Proceed });
    }

    // Bring the last session back. Only the current tab loads now; the
    // others load when you go to them.
    let restored: Vec<(u64, bool)> = {
        let a = shared.borrow();
        a.tabs.tabs.iter().enumerate().map(|(i, t)| (t.id, i == a.tabs.active)).collect()
    };
    for (id, _) in &restored {
        let view = make_view(&shared, *id, None);
        attach(&shared, *id, view);
    }
    if !restored.is_empty() { show_active(&shared); }
    window.present();
    adblock_start(&shared);
    shared
}

fn style() {
    let css = "
        .tabbar, .statusbar { font-family: monospace; font-size: 12px; padding: 2px 6px;
                              background: #1e1e1e; color: #c8c8c8; }
        .cmdline { font-family: monospace; font-size: 13px; background: #101010; color: #ffffff;
                   border: none; border-radius: 0; padding: 2px 6px; min-height: 0; }
        .completion { font-family: monospace; font-size: 12px; padding: 4px 6px; background: #141414; color: #c8c8c8; }
    ";
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

/// A web view for tab `id`, wired to the page script and the signals gaze
/// listens to. Each view gets its own content manager so a message from
/// the page says which tab sent it.
fn make_view(shared: &Shared, id: u64, related: Option<&WebView>) -> WebView {
    let (session, settings, zoom, filter) = {
        let a = shared.borrow();
        (a.session.clone(), a.settings.clone(), a.cfg.zoom, a.filter.clone())
    };
    let ucm = UserContentManager::new();
    ucm.add_script(&UserScript::new(js::PAGE, UserContentInjectedFrames::AllFrames, UserScriptInjectionTime::Start, &[], &[]));
    ucm.register_script_message_handler("gaze", None);
    {
        let s = shared.clone();
        ucm.connect_script_message_received(Some("gaze"), move |_, value| {
            let text = value.to_str();
            on_message(&s, id, &text);
        });
    }
    if let Some(f) = &filter { ucm.add_filter(f); }
    let view = match related {
        // A popup shares its opener's process and session, so the two
        // pages can talk (window.opener, postMessage).
        Some(parent) => WebView::builder().related_view(parent).user_content_manager(&ucm).settings(&settings).build(),
        None => WebView::builder().network_session(&session).user_content_manager(&ucm).settings(&settings).build(),
    };
    view.set_vexpand(true);
    view.set_hexpand(true);
    view.set_zoom_level(zoom);

    {
        let s = shared.clone();
        view.connect_load_changed(move |v, event| {
            match event {
                LoadEvent::Started => {
                    let mut a = s.borrow_mut();
                    if a.mode == Mode::Insert || a.mode == Mode::Hint { a.mode = Mode::Normal; }
                    a.hover.clear();
                    if let Some(i) = a.tabs.index_of(id) {
                        if let Some(u) = v.uri() { a.tabs.tabs[i].uri = u.to_string(); }
                    }
                    drop(a);
                    refresh(&s);
                }
                LoadEvent::Finished => {
                    {
                        let mut a = s.borrow_mut();
                        let (uri, title) = (v.uri().map(|u| u.to_string()).unwrap_or_default(),
                                            v.title().map(|t| t.to_string()).unwrap_or_default());
                        a.hist.record(&uri, &title);
                    }
                    refresh(&s);
                    schedule_save(&s);
                    maybe_fill(&s, id);
                }
                _ => refresh(&s),
            }
        });
    }
    {
        let s = shared.clone();
        view.connect_title_notify(move |v| {
            {
                let mut a = s.borrow_mut();
                let title = v.title().map(|t| t.to_string()).unwrap_or_default();
                if let Some(i) = a.tabs.index_of(id) {
                    a.tabs.tabs[i].title = title.clone();
                    let uri = a.tabs.tabs[i].uri.clone();
                    a.hist.retitle(&uri, &title);
                }
            }
            refresh(&s);
            schedule_save(&s);
        });
    }
    {
        let s = shared.clone();
        view.connect_uri_notify(move |v| {
            {
                let mut a = s.borrow_mut();
                if let Some(i) = a.tabs.index_of(id) {
                    if let Some(u) = v.uri() { a.tabs.tabs[i].uri = u.to_string(); }
                }
            }
            refresh(&s);
            schedule_save(&s);
        });
    }
    {
        let s = shared.clone();
        view.connect_estimated_load_progress_notify(move |_| refresh(&s));
    }
    {
        let s = shared.clone();
        view.connect_mouse_target_changed(move |_, hit, _| {
            s.borrow_mut().hover = hit.link_uri().map(|u| u.to_string()).unwrap_or_default();
            refresh(&s);
        });
    }
    {
        let s = shared.clone();
        view.connect_decide_policy(move |_, decision, kind| {
            match kind {
                PolicyDecisionType::NewWindowAction => {
                    if let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() {
                        if let Some(uri) = nav.navigation_action().and_then(|a| a.request()).and_then(|r| r.uri()) {
                            let s = s.clone();
                            glib::idle_add_local_once(move || { open_tab(&s, &uri, true); });
                        }
                    }
                    decision.ignore();
                    true
                }
                PolicyDecisionType::NavigationAction => {
                    let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else { return false };
                    let Some(action) = nav.navigation_action() else { return false };
                    let ctrl = action.modifiers() & gdk::ModifierType::CONTROL_MASK.bits() != 0;
                    let wants_tab = action.mouse_button() == 2 || (action.mouse_button() == 1 && ctrl);
                    if wants_tab && action.is_user_gesture() {
                        if let Some(uri) = action.request().and_then(|r| r.uri()) {
                            let s = s.clone();
                            glib::idle_add_local_once(move || { open_tab(&s, &uri, true); });
                            decision.ignore();
                            return true;
                        }
                    }
                    false
                }
                PolicyDecisionType::Response => {
                    if let Some(r) = decision.downcast_ref::<ResponsePolicyDecision>() {
                        if !r.is_mime_type_supported() {
                            decision.download();
                            return true;
                        }
                    }
                    false
                }
                _ => false,
            }
        });
    }
    {
        // window.open(): give the page a real window as a new tab, so
        // sign-in popups that report back to their opener keep working.
        let s = shared.clone();
        view.connect_create(move |parent, _| {
            let popup = popup_tab(&s, parent);
            Some(popup.upcast())
        });
    }
    {
        let s = shared.clone();
        view.connect_close(move |_| {
            let idx = s.borrow().tabs.index_of(id);
            if let Some(i) = idx { close_tab(&s, i); }
        });
    }
    {
        // HTTP basic auth: answer from the saved logins when they are
        // open and know the host; otherwise WebKit's own dialog asks.
        let s = shared.clone();
        view.connect_authenticate(move |_, request| {
            if request.is_retry() || request.is_for_proxy() { return false; }
            let host = request.host().map(|h| h.to_string()).unwrap_or_default();
            let login = {
                let a = s.borrow();
                if !a.store.unlocked() { None } else {
                    a.store.for_site(&format!("https://{}", host)).first().map(|l| (*l).clone())
                        .or_else(|| a.store.for_site(&format!("http://{}", host)).first().map(|l| (*l).clone()))
                }
            };
            match login {
                Some(l) => {
                    request.authenticate(Some(&Credential::new(&l.username, &l.password, CredentialPersistence::ForSession)));
                    true
                }
                None => false,
            }
        });
    }
    view
}

fn attach(shared: &Shared, id: u64, view: WebView) {
    let mut a = shared.borrow_mut();
    a.ui.stack.add_child(&view);
    a.views.insert(id, view);
}

fn on_download(download: &Download, dir: PathBuf) {
    download.connect_decide_destination(move |d, suggested| {
        let _ = std::fs::create_dir_all(&dir);
        let mut path = dir.join(suggested);
        let mut n = 1;
        while path.exists() {
            path = dir.join(format!("{}.{}", suggested, n));
            n += 1;
        }
        d.set_destination(&path.to_string_lossy());
        let name = suggested.to_string();
        with_app(|s| set_message(s, &format!("Downloading {}", name)));
        true
    });
    download.connect_finished(|d| {
        let name = d.destination().map(|p| p.to_string()).unwrap_or_default();
        with_app(|s| set_message(s, &format!("Downloaded {}", name)));
    });
    download.connect_failed(|_, err| {
        let text = err.to_string();
        with_app(|s| set_message(s, &format!("Download failed: {}", text)));
    });
}

// ------------------------------------------------------------- the view

fn current_view(a: &App) -> Option<WebView> {
    a.tabs.current().and_then(|t| a.views.get(&t.id)).cloned()
}

fn run_js(view: &WebView, code: &str) {
    view.evaluate_javascript(code, None, None, None::<&gio::Cancellable>, |_| {});
}

fn run_js_then(view: &WebView, code: &str, then: impl FnOnce(Option<jsc::Value>) + 'static) {
    view.evaluate_javascript(code, None, None, None::<&gio::Cancellable>, move |r| then(r.ok()));
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn refresh(shared: &Shared) {
    let (bar, left, right, title) = {
        let a = shared.borrow();
        let bar = tabbar_markup(&a.tabs);
        let mode = match a.mode {
            Mode::Insert => "<b>-- INSERT --</b>  ",
            Mode::Hint => "<b>HINT</b>  ",
            Mode::Prompt => "<b>?</b>  ",
            _ => "",
        };
        let keys = if a.keys.is_empty() { String::new() } else { format!("   <b>{}</b>", glib::markup_escape_text(&a.keys)) };
        let left = format!("{}{}{}", mode, glib::markup_escape_text(&a.message), keys);
        let uri = a.tabs.current().map(|t| t.uri.clone()).unwrap_or_default();
        let progress = current_view(&a).map(|v| v.estimated_load_progress()).unwrap_or(1.0);
        let right = if !a.hover.is_empty() {
            a.hover.clone()
        } else if progress < 1.0 {
            format!("{:.0}%  {}", progress * 100.0, uri)
        } else {
            uri
        };
        let t = a.tabs.current().map(|t| if t.title.is_empty() { t.uri.clone() } else { t.title.clone() }).unwrap_or_default();
        let title = if t.is_empty() { "gaze".to_string() } else { format!("{} - gaze", t) };
        (bar, left, right, title)
    };
    let a = shared.borrow();
    a.ui.tabbar.set_markup(&bar);
    a.ui.status.set_markup(&left);
    a.ui.right.set_text(&right);
    a.ui.window.set_title(Some(&title));
}

fn tabbar_markup(tabs: &Tabs) -> String {
    let mut out = String::new();
    let mut last_group: Option<u64> = None;
    let mut n = 0;
    for (i, t) in tabs.tabs.iter().enumerate() {
        let group = t.group.and_then(|g| tabs.group_by_id(g));
        if t.group != last_group {
            if let Some(g) = group {
                let hex = tabs::color_hex(&g.color);
                let mark = if g.collapsed { "▸" } else { "▾" };
                out.push_str(&format!(" <span foreground=\"{}\"><b>{}{}</b></span>", hex, mark, glib::markup_escape_text(&g.name)));
                if g.collapsed {
                    out.push_str(&format!("<span foreground=\"{}\">({})</span>", hex, tabs.tabs_in(g.id).len()));
                }
            }
            last_group = t.group;
        }
        if !tabs.visible(i) { continue; }
        n += 1;
        let raw = if t.title.is_empty() { t.uri.trim_start_matches("https://").trim_start_matches("http://").to_string() } else { t.title.clone() };
        let short: String = raw.chars().take(20).collect();
        let text = glib::markup_escape_text(short.trim());
        let color = group.map(|g| tabs::color_hex(&g.color)).unwrap_or_else(|| (if t.pending { "#7a7a7a" } else { "#c8c8c8" }).to_string());
        if i == tabs.active {
            out.push_str(&format!(" <span background=\"#e6e6e6\" foreground=\"#1e1e1e\"><b> {} {} </b></span>", n, text));
        } else {
            out.push_str(&format!(" <span foreground=\"{}\">{} {}</span>", color, n, text));
        }
    }
    for g in tabs.empty_groups() {
        out.push_str(&format!("  <span foreground=\"{}\" alpha=\"60%\">·{}</span>", tabs::color_hex(&g.color), glib::markup_escape_text(&g.name)));
    }
    out
}

fn set_message(shared: &Shared, text: &str) {
    shared.borrow_mut().message = text.to_string();
    refresh(shared);
}

fn set_mode(shared: &Shared, mode: Mode) {
    {
        let mut a = shared.borrow_mut();
        a.mode = mode;
        a.keys.clear();
        if mode == Mode::Normal { a.message.clear(); }
    }
    refresh(shared);
}

// ------------------------------------------------------------------ tabs

fn open_tab(shared: &Shared, uri: &str, background: bool) -> u64 {
    let id = shared.borrow_mut().tabs.open(uri, background);
    let view = make_view(shared, id, None);
    view.load_uri(uri);
    attach(shared, id, view);
    if background { refresh(shared); } else { show_active(shared); }
    save_session(shared);
    id
}

/// A tab for a window a page opens itself; WebKit loads it.
fn popup_tab(shared: &Shared, parent: &WebView) -> WebView {
    let id = shared.borrow_mut().tabs.open("about:blank", false);
    let view = make_view(shared, id, Some(parent));
    attach(shared, id, view.clone());
    show_active(shared);
    save_session(shared);
    view
}

/// Show the current tab, loading it first if it was only restored.
fn show_active(shared: &Shared) {
    let (view, pending, uri) = {
        let mut a = shared.borrow_mut();
        let Some(tab) = a.tabs.current_mut() else { return };
        let pending = std::mem::take(&mut tab.pending);
        let uri = tab.uri.clone();
        let id = tab.id;
        (a.views.get(&id).cloned(), pending, uri)
    };
    let Some(view) = view else { return };
    if pending { view.load_uri(&uri); }
    {
        let mut a = shared.borrow_mut();
        a.ui.stack.set_visible_child(&view);
        a.hover.clear();
        a.message.clear();
        if a.mode == Mode::Insert || a.mode == Mode::Hint { a.mode = Mode::Normal; }
    }
    view.grab_focus();
    refresh(shared);
}

fn goto_tab(shared: &Shared, idx: usize) {
    {
        let mut a = shared.borrow_mut();
        if idx >= a.tabs.len() { return; }
        a.tabs.active = idx;
    }
    show_active(shared);
    save_session(shared);
}

fn close_tab(shared: &Shared, idx: usize) {
    let (view, home) = {
        let mut a = shared.borrow_mut();
        let Some(tab) = a.tabs.close(idx) else { return };
        let group = tab.group.and_then(|g| a.tabs.group_by_id(g)).map(|g| g.name.clone());
        if tab.uri != "about:blank" { a.closed.push((tab.uri.clone(), group)); }
        if a.closed.len() > 50 { a.closed.remove(0); }
        a.fill_at.remove(&tab.id);
        (a.views.remove(&tab.id), a.cfg.home.clone())
    };
    if let Some(v) = view {
        shared.borrow().ui.stack.remove(&v);
        v.stop_loading();
    }
    if shared.borrow().tabs.is_empty() {
        open_tab(shared, &home, false);
        return;
    }
    show_active(shared);
    save_session(shared);
}

fn undo_close(shared: &Shared) {
    let Some((uri, group)) = shared.borrow_mut().closed.pop() else {
        set_message(shared, "Nothing to undo");
        return;
    };
    open_tab(shared, &uri, false);
    if let Some(name) = group {
        let idx = shared.borrow().tabs.active;
        shared.borrow_mut().tabs.set_group(idx, &name);
        refresh(shared);
        save_session(shared);
    }
}

fn save_session(shared: &Shared) {
    let a = shared.borrow();
    if let Err(e) = a.tabs.save(&a.session_path) { eprintln!("gaze: could not save the session: {}", e); }
}

/// Save a little later, so a page that changes its URI ten times while
/// loading costs one write.
fn schedule_save(shared: &Shared) {
    {
        let mut a = shared.borrow_mut();
        if a.save_pending { return; }
        a.save_pending = true;
    }
    let s = shared.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
        s.borrow_mut().save_pending = false;
        save_session(&s);
    });
}

// ------------------------------------------------------------------ keys

fn on_key(shared: &Shared, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
    use glib::Propagation::{Proceed, Stop};
    let mode = shared.borrow().mode;
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let ch = key.to_unicode();
    match mode {
        Mode::Command => {
            match key {
                gdk::Key::Escape => { end_ask(shared); Stop }
                gdk::Key::Tab | gdk::Key::Down => { complete_move(shared, 1); Stop }
                gdk::Key::ISO_Left_Tab | gdk::Key::Up => { complete_move(shared, -1); Stop }
                _ => Proceed,
            }
        }
        Mode::Insert => {
            if key == gdk::Key::Escape || (ctrl && ch == Some('[')) {
                with_view(shared, |v| run_js(v, "window.__gaze && window.__gaze.blur()"));
                set_mode(shared, Mode::Normal);
                return Stop;
            }
            Proceed
        }
        Mode::Hint => {
            if key == gdk::Key::Escape {
                with_view(shared, |v| run_js(v, "window.__gaze && window.__gaze.stopHints()"));
                set_mode(shared, Mode::Normal);
                return Stop;
            }
            let c = if key == gdk::Key::BackSpace { Some('\u{8}') }
                else { ch.filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_lowercase()) };
            if let Some(c) = c { hint_key(shared, c); }
            Stop
        }
        Mode::Prompt => {
            match ch {
                Some('y') | Some('Y') => prompt_answer(shared, true),
                Some('n') | Some('N') => prompt_answer(shared, false),
                _ if key == gdk::Key::Escape => prompt_answer(shared, false),
                _ => {}
            }
            Stop
        }
        Mode::Normal => { normal_key(shared, key, state); Stop }
    }
}

fn normal_key(shared: &Shared, key: gdk::Key, state: gdk::ModifierType) {
    if key == gdk::Key::Escape {
        {
            let mut a = shared.borrow_mut();
            a.keys.clear();
            a.message.clear();
            a.prompt = None;
        }
        with_find(shared, |f| f.search_finish());
        refresh(shared);
        return;
    }
    let Some(name) = keys::key_name(key, state) else { return };
    let seq = {
        let mut a = shared.borrow_mut();
        a.keys.push_str(&name);
        a.keys.clone()
    };
    let (exact, more) = shared.borrow().keymap.lookup(&seq);
    match exact {
        Some(cmd) => {
            shared.borrow_mut().keys.clear();
            refresh(shared);
            run_command(shared, &cmd);
        }
        None if more => refresh(shared),
        None => {
            shared.borrow_mut().keys.clear();
            refresh(shared);
        }
    }
}

/// Run `f` on the current view with no borrow of the app held: a call
/// like load_uri fires notify signals at once, and their handlers borrow.
fn with_view(shared: &Shared, f: impl FnOnce(&WebView)) {
    let view = current_view(&shared.borrow());
    if let Some(v) = view { f(&v); }
}

fn with_find(shared: &Shared, f: impl FnOnce(&webkit6::FindController)) {
    let fc = current_view(&shared.borrow()).and_then(|v| v.find_controller());
    if let Some(fc) = fc { f(&fc); }
}

fn scroll_by(shared: &Shared, dx: i32, dy: f64) {
    with_view(shared, |v| run_js(v, &js::scroll(dx, dy, false)));
}

fn scroll_page(shared: &Shared, share: f64) {
    with_view(shared, |v| run_js(v, &js::scroll(0, share, true)));
}

fn goto_visible(shared: &Shared, n: usize) {
    let idx = shared.borrow().tabs.visible_indices().get(n).copied();
    if let Some(i) = idx { goto_tab(shared, i); }
}

fn zoom(shared: &Shared, delta: f64) {
    with_view(shared, |v| v.set_zoom_level((v.zoom_level() + delta).clamp(0.3, 5.0)));
    let z = current_view(&shared.borrow()).map(|v| v.zoom_level()).unwrap_or(1.0);
    set_message(shared, &format!("Zoom {:.0}%", z * 100.0));
}

fn clipboard() -> gdk::Clipboard {
    gdk::Display::default().expect("a display").clipboard()
}

fn paste_and_open(shared: &Shared, new_tab: bool) {
    let s = shared.clone();
    clipboard().read_text_async(None::<&gio::Cancellable>, move |res| {
        let Ok(Some(text)) = res else { set_message(&s, "Clipboard is empty"); return };
        let uri = config::to_uri(&text, &s.borrow().cfg.search);
        if new_tab { open_tab(&s, &uri, false); } else { with_view(&s, |v| v.load_uri(&uri)); }
    });
}

fn quit(shared: &Shared) {
    save_session(shared);
    shared.borrow().ui.window.close();
}

fn fold_current(shared: &Shared, on: Option<bool>) {
    let (gid, collapsed) = {
        let a = shared.borrow();
        let Some(g) = a.tabs.current().and_then(|t| t.group).and_then(|g| a.tabs.group_by_id(g)) else {
            drop(a);
            set_message(shared, "This tab is in no group (:group <name> puts it in one)");
            return;
        };
        (g.id, g.collapsed)
    };
    shared.borrow_mut().tabs.collapse(gid, on.unwrap_or(!collapsed));
    refresh(shared);
    save_session(shared);
}

// ----------------------------------------------------------------- hints

fn start_hints(shared: &Shared, new_tab: bool) {
    let Some(view) = current_view(&shared.borrow()) else { return };
    set_mode(shared, Mode::Hint);
    set_message(shared, if new_tab { "type the letters (opens in a new tab)" } else { "type the letters" });
    let s = shared.clone();
    run_js_then(&view, &format!("window.__gaze ? window.__gaze.startHints({}) : 0", new_tab), move |v| {
        let n = v.map(|v| v.to_double()).unwrap_or(0.0);
        if n < 1.0 && s.borrow().mode == Mode::Hint {
            set_mode(&s, Mode::Normal);
            set_message(&s, "Nothing to click here");
        }
    });
}

fn hint_key(shared: &Shared, c: char) {
    let Some(view) = current_view(&shared.borrow()) else { return };
    let s = shared.clone();
    run_js_then(&view, &format!("window.__gaze ? window.__gaze.hintKey({}) : 'none'", js_str(&c.to_string())), move |v| {
        let r = v.map(|v| v.to_str().to_string()).unwrap_or_default();
        if (r == "done" || r == "none") && s.borrow().mode == Mode::Hint { set_mode(&s, Mode::Normal); }
    });
}

// --------------------------------------------------------- command line

fn begin_ask(shared: &Shared, ask: Ask, prefill: &str) {
    let entry = {
        let mut a = shared.borrow_mut();
        a.mode = Mode::Command;
        a.keys.clear();
        let hidden = matches!(ask, Ask::Master(_));
        let hint = match &ask {
            Ask::Command => ":",
            Ask::Find => "/",
            Ask::GroupName => "group name:",
            Ask::Master(_) => if a.store.exists() { "master password:" } else { "new master password (blank for none):" },
        };
        a.ask = ask;
        let e = a.ui.entry.clone();
        e.set_visibility(!hidden);
        e.set_placeholder_text(Some(hint));
        e
    };
    entry.set_text(prefill);
    refresh(shared);
    entry.set_visible(true);
    entry.grab_focus();
    entry.set_position(-1);
}

fn end_ask(shared: &Shared) {
    let (entry, completion) = {
        let mut a = shared.borrow_mut();
        a.mode = Mode::Normal;
        a.candidates.clear();
        a.selected = None;
        (a.ui.entry.clone(), a.ui.completion.clone())
    };
    completion.set_visible(false);
    entry.set_visible(false);
    entry.set_text("");
    entry.set_visibility(true);
    with_view(shared, |v| { v.grab_focus(); });
    refresh(shared);
}

fn entry_done(shared: &Shared) {
    let (text, ask) = {
        let a = shared.borrow();
        (a.ui.entry.text().to_string(), a.ask.clone())
    };
    end_ask(shared);
    match ask {
        Ask::Command => run_command(shared, &text),
        Ask::Find => find(shared, &text),
        Ask::GroupName => group_current(shared, &text),
        Ask::Master(then) => {
            let result = shared.borrow_mut().store.unlock(&text);
            match result {
                Ok(n) => {
                    set_message(shared, &format!("Passwords unlocked ({} saved)", n));
                    after_unlock(shared, then);
                }
                Err(e) => set_message(shared, &format!("Passwords: {}", e)),
            }
        }
    }
}

/// The verb and the query of an open prompt, or None for any other text.
fn open_query(text: &str) -> Option<(&str, &str)> {
    let (verb, rest) = text.split_once(' ')?;
    if matches!(verb, "open" | "o" | "tabopen" | "t") { Some((verb, rest)) } else { None }
}

fn on_entry_changed(shared: &Shared) {
    let text = {
        let a = shared.borrow();
        if a.setting_text || a.mode != Mode::Command || !matches!(a.ask, Ask::Command) { return; }
        a.ui.entry.text().to_string()
    };
    let candidates = match open_query(&text) {
        Some((_, query)) => {
            let a = shared.borrow();
            let marks: Vec<(String, String)> = a.marks.list().iter().map(|b| (b.url.clone(), b.title.clone())).collect();
            a.hist.matches(query, &marks, 10)
        }
        None => Vec::new(),
    };
    {
        let mut a = shared.borrow_mut();
        a.candidates = candidates;
        a.selected = None;
    }
    render_completion(shared);
}

fn render_completion(shared: &Shared) {
    let (label, markup) = {
        let a = shared.borrow();
        let lines: Vec<String> = a.candidates.iter().enumerate().map(|(i, c)| {
            let title: String = c.title.chars().take(48).collect();
            let url: String = c.url.chars().take(90).collect();
            let line = format!("{} {:<48}  {}", if c.bookmark { "★" } else { " " }, title, url);
            let text = glib::markup_escape_text(&line);
            if a.selected == Some(i) {
                format!("<span background=\"#e6e6e6\" foreground=\"#1e1e1e\">{}</span>", text)
            } else {
                text.to_string()
            }
        }).collect();
        (a.ui.completion.clone(), lines.join("\n"))
    };
    if markup.is_empty() {
        label.set_visible(false);
    } else {
        label.set_markup(&markup);
        label.set_visible(true);
    }
}

/// Tab and Shift-Tab walk the offered pages and put one on the command line.
fn complete_move(shared: &Shared, dir: i32) {
    let (entry, text) = {
        let mut a = shared.borrow_mut();
        let n = a.candidates.len();
        if n == 0 { return; }
        let next = match a.selected {
            Some(i) => ((i as i32 + dir).rem_euclid(n as i32)) as usize,
            None if dir > 0 => 0,
            None => n - 1,
        };
        a.selected = Some(next);
        let verb = open_query(&a.ui.entry.text()).map(|(v, _)| v).unwrap_or("open").to_string();
        a.setting_text = true;
        (a.ui.entry.clone(), format!("{} {}", verb, a.candidates[next].url))
    };
    entry.set_text(&text);
    entry.set_position(-1);
    shared.borrow_mut().setting_text = false;
    render_completion(shared);
}

fn find(shared: &Shared, text: &str) {
    let text = if text.is_empty() { shared.borrow().find.clone() } else { text.to_string() };
    if text.is_empty() { return; }
    shared.borrow_mut().find = text.clone();
    with_find(shared, |f| f.search(&text, (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits(), u32::MAX));
}

fn run_command(shared: &Shared, line: &str) {
    let line = line.trim_start();
    if line.trim().is_empty() { return; }
    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let (cmd, arg) = (cmd.trim(), rest.trim());
    let (search, step, uri, title) = {
        let a = shared.borrow();
        let t = a.tabs.current();
        (a.cfg.search.clone(), a.cfg.scroll_step as f64,
         t.map(|t| t.uri.clone()).unwrap_or_default(), t.map(|t| t.title.clone()).unwrap_or_default())
    };
    match cmd {
        "cmd" => begin_ask(shared, Ask::Command, &rest.trim_start().replace("{url}", &uri).replace("{title}", &title)),
        "open" | "o" => {
            if arg.is_empty() { begin_ask(shared, Ask::Command, "open "); }
            else { let u = config::to_uri(arg, &search); with_view(shared, |v| v.load_uri(&u)); }
        }
        "tabopen" | "t" => {
            if arg.is_empty() { begin_ask(shared, Ask::Command, "tabopen "); }
            else { open_tab(shared, &config::to_uri(arg, &search), false); }
        }
        "home" => { let h = shared.borrow().cfg.home.clone(); with_view(shared, |v| v.load_uri(&h)); }
        "back" => with_view(shared, |v| v.go_back()),
        "forward" => with_view(shared, |v| v.go_forward()),
        "reload" => with_view(shared, |v| v.reload()),
        "reload-force" => with_view(shared, |v| v.reload_bypass_cache()),
        "stop" => with_view(shared, |v| v.stop_loading()),
        "scroll-down" => scroll_by(shared, 0, step),
        "scroll-up" => scroll_by(shared, 0, -step),
        "scroll-left" => scroll_by(shared, -(step as i32), 0.0),
        "scroll-right" => scroll_by(shared, step as i32, 0.0),
        "scroll-page" => match arg.parse::<f64>() {
            Ok(share) => scroll_page(shared, share),
            Err(_) => set_message(shared, "scroll-page <share of the window>, 0.5 is half a page down"),
        },
        "scroll-top" => with_view(shared, |v| run_js(v, js::SCROLL_TOP)),
        "scroll-bottom" => with_view(shared, |v| run_js(v, js::SCROLL_BOTTOM)),
        "find" => { if arg.is_empty() { begin_ask(shared, Ask::Find, ""); } else { find(shared, arg); } }
        "find-next" => with_find(shared, |f| f.search_next()),
        "find-prev" => with_find(shared, |f| f.search_previous()),
        "hint" => start_hints(shared, false),
        "hint-tab" => start_hints(shared, true),
        "insert" => { set_mode(shared, Mode::Insert); with_view(shared, |v| { v.grab_focus(); }); }
        "focus-input" => with_view(shared, |v| run_js(v, "window.__gaze && window.__gaze.focusFirstInput()")),
        "yank" => {
            let text = if arg == "title" { title } else { uri };
            clipboard().set_text(&text);
            set_message(shared, &format!("Yanked {}", text));
        }
        "paste" => paste_and_open(shared, false),
        "paste-tab" => paste_and_open(shared, true),
        "zoom-in" => zoom(shared, 0.1),
        "zoom-out" => zoom(shared, -0.1),
        "zoom-reset" => { let z = shared.borrow().cfg.zoom; with_view(shared, |v| v.set_zoom_level(z)); set_message(shared, "Zoom reset"); }
        "zoom" => match arg.trim_end_matches('%').parse::<f64>() {
            Ok(p) if p > 0.0 => { with_view(shared, |v| v.set_zoom_level(p / 100.0)); set_message(shared, &format!("Zoom {:.0}%", p)); }
            _ => set_message(shared, "zoom <percent>"),
        },
        "tab-next" => { let i = shared.borrow().tabs.neighbour(1); goto_tab(shared, i); }
        "tab-prev" => { let i = shared.borrow().tabs.neighbour(-1); goto_tab(shared, i); }
        "tab-first" => goto_visible(shared, 0),
        "tab-last" => { let n = shared.borrow().tabs.visible_indices().len(); goto_visible(shared, n.saturating_sub(1)); }
        "tab" => match arg.parse::<usize>() {
            Ok(n) if n >= 1 => goto_visible(shared, n - 1),
            _ => set_message(shared, "tab <number>"),
        },
        "tab-move" => {
            let (idx, len) = { let a = shared.borrow(); (a.tabs.active, a.tabs.len()) };
            let delta = match arg {
                a if a.starts_with('+') || a.starts_with('-') => a.parse::<i32>().ok(),
                a => a.parse::<usize>().ok().filter(|&n| n >= 1 && n <= len).map(|n| n as i32 - 1 - idx as i32),
            };
            match delta {
                Some(d) => { shared.borrow_mut().tabs.move_tab(idx, d); refresh(shared); save_session(shared); }
                None => set_message(shared, "tab-move +1 | -1 | <number>"),
            }
        }
        "close" | "q" => { let i = shared.borrow().tabs.active; close_tab(shared, i); }
        "undo" => undo_close(shared),
        "group" => { if arg.is_empty() { begin_ask(shared, Ask::GroupName, ""); } else { group_current(shared, arg); } }
        "ungroup" => {
            let i = shared.borrow().tabs.active;
            shared.borrow_mut().tabs.ungroup(i);
            refresh(shared);
            save_session(shared);
        }
        "group-fold" => fold_current(shared, Some(true)),
        "group-unfold" => fold_current(shared, Some(false)),
        "group-toggle" => fold_current(shared, None),
        "groups-fold" => { shared.borrow_mut().tabs.collapse_all(true); refresh(shared); save_session(shared); }
        "groups-unfold" => { shared.borrow_mut().tabs.collapse_all(false); refresh(shared); save_session(shared); }
        "group-rename" | "group-color" | "group-close" | "group-collapse" | "group-expand" => group_command(shared, cmd, arg),
        "group-delete" => {
            let name = if arg.is_empty() {
                let a = shared.borrow();
                a.tabs.current().and_then(|t| t.group).and_then(|g| a.tabs.group_by_id(g)).map(|g| g.name.clone()).unwrap_or_default()
            } else { arg.to_string() };
            if name.is_empty() { set_message(shared, "group-delete <name>"); return; }
            let r = shared.borrow_mut().tabs.delete_group(&name);
            match r {
                Ok(()) => { set_message(shared, &format!("Group {} deleted", name)); refresh(shared); save_session(shared); }
                Err(e) => set_message(shared, &e),
            }
        }
        "groups" => {
            let text = {
                let a = shared.borrow();
                a.tabs.groups.iter().map(|g| format!("{} ({}, {}{})", g.name, a.tabs.tabs_in(g.id).len(), g.color, if g.collapsed { ", folded" } else { "" }))
                    .collect::<Vec<_>>().join("  ·  ")
            };
            set_message(shared, if text.is_empty() { "No groups" } else { &text });
        }
        "bookmark-add" => {
            if uri.is_empty() || uri.starts_with("about:") || uri.starts_with("gaze:") { set_message(shared, "Nothing to bookmark here"); return; }
            let name = if arg.is_empty() { title } else { arg.to_string() };
            let r = shared.borrow_mut().marks.add(&uri, &name);
            match r {
                Ok(true) => set_message(shared, &format!("Bookmarked {}", name)),
                Ok(false) => set_message(shared, "Already bookmarked; title updated"),
                Err(e) => set_message(shared, &format!("Bookmarks: {}", e)),
            }
        }
        "bookmark-del" => {
            let target = if arg.is_empty() { uri } else { arg.to_string() };
            let r = shared.borrow_mut().marks.remove(&target);
            match r {
                Ok(true) => set_message(shared, "Bookmark removed"),
                Ok(false) => set_message(shared, "Not a bookmark"),
                Err(e) => set_message(shared, &format!("Bookmarks: {}", e)),
            }
        }
        "bookmarks" => { open_tab(shared, "gaze://bookmarks", false); }
        "bookmark-import" => {
            if arg.is_empty() { set_message(shared, "bookmark-import <bookmarks.html> (Firefox: Manage Bookmarks → Import and Backup → Export)"); return; }
            let path = config::expand(arg);
            let r = std::fs::read_to_string(&path).map_err(|e| e.to_string())
                .and_then(|html| shared.borrow_mut().marks.import_html(&html));
            match r {
                Ok(n) => set_message(shared, &format!("Imported {} bookmarks", n)),
                Err(e) => set_message(shared, &format!("Import failed: {}", e)),
            }
        }
        "fill" => fill_next(shared, false),
        "passwords" => when_unlocked(shared, Then::List),
        "password-import" => {
            if arg.is_empty() { set_message(shared, "password-import <file.csv> (Firefox: about:logins → Export)"); }
            else { when_unlocked(shared, Then::Import(config::expand(arg).to_string_lossy().to_string())); }
        }
        "password-remove" => {
            if arg.is_empty() { set_message(shared, "password-remove <username> (for this site)"); }
            else { when_unlocked(shared, Then::Remove(arg.to_string())); }
        }
        "password-lock" => { shared.borrow_mut().store.lock(); set_message(shared, "Passwords locked"); }
        "adblock-update" => adblock_download(shared),
        "bind" => {
            let Some((k, c)) = arg.split_once(' ') else { set_message(shared, "bind <keys> <command>"); return };
            let r = shared.borrow_mut().keymap.bind(k.trim(), c.trim());
            match r {
                Ok(()) => set_message(shared, &format!("{} runs {}", k.trim(), c.trim())),
                Err(e) => set_message(shared, &e),
            }
        }
        "unbind" => {
            if arg.is_empty() { set_message(shared, "unbind <keys>"); return; }
            let r = shared.borrow_mut().keymap.unbind(arg);
            match r {
                Ok(true) => set_message(shared, &format!("{} unbound", arg)),
                Ok(false) => set_message(shared, &format!("{} was not bound", arg)),
                Err(e) => set_message(shared, &e),
            }
        }
        "help" => { open_tab(shared, "gaze://help", false); }
        "inspect" | "devtools" => with_view(shared, |v| { if let Some(i) = v.inspector() { i.show(); } }),
        "session-save" => { save_session(shared); set_message(shared, "Session saved"); }
        "quit" | "qa" | "wq" => quit(shared),
        _ => set_message(shared, &format!("Unknown command: {}", cmd)),
    }
}

fn group_current(shared: &Shared, name: &str) {
    if name.trim().is_empty() { return; }
    let idx = shared.borrow().tabs.active;
    let ok = shared.borrow_mut().tabs.set_group(idx, name).is_some();
    if ok { set_message(shared, &format!("In group {}", name.trim())); }
    refresh(shared);
    save_session(shared);
}

fn group_command(shared: &Shared, cmd: &str, arg: &str) {
    let gid = {
        let a = shared.borrow();
        let by_arg = if cmd == "group-collapse" || cmd == "group-expand" { a.tabs.group_by_name(arg).map(|g| g.id) } else { None };
        by_arg.or_else(|| a.tabs.current().and_then(|t| t.group))
    };
    let Some(gid) = gid else { set_message(shared, "This tab is in no group"); return };
    match cmd {
        "group-rename" => {
            if arg.is_empty() { set_message(shared, "group-rename <name>"); return; }
            shared.borrow_mut().tabs.rename_group(gid, arg);
        }
        "group-color" => {
            if !shared.borrow_mut().tabs.recolor_group(gid, arg) {
                let names = tabs::COLORS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" ");
                set_message(shared, &format!("group-color: {} or #rrggbb", names));
                return;
            }
        }
        "group-collapse" => shared.borrow_mut().tabs.collapse(gid, true),
        "group-expand" => shared.borrow_mut().tabs.collapse(gid, false),
        "group-close" => {
            let mut idxs = shared.borrow().tabs.tabs_in(gid);
            idxs.reverse();
            for i in idxs { close_tab(shared, i); }
            return;
        }
        _ => {}
    }
    refresh(shared);
    save_session(shared);
}

// ------------------------------------------------------------- passwords

fn when_unlocked(shared: &Shared, then: Then) {
    if shared.borrow().store.unlocked() { after_unlock(shared, then); } else { begin_ask(shared, Ask::Master(then), ""); }
}

fn after_unlock(shared: &Shared, then: Then) {
    match then {
        Then::Fill => fill_next(shared, true),
        Then::Save(login) => {
            let site = passwords::site_key(&login.origin);
            let user = login.username.clone();
            let r = shared.borrow_mut().store.remember(login);
            match r {
                Ok(Change::New) => set_message(shared, &format!("Saved {} for {}", user, site)),
                Ok(Change::Updated) => set_message(shared, &format!("Updated the password of {} for {}", user, site)),
                Ok(Change::Same) => set_message(shared, "Already saved"),
                Err(e) => set_message(shared, &format!("Passwords: {}", e)),
            }
        }
        Then::Import(path) => {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => { set_message(shared, &format!("{}: {}", path, e)); return; }
            };
            let r = shared.borrow_mut().store.import_csv(&text);
            match r {
                Ok((added, updated)) => set_message(shared, &format!("Imported {} logins, updated {}. Delete the CSV now.", added, updated)),
                Err(e) => set_message(shared, &format!("Import failed: {}", e)),
            }
        }
        Then::List => { open_tab(shared, "gaze://passwords", false); }
        Then::Remove(user) => {
            let uri = shared.borrow().tabs.current().map(|t| t.uri.clone()).unwrap_or_default();
            let r = shared.borrow_mut().store.remove(&uri, &user);
            match r {
                Ok(true) => set_message(shared, &format!("Removed {}", user)),
                Ok(false) => set_message(shared, &format!("No login {} for this site", user)),
                Err(e) => set_message(shared, &format!("Passwords: {}", e)),
            }
        }
    }
}

/// When a page has finished loading: fill its login form if the store is
/// open and knows the site, or say how to open the store if it is not.
fn maybe_fill(shared: &Shared, id: u64) {
    let (is_current, unlocked, exists) = {
        let a = shared.borrow();
        (a.tabs.current().map(|t| t.id) == Some(id), a.store.unlocked(), a.store.exists())
    };
    if !is_current { return; }
    if unlocked {
        let has = {
            let a = shared.borrow();
            a.tabs.current().map(|t| !a.store.for_site(&t.uri).is_empty()).unwrap_or(false)
        };
        if has { fill_next(shared, true); }
        return;
    }
    if !exists { return; }
    let Some(view) = current_view(&shared.borrow()) else { return };
    let s = shared.clone();
    run_js_then(&view, "window.__gaze ? window.__gaze.hasPasswordField() : false", move |v| {
        if v.map(|v| v.to_boolean()).unwrap_or(false) && s.borrow().mode == Mode::Normal {
            set_message(&s, "Login form: gp unlocks your passwords and fills it");
        }
    });
}

/// Fill the current page with a saved login: the most recent first, then
/// the next one each time `gp` is pressed.
fn fill_next(shared: &Shared, first: bool) {
    if !shared.borrow().store.unlocked() {
        begin_ask(shared, Ask::Master(Then::Fill), "");
        return;
    }
    let (view, id, uri) = {
        let a = shared.borrow();
        let Some(t) = a.tabs.current() else { return };
        (a.views.get(&t.id).cloned(), t.id, t.uri.clone())
    };
    let Some(view) = view else { return };
    let logins: Vec<Login> = shared.borrow().store.for_site(&uri).into_iter().cloned().collect();
    if logins.is_empty() {
        set_message(shared, &format!("No saved login for {}", passwords::site_key(&uri)));
        return;
    }
    let idx = {
        let mut a = shared.borrow_mut();
        let last = a.fill_at.get(&id).copied();
        let idx = if first || last.is_none() { 0 } else { (last.unwrap() + 1) % logins.len() };
        a.fill_at.insert(id, idx);
        idx
    };
    let login = logins[idx].clone();
    let s = shared.clone();
    let code = format!("window.__gaze ? window.__gaze.fill({}, {}) : 'none'", js_str(&login.username), js_str(&login.password));
    run_js_then(&view, &code, move |v| {
        let r = v.map(|v| v.to_str().to_string()).unwrap_or_default();
        let more = if logins.len() > 1 { format!(" ({} of {}, gp for the next)", idx + 1, logins.len()) } else { String::new() };
        match r.as_str() {
            "both" => {
                s.borrow_mut().store.touch(&login.origin, &login.username);
                set_message(&s, &format!("Filled {}{}", login.username, more));
            }
            "user" => set_message(&s, &format!("Filled the username {}{}", login.username, more)),
            _ => set_message(&s, &format!("No login form here; {} is saved for this site", login.username)),
        }
    });
}

fn prompt_answer(shared: &Shared, yes: bool) {
    let prompt = shared.borrow_mut().prompt.take();
    set_mode(shared, Mode::Normal);
    let Some(prompt) = prompt else { return };
    match prompt {
        Prompt::SaveLogin(login) => {
            if yes { when_unlocked(shared, Then::Save(login)); }
        }
    }
}

/// A message from the page script of tab `id`.
fn on_message(shared: &Shared, id: u64, text: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return };
    let kind = v.get("t").and_then(|t| t.as_str()).unwrap_or("");
    let is_current = shared.borrow().tabs.current().map(|t| t.id) == Some(id);
    match kind {
        "focus" => {
            if !is_current { return; }
            let editable = v.get("editable").and_then(|e| e.as_bool()).unwrap_or(false);
            let mode = shared.borrow().mode;
            if editable && mode == Mode::Normal { set_mode(shared, Mode::Insert); }
            else if !editable && mode == Mode::Insert { set_mode(shared, Mode::Normal); }
        }
        "open" => {
            let uri = v.get("uri").and_then(|u| u.as_str()).unwrap_or("").to_string();
            let background = v.get("background").and_then(|b| b.as_bool()).unwrap_or(true);
            if !uri.is_empty() { open_tab(shared, &uri, background); }
        }
        "login" => {
            let login = Login {
                origin: v.get("origin").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                username: v.get("username").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                password: v.get("password").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                used: 0,
            };
            if login.password.is_empty() || login.origin.is_empty() || login.origin == "null" { return; }
            let known = {
                let a = shared.borrow();
                a.store.unlocked() && a.store.for_site(&login.origin).iter()
                    .any(|l| l.username == login.username && l.password == login.password)
            };
            if known { return; }
            let busy = matches!(shared.borrow().mode, Mode::Command | Mode::Prompt);
            if busy { return; }
            let text = format!("Save the password of {} for {}? (y/n)",
                if login.username.is_empty() { "(no username)" } else { &login.username }, passwords::site_key(&login.origin));
            {
                let mut a = shared.borrow_mut();
                a.prompt = Some(Prompt::SaveLogin(login));
                a.mode = Mode::Prompt;
                a.keys.clear();
                a.message = text;
            }
            refresh(shared);
        }
        _ => {}
    }
}

// ------------------------------------------------------------- ad block

thread_local! {
    static SOUP: soup::Session = soup::Session::new();
}

fn adblock_dir() -> PathBuf { config::gaze_dir().join("adblock") }

fn adblock_store() -> UserContentFilterStore {
    let dir = adblock_dir().join("store");
    let _ = std::fs::create_dir_all(&dir);
    UserContentFilterStore::new(&dir.to_string_lossy())
}

/// At start: use the compiled filter when there is one, else build it
/// from the hosts list, fetching the list first when it is missing.
fn adblock_start(shared: &Shared) {
    if !shared.borrow().cfg.adblock { return; }
    let s = shared.clone();
    adblock_store().load("ads", None::<&gio::Cancellable>, move |r| match r {
        Ok(filter) => apply_filter(&s, filter, None),
        Err(_) => {
            if adblock_dir().join("hosts").exists() { adblock_compile(&s); } else { adblock_download(&s); }
        }
    });
}

fn adblock_download(shared: &Shared) {
    let msg = match soup::Message::new("GET", adblock::SOURCE) {
        Ok(m) => m,
        Err(e) => { set_message(shared, &format!("Ad blocker: {}", e)); return; }
    };
    set_message(shared, "Ad blocker: fetching the hosts list…");
    let s = shared.clone();
    SOUP.with(|session| {
        session.send_and_read_async(&msg, glib::Priority::DEFAULT, None::<&gio::Cancellable>, move |r| match r {
            Ok(bytes) if bytes.len() > 10_000 => {
                let _ = std::fs::create_dir_all(adblock_dir());
                match std::fs::write(adblock_dir().join("hosts"), &bytes) {
                    Ok(()) => adblock_compile(&s),
                    Err(e) => set_message(&s, &format!("Ad blocker: {}", e)),
                }
            }
            Ok(_) => set_message(&s, "Ad blocker: the hosts list came back empty"),
            Err(e) => set_message(&s, &format!("Ad blocker: {}", e)),
        });
    });
}

/// Turn the hosts list into a content filter. WebKit compiles it on a
/// thread of its own and keeps the result, so this runs once per list.
fn adblock_compile(shared: &Shared) {
    let text = match std::fs::read_to_string(adblock_dir().join("hosts")) {
        Ok(t) => t,
        Err(e) => { set_message(shared, &format!("Ad blocker: {}", e)); return; }
    };
    let (json, n) = adblock::rules_from_hosts(&text);
    set_message(shared, &format!("Ad blocker: compiling {} domains…", n));
    let bytes = glib::Bytes::from_owned(json.into_bytes());
    let s = shared.clone();
    adblock_store().save("ads", &bytes, None::<&gio::Cancellable>, move |r| match r {
        Ok(filter) => apply_filter(&s, filter, Some(n)),
        Err(e) => set_message(&s, &format!("Ad blocker: {}", e)),
    });
}

fn apply_filter(shared: &Shared, filter: UserContentFilter, count: Option<usize>) {
    let views: Vec<WebView> = shared.borrow().views.values().cloned().collect();
    for v in &views {
        if let Some(ucm) = v.user_content_manager() {
            ucm.remove_all_filters();
            ucm.add_filter(&filter);
        }
    }
    shared.borrow_mut().filter = Some(filter);
    if let Some(n) = count { set_message(shared, &format!("Ad blocker ready: {} domains blocked", n)); }
}

// -------------------------------------------------------- gaze:// pages

fn serve_internal(req: &URISchemeRequest) {
    let uri = req.uri().map(|u| u.to_string()).unwrap_or_default();
    let name = uri.trim_start_matches("gaze:").trim_matches('/').to_string();
    let body = match name.as_str() {
        "help" => help_page(),
        "passwords" => passwords_page(),
        "bookmarks" => bookmarks_page(),
        other => format!("<h1>gaze://{}</h1><p>No such page. Try gaze://help.</p>", esc(other)),
    };
    let html = format!("<!doctype html><meta charset=utf-8><title>gaze</title><style>{}</style>{}", PAGE_CSS, body);
    let bytes = glib::Bytes::from_owned(html.into_bytes());
    let len = bytes.len() as i64;
    let stream = gio::MemoryInputStream::from_bytes(&bytes);
    req.finish(&stream, len, Some("text/html"));
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn passwords_page() -> String {
    let rows = with_app(|s| {
        let a = s.borrow();
        if !a.store.unlocked() { return None; }
        let mut logins: Vec<&Login> = a.store.logins().iter().collect();
        logins.sort_by(|x, y| passwords::site_key(&x.origin).cmp(&passwords::site_key(&y.origin)).then(x.username.cmp(&y.username)));
        Some(logins.iter().map(|l| format!("<tr><td>{}</td><td>{}</td></tr>", esc(&passwords::site_key(&l.origin)), esc(&l.username))).collect::<String>())
    }).flatten();
    match rows {
        None => "<h1>Passwords</h1><p>Locked. <code>gp</code> on a login page or <code>:passwords</code> unlocks them.</p>".to_string(),
        Some(rows) => format!(
            "<h1>Passwords</h1><p>Passwords themselves are never shown. On a site, <code>gp</code> fills the next saved login; \
             <code>:password-remove &lt;username&gt;</code> forgets one.</p><table><tr><th>Site</th><th>Username</th></tr>{}</table>", rows),
    }
}

const PAGE_CSS: &str = "body{background:#1e1e1e;color:#d0d0d0;font:15px/1.5 sans-serif;max-width:56em;margin:2em auto;padding:0 1em}\
 h1{color:#f0883e}h2{color:#e0b93a;margin-top:1.6em}code,kbd{font-family:monospace;background:#2c2c2c;padding:1px 5px;border-radius:3px;color:#fff}\
 table{border-collapse:collapse}td,th{text-align:left;padding:3px 14px 3px 0;vertical-align:top}th{color:#e0b93a;padding-top:1.2em}\
 a{color:#7ab7ff;text-decoration:none}a:hover{text-decoration:underline}.u{color:#8a8a8a;font-size:13px}";

fn bookmarks_page() -> String {
    let rows = with_app(|s| {
        let a = s.borrow();
        a.marks.list().iter().map(|b| {
            let title = if b.title.is_empty() { b.url.clone() } else { b.title.clone() };
            format!("<tr><td><a href=\"{}\">{}</a></td><td class=u>{}</td></tr>", esc(&b.url), esc(&title), esc(&b.url))
        }).collect::<String>()
    }).unwrap_or_default();
    if rows.is_empty() {
        return "<h1>Bookmarks</h1><p>None yet. <kbd>M</kbd> bookmarks the page you are on; \
                <code>:bookmark-import ~/bookmarks.html</code> reads a Firefox export.</p>".to_string();
    }
    format!("<h1>Bookmarks</h1><p><kbd>f</kbd> then the letters opens one. <kbd>M</kbd> adds the current page, \
             <code>:bookmark-del</code> removes it. The list is <code>~/.gaze/bookmarks</code>, a text file.</p>\
             <table>{}</table>", rows)
}

/// Every command, grouped, for the help page. Keys come from the keymap.
const COMMANDS: &[(&str, &str, &str)] = &[
    ("Pages", "open [url]", "open a URL, a search or a file here; asks when given nothing"),
    ("Pages", "tabopen [url]", "the same in a new tab"),
    ("Pages", "cmd <text>", "open the command line with this text; {url} and {title} are filled in"),
    ("Pages", "back", "go back"), ("Pages", "forward", "go forward"),
    ("Pages", "reload", "reload"), ("Pages", "reload-force", "reload without the cache"), ("Pages", "stop", "stop loading"),
    ("Pages", "home", "the home page from config.yml"),
    ("Pages", "hint", "type the letters on a link to follow it"), ("Pages", "hint-tab", "the same, into a background tab"),
    ("Pages", "insert", "insert mode: keys go to the page until Esc"), ("Pages", "focus-input", "focus the first field on the page"),
    ("Pages", "scroll-down", "scroll"), ("Pages", "scroll-up", ""), ("Pages", "scroll-left", ""), ("Pages", "scroll-right", ""),
    ("Pages", "scroll-page <share>", "scroll by a share of the window; 0.5 is half a page down, -0.5 up"),
    ("Pages", "scroll-top", "to the top"), ("Pages", "scroll-bottom", "to the bottom"),
    ("Pages", "find [text]", "find on the page; asks when given nothing"), ("Pages", "find-next", ""), ("Pages", "find-prev", ""),
    ("Pages", "yank url|title", "copy to the clipboard"), ("Pages", "paste", "open what the clipboard holds here"), ("Pages", "paste-tab", "the same in a new tab"),
    ("Pages", "zoom-in", ""), ("Pages", "zoom-out", ""), ("Pages", "zoom-reset", ""), ("Pages", "zoom <percent>", ""),
    ("Tabs", "tab-next", "the next visible tab"), ("Tabs", "tab-prev", "the previous one"),
    ("Tabs", "tab <n>", "the n-th visible tab"), ("Tabs", "tab-first", ""), ("Tabs", "tab-last", ""),
    ("Tabs", "tab-move +1|-1|<n>", "move this tab"), ("Tabs", "close", "close this tab"), ("Tabs", "undo", "bring back the last closed tab"),
    ("Tab groups", "group [name]", "put this tab in the group, made on the spot when new; asks for the name when given none"),
    ("Tab groups", "ungroup", "take it out again"),
    ("Tab groups", "group-fold", "fold this tab's group away"), ("Tab groups", "group-unfold", ""), ("Tab groups", "group-toggle", ""),
    ("Tab groups", "groups-fold", "fold every group"), ("Tab groups", "groups-unfold", ""),
    ("Tab groups", "group-rename <name>", ""), ("Tab groups", "group-color <colour>", "blue red yellow green pink purple orange cyan gray, or #rrggbb"),
    ("Tab groups", "group-close", "close every tab of the group"), ("Tab groups", "group-delete [name]", "drop an empty group"),
    ("Tab groups", "groups", "list the groups"),
    ("Bookmarks", "bookmark-add [title]", "bookmark this page"), ("Bookmarks", "bookmark-del [url]", "forget this page's bookmark"),
    ("Bookmarks", "bookmarks", "the list, at gaze://bookmarks"), ("Bookmarks", "bookmark-import <file>", "read a Firefox HTML export"),
    ("Passwords", "fill", "fill the login form; again for the next saved login of the site"),
    ("Passwords", "passwords", "list the sites and usernames"), ("Passwords", "password-import <csv>", "read the CSV Firefox writes from about:logins → Export"),
    ("Passwords", "password-remove <username>", "forget one login for this site"), ("Passwords", "password-lock", "lock the store for this session"),
    ("Other", "bind <keys> <command>", "bind keys; kept in ~/.gaze/keys.yml"), ("Other", "unbind <keys>", ""),
    ("Other", "adblock-update", "fetch the hosts list again and rebuild the ad blocker"),
    ("Other", "help", "this page"), ("Other", "inspect", "the web inspector"), ("Other", "session-save", ""), ("Other", "quit", ""),
];

fn help_page() -> String {
    let table = with_app(|s| {
        let a = s.borrow();
        let mut out = String::new();
        let mut group = "";
        for (g, cmd, what) in COMMANDS {
            if *g != group {
                group = g;
                out.push_str(&format!("<tr><th colspan=3>{}</th></tr>", g));
            }
            let word = cmd.split_whitespace().next().unwrap_or(cmd);
            let keys = a.keymap.keys_for(word).iter().map(|k| format!("<kbd>{}</kbd>", esc(k))).collect::<Vec<_>>().join(" ");
            out.push_str(&format!("<tr><td>{}</td><td><code>{}</code></td><td>{}</td></tr>", keys, esc(cmd), esc(what)));
        }
        out
    }).unwrap_or_default();
    format!(r#"<h1>gaze</h1>
<p>Looking out onto the web. <kbd>Esc</kbd> always returns to normal mode; <kbd>:</kbd> opens the command line.
Every key runs a command from the table; <code>:bind &lt;keys&gt; &lt;command&gt;</code> changes one and
<code>~/.gaze/keys.yml</code> keeps the change. Key names: plain characters as they are, else
<code>&lt;Ctrl-d&gt;</code>, <code>&lt;Alt-1&gt;</code>, <code>&lt;Shift-Left&gt;</code>, <code>&lt;Space&gt;</code>.</p>
<table>{}</table>
<h2>Tab groups</h2>
<p>A group is a named, coloured run of tabs, as in Firefox. A tab opened from a grouped tab joins the group.
A folded group shows as its name and a count; its tabs are skipped until it is unfolded. A group with no tabs
stays, dimmed at the end of the bar, until <code>:group-delete</code>. Groups come back with the session, and
the <code>groups:</code> list in config.yml names groups that exist from the start.</p>
<h2>Open prompt</h2>
<p><kbd>o</kbd> lists the pages you were at last; typing narrows the list to pages whose URL or title holds every word,
bookmarks (★) first. <kbd>Tab</kbd> and <kbd>Shift-Tab</kbd> put one on the line, <kbd>Return</kbd> opens it.
Visits are kept in <code>~/.gaze/history</code>, the last five thousand pages.</p>
<h2>Passwords</h2>
<p>Logins live in <code>~/.gaze/passwords</code>, sealed with a master password you choose the first time.
A login form is filled when the page loads; after a sign-in with a new or changed password gaze asks whether to save it.
A site's HTTP password dialog is answered from the store too, when it is open.</p>
<h2>Ad blocking</h2>
<p>On by default (<code>adblock: false</code> in config.yml turns it off). The first start fetches Steven Black's hosts list
to <code>~/.gaze/adblock/hosts</code> and compiles it into a WebKit content filter; every domain on the list is blocked.
<code>:adblock-update</code> fetches it again.</p>
<h2>Files</h2>
<p><code>~/.gaze/config.yml</code>: home page, search engine, download folder, zoom, scroll step, ad blocking.
<code>~/.gaze/keys.yml</code>: your key changes. <code>~/.gaze/bookmarks</code>: one per line.
<code>~/.gaze/session.json</code>: the open tabs and groups.</p>
"#, table)
}
