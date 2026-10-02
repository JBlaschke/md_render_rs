// Helpers shared by md-render's flavor scripts (owner: page). Contract: docs/DESIGN.md §7.
//
// Loaded right before github.js / gitlab.js, which keep only their site's behaviours:
//   * the page configuration from <script id="mdr-config" type="application/json">,
//   * collecting elements among the nodes a run processes, and claiming them so that nothing
//     is rendered twice (live updates keep rendered nodes, DESIGN §7),
//   * loading client libraries lazily: by URL, or from an export's inert
//     <script type="text/plain" id="mdr-lib-NAME"> holder,
//   * writing to the clipboard (with the legacy fallback for non-secure origins),
//   * hash navigation: links without the `user-content-` prefix reach prefixed ids (GH §3.4,
//     GL §10.5), on load, on hashchange, on clicks and into live-updated content.
(function () {
  'use strict';

  if (window.mdRenderCommon) return;

  var DONE = 'data-mdr-rendered';

  function readConfig() {
    var el = document.getElementById('mdr-config');
    if (!el) return {};
    try {
      return JSON.parse(el.textContent || '{}') || {};
    } catch (e) {
      console.warn('md-render: invalid mdr-config', e);
      return {};
    }
  }

  var CONFIG = readConfig();

  // Elements matching `selector` among `nodes` and their descendants (each element once).
  function collect(nodes, selector) {
    var out = [];
    var seen = new Set();
    (nodes || []).forEach(function (node) {
      if (!node || node.nodeType !== 1) return;
      if (node.matches(selector) && !seen.has(node)) {
        seen.add(node);
        out.push(node);
      }
      node.querySelectorAll(selector).forEach(function (el) {
        if (!seen.has(el)) {
          seen.add(el);
          out.push(el);
        }
      });
    });
    return out;
  }

  // Mark `el` as processed; false if it already was.
  function claim(el) {
    if (el.hasAttribute(DONE)) return false;
    el.setAttribute(DONE, '');
    return true;
  }

  // The configured reference of library `name` (URL, or `#mdr-lib-NAME` in exports), or null.
  function libRef(name) {
    var scripts = CONFIG.scripts || {};
    return typeof scripts[name] === 'string' && scripts[name] ? scripts[name] : null;
  }

  // Execute an inlined library (export: <script type="text/plain" id="mdr-lib-NAME">).
  function runInline(ref) {
    var holder = document.getElementById(ref.slice(1));
    if (!holder) throw new Error('md-render: missing inline library ' + ref);
    var script = document.createElement('script');
    script.textContent = holder.textContent;
    document.head.appendChild(script); // inline scripts run synchronously on insertion
  }

  var libraries = new Map();

  // Load library `name` once; the promise rejects when it is not vendored or fails to load.
  function loadLibrary(name) {
    if (libraries.has(name)) return libraries.get(name);
    var ref = libRef(name);
    var promise;
    if (!ref) {
      var flavor = CONFIG.flavor === 'gitlab' ? 'gitlab' : 'github';
      promise = Promise.reject(new Error(name + ' is not vendored (run `md-render vendor --flavor ' + flavor + '`)'));
    } else if (ref.charAt(0) === '#') {
      promise = new Promise(function (resolve) {
        runInline(ref);
        resolve();
      });
    } else {
      promise = new Promise(function (resolve, reject) {
        var script = document.createElement('script');
        script.src = ref;
        script.async = true;
        script.onload = function () { resolve(); };
        script.onerror = function () { reject(new Error('failed to load ' + ref)); };
        document.head.appendChild(script);
      });
    }
    libraries.set(name, promise);
    return promise;
  }

  function writeClipboard(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    // Non-secure origins (e.g. served on a LAN address): legacy copy through a textarea.
    return new Promise(function (resolve, reject) {
      var area = document.createElement('textarea');
      area.value = text;
      area.setAttribute('readonly', '');
      area.style.position = 'fixed';
      area.style.opacity = '0';
      document.body.appendChild(area);
      area.select();
      var ok = false;
      try {
        ok = document.execCommand('copy');
      } catch (e) {
        ok = false;
      }
      area.remove();
      if (ok) resolve(); else reject(new Error('copy failed'));
    });
  }

  function decodeHash(hash) {
    var raw = (hash || '').replace(/^#/, '');
    try {
      return decodeURIComponent(raw);
    } catch (e) {
      return raw;
    }
  }

  // Hash navigation with the flavor's `target(id)` rule (the element a fragment designates,
  // or null). Returns { scrollToHash(), follow(nodes) }: `follow` scrolls to the hash target
  // when a live update replaced it, unless the reader has moved on.
  function hashNavigation(target) {
    var userScrolled = false;
    ['wheel', 'touchmove', 'keydown', 'mousedown'].forEach(function (type) {
      window.addEventListener(type, function () { userScrolled = true; }, { passive: true, once: true });
    });

    function scrollToHash() {
      if (!location.hash) return;
      var targeted = null;
      try {
        targeted = document.querySelector(':target');
      } catch (e) {
        targeted = null;
      }
      if (targeted) return; // the browser already scrolled to it
      var el = target(decodeHash(location.hash));
      if (el) el.scrollIntoView(true);
    }

    window.addEventListener('hashchange', scrollToHash);
    window.addEventListener('load', scrollToHash);
    // Re-clicking the current hash fires no hashchange; scroll explicitly to prefixed targets.
    document.addEventListener('click', function (event) {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      var link = event.target && event.target.closest && event.target.closest('a[href^="#"]');
      if (!link) return;
      var id = decodeHash(link.getAttribute('href'));
      if (!id || document.getElementById(id)) return;
      var el = target(id);
      if (!el) return;
      event.preventDefault();
      if (location.hash !== link.getAttribute('href')) history.pushState(null, '', link.getAttribute('href'));
      el.scrollIntoView(true);
    });

    return {
      scrollToHash: scrollToHash,
      follow: function (nodes) {
        if (!location.hash || userScrolled) return;
        var el = target(decodeHash(location.hash));
        if (el && nodes.some(function (n) { return n && n.nodeType === 1 && (n === el || n.contains(el)); })) {
          el.scrollIntoView(true);
        }
      }
    };
  }

  // The nodes an `mdr:content-updated` event asks to process ({detail: {root, added}}).
  function updatedNodes(event) {
    var detail = (event && event.detail) || {};
    return Array.isArray(detail.added) ? detail.added : [detail.root || document.getElementById('md-content')];
  }

  window.mdRenderCommon = {
    DONE: DONE,
    config: CONFIG,
    collect: collect,
    claim: claim,
    libRef: libRef,
    runInline: runInline,
    loadLibrary: loadLibrary,
    writeClipboard: writeClipboard,
    decodeHash: decodeHash,
    hashNavigation: hashNavigation,
    updatedNodes: updatedNodes
  };
})();
