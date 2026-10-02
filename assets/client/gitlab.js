// GitLab page behaviours for md-render (owner: page). Contract: docs/DESIGN.md §7.
//
// Emulates the parts of GitLab's renderGFM() that affect how a rendered file looks (GL §10):
//   * syntax theme class on highlighted blocks (GL §10.1),
//   * KaTeX math with GitLab's options and limits (GL §10.2, render_math.js),
//   * mermaid rendered inline with the sandbox's configuration (GL §10.3),
//   * the <copy-code> button (GL §10.4, copy_code.js),
//   * hash navigation: getElementById(h) || getElementById('user-content-' + h) (GL §10.5),
//   * gl-system auto dark mode: keep html.gl-dark in sync with prefers-color-scheme (GL §11.3).
// Runs on DOMContentLoaded and on every `mdr:content-updated` event ({detail: {root, added}}),
// processing only the given nodes; rendered elements are never rendered twice. The helpers
// shared with github.js live in common.js, which the page loads first.
(function () {
  'use strict';

  if (window.mdRenderGitLab) return;

  // Shared helpers (assets/client/common.js, loaded first).
  var common = window.mdRenderCommon;
  if (!common) {
    console.warn('md-render: common.js did not load; GitLab page behaviours are off');
    return;
  }
  var CONFIG = common.config;
  var DONE = common.DONE;
  var collect = common.collect;
  var claim = common.claim;
  var libRef = common.libRef;
  var loadLibrary = common.loadLibrary;

  // ------------------------------------------------------------------------------------------
  // Utilities

  // A stylesheet library (KaTeX CSS): already present as <link>/<style data-mdr-lib>, or added.
  function ensureStylesheet(name) {
    var ref = libRef(name);
    if (!ref || ref.charAt(0) === '#') return;
    if (document.querySelector('[data-mdr-lib="' + name + '"]')) return;
    var link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = ref;
    link.setAttribute('data-mdr-lib', name);
    document.head.appendChild(link);
  }

  // GitLab UI alert (used for the math / mermaid performance warnings).
  function alertBox(message, actionLabel, onAction) {
    var alert = document.createElement('div');
    alert.className = 'gl-alert gl-alert-warning gl-alert-not-dismissible gl-mb-5';
    alert.setAttribute('role', 'alert');
    var content = document.createElement('div');
    content.className = 'gl-alert-content';
    var body = document.createElement('div');
    body.className = 'gl-alert-body';
    body.textContent = message;
    content.appendChild(body);
    if (actionLabel) {
      var actions = document.createElement('div');
      actions.className = 'gl-alert-actions';
      var button = document.createElement('button');
      button.type = 'button';
      button.className = 'btn gl-alert-action btn-confirm btn-md gl-button';
      button.textContent = actionLabel;
      button.addEventListener('click', function () {
        alert.remove();
        onAction();
      });
      actions.appendChild(button);
      content.appendChild(actions);
    }
    alert.appendChild(content);
    return alert;
  }

  // ------------------------------------------------------------------------------------------
  // Syntax theme (GL §10.1): the highlight-theme CSS only matches .code-syntax-highlight-theme.

  function syntaxHighlight(nodes) {
    collect(nodes, '.js-syntax-highlight').forEach(function (el) {
      el.classList.add('code-syntax-highlight-theme');
    });
  }

  // ------------------------------------------------------------------------------------------
  // Math (GL §10.2): each .js-render-math is replaced by <div data-math-style> holding KaTeX
  // output; GitLab's limits apply (1000 characters per block, 2000 ms total render time for the
  // math on the page).

  var MAX_MATH_CHARS = 1000;
  var MAX_RENDER_TIME_MS = 2000;
  // KaTeX time per placeholder. GitLab's budget is per page load; a live page keeps loading
  // new content, so only math still on the page counts (like github.js's brace budget).
  var mathTime = new Map();

  function totalMathMs() {
    var total = 0;
    mathTime.forEach(function (ms, placeholder) {
      if (placeholder.isConnected) total += ms;
      else mathTime.delete(placeholder); // removed by a live update
    });
    return total;
  }

  function katexOptions(style) {
    return {
      displayMode: style === 'display',
      throwOnError: true,
      trust: function (context) {
        return ['\\url', '\\href'].indexOf(context.command) !== -1 &&
          ['http', 'https', 'mailto', '_relative'].indexOf(context.protocol) !== -1;
      },
      maxSize: 20,
      maxExpand: 1000
    };
  }

  function renderMathPlaceholder(placeholder, text) {
    var style = placeholder.getAttribute('data-math-style');
    placeholder.classList.add(style === 'inline' ? 'math-content-inline' : 'math-content-display');
    var start = Date.now();
    try {
      placeholder.innerHTML = window.katex.renderToString(text, katexOptions(style));
    } catch (e) {
      var message = (e && e.message) || String(e);
      placeholder.textContent = /Too many expansions/.test(message)
        ? 'Too many expansions. Consider using multiple math blocks.'
        : 'There was an error rendering this math block. ' + message;
      placeholder.className = 'katex-error';
    }
    mathTime.set(placeholder, (mathTime.get(placeholder) || 0) + (Date.now() - start));
  }

  // render_math.js's placeholder for one .js-render-math element: <div data-math-style> with the
  // TeX as text and GitLab's layout class (.math-content-inline is display:inline-flex, so
  // inline math stays inside its line).
  function mathPlaceholder(el) {
    var style = el.getAttribute('data-math-style') || (el.tagName === 'PRE' ? 'display' : 'inline');
    var placeholder = document.createElement('div');
    placeholder.setAttribute('data-math-style', style);
    placeholder.className = style === 'inline' ? 'math-content-inline' : 'math-content-display';
    if (el.hasAttribute('data-mdr-key')) placeholder.setAttribute('data-mdr-key', el.getAttribute('data-mdr-key'));
    placeholder.setAttribute(DONE, '');
    placeholder.textContent = el.textContent;
    el.parentNode.replaceChild(placeholder, el);
    return placeholder;
  }

  function renderMath(nodes) {
    // Claiming marks the elements, so nothing is rendered twice. The source markup is only
    // replaced once KaTeX has loaded: without it (not vendored, failed download) the TeX keeps
    // its <span>/<code>/<pre> styling and line breaks, as in the GitHub flavor.
    var elements = collect(nodes, '.js-render-math').filter(claim);
    if (!elements.length) return;
    ensureStylesheet('katex-css');
    loadLibrary('katex').then(function () {
      if (!window.katex) throw new Error('KaTeX did not initialize');
      elements.forEach(function (el) {
        if (!el.isConnected || !el.parentNode) return;
        var placeholder = mathPlaceholder(el);
        var text = placeholder.textContent;
        if (text.length > MAX_MATH_CHARS || totalMathMs() >= MAX_RENDER_TIME_MS) {
          var message = text.length > MAX_MATH_CHARS
            ? 'This math block exceeds ' + MAX_MATH_CHARS + ' characters, and may cause performance issues on this page.'
            : 'Displaying this math block may cause performance issues on this page.';
          var code = document.createElement('pre');
          code.className = 'code';
          code.textContent = text;
          placeholder.replaceChildren(code);
          var anchor = placeholder.closest('.js-markdown-code') || placeholder;
          anchor.parentNode.insertBefore(alertBox(message, 'Display anyway', function () {
            renderMathPlaceholder(placeholder, text);
          }), anchor);
          return;
        }
        renderMathPlaceholder(placeholder, text);
      });
    }).catch(function (err) {
      console.warn('md-render: math is shown as TeX source:', err.message || err);
    });
  }

  // ------------------------------------------------------------------------------------------
  // Mermaid (GL §10.3): rendered inline with the sandbox's configuration
  // (mermaid_sandbox.js); the source <pre> is hidden like GitLab does, and diagrams are redrawn
  // when the colour mode changes. GitLab limits: 2000 characters per diagram, 50 per page.

  var MAX_MERMAID_CHARS = 2000;
  var MAX_MERMAID_BLOCKS = 50;
  var diagrams = new Set();
  var mermaidSeq = 0;
  var mermaidQueue = Promise.resolve();

  // Diagrams on the page (started ones, including those still queued). GitLab's limit is per
  // page load; on a live page replaced diagrams must not count.
  function liveDiagramCount() {
    diagrams.forEach(function (item) {
      if (!item.wrapper.isConnected) diagrams.delete(item);
    });
    return diagrams.size;
  }

  function isDark() {
    return document.documentElement.classList.contains('gl-dark');
  }

  function drawDiagram(item) {
    mermaidQueue = mermaidQueue.then(function () {
      if (!item.wrapper.isConnected) {
        diagrams.delete(item);
        return undefined;
      }
      window.mermaid.initialize({
        startOnLoad: false,
        theme: isDark() ? 'dark' : 'neutral',
        flowchart: { useMaxWidth: true, htmlLabels: true },
        securityLevel: 'strict',
        dompurifyConfig: { ADD_TAGS: ['foreignObject'], HTML_INTEGRATION_POINTS: { foreignobject: true } }
      });
      var id = 'mdr-mermaid-' + (++mermaidSeq);
      return Promise.resolve(window.mermaid.render(id, item.source)).then(function (result) {
        var container = item.container || document.createElement('div');
        container.className = 'md-render-mermaid';
        container.innerHTML = result.svg;
        if (typeof result.bindFunctions === 'function') result.bindFunctions(container);
        if (!container.parentNode) item.wrapper.appendChild(container);
        item.container = container;
        if (item.wrapper.classList.contains('js-markdown-code')) item.wrapper.classList.add('!gl-block');
        item.pre.classList.add('gl-sr-only', '!gl-p-0', '!gl-border-none');
      }, function (err) {
        var leftover = document.getElementById('d' + id);
        if (leftover) leftover.remove();
        diagrams.delete(item);
        if (item.container) item.container.remove();
        item.pre.classList.remove('gl-sr-only', '!gl-p-0', '!gl-border-none');
        var error = document.createElement('div');
        error.className = 'md-render-mermaid katex-error';
        error.textContent = 'Mermaid error: ' + ((err && err.message) || err);
        item.wrapper.appendChild(error);
      });
    });
    return mermaidQueue;
  }

  function startDiagram(item) {
    diagrams.add(item);
    drawDiagram(item);
  }

  function renderMermaid(nodes) {
    var codes = collect(nodes, 'code.js-render-mermaid, pre.language-mermaid > code').filter(claim);
    if (!codes.length) return;
    var items = [];
    codes.forEach(function (code) {
      var pre = code.closest('pre');
      if (!pre) return;
      var wrapper = pre.closest('.js-markdown-code') || pre.parentNode;
      if (!wrapper) return;
      items.push({ pre: pre, wrapper: wrapper, source: code.textContent.replace(/<br\s*\/>/g, '<br>'), container: null });
    });
    if (!items.length) return;
    loadLibrary('mermaid').then(function () {
      if (!window.mermaid) throw new Error('mermaid did not initialize');
      items.forEach(function (item) {
        if (item.source.length > MAX_MERMAID_CHARS) {
          item.wrapper.parentNode.insertBefore(alertBox(
            'Warning: Displaying this diagram might cause performance issues on this page.', 'Display',
            function () { startDiagram(item); }), item.wrapper);
        } else if (liveDiagramCount() >= MAX_MERMAID_BLOCKS) {
          item.wrapper.parentNode.insertBefore(alertBox(
            'Warning: Displaying this diagram might cause performance issues on this page.', 'Display',
            function () { startDiagram(item); }), item.wrapper);
        } else {
          startDiagram(item);
        }
      });
    }).catch(function (err) {
      console.warn('md-render: mermaid diagrams are shown as source:', err.message || err);
    });
  }

  function redrawDiagrams() {
    if (!window.mermaid) return;
    diagrams.forEach(function (item) { drawDiagram(item); });
  }

  // ------------------------------------------------------------------------------------------
  // <copy-code> (GL §10.4): GitLab's custom element appends a button that copies the <pre>.

  // Used when no GitLab icon sprite is vendored: the Primer Octicons `copy` icon (MIT,
  // (c) GitHub Inc., see THIRD_PARTY_NOTICES.md), drawn like a GitLab button icon.
  var COPY_ICON = '<svg class="s16 gl-icon gl-button-icon" aria-hidden="true" viewBox="0 0 16 16" width="16" height="16" fill="currentColor">' +
    '<path d="M0 6.75C0 5.784.784 5 1.75 5h1.5a.75.75 0 0 1 0 1.5h-1.5a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-1.5a.75.75 0 0 1 1.5 0v1.5A1.75 1.75 0 0 1 9.25 16h-7.5A1.75 1.75 0 0 1 0 14.25Z"></path>' +
    '<path d="M5 1.75C5 .784 5.784 0 6.75 0h7.5C15.216 0 16 .784 16 1.75v7.5A1.75 1.75 0 0 1 14.25 11h-7.5A1.75 1.75 0 0 1 5 9.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .138.112.25.25.25h7.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z"></path></svg>';
  var codeSeq = 0;

  // data-icon-sprite: the URL of GitLab's sprite, or "" when the page carries the symbol
  // itself (self-contained exports); absent without a vendored sprite.
  function iconMarkup() {
    var sprite = document.documentElement.getAttribute('data-icon-sprite');
    if (sprite === null || (!sprite && !document.getElementById('copy-to-clipboard'))) return COPY_ICON;
    var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('class', 's16 gl-icon gl-button-icon');
    svg.setAttribute('aria-hidden', 'true');
    var use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
    use.setAttribute('href', sprite + '#copy-to-clipboard');
    svg.appendChild(use);
    return svg.outerHTML;
  }

  function defineCopyCode() {
    if (!window.customElements || customElements.get('copy-code')) return;
    customElements.define('copy-code', class extends HTMLElement {
      connectedCallback() {
        if (this.querySelector('.btn')) return;
        var target = this.parentNode && this.parentNode.querySelector('pre');
        if (!target || this.closest('.md-suggestion')) return;
        if (!target.id) target.id = 'code-' + (++codeSeq);
        // Math rendering replaces a ```math <pre> by the KaTeX output; keep its source so the
        // button still copies the TeX (a hidden mermaid <pre> stays in place and is re-read).
        var source = target.textContent;
        var self = this;
        var button = document.createElement('button');
        button.type = 'button';
        button.className = 'btn btn-default btn-md gl-button btn-icon has-tooltip';
        button.setAttribute('data-title', 'Copy to clipboard');
        button.setAttribute('title', 'Copy to clipboard');
        button.setAttribute('aria-label', 'Copy to clipboard');
        button.setAttribute('data-clipboard-target', 'pre#' + target.id);
        button.innerHTML = iconMarkup();
        button.addEventListener('click', function () {
          var pre = self.parentNode && self.parentNode.querySelector('pre');
          var text = pre ? pre.textContent : source;
          common.writeClipboard(text).then(function () {
            button.setAttribute('title', 'Copied');
            button.setAttribute('aria-label', 'Copied');
            clearTimeout(button.mdrTimer);
            button.mdrTimer = setTimeout(function () {
              button.setAttribute('title', 'Copy to clipboard');
              button.setAttribute('aria-label', 'Copy to clipboard');
            }, 2000);
          }, function (err) {
            console.warn('md-render: could not copy to the clipboard', err);
          });
        });
        this.appendChild(button);
      }
    });
  }

  // ------------------------------------------------------------------------------------------
  // Hash navigation (GL §10.5)

  function hashTarget(id) {
    if (!id) return null;
    return document.getElementById(id) || document.getElementById('user-content-' + id);
  }

  var hashNav = common.hashNavigation(hashTarget);

  // ------------------------------------------------------------------------------------------
  // Colour mode (GL §11.3): in system mode html.gl-dark follows prefers-color-scheme.

  function syncSystemDark(dark) {
    var root = document.documentElement;
    if (!root.classList.contains('gl-system')) return;
    var had = root.classList.contains('gl-dark');
    root.classList.toggle('gl-dark', dark);
    if (had !== dark) redrawDiagrams();
  }

  if (window.matchMedia) {
    var media = window.matchMedia('(prefers-color-scheme: dark)');
    syncSystemDark(media.matches);
    var onScheme = function (e) { syncSystemDark(e.matches); };
    if (media.addEventListener) media.addEventListener('change', onScheme);
    else if (media.addListener) media.addListener(onScheme);
  }

  // ------------------------------------------------------------------------------------------
  // Entry points

  function renderGFM(nodes) {
    syntaxHighlight(nodes);
    renderMath(nodes);
    renderMermaid(nodes);
  }

  function init() {
    defineCopyCode();
    var root = document.getElementById('md-content') || document.body;
    renderGFM([root]);
    hashNav.scrollToHash();
  }

  document.addEventListener('mdr:content-updated', function (event) {
    var nodes = common.updatedNodes(event);
    renderGFM(nodes);
    hashNav.follow(nodes);
  });

  window.mdRenderGitLab = {
    renderGFM: function (root) { renderGFM([root]); },
    config: CONFIG
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
