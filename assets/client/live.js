// md-render live-update client (owner: server). Protocol: docs/DESIGN.md §7, GRIP §11.1.
//
// Keeps #md-content in sync with the Markdown file on disk:
//   * one EventSource per page (`retry: 1000` from the server; manual back-off when the
//     browser gives up), closed on `pagehide`, resynced on `pageshow` / visibility;
//   * content-keyed reconciliation of the top-level blocks, so unchanged blocks keep their
//     DOM (rendered math and mermaid, open <details>, loaded images, text selection);
//   * scroll anchoring by the `data-sourcepos` start line of the block at the top of the
//     viewport (pinned-to-bottom stays pinned; heading / scroll-ratio fallbacks);
//   * a page reload when the server restarted, stylesheet hot-swap, image cache-busting,
//     dismissible error banners and an `(Error) ` title prefix while an error is active.
//
// Configuration: <script id="mdr-config" type="application/json"> with
// {"live": {"events_url", "render_url", "doc", "version", "server_id"}} (null = no live
// updates). After every update the event `mdr:content-updated` is dispatched on `document`
// with `detail: {root, added}` so the flavor scripts process the new nodes only.
//
// Plain ES2018, no dependencies.
(() => {
  'use strict';

  const PREFIX = '/__/md-render';
  const ANCHOR_KEY = 'mdr-live-anchor:';
  const RELOAD_KEY = 'mdr-live-reload:';
  const MAX_BACKOFF_MS = 10000;
  const BOTTOM_SLACK_PX = 4;
  const RENDERING_TIMEOUT_MS = 10000;

  const live = readConfig();
  if (!live || !live.events_url || typeof window.EventSource !== 'function') {
    return;
  }

  const state = {
    doc: String(live.doc || ''),
    serverId: String(live.server_id || ''),
    // Latest document state delivered (content, error or deleted) and the version of the
    // content currently in the DOM; the two differ while an error is shown.
    version: Number(live.version) || 0,
    contentVersion: Number(live.version) || 0,
    errorVersion: 0, // version of the error / deletion behind the "doc" banner
    stylesVersion: null, // baseline from the first `hello`
    baseTitle: document.title.replace(/^\(Error\) /, ''),
    source: null,
    backoff: 1000,
    reconnectTimer: 0,
    renderingTimer: 0,
    pending: null, // newest `content` waiting for the next animation frame
    frame: 0,
    lastResync: 0,
    reloading: false,
  };
  const keys = new WeakMap(); // top-level node -> key computed from pristine server HTML
  const banners = new Map(); // slot ("doc" | "bundle" | "warnings") -> element
  const activeErrors = new Set(); // slots whose banner is an error
  let statusDot = null;

  // ---------------------------------------------------------------------------------------
  // Setup

  function readConfig() {
    const el = document.getElementById('mdr-config');
    if (!el) return null;
    try {
      const cfg = JSON.parse(el.textContent || 'null');
      return cfg && typeof cfg.live === 'object' ? cfg.live : null;
    } catch (e) {
      console.warn('md-render: cannot parse #mdr-config', e);
      return null;
    }
  }

  function root() {
    return document.getElementById('md-content');
  }

  function init() {
    const container = root();
    if (!container) return;
    injectStyle();
    // Keys come from the pristine server HTML, so compute them before flavor scripts
    // (math, mermaid, copy buttons) mutate the blocks.
    for (let n = container.firstChild; n; n = n.nextSibling) keys.set(n, keyOf(n));
    restoreSavedAnchor(container);
    window.addEventListener('pagehide', closeSource);
    window.addEventListener('pageshow', (ev) => {
      if (ev.persisted) {
        connect(); // back/forward cache: the page is stale (GRIP #255)
        resync();
      }
    });
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState !== 'visible') return;
      if (!state.source || state.source.readyState === EventSource.CLOSED) connect();
      resync();
    });
    connect();
  }

  // ---------------------------------------------------------------------------------------
  // Connection

  function connect() {
    clearTimeout(state.reconnectTimer);
    closeSource();
    // `v`/`sid` tell the server which state this page shows, so it replays what we missed;
    // EventSource adds `Last-Event-ID` on its own reconnects.
    const sep = live.events_url.indexOf('?') >= 0 ? '&' : '?';
    const url = live.events_url + sep + 'v=' + encodeURIComponent(state.version) +
      '&sid=' + encodeURIComponent(state.serverId);
    let es;
    try {
      es = new EventSource(url);
    } catch (e) {
      scheduleReconnect();
      return;
    }
    state.source = es;
    const handlers = {
      hello: onHello,
      rendering: onRendering,
      content: onContent,
      error: onError,
      deleted: onDeleted,
      assets: onAssets,
      styles: onStyles,
      reload: () => reloadPage(null),
      shutdown: () => setStatus('stopped', 'md-render stopped; reconnecting…'),
    };
    Object.keys(handlers).forEach((name) => {
      es.addEventListener(name, (ev) => {
        if (es !== state.source) return;
        let data;
        try {
          data = JSON.parse(ev.data);
        } catch (e) {
          return;
        }
        if (data && typeof data === 'object') handlers[name](data);
      });
    });
    es.onopen = () => {
      state.backoff = 1000;
      setStatus('connected', '');
    };
    es.onerror = () => {
      if (es !== state.source) return;
      setStatus('disconnected', 'Disconnected from md-render; reconnecting…');
      // CLOSED means the browser will not retry (e.g. an HTTP error status): we do.
      if (es.readyState === EventSource.CLOSED) scheduleReconnect();
    };
  }

  function closeSource() {
    if (state.source) {
      state.source.close();
      state.source = null;
    }
  }

  function scheduleReconnect() {
    closeSource();
    clearTimeout(state.reconnectTimer);
    state.reconnectTimer = setTimeout(connect, state.backoff);
    state.backoff = Math.min(state.backoff * 2, MAX_BACKOFF_MS);
  }

  // ---------------------------------------------------------------------------------------
  // Events

  function onHello(d) {
    if (d.server_id && state.serverId && d.server_id !== state.serverId) {
      // The server restarted: templates, styles and versions may all differ.
      reloadPage(String(d.server_id));
      return;
    }
    if (state.stylesVersion === null) {
      state.stylesVersion = Number(d.styles_version) || 0;
    } else if (Number(d.styles_version) > state.stylesVersion) {
      onStyles({ styles_version: d.styles_version }); // changed while we were disconnected
    }
    setStatus('connected', '');
    // The server replays missed state right after `hello`; resync if that did not happen.
    if (Number(d.version) > state.version) {
      setTimeout(() => {
        if (Number(d.version) > state.version) resync();
      }, 1000);
    }
  }

  function onRendering() {
    setStatus('rendering', 'Rendering…');
    clearTimeout(state.renderingTimer);
    state.renderingTimer = setTimeout(() => setStatus('connected', ''), RENDERING_TIMEOUT_MS);
  }

  function onContent(d) {
    const v = Number(d.version);
    if (!(v > state.contentVersion) || typeof d.html !== 'string') return;
    state.version = Math.max(state.version, v);
    // Apply at most once per frame; in a hidden tab this waits until it becomes visible.
    state.pending = d;
    if (!state.frame) {
      state.frame = requestAnimationFrame(() => {
        state.frame = 0;
        const msg = state.pending;
        state.pending = null;
        if (msg && Number(msg.version) > state.contentVersion) applyContent(msg);
      });
    }
  }

  function onError(d) {
    if (d.kind === 'bundle') {
      showBanner('bundle', 'error', 'Style bundle: ' + (d.message || 'cannot be loaded'));
      return;
    }
    const v = Number(d.version);
    if (v && v <= state.version) return; // already superseded
    if (v) state.version = v;
    state.errorVersion = state.version;
    stopRendering();
    showBanner('doc', 'error', (d.kind ? d.kind + ' error: ' : '') + (d.message || 'unknown error'));
  }

  function onDeleted(d) {
    const v = Number(d.version);
    if (v && v <= state.version) return;
    if (v) state.version = v;
    state.errorVersion = state.version;
    stopRendering();
    showBanner('doc', 'error', (d.path || state.doc) + ' was deleted — waiting for it to reappear');
  }

  function onAssets(d) {
    const container = root();
    if (!container || !Array.isArray(d.urls)) return;
    bustMedia(container, d.urls, String(d.v || Date.now()));
  }

  function onStyles(d) {
    const v = Number(d.styles_version) || 0;
    state.stylesVersion = Math.max(state.stylesVersion || 0, v);
    clearBanner('bundle');
    swapStyles(v);
  }

  // ---------------------------------------------------------------------------------------
  // Content updates

  function applyContent(msg) {
    const container = root();
    if (!container) return;
    const anchor = captureAnchor(container);
    let added;
    try {
      added = reconcile(container, msg.html);
    } catch (e) {
      console.warn('md-render: reconciliation failed, replacing the content', e);
      container.innerHTML = msg.html;
      added = [];
      for (let n = container.firstChild; n; n = n.nextSibling) {
        keys.set(n, keyOf(n));
        if (n.nodeType === Node.ELEMENT_NODE) added.push(n);
      }
    }
    state.contentVersion = Number(msg.version);
    state.version = Math.max(state.version, state.contentVersion);
    stopRendering();
    // Fresh content replaces older errors and the banners the server rendered into the page
    // (content that was waiting for a frame must not hide an error that came after it).
    if (state.contentVersion > state.errorVersion) clearBanner('doc');
    document.querySelectorAll('[data-mdr-banner]').forEach((b) => b.remove());
    const warnings = Array.isArray(msg.warnings) ? msg.warnings.filter(Boolean) : [];
    if (warnings.length) showBanner('warnings', 'warning', warnings.join('\n'));
    else clearBanner('warnings');
    document.dispatchEvent(new CustomEvent('mdr:content-updated', { detail: { root: container, added } }));
    // Flavor scripts may have changed heights synchronously; images and diagrams load later.
    restoreAnchor(container, anchor);
    requestAnimationFrame(() => restoreAnchor(container, anchor));
  }

  const SOURCEPOS_RE = /\sdata-sourcepos="[^"]*"/g;

  // Key of a top-level node: its HTML without `data-sourcepos` (which shifts whenever lines
  // are inserted above), with every `[data-mdr-key]` element reduced to its key so client
  // mutations inside it (rendered math, diagrams, copy buttons) do not matter.
  function keyOf(node) {
    if (node.nodeType === Node.TEXT_NODE) return 't:' + node.data;
    if (node.nodeType === Node.COMMENT_NODE) return 'c:' + node.data;
    if (node.nodeType !== Node.ELEMENT_NODE) return 'o:' + node.nodeType;
    const own = node.getAttribute('data-mdr-key');
    if (own) return 'k:' + node.localName + ':' + own;
    let el = node;
    if (node.querySelector('[data-mdr-key]')) {
      el = node.cloneNode(true);
      const keyed = el.querySelectorAll('[data-mdr-key]');
      for (let i = 0; i < keyed.length; i++) {
        const k = keyed[i];
        const stub = document.createElement('mdr-key');
        stub.setAttribute('k', k.localName + ':' + k.getAttribute('data-mdr-key'));
        if (k.parentNode) k.parentNode.replaceChild(stub, k);
      }
    }
    return 'e:' + el.outerHTML.replace(SOURCEPOS_RE, '');
  }

  // Replace the children of `container` by the nodes of `html`, keeping every old node
  // whose key matches a new one (first unused match, in order). Returns the inserted
  // elements.
  function reconcile(container, html) {
    const tpl = document.createElement('template');
    tpl.innerHTML = html;
    const incoming = Array.from(tpl.content.childNodes);
    const pool = new Map();
    for (let n = container.firstChild; n; n = n.nextSibling) {
      let k = keys.get(n);
      if (k === undefined) {
        k = keyOf(n);
        keys.set(n, k);
      }
      const list = pool.get(k);
      if (list) list.push(n);
      else pool.set(k, [n]);
    }
    const result = [];
    const added = [];
    incoming.forEach((n) => {
      const k = keyOf(n);
      const list = pool.get(k);
      if (list && list.length) {
        const old = list.shift();
        if (old.nodeType === Node.ELEMENT_NODE) syncSourcepos(old, n);
        result.push(old);
      } else {
        keys.set(n, k);
        result.push(n);
        if (n.nodeType === Node.ELEMENT_NODE) added.push(n);
      }
    });
    pool.forEach((list) => list.forEach((n) => container.removeChild(n)));
    // Every remaining child is in `result`; walk it with a cursor so nodes that are
    // already in place are not moved (moving reloads iframes and restarts media).
    let cursor = container.firstChild;
    result.forEach((n) => {
      if (n === cursor) cursor = cursor.nextSibling;
      else container.insertBefore(n, cursor);
    });
    return added;
  }

  // A kept block may have moved in the source: take over the new line numbers.
  function syncSourcepos(old, fresh) {
    copyAttr(old, fresh, 'data-sourcepos');
    const a = old.querySelectorAll('[data-sourcepos]');
    const b = fresh.querySelectorAll('[data-sourcepos]');
    if (a.length === b.length) {
      for (let i = 0; i < a.length; i++) copyAttr(a[i], b[i], 'data-sourcepos');
    }
  }

  function copyAttr(to, from, name) {
    const v = from.getAttribute(name);
    if (v === null) to.removeAttribute(name);
    else if (to.getAttribute(name) !== v) to.setAttribute(name, v);
  }

  // ---------------------------------------------------------------------------------------
  // Scroll anchoring

  function startLine(el) {
    const sp = el.getAttribute('data-sourcepos');
    const m = sp ? /^(\d+):/.exec(sp) : null;
    return m ? Number(m[1]) : null;
  }

  function maxScroll() {
    const se = document.scrollingElement || document.documentElement;
    return se.scrollHeight - window.innerHeight;
  }

  function captureAnchor(container) {
    const y = window.scrollY;
    if (y <= 0) return { top: true };
    const max = maxScroll();
    if (max > 0 && y >= max - BOTTOM_SLACK_PX) return { bottom: true };
    // The first top-level block reaching below the top of the viewport. Reconciliation
    // usually keeps that very node (or at least its predecessor), which is the most precise
    // anchor; its source line is the fallback (and what survives a page reload).
    const blocks = container.children;
    for (let i = 0; i < blocks.length; i++) {
      const r = blocks[i].getBoundingClientRect();
      if (r.bottom > 0) {
        const line = startLine(blocks[i]);
        if (line !== null) return { el: blocks[i], prev: blocks[i].previousElementSibling, line, offset: r.top };
        break;
      }
    }
    // No source positions (e.g. --api output): the last heading above the viewport top.
    const hs = container.querySelectorAll('h1, h2, h3, h4, h5, h6');
    let heading = null;
    for (let i = 0; i < hs.length; i++) {
      const top = hs[i].getBoundingClientRect().top;
      if (top > 0) break;
      heading = { text: hs[i].textContent, index: i, offset: top };
    }
    if (heading) return { heading };
    return { ratio: max > 0 ? y / max : 0 };
  }

  function restoreAnchor(container, a) {
    if (!a || a.top) return;
    if (a.bottom) {
      window.scrollTo(0, maxScroll());
      return;
    }
    let target = null;
    let offset = 0;
    if (a.el && a.el.parentNode === container) {
      target = a.el; // kept by the reconciliation
      offset = a.offset;
    } else if (a.prev && a.prev.parentNode === container) {
      target = a.prev.nextElementSibling || a.prev; // the anchor block itself was edited
      offset = a.offset;
    } else if (typeof a.line === 'number') {
      // The block starting at that line, else the nearest one before it.
      let best = -1;
      const blocks = container.children;
      for (let i = 0; i < blocks.length; i++) {
        const l = startLine(blocks[i]);
        if (l !== null && l <= a.line && l > best) {
          best = l;
          target = blocks[i];
        }
      }
      offset = a.offset;
    } else if (a.heading) {
      const hs = container.querySelectorAll('h1, h2, h3, h4, h5, h6');
      let bestDist = Infinity;
      for (let i = 0; i < hs.length; i++) {
        if (hs[i].textContent === a.heading.text && Math.abs(i - a.heading.index) < bestDist) {
          bestDist = Math.abs(i - a.heading.index);
          target = hs[i];
        }
      }
      offset = a.heading.offset;
    }
    if (target) {
      const dy = target.getBoundingClientRect().top - offset;
      if (Math.abs(dy) >= 1) window.scrollBy(0, dy);
    } else if (typeof a.ratio === 'number') {
      window.scrollTo(0, a.ratio * maxScroll());
    }
  }

  function restoreSavedAnchor(container) {
    let saved = null;
    try {
      const raw = sessionStorage.getItem(ANCHOR_KEY + state.doc);
      sessionStorage.removeItem(ANCHOR_KEY + state.doc);
      saved = raw ? JSON.parse(raw) : null;
    } catch (e) {
      saved = null; // storage disabled or a corrupt entry
    }
    if (!saved) return;
    const apply = () => restoreAnchor(container, saved);
    apply();
    // Again once images are laid out (and after the browser's own scroll restoration).
    window.addEventListener('load', () => {
      apply();
      requestAnimationFrame(apply);
    });
  }

  // Reload the page (server restart, renamed stylesheets), keeping the scroll position.
  // A reload that did not help within a few seconds is not repeated (no reload loops).
  function reloadPage(newServerId) {
    if (state.reloading) return;
    try {
      const last = Number(sessionStorage.getItem(RELOAD_KEY + state.doc)) || 0;
      if (Date.now() - last < 5000) {
        // Reloading again would loop: adopt the new server instead. Its versions restart
        // at 1, so forget ours and fetch its current state.
        if (newServerId) {
          state.serverId = newServerId;
          state.version = 0;
          state.contentVersion = 0;
          state.errorVersion = 0;
          state.lastResync = 0;
          resync();
        }
        return;
      }
      sessionStorage.setItem(RELOAD_KEY + state.doc, String(Date.now()));
      const container = root();
      if (container) {
        // Elements do not survive the reload: keep the source line (or the fallbacks).
        const a = captureAnchor(container);
        delete a.el;
        delete a.prev;
        sessionStorage.setItem(ANCHOR_KEY + state.doc, JSON.stringify(a));
      }
    } catch (e) {
      // Storage unavailable: reload without remembering the position.
    }
    state.reloading = true;
    closeSource();
    location.reload();
  }

  // Ask the server for the current state (bfcache restore, tab shown again, missed replay).
  function resync() {
    const now = Date.now();
    if (!live.render_url || typeof window.fetch !== 'function' || now - state.lastResync < 1000) return;
    state.lastResync = now;
    fetch(live.render_url, { cache: 'no-store', credentials: 'same-origin' })
      .then((r) => r.json()) // error statuses (404 deleted, 500 failed) carry JSON too
      .then((d) => {
        if (!d || typeof d !== 'object') return;
        if (d.server_id && state.serverId && d.server_id !== state.serverId) {
          reloadPage(String(d.server_id));
          return;
        }
        const v = Number(d.version);
        if (!(v > state.version)) return;
        if (typeof d.html === 'string') applyContent({ version: v, html: d.html, warnings: d.warnings });
        state.version = v;
        if (d.deleted) onDeleted({ path: d.path });
        else if (d.error && typeof d.error === 'object') onError({ kind: d.error.kind, message: d.error.message });
      })
      .catch(() => {
        // Server unreachable: the EventSource reconnect will catch up.
      });
  }

  // ---------------------------------------------------------------------------------------
  // Styles and assets

  function sameOrigin(value) {
    if (!value) return null;
    try {
      const u = new URL(value, document.baseURI);
      return u.origin === location.origin ? u : null;
    } catch (e) {
      return null;
    }
  }

  function decodePath(p) {
    try {
      return decodeURIComponent(p);
    } catch (e) {
      return p;
    }
  }

  // Swap each bundle / --css stylesheet for one with `?v=<styles_version>`; the old one is
  // removed once the new one loaded (no flash of unstyled content). A stylesheet that no
  // longer exists (re-vendored, hashed names changed) means the page must be rebuilt.
  function swapStyles(v) {
    document.querySelectorAll('link[rel~="stylesheet"]').forEach((link) => {
      if (link.getAttribute('data-mdr-replaced') !== null) return;
      const u = sameOrigin(link.getAttribute('href'));
      if (!u) return;
      if (u.pathname.indexOf(PREFIX + '/bundle/') !== 0 && u.pathname.indexOf(PREFIX + '/css/') !== 0) return;
      u.searchParams.set('v', String(v));
      const fresh = link.cloneNode(false);
      fresh.setAttribute('href', u.pathname + u.search + u.hash);
      link.setAttribute('data-mdr-replaced', '');
      fresh.addEventListener('load', () => link.remove());
      fresh.addEventListener('error', () => {
        fresh.remove();
        link.removeAttribute('data-mdr-replaced');
        reloadPage(null);
      });
      link.parentNode.insertBefore(fresh, link.nextSibling);
    });
  }

  // Cache-bust local media whose files changed (`assets` event).
  function bustMedia(container, urls, token) {
    const wanted = new Set(urls.map((u) => decodePath(String(u))));
    const bust = (value) => {
      const u = sameOrigin(value);
      if (!u || !wanted.has(decodePath(u.pathname))) return value;
      u.searchParams.set('mdrv', token);
      return u.pathname + u.search + u.hash;
    };
    const reload = new Set();
    container.querySelectorAll('img, source, video, audio, track, input[type="image"]').forEach((el) => {
      let changed = false;
      ['src', 'poster'].forEach((attr) => {
        const old = el.getAttribute(attr);
        if (old === null) return;
        const nv = bust(old);
        if (nv !== old) {
          el.setAttribute(attr, nv);
          changed = true;
        }
      });
      const set = el.getAttribute('srcset');
      if (set) {
        const nv = set.split(',').map((c) => {
          const parts = c.trim().split(/\s+/);
          parts[0] = bust(parts[0]);
          return parts.join(' ');
        }).join(', ');
        if (nv !== set) {
          el.setAttribute('srcset', nv);
          changed = true;
        }
      }
      // A changed <source>/<track> only takes effect after the media element reloads.
      const media = el.parentElement;
      if (changed && el.localName !== 'img' && media && typeof media.load === 'function') reload.add(media);
    });
    reload.forEach((m) => m.load());
  }

  // ---------------------------------------------------------------------------------------
  // Banners, title and status

  function injectStyle() {
    if (document.getElementById('mdr-live-style')) return;
    const style = document.createElement('style');
    style.id = 'mdr-live-style';
    style.textContent = [
      // Sticky, so it is seen wherever the reader is; opaque system colours (a matching
      // pair from the page's color-scheme) under a tint, so it reads in light and dark.
      '.mdr-live-banner{position:sticky;top:8px;z-index:100;display:flex;align-items:flex-start;gap:8px;',
      'margin:0 0 16px;padding:8px 12px;border:1px solid;border-radius:6px;color:CanvasText;',
      'font:14px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;',
      'white-space:pre-wrap;overflow-wrap:anywhere;box-shadow:0 1px 3px rgba(0,0,0,.12)}',
      '.mdr-live-banner-error{background:linear-gradient(rgba(248,81,73,.15),rgba(248,81,73,.15)),Canvas;',
      'border-color:rgba(248,81,73,.5)}',
      '.mdr-live-banner-warning{background:linear-gradient(rgba(210,153,34,.18),rgba(210,153,34,.18)),Canvas;',
      'border-color:rgba(210,153,34,.5)}',
      '.mdr-live-banner-text{flex:1}',
      '.mdr-live-banner-close{flex:none;border:0;background:none;color:inherit;font-size:18px;',
      'line-height:1;cursor:pointer;padding:0 2px;opacity:.7}',
      '.mdr-live-banner-close:hover,.mdr-live-banner-close:focus{opacity:1}',
      '#mdr-live-status{position:fixed;right:10px;bottom:10px;width:8px;height:8px;border-radius:50%;',
      'pointer-events:none;opacity:0;transition:opacity .3s;z-index:2147483647}',
      '#mdr-live-status[data-state=rendering]{opacity:1;background:#d29922}',
      '#mdr-live-status[data-state=disconnected],#mdr-live-status[data-state=stopped]{opacity:.8;background:#8c959f}',
    ].join('');
    (document.head || document.documentElement).appendChild(style);
  }

  function showBanner(slot, kind, message) {
    const container = root();
    if (!container || !container.parentNode) return;
    let b = banners.get(slot);
    if (!b) {
      b = document.createElement('div');
      b.className = 'mdr-live-banner mdr-live-banner-' + kind;
      b.setAttribute('role', 'alert');
      const text = document.createElement('span');
      text.className = 'mdr-live-banner-text';
      const close = document.createElement('button');
      close.type = 'button';
      close.className = 'mdr-live-banner-close';
      close.setAttribute('aria-label', 'Dismiss');
      close.textContent = '×';
      close.addEventListener('click', () => clearBanner(slot));
      b.appendChild(text);
      b.appendChild(close);
      keepScroll(container, () => container.parentNode.insertBefore(b, container));
      banners.set(slot, b);
    }
    b.firstChild.textContent = message;
    if (kind === 'error') activeErrors.add(slot);
    else activeErrors.delete(slot);
    updateTitle();
  }

  // Remove a banner (also when the user dismisses it: the error is acknowledged).
  function clearBanner(slot) {
    const b = banners.get(slot);
    if (b) {
      const container = root();
      if (container) keepScroll(container, () => b.remove());
      else b.remove();
      banners.delete(slot);
    }
    activeErrors.delete(slot);
    updateTitle();
  }

  // Run a layout-changing DOM operation above the content without moving what is read.
  function keepScroll(container, change) {
    const anchor = captureAnchor(container);
    change();
    restoreAnchor(container, anchor);
  }

  function updateTitle() {
    const title = (activeErrors.size ? '(Error) ' : '') + state.baseTitle;
    if (document.title !== title) document.title = title;
  }

  function stopRendering() {
    clearTimeout(state.renderingTimer);
    setStatus('connected', '');
  }

  // A small dot in the corner: amber while rendering, grey while disconnected.
  function setStatus(name, label) {
    if (!statusDot) {
      if (!document.body) return;
      statusDot = document.createElement('div');
      statusDot.id = 'mdr-live-status';
      statusDot.setAttribute('aria-hidden', 'true');
      document.body.appendChild(statusDot);
    }
    statusDot.setAttribute('data-state', name);
    statusDot.title = label;
  }

  // Start last: every `const` above must be initialised before `init` runs.
  if (root()) {
    init();
  } else {
    document.addEventListener('DOMContentLoaded', init);
  }
})();
