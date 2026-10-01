// Minimal DOM + script runner for md-render's client scripts (tests only; no dependencies).
//
//   node dom_harness.js PAGE.html SCENARIO.js
//
// PAGE.html (a page produced by md_render::page::render_page) is parsed into a small DOM and its
// scripts run in document order, as a browser would. `<script src>` elements (also ones the page
// scripts add later) are handed to `window.mdrTest.loadScript(script)` when the scenario defines
// it (return true: loaded, false: failed, undefined: default handling). By default
// `<prefix>/static/NAME.js` runs assets/client/NAME.js and anything else fails to load. Then DOMContentLoaded and
// `load` fire, and SCENARIO.js runs. A scenario is a CommonJS module:
//
//   module.exports = {
//     before(window) {},                  // optional: runs before the page is parsed
//     async run(window, t) {},            // t: { ok, equal, flush, log }
//     allowScriptErrors: false,           // optional
//   };
//
// The process exits with status 1 when an assertion fails, a page script throws, or a promise
// rejection goes unhandled. Supported selectors: type, #id, .class, [attr], [attr=v] (also ^=,
// $=, *=, ~=), :scope, descendant and child combinators, and comma lists; anything else throws
// a SyntaxError (like `:target` in old browsers, which the client scripts guard against).
'use strict';

const fs = require('fs');
const path = require('path');
const vm = require('vm');

// ---------------------------------------------------------------------------------------------
// HTML parsing and serialization (well-formed input only: md-render's own pages and fragments)

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);
const RAW_TEXT = new Set(['script', 'style', 'textarea', 'title']);
const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };

function decodeEntities(s) {
  return s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (all, e) => {
    if (e[0] === '#') {
      const n = e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
      return Number.isFinite(n) ? String.fromCodePoint(n) : all;
    }
    const v = ENTITIES[e.toLowerCase()];
    return v === undefined ? all : v;
  });
}

function escapeText(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function escapeAttr(s) {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;');
}

const TOKEN_RE = /<!--[\s\S]*?-->|<![^>]*>|<\/([a-zA-Z][\w:-]*)\s*>|<([a-zA-Z][\w:-]*)((?:\s+[^\s"'>/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>]+))?)*)\s*(\/?)>|[^<]+|</g;
const ATTR_RE = /([^\s"'>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?/g;

// Parse `html` and append the nodes to `parent`. Scripts parsed with `execute: false` (innerHTML,
// DOMParser) never run, as in browsers.
function parseInto(doc, parent, html, execute) {
  const re = new RegExp(TOKEN_RE.source, 'g');
  const stack = [parent];
  let m;
  while ((m = re.exec(html))) {
    const top = stack[stack.length - 1];
    if (m[0].startsWith('<!')) continue;
    if (m[1]) {
      const name = m[1].toLowerCase();
      for (let i = stack.length - 1; i > 0; i--) {
        if ((stack[i]._host || stack[i]).localName === name) {
          stack.length = i;
          break;
        }
      }
      continue;
    }
    if (m[2]) {
      const name = m[2].toLowerCase();
      const el = doc.createElement(name);
      const attrs = new RegExp(ATTR_RE.source, 'g');
      let a;
      while ((a = attrs.exec(m[3] || ''))) {
        const value = a[2] !== undefined ? a[2] : a[3] !== undefined ? a[3] : a[4] !== undefined ? a[4] : '';
        el.setAttribute(a[1].toLowerCase(), decodeEntities(value));
      }
      if (!execute && name === 'script') el._started = true;
      if (RAW_TEXT.has(name) && !m[4]) {
        const close = html.toLowerCase().indexOf('</' + name, re.lastIndex);
        const end = close < 0 ? html.length : close;
        const raw = html.slice(re.lastIndex, end);
        if (raw) el.appendChild(doc.createTextNode(name === 'script' || name === 'style' ? raw : decodeEntities(raw)));
        re.lastIndex = close < 0 ? html.length : html.indexOf('>', close) + 1;
        top.appendChild(el);
        continue;
      }
      top.appendChild(el);
      if (!VOID.has(name) && !m[4]) stack.push(name === 'template' ? el.content : el);
      continue;
    }
    top.appendChild(doc.createTextNode(decodeEntities(m[0])));
  }
}

function serialize(node) {
  if (node.nodeType === 3) {
    const p = node.parentNode;
    return p && p.nodeType === 1 && (p.localName === 'script' || p.localName === 'style') ? node.data : escapeText(node.data);
  }
  if (node.nodeType !== 1) return node.childNodes.map(serialize).join('');
  const attrs = Array.from(node._attrs, ([k, v]) => ' ' + k + '="' + escapeAttr(v) + '"').join('');
  const open = '<' + node.localName + attrs + '>';
  if (VOID.has(node.localName)) return open;
  const kids = node.localName === 'template' ? node.content.childNodes : node.childNodes;
  return open + kids.map(serialize).join('') + '</' + node.localName + '>';
}

// ---------------------------------------------------------------------------------------------
// Selectors

function splitTop(s, sep) {
  const out = [];
  let depth = 0;
  let quote = null;
  let cur = '';
  for (const c of s) {
    if (quote) {
      if (c === quote) quote = null;
    } else if (c === '"' || c === "'") {
      quote = c;
    } else if (c === '[' || c === '(') {
      depth++;
    } else if (c === ']' || c === ')') {
      depth--;
    } else if (depth === 0 && sep(c)) {
      out.push(cur);
      cur = '';
      continue;
    }
    cur += c;
  }
  out.push(cur);
  return out;
}

function parseCompound(s, whole) {
  const c = { tag: null, ids: [], classes: [], attrs: [], scope: false };
  let i = 0;
  let m = /^(\*|[a-zA-Z][\w-]*)/.exec(s);
  if (m) {
    c.tag = m[1] === '*' ? null : m[1].toLowerCase();
    i = m[0].length;
  }
  while (i < s.length) {
    const rest = s.slice(i);
    if ((m = /^#((?:\\.|[\w-])+)/.exec(rest))) {
      c.ids.push(m[1].replace(/\\(.)/g, '$1'));
    } else if ((m = /^\.((?:\\.|[\w-])+)/.exec(rest))) {
      c.classes.push(m[1].replace(/\\(.)/g, '$1'));
    } else if ((m = /^\[\s*([\w:-]+)\s*(?:([~^$*|]?=)\s*(?:"([^"]*)"|'([^']*)'|([^\]\s]+))\s*)?\]/.exec(rest))) {
      c.attrs.push({ name: m[1].toLowerCase(), op: m[2] || null, value: m[3] !== undefined ? m[3] : m[4] !== undefined ? m[4] : m[5] });
    } else if ((m = /^:scope/.exec(rest))) {
      c.scope = true;
    } else {
      throw new SyntaxError("'" + whole + "' is not a valid selector");
    }
    i += m[0].length;
  }
  return c;
}

const selectorCache = new Map();

// A selector list: [[{comb, compound}, ...], ...] (combinator before each compound).
function parseSelector(sel) {
  if (selectorCache.has(sel)) return selectorCache.get(sel);
  const list = splitTop(sel, (c) => c === ',').map((complex) => {
    const parts = [];
    const tokens = complex.trim().replace(/\s*>\s*/g, ' > ').split(/\s+/).filter(Boolean);
    let comb = null;
    for (const tok of tokens) {
      if (tok === '>') {
        comb = '>';
        continue;
      }
      parts.push({ comb: parts.length ? comb || ' ' : null, compound: parseCompound(tok, sel) });
      comb = null;
    }
    if (!parts.length) throw new SyntaxError("'" + sel + "' is not a valid selector");
    return parts;
  });
  selectorCache.set(sel, list);
  return list;
}

function matchCompound(el, c, scope) {
  if (!el || el.nodeType !== 1) return false;
  if (c.tag && el.localName !== c.tag) return false;
  if (c.scope && el !== scope) return false;
  for (const id of c.ids) if (el.getAttribute('id') !== id) return false;
  for (const cls of c.classes) if (!el.classList.contains(cls)) return false;
  for (const a of c.attrs) {
    const v = el.getAttribute(a.name);
    if (v === null) return false;
    switch (a.op) {
      case null: break;
      case '=': if (v !== a.value) return false; break;
      case '^=': if (!a.value || !v.startsWith(a.value)) return false; break;
      case '$=': if (!a.value || !v.endsWith(a.value)) return false; break;
      case '*=': if (!a.value || !v.includes(a.value)) return false; break;
      case '~=': if (!v.split(/\s+/).includes(a.value)) return false; break;
      case '|=': if (v !== a.value && !v.startsWith(a.value + '-')) return false; break;
      default: return false;
    }
  }
  return true;
}

function matchComplex(el, parts, idx, scope) {
  if (!matchCompound(el, parts[idx].compound, scope)) return false;
  if (idx === 0) return true;
  if (parts[idx].comb === '>') return matchComplex(el.parentElement, parts, idx - 1, scope);
  for (let p = el.parentElement; p; p = p.parentElement) {
    if (matchComplex(p, parts, idx - 1, scope)) return true;
  }
  return false;
}

function matchesSelector(el, sel, scope) {
  return parseSelector(sel).some((parts) => matchComplex(el, parts, parts.length - 1, scope));
}

// ---------------------------------------------------------------------------------------------
// Nodes

class ClassList {
  constructor(el) { this._el = el; }
  _get() { return (this._el.getAttribute('class') || '').split(/\s+/).filter(Boolean); }
  _set(list) { this._el.setAttribute('class', list.join(' ')); }
  get length() { return this._get().length; }
  contains(c) { return this._get().includes(c); }
  add(...cs) { const l = this._get(); cs.forEach((c) => { if (!l.includes(c)) l.push(c); }); this._set(l); }
  remove(...cs) { this._set(this._get().filter((c) => !cs.includes(c))); }
  toggle(c, force) {
    const on = force === undefined ? !this.contains(c) : !!force;
    if (on) this.add(c); else this.remove(c);
    return on;
  }
  forEach(f) { this._get().forEach(f); }
  toString() { return this._get().join(' '); }
}

class EventTarget {
  addEventListener(type, f, opts) {
    this._listeners = this._listeners || {};
    (this._listeners[type] = this._listeners[type] || []).push({ f, once: !!(opts && opts.once) });
  }
  removeEventListener(type, f) {
    const l = this._listeners && this._listeners[type];
    if (l) this._listeners[type] = l.filter((x) => x.f !== f);
  }
  dispatchEvent(event) {
    if (!event.target) event.target = this;
    if (typeof event.preventDefault !== 'function') {
      event.defaultPrevented = false;
      event.preventDefault = function () { this.defaultPrevented = true; };
    }
    for (let node = this; node; node = event.bubbles ? node.parentNode || (node.nodeType === 9 ? node.defaultView : null) : null) {
      const l = (node._listeners && node._listeners[event.type]) || [];
      node._listeners && (node._listeners[event.type] = l.filter((x) => !x.once));
      l.forEach((x) => x.f.call(node, event));
      if (node === node.defaultView) break;
    }
    return !event.defaultPrevented;
  }
}

class Node extends EventTarget {
  constructor(doc) {
    super();
    this._doc = doc;
    this.parentNode = null;
    this.childNodes = [];
  }
  get ownerDocument() { return this._doc; }
  get parentElement() { return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null; }
  get isConnected() {
    let n = this;
    while (n.parentNode) n = n.parentNode;
    return n.nodeType === 9;
  }
  get firstChild() { return this.childNodes[0] || null; }
  get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
  get children() { return this.childNodes.filter((c) => c.nodeType === 1); }
  get firstElementChild() { return this.children[0] || null; }
  get nextSibling() {
    const p = this.parentNode;
    return p ? p.childNodes[p.childNodes.indexOf(this) + 1] || null : null;
  }
  appendChild(c) { return this.insertBefore(c, null); }
  append(...cs) { cs.forEach((c) => this.appendChild(typeof c === 'string' ? this._doc.createTextNode(c) : c)); }
  insertBefore(c, ref) {
    if (c.nodeType === 11) {
      c.childNodes.slice().forEach((k) => this.insertBefore(k, ref));
      return c;
    }
    if (c.parentNode) c.parentNode.removeChild(c);
    const i = ref ? this.childNodes.indexOf(ref) : -1;
    c.parentNode = this;
    if (i < 0) this.childNodes.push(c); else this.childNodes.splice(i, 0, c);
    if (c.isConnected) this._doc._inserted(c);
    return c;
  }
  removeChild(c) {
    const i = this.childNodes.indexOf(c);
    if (i >= 0) this.childNodes.splice(i, 1);
    c.parentNode = null;
    return c;
  }
  replaceChild(n, old) {
    this.insertBefore(n, old);
    this.removeChild(old);
    return old;
  }
  replaceChildren(...nodes) {
    this.childNodes.slice().forEach((c) => this.removeChild(c));
    nodes.forEach((n) => this.appendChild(n));
  }
  remove() { if (this.parentNode) this.parentNode.removeChild(this); }
  replaceWith(n) { if (this.parentNode) this.parentNode.replaceChild(n, this); }
  before(n) { if (this.parentNode) this.parentNode.insertBefore(n, this); }
  contains(n) {
    for (; n; n = n.parentNode) if (n === this) return true;
    return false;
  }
  get textContent() {
    return this.nodeType === 3 ? this.data : this.childNodes.map((c) => (c.nodeType === 8 ? '' : c.textContent)).join('');
  }
  set textContent(v) {
    this.childNodes.slice().forEach((c) => this.removeChild(c));
    const s = v === null || v === undefined ? '' : String(v);
    if (s !== '') this.appendChild(this._doc.createTextNode(s));
  }
  querySelectorAll(sel) {
    parseSelector(sel);
    const out = [];
    const scope = this.nodeType === 1 ? this : null;
    const walk = (n) => n.childNodes.forEach((c) => {
      if (c.nodeType !== 1) return;
      if (matchesSelector(c, sel, scope)) out.push(c);
      walk(c);
    });
    walk(this);
    return out;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
}

class Text extends Node {
  constructor(doc, data) {
    super(doc);
    this.nodeType = 3;
    this.data = data;
  }
  get nodeValue() { return this.data; }
  cloneNode() { return new Text(this._doc, this.data); }
}

class DocumentFragment extends Node {
  constructor(doc, host) {
    super(doc);
    this.nodeType = 11;
    this._host = host || null;
  }
}

class Element extends Node {
  constructor(doc, localName) {
    super(doc);
    this.nodeType = 1;
    this.localName = localName;
    this._attrs = new Map();
    this.classList = new ClassList(this);
    this.style = {};
    if (localName === 'template') this.content = new DocumentFragment(doc, this);
  }
  get tagName() { return this.localName.toUpperCase(); }
  get nodeName() { return this.tagName; }
  get attributes() { return Array.from(this._attrs, ([name, value]) => ({ name, value })); }
  getAttribute(n) { n = String(n).toLowerCase(); return this._attrs.has(n) ? this._attrs.get(n) : null; }
  setAttribute(n, v) { this._attrs.set(String(n).toLowerCase(), String(v)); }
  hasAttribute(n) { return this._attrs.has(String(n).toLowerCase()); }
  removeAttribute(n) { this._attrs.delete(String(n).toLowerCase()); }
  get id() { return this.getAttribute('id') || ''; }
  set id(v) { this.setAttribute('id', v); }
  get className() { return this.getAttribute('class') || ''; }
  set className(v) { this.setAttribute('class', v); }
  get type() { return this.getAttribute('type') || ''; }
  set type(v) { this.setAttribute('type', v); }
  get src() { return this.getAttribute('src') || ''; }
  set src(v) { this.setAttribute('src', v); }
  get href() { return this.getAttribute('href') || ''; }
  set href(v) { this.setAttribute('href', v); }
  get rel() { return this.getAttribute('rel') || ''; }
  set rel(v) { this.setAttribute('rel', v); }
  get value() { return this._value !== undefined ? this._value : this.getAttribute('value') || ''; }
  set value(v) { this._value = String(v); }
  get dataset() {
    const el = this;
    return new Proxy({}, {
      get(_, k) { return el.getAttribute('data-' + String(k).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase())) ?? undefined; },
      set(_, k, v) { el.setAttribute('data-' + String(k).replace(/[A-Z]/g, (c) => '-' + c.toLowerCase()), v); return true; },
    });
  }
  get innerHTML() { return (this.localName === 'template' ? this.content : this).childNodes.map(serialize).join(''); }
  set innerHTML(v) {
    const target = this.localName === 'template' ? this.content : this;
    target.childNodes.slice().forEach((c) => target.removeChild(c));
    parseInto(this._doc, target, String(v), false);
  }
  get outerHTML() { return serialize(this); }
  matches(sel) { return matchesSelector(this, sel, this); }
  closest(sel) {
    for (let n = this; n && n.nodeType === 1; n = n.parentNode) if (matchesSelector(n, sel, this)) return n;
    return null;
  }
  cloneNode(deep) {
    const el = this._doc.createElement(this.localName);
    this._attrs.forEach((v, k) => el.setAttribute(k, v));
    if (deep) {
      this.childNodes.forEach((c) => el.appendChild(c.cloneNode(true)));
      if (this.content) this.content.childNodes.forEach((c) => el.content.appendChild(c.cloneNode(true)));
    }
    return el;
  }
  scrollIntoView(arg) {
    const w = this._doc.defaultView;
    if (w) w.mdrTest.scrolledTo.push({ el: this, arg });
  }
  select() {}
  focus() {}
  click() { this.dispatchEvent({ type: 'click', bubbles: true, button: 0 }); }
}

class Document extends Node {
  constructor(win) {
    super(null);
    this._doc = this;
    this.nodeType = 9;
    this.defaultView = win || null;
    this.documentElement = null;
    this.head = null;
    this.body = null;
    this.readyState = 'loading';
    this.baseURI = 'http://localhost/doc.html';
  }
  createElement(name) { return new Element(this, String(name).toLowerCase()); }
  createElementNS(_ns, name) { return new Element(this, String(name)); }
  createTextNode(data) { return new Text(this, String(data)); }
  createComment() { const t = new Text(this, ''); t.nodeType = 8; return t; }
  createDocumentFragment() { return new DocumentFragment(this); }
  importNode(n, deep) { return n.cloneNode(deep); }
  getElementById(id) {
    let found = null;
    const walk = (n) => {
      for (const c of n.childNodes) {
        if (found) return;
        if (c.nodeType !== 1) continue;
        if (c.getAttribute('id') === id) { found = c; return; }
        walk(c);
      }
    };
    walk(this);
    return found;
  }
  getElementsByName(name) { return this.querySelectorAll('[name="' + String(name).replace(/"/g, '') + '"]'); }
  // Called for every node inserted into the document: track html/head/body, upgrade custom
  // elements and run scripts.
  _inserted(node) {
    if (node.nodeType !== 1) return;
    if (node.localName === 'html' && node.parentNode === this) this.documentElement = node;
    if (node.localName === 'head' && node.parentNode === this.documentElement) this.head = node;
    if (node.localName === 'body' && node.parentNode === this.documentElement) this.body = node;
    const win = this.defaultView;
    if (win) {
      win.customElements._connected(node);
      if (node.localName === 'script') win.mdrTest._script(node);
    }
    node.childNodes.slice().forEach((c) => this._inserted(c));
  }
}

class CustomElementRegistry {
  constructor() { this._defs = new Map(); }
  get(name) { return this._defs.get(name); }
  define(name, cls) {
    if (this._defs.has(name)) throw new Error('already defined: ' + name);
    this._defs.set(name, cls);
    if (this._doc) this._doc.querySelectorAll(name).forEach((el) => this._upgrade(el));
  }
  _upgrade(el) {
    const cls = this._defs.get(el.localName);
    if (!cls || el._upgraded) return;
    el._upgraded = true;
    Object.setPrototypeOf(el, cls.prototype);
    if (typeof el.connectedCallback === 'function') el.connectedCallback();
  }
  _connected(el) {
    if (this._defs.has(el.localName)) {
      if (el._upgraded && typeof el.connectedCallback === 'function') el.connectedCallback();
      else this._upgrade(el);
    }
  }
}

// ---------------------------------------------------------------------------------------------
// Window and script execution

// `<prefix>/static/NAME.js` (md-render's embedded client scripts) -> assets/client/NAME.js.
function staticScript(src) {
  const m = /(?:^|\/)static\/([\w.-]+\.js)(?:[?#].*)?$/.exec(src || '');
  if (!m) return null;
  const file = path.join(__dirname, '..', '..', 'assets', 'client', m[1]);
  return fs.existsSync(file) ? file : null;
}

function makeWindow(scriptErrors, logs) {
  const win = new EventTarget();
  const doc = new Document(win);
  const registry = new CustomElementRegistry();
  registry._doc = doc;
  const record = (level) => (...args) => logs.push({ level, text: args.map((a) => (a && a.message) || String(a)).join(' ') });
  Object.assign(win, {
    document: doc,
    customElements: registry,
    HTMLElement: Element,
    Node,
    Element,
    console: { log: record('log'), info: record('info'), warn: record('warn'), error: record('error'), debug: record('debug') },
    location: { hash: '', href: 'http://localhost/doc.html', pathname: '/doc.html', search: '' },
    history: { pushState(_s, _t, url) { win.location.hash = String(url).replace(/^[^#]*/, ''); }, replaceState() {} },
    navigator: {},
    isSecureContext: false,
    prefersDark: false,
    matchMedia(query) {
      const dark = /prefers-color-scheme:\s*dark/.test(query);
      const light = /prefers-color-scheme:\s*light/.test(query);
      return {
        media: query,
        matches: dark ? win.prefersDark : light ? !win.prefersDark : false,
        addEventListener() {},
        removeEventListener() {},
        addListener() {},
      };
    },
    MutationObserver: class { observe() {} disconnect() {} takeRecords() { return []; } },
    DOMParser: class {
      parseFromString(markup) {
        const d = new Document(null);
        const html = d.createElement('html');
        d.appendChild(html);
        html.appendChild(d.createElement('head'));
        const body = d.createElement('body');
        html.appendChild(body);
        parseInto(d, body, String(markup), false);
        return d;
      }
    },
    Promise, Set, Map, WeakMap, Symbol, Proxy, Date, JSON, Math, Array, Object, String, Number, Boolean,
    Error, TypeError, SyntaxError, RegExp, URL, parseInt, parseFloat, isFinite, isNaN,
    encodeURIComponent, decodeURIComponent,
    setTimeout, clearTimeout, setInterval, clearInterval, queueMicrotask,
  });
  win.window = win;
  win.self = win;
  win.globalThis = win;
  vm.createContext(win);
  win.mdrTest = {
    scrolledTo: [],
    loadScript: null,
    run(code, name) {
      try {
        vm.runInContext(code, win, { filename: name || 'inline-script' });
      } catch (e) {
        scriptErrors.push((name || 'inline script') + ': ' + ((e && e.stack) || e));
      }
    },
    _script(script) {
      if (script._started) return;
      script._started = true;
      const type = (script.getAttribute('type') || '').trim().toLowerCase();
      if (type && type !== 'text/javascript' && type !== 'application/javascript' && type !== 'module') return;
      if (script.hasAttribute('src')) {
        const load = () => {
          const loader = win.mdrTest.loadScript;
          let ok = typeof loader === 'function' ? loader(script) : undefined;
          if (ok === undefined || ok === null) {
            const local = staticScript(script.getAttribute('src'));
            if (local) win.mdrTest.run(fs.readFileSync(local, 'utf8'), path.basename(local));
            ok = !!local;
          }
          const handler = ok ? script.onload : script.onerror;
          if (typeof handler === 'function') handler.call(script, { type: ok ? 'load' : 'error' });
        };
        // Parser-inserted scripts block parsing (run now); script-inserted ones are async.
        if (doc._parsing) load(); else setTimeout(load, 0);
        return;
      }
      win.mdrTest.run(script.textContent, script.getAttribute('data-mdr-static') || 'inline-script');
    },
  };
  return win;
}

// ---------------------------------------------------------------------------------------------
// Runner

async function main() {
  const [pagePath, scenarioPath] = process.argv.slice(2);
  if (!pagePath || !scenarioPath) {
    console.error('usage: node dom_harness.js PAGE.html SCENARIO.js');
    process.exit(2);
  }
  const scriptErrors = [];
  const logs = [];
  const failures = [];
  const rejections = [];
  process.on('unhandledRejection', (e) => rejections.push(String((e && e.stack) || e)));

  const scenario = require(path.resolve(scenarioPath));
  const win = makeWindow(scriptErrors, logs);
  const flush = async (n) => {
    for (let i = 0; i < (n || 4); i++) {
      await new Promise((r) => setImmediate(r));
      await new Promise((r) => setTimeout(r, 1));
    }
  };
  const t = {
    flush,
    log: (...a) => console.log('#', ...a),
    logs,
    ok(cond, message) {
      if (!cond) failures.push(message || 'assertion failed');
    },
    equal(actual, expected, message) {
      const a = JSON.stringify(actual);
      const e = JSON.stringify(expected);
      if (a !== e) failures.push((message || 'not equal') + ': got ' + a + ', expected ' + e);
    },
  };

  if (scenario.before) scenario.before(win);
  const doc = win.document;
  doc._parsing = true;
  parseInto(doc, doc, fs.readFileSync(pagePath, 'utf8'), true);
  doc._parsing = false;
  doc.readyState = 'interactive';
  doc.dispatchEvent({ type: 'DOMContentLoaded' });
  doc.readyState = 'complete';
  win.dispatchEvent({ type: 'load' });
  await flush();
  try {
    await scenario.run(win, t);
  } catch (e) {
    failures.push('scenario threw: ' + ((e && e.stack) || e));
  }
  await flush();

  if (!scenario.allowScriptErrors) scriptErrors.forEach((e) => failures.push('page script error: ' + e));
  rejections.forEach((e) => failures.push('unhandled rejection: ' + e));
  logs.forEach((l) => console.log('# console.' + l.level + ': ' + l.text));
  if (failures.length) {
    failures.forEach((f) => console.log('not ok - ' + f));
    process.exit(1);
  }
  console.log('ok');
}

main().catch((e) => {
  console.log('not ok - harness error: ' + ((e && e.stack) || e));
  process.exit(1);
});
