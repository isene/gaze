//! The script gaze puts into every page: hint mode, insert-mode tracking,
//! login capture and login fill. Pages talk back through
//! `window.webkit.messageHandlers.gaze` with one JSON object per message.

pub const PAGE: &str = r#"
(function () {
  if (window.__gaze) return;
  const post = m => { try { window.webkit.messageHandlers.gaze.postMessage(JSON.stringify(m)); } catch (e) {} };
  const visible = el => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    const cs = getComputedStyle(el);
    return cs.visibility !== 'hidden' && cs.display !== 'none';
  };
  const editable = el => {
    if (!el || !el.tagName) return false;
    if (el.isContentEditable) return true;
    const tag = el.tagName;
    if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
    if (tag !== 'INPUT') return false;
    const t = (el.type || 'text').toLowerCase();
    return !['button', 'submit', 'checkbox', 'radio', 'reset', 'file', 'image', 'range', 'color'].includes(t);
  };
  const setValue = (el, v) => {
    if (!el || v == null) return;
    const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    const d = Object.getOwnPropertyDescriptor(proto, 'value');
    if (d && d.set) d.set.call(el, v); else el.value = v;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };
  const userField = pw => {
    const scope = pw.form || document;
    const inputs = [...scope.querySelectorAll('input')];
    const i = inputs.indexOf(pw);
    for (let k = i - 1; k >= 0; k--) {
      const t = (inputs[k].type || 'text').toLowerCase();
      if (['text', 'email', 'tel'].includes(t) && visible(inputs[k])) return inputs[k];
    }
    return null;
  };
  const passwordFields = () => [...document.querySelectorAll('input[type=password]')].filter(visible);

  const G = window.__gaze = {
    // ---- insert mode follows the focus ----
    trackFocus() {
      document.addEventListener('focusin', e => { if (editable(e.target)) post({ t: 'focus', editable: true }); }, true);
      document.addEventListener('focusout', e => { if (editable(e.target)) post({ t: 'focus', editable: false }); }, true);
      // A click says it too: focusin does not fire again for a field that
      // already had the focus when the window lost and regained it.
      document.addEventListener('mousedown', e => {
        const t = e.target;
        const field = t && t.closest ? (editable(t) ? t : t.closest('input, textarea, select, [contenteditable]')) : null;
        post({ t: 'focus', editable: !!(field && editable(field)) });
      }, true);
    },
    focusables() {
      const sel = 'input:not([type=hidden]):not([disabled]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), a[href], [contenteditable], [tabindex]:not([tabindex="-1"])';
      return [...document.querySelectorAll(sel)].filter(visible);
    },
    focusNext(dir) {
      const list = this.focusables();
      if (!list.length) return false;
      const i = list.indexOf(document.activeElement);
      const next = list[(i + dir + list.length) % list.length];
      next.focus();
      if (next.select && (next.tagName === 'INPUT' || next.tagName === 'TEXTAREA')) { try { next.select(); } catch (e) {} }
      return true;
    },
    blur() { if (document.activeElement) document.activeElement.blur(); },
    focusFirstInput() {
      const el = [...document.querySelectorAll('input, textarea, [contenteditable]')].find(e => editable(e) && visible(e));
      if (el) { el.focus(); return true; }
      return false;
    },
    hasPasswordField() { return passwordFields().length > 0; },

    // ---- logins ----
    captureLogins() {
      const remember = () => {
        const pws = passwordFields().filter(p => p.value);
        if (!pws.length) return;
        const pw = pws[0];
        const u = userField(pw);
        post({ t: 'login', origin: location.origin, username: u ? u.value : '', password: pw.value });
      };
      document.addEventListener('submit', remember, true);
      document.addEventListener('keydown', e => { if (e.key === 'Enter' && e.target && e.target.type === 'password') remember(); }, true);
      document.addEventListener('click', e => {
        const b = e.target && e.target.closest && e.target.closest('button, input[type=submit], [role=button]');
        if (b) remember();
      }, true);
    },
    fill(u, p) {
      const pws = passwordFields();
      if (pws.length) {
        const pw = pws[0];
        setValue(userField(pw), u);
        setValue(pw, p);
        return 'both';
      }
      const uf = [...document.querySelectorAll(
        'input[type=email], input[autocomplete=username], input[name*=user i], input[name*=email i], input[id*=user i], input[id*=email i], input[type=text]')]
        .find(visible);
      if (uf) { setValue(uf, u); return 'user'; }
      return 'none';
    },

    // ---- hints ----
    hints: [], typed: '', newTab: false,
    clickable() {
      const sel = 'a[href], button, input:not([type=hidden]), select, textarea, summary, [role=button], [role=link], [role=menuitem], [role=tab], [role=checkbox], [role=option], [onclick], [tabindex]:not([tabindex="-1"]), label, video, audio, [contenteditable]';
      const out = [], vw = innerWidth, vh = innerHeight;
      for (const el of document.querySelectorAll(sel)) {
        const r = el.getBoundingClientRect();
        if (r.width < 2 || r.height < 2 || r.bottom < 0 || r.right < 0 || r.top > vh || r.left > vw) continue;
        const cs = getComputedStyle(el);
        if (cs.visibility === 'hidden' || cs.display === 'none' || cs.opacity === '0') continue;
        const x = Math.min(vw - 1, Math.max(0, r.left + Math.min(r.width / 2, 8)));
        const y = Math.min(vh - 1, Math.max(0, r.top + Math.min(r.height / 2, 8)));
        const top = document.elementFromPoint(x, y);
        if (top && top !== el && !el.contains(top) && !top.contains(el)) continue;
        out.push({ el, r });
      }
      return out;
    },
    labels(n) {
      const chars = 'asdfghjkl';
      let len = 1;
      while (Math.pow(chars.length, len) < n) len++;
      const out = [];
      for (let i = 0; i < n; i++) {
        let s = '', k = i;
        for (let j = 0; j < len; j++) { s = chars[k % chars.length] + s; k = Math.floor(k / chars.length); }
        out.push(s);
      }
      return out;
    },
    startHints(newTab) {
      this.stopHints();
      this.newTab = newTab;
      const items = this.clickable();
      const labels = this.labels(items.length);
      const box = document.createElement('div');
      box.id = '__gaze_hints';
      box.style.cssText = 'position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none;';
      items.forEach((it, i) => {
        const s = document.createElement('span');
        s.style.cssText = 'position:fixed;left:' + Math.max(0, it.r.left) + 'px;top:' + Math.max(0, it.r.top) + 'px;' +
          'background:#f5c542;color:#000;font:bold 11px monospace;line-height:14px;padding:0 3px;border-radius:2px;border:1px solid #6b5410;pointer-events:none;';
        box.appendChild(s);
        this.hints.push({ el: it.el, label: labels[i], span: s });
      });
      document.documentElement.appendChild(box);
      this.showHints();
      return items.length;
    },
    showHints() {
      for (const h of this.hints) {
        if (h.label.startsWith(this.typed)) {
          h.span.style.display = '';
          h.span.innerHTML = '<b style="color:#a00">' + this.typed.toUpperCase() + '</b>' + h.label.slice(this.typed.length).toUpperCase();
        } else h.span.style.display = 'none';
      }
    },
    hintKey(ch) {
      if (ch === '\b') this.typed = this.typed.slice(0, -1); else this.typed += ch;
      const left = this.hints.filter(h => h.label.startsWith(this.typed));
      if (!left.length) { this.stopHints(); return 'none'; }
      if (left.length === 1 && left[0].label === this.typed) {
        const el = left[0].el;
        this.stopHints();
        this.activate(el);
        return 'done';
      }
      this.showHints();
      return 'more';
    },
    activate(el) {
      if (this.newTab) {
        const a = el.closest('a[href]');
        if (a) { post({ t: 'open', uri: a.href, background: true }); return; }
      }
      if (editable(el)) { el.focus(); return; }
      if (el.focus) el.focus();
      el.click();
    },
    stopHints() {
      const b = document.getElementById('__gaze_hints');
      if (b) b.remove();
      this.hints = [];
      this.typed = '';
    },
  };
  G.trackFocus();
  G.captureLogins();
})();
"#;

/// Scroll by a number of pixels, or a share of the window when `page`.
pub fn scroll(dx: i32, dy: f64, page: bool) -> String {
    if page {
        format!("window.scrollBy({{left: 0, top: innerHeight * {}, behavior: 'instant'}})", dy)
    } else {
        format!("window.scrollBy({{left: {}, top: {}, behavior: 'instant'}})", dx, dy)
    }
}

pub const SCROLL_TOP: &str = "window.scrollTo({top: 0, behavior: 'instant'})";
pub const SCROLL_BOTTOM: &str = "window.scrollTo({top: document.documentElement.scrollHeight, behavior: 'instant'})";

/// Dark mode for pages that have none of their own. gaze first asks for
/// a dark page through the GTK theme, which sites with a dark style pick
/// up as `prefers-color-scheme: dark`. What is left light after that is
/// turned around here: the whole page is inverted and the hues turned
/// back, then pictures and video are inverted a second time so they look
/// as they should. Added to a view only while dark mode is on.
pub const DARK: &str = r#"
(function () {
  // Dark pages can be off for this site alone. gaze puts the list and
  // the default in front of this script; a page filed under neither
  // follows the default.
  const filed = window.__gazeDarkSites || {};
  const site = (location.protocol === 'http:' || location.protocol === 'https:')
    ? location.hostname.replace(/^www\./, '').toLowerCase()
    : location.protocol.replace(':', '').toLowerCase();
  const want = Object.prototype.hasOwnProperty.call(filed, site)
    ? filed[site] : !!window.__gazeDarkDefault;
  if (!want) return;
  const P = 'data-gaze-photo', G = 'data-gaze-backdrop';
  const TURN = '{filter:invert(1) hue-rotate(180deg)}';
  const MEDIA = 'img,video,canvas,embed,object,iframe';
  const each = (pre, post) => MEDIA.split(',').map(t => pre + t + post).join(',');
  const CSS =
    'html{filter:invert(1) hue-rotate(180deg);background:#fff}' +
    MEDIA + ',[' + P + '],[' + G + ']' + TURN +
    '[' + G + ']>*' + TURN +
    each('[' + G + ']>', '') + '{filter:none}' +
    each('[' + P + '] ', '') + '{filter:none}';
  const colour = el => {
    if (!el) return null;
    const m = getComputedStyle(el).backgroundColor.match(/[\d.]+/g);
    if (!m) return null;
    if (m.length > 3 && +m[3] < 0.5) return null;
    return 0.2126 * +m[0] + 0.7152 * +m[1] + 0.0722 * +m[2];
  };
  // How bright the page looks where you are reading it. Asking the body
  // or the root is not enough: a site can paint the root dark and still
  // lay every box of text on white. So a few points across the window
  // are sampled, and each one climbs to the first thing with a colour
  // behind it.
  const behind = (x, y) => {
    let el = document.elementFromPoint(x, y);
    while (el) {
      const c = colour(el);
      if (c !== null) return c;
      el = el.parentElement;
    }
    return null;
  };
  const light = () => {
    const w = innerWidth, h = innerHeight;
    const spots = [[0.5, 0.3], [0.5, 0.6], [0.5, 0.9], [0.2, 0.5], [0.8, 0.5]];
    let sum = 0, seen = 0;
    for (const [fx, fy] of spots) {
      const c = behind(Math.round(w * fx), Math.round(h * fy));
      if (c !== null) { sum += c; seen++; }
    }
    if (!seen) {
      const c = colour(document.body);
      return c === null ? true : c > 140;
    }
    return sum / seen > 140;
  };
  // A picture set as an element's background is no <img>, and CSS cannot
  // ask for one, so the elements carrying one are marked here. Each is
  // turned back so the picture keeps its colours, and the walk stops
  // there: a second turn inside the first would undo it.
  //
  // An element that fills the window is the page itself with a picture
  // behind it. Turning that one back would take the whole page light
  // again, so what it holds is turned once more and stays dark.
  const photos = root => {
    const room = innerWidth * innerHeight;
    const walk = el => {
      for (let n = el.firstElementChild; n; n = n.nextElementSibling) {
        const bg = getComputedStyle(n).backgroundImage;
        if (bg === 'none' || bg.indexOf('url(') < 0) { walk(n); continue; }
        const box = n.getBoundingClientRect();
        // An icon drawn as a background is part of the writing and turns
        // with it. Only a box big enough to hold a picture is turned back.
        if (box.width < 48 || box.height < 48) { walk(n); continue; }
        n.setAttribute(box.width * box.height > 0.6 * room ? G : P, '');
      }
    };
    walk(root);
  };
  const apply = () => {
    if (!document.body) return;
    const on = light();
    const sheet = document.getElementById('__gaze_dark');
    // The first look comes before the page is fully laid out, so the
    // second one may find it was wrong and take the sheet off again.
    if (!on) {
      if (sheet) {
        sheet.remove();
        document.querySelectorAll('[' + P + '],[' + G + ']').forEach(e => {
          e.removeAttribute(P); e.removeAttribute(G);
        });
      }
      return;
    }
    if (!sheet) {
      const s = document.createElement('style');
      s.id = '__gaze_dark';
      s.textContent = CSS;
      (document.head || document.documentElement).appendChild(s);
    }
    photos(document.body);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', apply, { once: true });
  else apply();
  // Pictures that arrive after the page is built get their turn here.
  window.addEventListener('load', apply, { once: true });
})();
"#;

/// Take the dark stylesheet off a page again.
pub const UNDARK: &str = "{ const s = document.getElementById('__gaze_dark'); if (s) s.remove(); document.querySelectorAll('[data-gaze-photo],[data-gaze-backdrop]').forEach(e => { e.removeAttribute('data-gaze-photo'); e.removeAttribute('data-gaze-backdrop'); }); }";
