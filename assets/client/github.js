// GitHub page behaviours for md-render (owner: page). Contract: docs/DESIGN.md §7.
//
// On DOMContentLoaded and on every `mdr:content-updated` event ({detail: {root, added}}) this
// processes only the given nodes:
//   * code-block copy buttons (GH §5.3: clone the unpositioned clipboard template),
//   * <math-renderer> elements via MathJax 3.2.0 tex2mml (GH §8.6, CQ §1.4),
//   * mermaid enrichment sections rendered inline (GH §8.7),
//   * hash navigation with the `user-content-` prefix fallback (GH §3.4, GRIP C52).
// Libraries are loaded lazily from the URLs in <script id="mdr-config">; an element that was
// already rendered is never rendered again (live updates keep such nodes, DESIGN §7).
(function () {
  'use strict';

  if (window.mdrGithub) return;

  var CONFIG = readConfig();
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

  // ------------------------------------------------------------------------------------------
  // Utilities

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

  function claim(el) {
    if (el.hasAttribute(DONE)) return false;
    el.setAttribute(DONE, '');
    return true;
  }

  function libRef(name) {
    var scripts = CONFIG.scripts || {};
    return typeof scripts[name] === 'string' && scripts[name] ? scripts[name] : null;
  }

  function absoluteUrl(url) {
    try {
      return new URL(url, document.baseURI).href;
    } catch (e) {
      return url;
    }
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

  function loadLibrary(name) {
    if (libraries.has(name)) return libraries.get(name);
    var ref = libRef(name);
    var promise;
    if (!ref) {
      promise = Promise.reject(new Error(name + ' is not vendored (run `md-render vendor --flavor github`)'));
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

  // ------------------------------------------------------------------------------------------
  // Clipboard buttons (GH §5.3): the server only emits data-snippet-clipboard-copy-content;
  // github.com's JS moves the text onto a <clipboard-copy> cloned from the unpositioned
  // template. We do the same and implement the copy ourselves.

  var CLIPBOARD_FALLBACK =
    '<div class="zeroclipboard-container"><clipboard-copy aria-label="Copy code to clipboard" ' +
    'class="ClipboardButton btn btn-invisible js-clipboard-copy m-2 p-0 d-flex flex-justify-center flex-items-center" ' +
    'data-copy-feedback="Copied!" data-tooltip-direction="w">' +
    '<svg aria-hidden="true" height="16" viewBox="0 0 16 16" version="1.1" width="16" class="octicon octicon-copy js-clipboard-copy-icon">' +
    '<path d="M0 6.75C0 5.784.784 5 1.75 5h1.5a.75.75 0 0 1 0 1.5h-1.5a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-1.5a.75.75 0 0 1 1.5 0v1.5A1.75 1.75 0 0 1 9.25 16h-7.5A1.75 1.75 0 0 1 0 14.25Z"></path>' +
    '<path d="M5 1.75C5 .784 5.784 0 6.75 0h7.5C15.216 0 16 .784 16 1.75v7.5A1.75 1.75 0 0 1 14.25 11h-7.5A1.75 1.75 0 0 1 5 9.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z"></path></svg>' +
    '<svg aria-hidden="true" height="16" viewBox="0 0 16 16" version="1.1" width="16" class="octicon octicon-check js-clipboard-check-icon color-fg-success d-none">' +
    '<path d="M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.751.751 0 0 1 .018-1.042.751.751 0 0 1 1.042-.018L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0Z"></path></svg>' +
    '</clipboard-copy></div>';

  function clipboardButton() {
    var template = document.getElementById('snippet-clipboard-copy-button-unpositioned');
    if (template && template.content && template.content.firstElementChild) {
      return template.content.firstElementChild.cloneNode(true);
    }
    var holder = document.createElement('div');
    holder.innerHTML = CLIPBOARD_FALLBACK;
    return holder.firstElementChild;
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

  function onCopy(button) {
    var value = button.getAttribute('value') || '';
    writeClipboard(value).then(function () {
      var copyIcon = button.querySelector('.js-clipboard-copy-icon');
      var checkIcon = button.querySelector('.js-clipboard-check-icon');
      var label = button.getAttribute('aria-label');
      if (copyIcon) copyIcon.classList.add('d-none');
      if (checkIcon) checkIcon.classList.remove('d-none');
      button.setAttribute('aria-label', button.getAttribute('data-copy-feedback') || 'Copied!');
      clearTimeout(button.mdrTimer);
      button.mdrTimer = setTimeout(function () {
        if (copyIcon) copyIcon.classList.remove('d-none');
        if (checkIcon) checkIcon.classList.add('d-none');
        if (label) button.setAttribute('aria-label', label);
      }, 2000);
    }, function (err) {
      console.warn('md-render: could not copy to the clipboard', err);
    });
  }

  function addClipboardButtons(nodes) {
    collect(nodes, '[data-snippet-clipboard-copy-content]').forEach(function (wrapper) {
      if (wrapper.querySelector(':scope > .zeroclipboard-container')) return;
      var value = wrapper.getAttribute('data-snippet-clipboard-copy-content');
      wrapper.removeAttribute('data-snippet-clipboard-copy-content');
      var container = clipboardButton();
      if (!container) return;
      var button = container.querySelector('clipboard-copy') || container;
      button.setAttribute('value', value);
      button.setAttribute('tabindex', '0');
      button.setAttribute('role', 'button');
      wrapper.appendChild(container);
    });
  }

  document.addEventListener('click', function (event) {
    var button = event.target && event.target.closest && event.target.closest('clipboard-copy');
    if (button) onCopy(button);
  });
  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    var button = event.target && event.target.closest && event.target.closest('clipboard-copy');
    if (!button) return;
    event.preventDefault();
    onCopy(button);
  });

  // ------------------------------------------------------------------------------------------
  // Math (GH §8.6 + CQ §1.4): MathJax 3.2.0 `es5/tex-chtml-full.js` with `ui/safe`, GitHub's
  // configuration, synchronous tex2mml, native MathML inserted into <math-renderer>.

  var UNSUPPORTED_MACROS = ['DeclareMathOperator', 'DeclarePairedDelimiters', 'renewtagform', 'newtagform',
    'colorbox', 'fcolorbox', 'hphantom', 'vphantom', 'phantom', 'operatorname', 'Newextarrow', 'definecolor',
    'mathchoice', 'unicode', 'mmlToken'];
  var GENERIC_MATH_ERROR = 'Unable to render expression.';
  var MAX_BRACES = 1000;
  // Largest allowed explicit spacing per unit (DoS guard for huge \hspace etc.).
  var MAX_LENGTH = { em: 2, pt: 20, pc: 0.75, ex: 2.5 };
  var SPACING_RE = /\\(hskip|hspace|raise|mspace|mskip|kern|lower|above|mkern|moveleft|moveright|rule|Rule|space|Space)((?:\{\s*-?\d+(?:\.\d*)?\s*(?:em|pt|pc|ex)\s*\})+)/g;

  function clampLengths(tex) {
    return tex.replace(SPACING_RE, function (all, command, args) {
      var clamped = args.replace(/\{\s*(-?\d+(?:\.\d*)?)\s*(em|pt|pc|ex)\s*\}/g, function (arg, num, unit) {
        var value = Number(num);
        var max = MAX_LENGTH[unit];
        if (!isFinite(value) || Math.abs(value) > max) value = value < 0 ? -max : max;
        return '{' + value + unit + '}';
      });
      return '\\' + command + clamped;
    });
  }

  function mathJaxConfig() {
    var safeRef = libRef('mathjax-safe');
    var loader = { load: safeRef ? ['ui/safe'] : [] };
    if (safeRef && safeRef.charAt(0) === '#') {
      // Export: run the inlined component from MathJax's custom loader hook.
      loader.require = function (url) {
        if (/ui\/safe(\.js)?$/.test(url)) {
          runInline(safeRef);
          return undefined;
        }
        return Promise.reject(new Error('MathJax component ' + url + ' is not bundled'));
      };
    } else if (safeRef) {
      loader.source = { 'ui/safe': absoluteUrl(safeRef) };
    }
    return {
      loader: loader,
      tex: {
        inlineMath: [['$', '$']],
        displayMath: [['$$', '$$']],
        packages: { '[-]': ['noerrors', 'bbox', 'html', 'require', 'newcommand', 'action', 'colortbl'] },
        maxMacros: 1000
      },
      startup: {
        typeset: false,
        ready: function () {
          var mj = window.MathJax;
          mj.startup.defaultReady();
          var doc = mj.startup.document;
          ((doc && doc.inputJax) || []).forEach(function (jax) {
            jax.preFilters.add(function (data) {
              var item = data.math;
              UNSUPPORTED_MACROS.forEach(function (name) {
                if (item.math.indexOf('\\' + name) !== -1) item.math = '\\text{' + name + ' macro is not supported}';
              });
            }, -1000);
          });
          var safe = doc && doc.safe;
          if (safe && safe.filterAttributes && safe.filterMethods) {
            ['fontfamily', 'fontstyle', 'fontweight'].forEach(function (a) { safe.filterAttributes.set(a, 'filterSemiColon'); });
            safe.filterMethods.filterSemiColon = function (s, value) { return String(value).split(/;/)[0]; };
          }
        }
      },
      options: {
        enableMenu: false,
        safeOptions: {
          allow: { URLs: 'none', classes: 'none', cssIDs: 'none', styles: 'safe' },
          lengthMax: 2,
          safeStyles: {},
          styleLengths: {}
        }
      }
    };
  }

  var mathJaxReady = null;

  function loadMathJax() {
    if (mathJaxReady) return mathJaxReady;
    if (window.MathJax && window.MathJax.startup && window.MathJax.tex2mml) {
      mathJaxReady = Promise.resolve(window.MathJax.startup.promise);
    } else {
      window.MathJax = mathJaxConfig();
      mathJaxReady = loadLibrary('mathjax').then(function () {
        if (!window.MathJax || !window.MathJax.startup) throw new Error('MathJax did not initialize');
        return window.MathJax.startup.promise;
      });
    }
    return mathJaxReady;
  }

  var MATHML_TAGS = new Set(['math', 'maction', 'annotation', 'menclose', 'merror', 'mfenced', 'mfrac', 'mglyph',
    'mi', 'mlabeledtr', 'mmultiscripts', 'mn', 'mo', 'mover', 'mpadded', 'mphantom', 'mprescripts', 'mroot',
    'mrow', 'ms', 'mspace', 'msqrt', 'mstyle', 'msub', 'msubsup', 'msup', 'mtable', 'mtd', 'mtext', 'mtr',
    'munder', 'munderover', 'none', 'semantics']);
  var DROPPED_ATTRS = /^(on|xlink:|xmlns:)|^(href|src|id|action|formaction|srcdoc)$/i;

  // MathJax's ui/safe already filters URLs, ids, classes and styles; this is a second, strict
  // pass that keeps only MathML elements and inert attributes.
  function sanitizeMathML(markup) {
    var doc = new DOMParser().parseFromString(markup, 'text/html');
    var math = doc.body.querySelector('math');
    if (!math) return null;
    (function clean(el) {
      Array.prototype.slice.call(el.children).forEach(function (child) {
        if (!MATHML_TAGS.has(child.localName)) {
          child.remove();
          return;
        }
        clean(child);
      });
      Array.prototype.slice.call(el.attributes).forEach(function (attr) {
        var name = attr.name;
        if (DROPPED_ATTRS.test(name)) {
          el.removeAttribute(name);
        } else if (name === 'style' && /url\s*\(|expression\s*\(|javascript:/i.test(attr.value)) {
          el.removeAttribute(name);
        } else if (name === 'class') {
          var kept = attr.value.split(/\s+/).filter(function (c) { return /^(MJX|mjx|TEX)-/.test(c) || c === 'MathJax'; });
          if (kept.length) el.setAttribute('class', kept.join(' ')); else el.removeAttribute('class');
        }
      });
    })(math);
    return document.importNode(math, true);
  }

  // MathML Core ignores mtable@columnalign: copy it onto the cells.
  function applyColumnAlign(root) {
    root.querySelectorAll('mtable[columnalign]').forEach(function (table) {
      var aligns = (table.getAttribute('columnalign') || '').trim().split(/\s+/);
      if (aligns.length < 2) return;
      table.querySelectorAll(':scope > mtr, :scope > mlabeledtr').forEach(function (row) {
        Array.prototype.forEach.call(row.querySelectorAll(':scope > mtd'), function (cell, i) {
          var align = aligns[Math.min(i, aligns.length - 1)];
          if (['left', 'center', 'right'].indexOf(align) !== -1 && cell.style) cell.style.textAlign = align;
        });
      });
    });
  }

  function showMathError(el, message, raw) {
    var flash = document.createElement('div');
    flash.className = 'flash flash-error';
    flash.textContent = message;
    var nodes = [flash];
    if (raw) {
      var pre = document.createElement('pre');
      pre.textContent = raw;
      nodes.push(document.createElement('br'), pre);
    }
    el.replaceChildren.apply(el, nodes);
  }

  function renderMathElement(el) {
    var raw = el.textContent || '';
    if (raw.split('{').length - 1 > MAX_BRACES) {
      showMathError(el, GENERIC_MATH_ERROR, raw);
      return;
    }
    var banned = UNSUPPORTED_MACROS.filter(function (m) { return raw.indexOf('\\' + m) !== -1; });
    if (banned.length) {
      showMathError(el, 'The following macros are not allowed: ' + banned.join(', '), raw);
      return;
    }
    var tex = clampLengths(raw.trim().replace(/^\${1,2}|\${1,2}$/g, ''));
    var math;
    try {
      math = sanitizeMathML(window.MathJax.tex2mml(tex, { display: !el.classList.contains('js-inline-math') }));
    } catch (e) {
      showMathError(el, GENERIC_MATH_ERROR, raw);
      return;
    }
    if (!math) {
      showMathError(el, GENERIC_MATH_ERROR, raw);
      return;
    }
    var merror = math.querySelector('merror');
    if (merror) {
      var message = merror.textContent || GENERIC_MATH_ERROR;
      showMathError(el, /Misplaced &/.test(message) ? GENERIC_MATH_ERROR : message, raw);
      return;
    }
    el.replaceChildren(math);
    applyColumnAlign(el);
  }

  function renderMath(nodes) {
    var elements = collect(nodes, 'math-renderer').filter(claim);
    if (!elements.length) return;
    loadMathJax().then(function () {
      elements.forEach(function (el) {
        if (el.isConnected) renderMathElement(el);
      });
    }, function (err) {
      console.warn('md-render: math is shown as TeX source:', err.message || err);
    });
  }

  // ------------------------------------------------------------------------------------------
  // Mermaid (GH §8.7): github.com renders in a viewscreen iframe with mermaid 11.17.2; we render
  // inline with the same options (theme by color mode) and keep the source for re-rendering
  // when the color mode changes.

  var diagrams = new Set();
  var mermaidSeq = 0;
  var mermaidQueue = Promise.resolve();

  function isDark() {
    var root = document.documentElement;
    var mode = root.getAttribute('data-color-mode') || 'auto';
    var prefersDark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
    var slot = mode === 'dark' || (mode === 'auto' && prefersDark) ? 'dark' : 'light';
    var id = root.getAttribute(slot === 'dark' ? 'data-dark-theme' : 'data-light-theme') || slot;
    return /^dark/.test(id);
  }

  function enrichmentSource(section) {
    var target = section.querySelector('.js-render-enrichment-target');
    if (target) {
      try {
        var data = JSON.parse(target.getAttribute('data-json') || 'null');
        if (data && typeof data.data === 'string') return data.data;
      } catch (e) { /* fall through */ }
      if (target.hasAttribute('data-plain')) return target.getAttribute('data-plain');
    }
    var pre = section.querySelector('pre');
    return pre ? pre.textContent : '';
  }

  // Show the raw source instead of a spinner (no renderer, or rendering failed).
  function showPlain(section, message) {
    section.classList.remove('render-needs-enrichment', 'js-render-needs-enrichment');
    var loader = section.querySelector('.js-render-enrichment-loader');
    if (loader) loader.remove();
    var old = section.querySelector(':scope > .mdr-mermaid');
    if (old) old.remove();
    if (message) {
      var flash = section.querySelector(':scope > .mdr-enrichment-error');
      if (!flash) {
        flash = document.createElement('div');
        flash.className = 'flash flash-error mb-2 mdr-enrichment-error';
        section.insertBefore(flash, section.firstChild);
      }
      flash.textContent = message;
    }
  }

  function drawDiagram(section) {
    mermaidQueue = mermaidQueue.then(function () {
      if (!section.isConnected) {
        diagrams.delete(section);
        return undefined;
      }
      var mermaid = window.mermaid;
      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: isDark() ? 'dark' : 'default',
        flowchart: { diagramPadding: 48 },
        gantt: { useWidth: 1200 },
        pie: { useWidth: 1200 },
        sequence: { diagramMarginY: 40 }
      });
      var id = 'mdr-mermaid-' + (++mermaidSeq);
      return Promise.resolve(mermaid.render(id, section.mdrSource)).then(function (result) {
        var container = section.querySelector(':scope > .mdr-mermaid');
        if (!container) {
          container = document.createElement('div');
          container.className = 'mdr-mermaid';
        }
        container.innerHTML = result.svg;
        if (typeof result.bindFunctions === 'function') result.bindFunctions(container);
        var loader = section.querySelector('.js-render-enrichment-loader');
        if (loader) loader.replaceWith(container);
        else if (!container.parentNode) section.appendChild(container);
      }, function (err) {
        var leftover = document.getElementById('d' + id);
        if (leftover) leftover.remove();
        diagrams.delete(section);
        showPlain(section, 'Unable to render rich display: ' + ((err && err.message) || err));
      });
    });
    return mermaidQueue;
  }

  function renderEnrichments(nodes) {
    var sections = collect(nodes, 'section.js-render-needs-enrichment, section[data-type="mermaid"]').filter(claim);
    if (!sections.length) return;
    var mermaidSections = [];
    sections.forEach(function (section) {
      if (section.getAttribute('data-type') !== 'mermaid') {
        showPlain(section); // geojson / topojson / stl: no offline renderer
        return;
      }
      section.mdrSource = enrichmentSource(section);
      mermaidSections.push(section);
    });
    if (!mermaidSections.length) return;
    loadLibrary('mermaid').then(function () {
      if (!window.mermaid) throw new Error('mermaid did not initialize');
      mermaidSections.forEach(function (section) {
        diagrams.add(section);
        drawDiagram(section);
      });
    }).catch(function (err) {
      mermaidSections.forEach(function (section) { showPlain(section); });
      console.warn('md-render: mermaid diagrams are shown as source:', err.message || err);
    });
  }

  function redrawDiagrams() {
    if (!window.mermaid) return;
    diagrams.forEach(function (section) { drawDiagram(section); });
  }

  if (window.matchMedia) {
    var media = window.matchMedia('(prefers-color-scheme: dark)');
    var onScheme = function () {
      if ((document.documentElement.getAttribute('data-color-mode') || 'auto') === 'auto') redrawDiagrams();
    };
    if (media.addEventListener) media.addEventListener('change', onScheme);
    else if (media.addListener) media.addListener(onScheme);
  }
  new MutationObserver(redrawDiagrams).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-color-mode', 'data-light-theme', 'data-dark-theme']
  });

  // ------------------------------------------------------------------------------------------
  // Hash navigation (GH §3.4): ids are prefixed `user-content-`, links are not.

  var userScrolled = false;
  ['wheel', 'touchmove', 'keydown', 'mousedown'].forEach(function (type) {
    window.addEventListener(type, function () { userScrolled = true; }, { passive: true, once: true });
  });

  function decodeHash(hash) {
    var raw = (hash || '').replace(/^#/, '');
    try {
      return decodeURIComponent(raw);
    } catch (e) {
      return raw;
    }
  }

  function hashTarget(id) {
    if (!id) return null;
    return document.getElementById(id) ||
      document.getElementById('user-content-' + id) ||
      document.getElementsByName('user-content-' + id)[0] ||
      null;
  }

  function scrollToHash() {
    if (!location.hash) return;
    var targeted = null;
    try {
      targeted = document.querySelector(':target');
    } catch (e) {
      targeted = null;
    }
    if (targeted) return;
    var el = hashTarget(decodeHash(location.hash));
    if (el) el.scrollIntoView();
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
    var el = hashTarget(id);
    if (!el) return;
    event.preventDefault();
    if (location.hash !== link.getAttribute('href')) history.pushState(null, '', link.getAttribute('href'));
    el.scrollIntoView();
  });

  // ------------------------------------------------------------------------------------------
  // Entry points

  function process(nodes) {
    addClipboardButtons(nodes);
    renderMath(nodes);
    renderEnrichments(nodes);
  }

  function init() {
    var root = document.getElementById('md-content') || document.body;
    process([root]);
    scrollToHash();
  }

  document.addEventListener('mdr:content-updated', function (event) {
    var detail = (event && event.detail) || {};
    var nodes = Array.isArray(detail.added) ? detail.added : [detail.root || document.getElementById('md-content')];
    process(nodes);
    // Follow the hash into replaced content unless the reader has moved on.
    if (location.hash && !userScrolled) {
      var target = hashTarget(decodeHash(location.hash));
      if (target && nodes.some(function (n) { return n && n.nodeType === 1 && (n === target || n.contains(target)); })) {
        target.scrollIntoView();
      }
    }
  });

  window.mdrGithub = { process: process, config: CONFIG };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
