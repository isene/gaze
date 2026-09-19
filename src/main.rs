//! gaze: looking out onto the web. A keyboard-driven browser around
//! WebKitGTK, with tab groups and saved logins.

mod config;
mod js;
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
    CookiePersistentStorage, Download, FindOptions, LoadEvent, NavigationPolicyDecision, NetworkSession,
    PolicyDecisionType, ResponsePolicyDecision, Settings, URISchemeRequest, UserContentInjectedFrames,
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
    vbox.append(&tabbar);
    vbox.append(&stack);
    vbox.append(&bottom);
    vbox.append(&entry);
    window.set_child(Some(&vbox));

    let session_path = dir.join("session.json");
    let tabs = Tabs::load(&session_path);
    let store = Store::new(dir.join("passwords"));
    let shared: Shared = Rc::new(RefCell::new(App {
        ui: Ui { window: window.clone(), tabbar, stack, status, right, entry: entry.clone() },
        cfg, tabs, views: HashMap::new(), session, settings,
        mode: Mode::Normal, keys: String::new(), ask: Ask::Command, prompt: None,
        message: String::new(), hover: String::new(), store, fill_at: HashMap::new(),
        closed: Vec::new(), find: String::new(), session_path, save_pending: false,
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
        window.connect_close_request(move |_| { save_session(&s); glib::Propagation::Proceed });
    }

    // Bring the last session back. Only the current tab loads now; the
    // others load when you go to them.
    let restored: Vec<(u64, bool)> = {
        let a = shared.borrow();
        a.tabs.tabs.iter().enumerate().map(|(i, t)| (t.id, i == a.tabs.active)).collect()
    };
    for (id, _) in &restored {
        let view = make_view(&shared, *id);
        attach(&shared, *id, view);
    }
    if !restored.is_empty() { show_active(&shared); }
    window.present();
    shared
}

fn style() {
    let css = "
        .tabbar, .statusbar { font-family: monospace; font-size: 12px; padding: 2px 6px;
                              background: #1e1e1e; color: #c8c8c8; }
        .cmdline { font-family: monospace; font-size: 13px; background: #101010; color: #ffffff;
                   border: none; border-radius: 0; padding: 2px 6px; min-height: 0; }
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
fn make_view(shared: &Shared, id: u64) -> WebView {
    let (session, settings, zoom) = {
        let a = shared.borrow();
        (a.session.clone(), a.settings.clone(), a.cfg.zoom)
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
    let view = WebView::builder().network_session(&session).user_content_manager(&ucm).settings(&settings).build();
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
                if let Some(i) = a.tabs.index_of(id) {
                    a.tabs.tabs[i].title = v.title().map(|t| t.to_string()).unwrap_or_default();
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
        let s = shared.clone();
        view.connect_create(move |_, action| {
            if let Some(uri) = action.request().and_then(|r| r.uri()) {
                let s = s.clone();
                glib::idle_add_local_once(move || { open_tab(&s, &uri, false); });
            }
            None
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
        let color = group.map(|g| tabs::color_hex(&g.color)).unwrap_or(if t.pending { "#7a7a7a" } else { "#c8c8c8" });
        if i == tabs.active {
            out.push_str(&format!(" <span background=\"#3c3c3c\" foreground=\"#ffffff\"><b> {} {} </b></span>", n, text));
        } else {
            out.push_str(&format!(" <span foreground=\"{}\">{} {}</span>", color, n, text));
        }
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
    let view = make_view(shared, id);
    view.load_uri(uri);
    attach(shared, id, view);
    if background { refresh(shared); } else { show_active(shared); }
    save_session(shared);
    id
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
    let alt = state.contains(gdk::ModifierType::ALT_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    let ch = key.to_unicode();
    match mode {
        Mode::Command => {
            if key == gdk::Key::Escape { end_ask(shared); return Stop; }
            Proceed
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
        Mode::Normal => { normal_key(shared, key, ch, ctrl, alt, shift); Stop }
    }
}

fn normal_key(shared: &Shared, key: gdk::Key, ch: Option<char>, ctrl: bool, alt: bool, shift: bool) {
    use gdk::Key;
    let step = shared.borrow().cfg.scroll_step as f64;
    if alt {
        if let Some(d) = ch.and_then(|c| c.to_digit(10)) {
            if d >= 1 { goto_visible(shared, d as usize - 1); }
        }
        return;
    }
    if ctrl {
        match ch {
            Some('d') => scroll_page(shared, 0.5),
            Some('u') => scroll_page(shared, -0.5),
            Some('f') => scroll_page(shared, 0.9),
            Some('b') => scroll_page(shared, -0.9),
            Some('q') => quit(shared),
            _ => {}
        }
        return;
    }
    let handled = match key {
        Key::Escape => {
            {
                let mut a = shared.borrow_mut();
                a.keys.clear();
                a.message.clear();
                a.prompt = None;
            }
            with_find(shared, |f| f.search_finish());
            refresh(shared);
            true
        }
        Key::Down => { scroll_by(shared, 0, step); true }
        Key::Up => { scroll_by(shared, 0, -step); true }
        Key::Left => { scroll_by(shared, -(step as i32), 0.0); true }
        Key::Right => { scroll_by(shared, step as i32, 0.0); true }
        Key::Page_Down => { scroll_page(shared, 0.9); true }
        Key::Page_Up => { scroll_page(shared, -0.9); true }
        Key::space => { scroll_page(shared, if shift { -0.9 } else { 0.9 }); true }
        Key::Home => { with_view(shared, |v| run_js(v, js::SCROLL_TOP)); true }
        Key::End => { with_view(shared, |v| run_js(v, js::SCROLL_BOTTOM)); true }
        Key::BackSpace => { with_view(shared, |v| v.go_back()); true }
        _ => false,
    };
    if handled { return; }
    let Some(c) = ch else { return };
    if c.is_control() { return; }
    let seq = {
        let mut a = shared.borrow_mut();
        a.keys.push(c);
        a.keys.clone()
    };
    let uri = shared.borrow().tabs.current().map(|t| t.uri.clone()).unwrap_or_default();
    match seq.as_str() {
        "g" | "z" | "y" | "p" | "P" | "Z" => { refresh(shared); return; }
        "j" => scroll_by(shared, 0, step),
        "k" => scroll_by(shared, 0, -step),
        "h" => scroll_by(shared, -(step as i32), 0.0),
        "l" => scroll_by(shared, step as i32, 0.0),
        "gg" => with_view(shared, |v| run_js(v, js::SCROLL_TOP)),
        "G" => with_view(shared, |v| run_js(v, js::SCROLL_BOTTOM)),
        "H" => with_view(shared, |v| v.go_back()),
        "L" => with_view(shared, |v| v.go_forward()),
        "r" => with_view(shared, |v| v.reload()),
        "R" => with_view(shared, |v| v.reload_bypass_cache()),
        "o" => begin_ask(shared, Ask::Command, "open "),
        "O" | "t" => begin_ask(shared, Ask::Command, "tabopen "),
        "go" => begin_ask(shared, Ask::Command, &format!("open {}", uri)),
        "gO" => begin_ask(shared, Ask::Command, &format!("tabopen {}", uri)),
        ":" => begin_ask(shared, Ask::Command, ""),
        "/" => begin_ask(shared, Ask::Find, ""),
        "n" => with_find(shared, |f| f.search_next()),
        "N" => with_find(shared, |f| f.search_previous()),
        "d" => { let i = shared.borrow().tabs.active; close_tab(shared, i); }
        "u" => undo_close(shared),
        "J" => { let i = shared.borrow().tabs.neighbour(1); goto_tab(shared, i); }
        "K" => { let i = shared.borrow().tabs.neighbour(-1); goto_tab(shared, i); }
        "g0" => goto_visible(shared, 0),
        "g$" => { let n = shared.borrow().tabs.visible_indices().len(); goto_visible(shared, n.saturating_sub(1)); }
        "f" => start_hints(shared, false),
        "F" => start_hints(shared, true),
        "i" => { set_mode(shared, Mode::Insert); with_view(shared, |v| { v.grab_focus(); }); }
        "gi" => with_view(shared, |v| run_js(v, "window.__gaze && window.__gaze.focusFirstInput()")),
        "yy" => { clipboard().set_text(&uri); set_message(shared, &format!("Yanked {}", uri)); }
        "yt" => {
            let t = shared.borrow().tabs.current().map(|t| t.title.clone()).unwrap_or_default();
            clipboard().set_text(&t);
            set_message(shared, &format!("Yanked {}", t));
        }
        "pp" => paste_and_open(shared, false),
        "PP" => paste_and_open(shared, true),
        "+" => zoom(shared, 0.1),
        "-" => zoom(shared, -0.1),
        "=" => { let z = shared.borrow().cfg.zoom; with_view(shared, |v| v.set_zoom_level(z)); set_message(shared, "Zoom reset"); }
        "zc" => fold_current(shared, Some(true)),
        "zo" => fold_current(shared, Some(false)),
        "za" => fold_current(shared, None),
        "zM" => { shared.borrow_mut().tabs.collapse_all(true); refresh(shared); save_session(shared); }
        "zR" => { shared.borrow_mut().tabs.collapse_all(false); refresh(shared); save_session(shared); }
        "gp" => fill_next(shared, false),
        "ZZ" => quit(shared),
        "?" => { open_tab(shared, "gaze://help", false); }
        _ => {}
    }
    shared.borrow_mut().keys.clear();
    refresh(shared);
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
        e.set_text(prefill);
        e
    };
    refresh(shared);
    entry.set_visible(true);
    entry.grab_focus();
    entry.set_position(-1);
}

fn end_ask(shared: &Shared) {
    let entry = {
        let mut a = shared.borrow_mut();
        a.mode = Mode::Normal;
        a.ui.entry.clone()
    };
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

fn find(shared: &Shared, text: &str) {
    let text = if text.is_empty() { shared.borrow().find.clone() } else { text.to_string() };
    if text.is_empty() { return; }
    shared.borrow_mut().find = text.clone();
    with_find(shared, |f| f.search(&text, (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits(), u32::MAX));
}

fn run_command(shared: &Shared, line: &str) {
    let line = line.trim();
    if line.is_empty() { return; }
    let (cmd, arg) = line.split_once(char::is_whitespace).map(|(c, a)| (c, a.trim())).unwrap_or((line, ""));
    let search = shared.borrow().cfg.search.clone();
    match cmd {
        "open" | "o" => { let uri = config::to_uri(arg, &search); with_view(shared, |v| v.load_uri(&uri)); }
        "tabopen" | "t" => { let uri = config::to_uri(arg, &search); open_tab(shared, &uri, false); }
        "home" => { let h = shared.borrow().cfg.home.clone(); with_view(shared, |v| v.load_uri(&h)); }
        "back" => with_view(shared, |v| v.go_back()),
        "forward" => with_view(shared, |v| v.go_forward()),
        "reload" => with_view(shared, |v| v.reload()),
        "stop" => with_view(shared, |v| v.stop_loading()),
        "close" | "q" => { let i = shared.borrow().tabs.active; close_tab(shared, i); }
        "quit" | "qa" | "wq" => quit(shared),
        "undo" => undo_close(shared),
        "tab" => {
            match arg.parse::<usize>() {
                Ok(n) if n >= 1 => goto_visible(shared, n - 1),
                _ => set_message(shared, "tab <number>"),
            }
        }
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
        "group" => {
            if arg.is_empty() { begin_ask(shared, Ask::GroupName, ""); } else { group_current(shared, arg); }
        }
        "ungroup" => {
            let i = shared.borrow().tabs.active;
            shared.borrow_mut().tabs.ungroup(i);
            refresh(shared);
            save_session(shared);
        }
        "group-rename" | "group-color" | "group-close" | "group-collapse" | "group-expand" => group_command(shared, cmd, arg),
        "groups" => {
            let text = {
                let a = shared.borrow();
                a.tabs.groups.iter().map(|g| format!("{} ({}, {}{})", g.name, a.tabs.tabs_in(g.id).len(), g.color, if g.collapsed { ", folded" } else { "" }))
                    .collect::<Vec<_>>().join("  ·  ")
            };
            set_message(shared, if text.is_empty() { "No groups" } else { &text });
        }
        "zoom" => {
            match arg.trim_end_matches('%').parse::<f64>() {
                Ok(p) if p > 0.0 => { with_view(shared, |v| v.set_zoom_level(p / 100.0)); set_message(shared, &format!("Zoom {:.0}%", p)); }
                _ => set_message(shared, "zoom <percent>"),
            }
        }
        "find" => find(shared, arg),
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
        "help" => { open_tab(shared, "gaze://help", false); }
        "inspect" | "devtools" => with_view(shared, |v| { if let Some(i) = v.inspector() { i.show(); } }),
        "session-save" => { save_session(shared); set_message(shared, "Session saved"); }
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
                set_message(shared, &format!("group-color: {}", names));
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

// -------------------------------------------------------- gaze:// pages

fn serve_internal(req: &URISchemeRequest) {
    let uri = req.uri().map(|u| u.to_string()).unwrap_or_default();
    let name = uri.trim_start_matches("gaze:").trim_matches('/').to_string();
    let body = match name.as_str() {
        "help" => HELP.to_string(),
        "passwords" => passwords_page(),
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
 table{border-collapse:collapse}td,th{text-align:left;padding:3px 14px 3px 0;vertical-align:top}th{color:#e0b93a}";

const HELP: &str = r#"<h1>gaze</h1>
<p>Looking out onto the web. Keys work like qutebrowser and vim; <kbd>Esc</kbd> always returns to normal mode.</p>
<h2>Pages</h2>
<table>
<tr><td><kbd>o</kbd> / <kbd>O</kbd></td><td>open a URL or search here / in a new tab (<kbd>go</kbd>, <kbd>gO</kbd> start from the current URL)</td></tr>
<tr><td><kbd>f</kbd> / <kbd>F</kbd></td><td>hints: type the letters on a link to follow it / open it in a background tab</td></tr>
<tr><td><kbd>H</kbd> / <kbd>L</kbd></td><td>back / forward</td></tr>
<tr><td><kbd>r</kbd> / <kbd>R</kbd></td><td>reload / reload without the cache</td></tr>
<tr><td><kbd>j k h l</kbd>, <kbd>gg</kbd>, <kbd>G</kbd>, <kbd>Ctrl-d</kbd> / <kbd>Ctrl-u</kbd>, <kbd>Space</kbd></td><td>scroll</td></tr>
<tr><td><kbd>/</kbd>, <kbd>n</kbd> / <kbd>N</kbd></td><td>find on the page, next / previous</td></tr>
<tr><td><kbd>i</kbd>, <kbd>gi</kbd></td><td>insert mode (type into the page) / focus the first field</td></tr>
<tr><td><kbd>yy</kbd>, <kbd>yt</kbd></td><td>copy the URL / the title</td></tr>
<tr><td><kbd>pp</kbd> / <kbd>PP</kbd></td><td>open what the clipboard holds here / in a new tab</td></tr>
<tr><td><kbd>+</kbd> <kbd>-</kbd> <kbd>=</kbd></td><td>zoom in, out, reset</td></tr>
</table>
<h2>Tabs</h2>
<table>
<tr><td><kbd>t</kbd></td><td>new tab</td></tr>
<tr><td><kbd>J</kbd> / <kbd>K</kbd>, <kbd>Alt-1</kbd>…<kbd>Alt-9</kbd>, <kbd>g0</kbd>, <kbd>g$</kbd></td><td>next / previous, by number, first, last</td></tr>
<tr><td><kbd>d</kbd> / <kbd>u</kbd></td><td>close / bring back the last closed</td></tr>
<tr><td><code>:tab-move +1</code>, <code>:tab-move 3</code></td><td>move the tab</td></tr>
</table>
<h2>Tab groups</h2>
<p>A group is a named, coloured run of tabs, as in Firefox. A new tab opened from a grouped tab joins the group.</p>
<table>
<tr><td><code>:group &lt;name&gt;</code></td><td>put this tab in the group (made on the spot when new)</td></tr>
<tr><td><code>:ungroup</code></td><td>take it out again</td></tr>
<tr><td><kbd>zc</kbd> / <kbd>zo</kbd> / <kbd>za</kbd></td><td>fold / unfold / toggle this tab's group; <kbd>zM</kbd> and <kbd>zR</kbd> do all groups</td></tr>
<tr><td><code>:group-rename</code>, <code>:group-color</code>, <code>:group-close</code>, <code>:groups</code></td><td>rename, recolour (blue red yellow green pink purple orange cyan gray), close all its tabs, list the groups</td></tr>
</table>
<h2>Passwords</h2>
<p>Logins live in <code>~/.gaze/passwords</code>, sealed with a master password you choose the first time. A login form gets filled when the page loads.</p>
<table>
<tr><td><kbd>gp</kbd></td><td>unlock and fill; press again for the next saved login of the site</td></tr>
<tr><td>after logging in</td><td>gaze asks whether to save a new or changed password</td></tr>
<tr><td><code>:passwords</code></td><td>list the sites and usernames</td></tr>
<tr><td><code>:password-import ~/logins.csv</code></td><td>import the CSV that Firefox writes from about:logins → Export Logins</td></tr>
<tr><td><code>:password-remove &lt;username&gt;</code>, <code>:password-lock</code></td><td>forget one login for this site; lock the store for this session</td></tr>
</table>
<h2>Commands</h2>
<p><kbd>:</kbd> opens the command line. <code>open</code>, <code>tabopen</code>, <code>home</code>, <code>back</code>, <code>forward</code>, <code>reload</code>, <code>stop</code>, <code>close</code>, <code>undo</code>, <code>tab</code>, <code>zoom 120</code>, <code>find</code>, <code>inspect</code>, <code>session-save</code>, <code>quit</code>.</p>
<p>Settings live in <code>~/.gaze/config.yml</code>: home page, search engine, download folder, zoom, scroll step.</p>
"#;
