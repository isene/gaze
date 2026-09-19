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
