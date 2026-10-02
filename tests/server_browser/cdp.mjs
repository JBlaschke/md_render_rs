// Headless Chrome bridge for the browser-driven `server_browser` tests (live.js behaviour).
//
//   node cdp.mjs <chrome-executable>
//
// Starts Chrome (`--headless=new`, fresh profile, DevTools over `--remote-debugging-pipe`)
// and then reads one JSON command per line on stdin, answering each with one JSON line on
// stdout ({"ok": value} or {"error": message}):
//
//   {"cmd": "open", "url": U, "background": B}  new tab (background tabs are hidden) -> tab id
//   {"cmd": "eval", "tab": T, "js": J}          evaluate J (awaited, by value)       -> value
//   {"cmd": "activate", "tab": T}               bring the tab to the front
//   {"cmd": "close", "tab": T}
//   {"cmd": "quit"}
//
// No dependencies beyond Node's standard library (Node 18+).
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const chromePath = process.argv[2];
const profile = mkdtempSync(join(tmpdir(), 'mdr-chrome-'));
const chrome = spawn(chromePath, [
  '--headless=new',
  '--remote-debugging-pipe',
  `--user-data-dir=${profile}`,
  '--no-first-run',
  '--no-default-browser-check',
  '--disable-extensions',
  '--disable-background-networking',
  '--disable-component-update',
  '--disable-sync',
  '--window-size=1000,800',
  'about:blank',
], { stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] });

const toChrome = chrome.stdio[3];
const fromChrome = chrome.stdio[4];
let nextId = 0;
const pending = new Map();
const sessions = new Map(); // tab id -> session id
let buffered = '';

fromChrome.on('data', (chunk) => {
  buffered += chunk.toString('utf8');
  let end;
  while ((end = buffered.indexOf('\0')) >= 0) {
    const msg = JSON.parse(buffered.slice(0, end));
    buffered = buffered.slice(end + 1);
    if (msg.id !== undefined && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message));
      else resolve(msg.result);
    }
  }
});

function send(method, params = {}, sessionId) {
  const id = ++nextId;
  const msg = { id, method, params };
  if (sessionId) msg.sessionId = sessionId;
  toChrome.write(JSON.stringify(msg) + '\0');
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error('timeout: ' + method));
      }
    }, 30000);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function evaluate(tab, js) {
  const sid = sessions.get(tab);
  if (!sid) throw new Error('no such tab: ' + tab);
  const r = await send('Runtime.evaluate', {
    expression: js,
    awaitPromise: true,
    returnByValue: true,
  }, sid);
  if (r.exceptionDetails) {
    const ex = r.exceptionDetails.exception;
    throw new Error((ex && ex.description) || r.exceptionDetails.text);
  }
  return r.result.value === undefined ? null : r.result.value;
}

async function open(url, background) {
  const { targetId } = await send('Target.createTarget', {
    url: 'about:blank',
    background: !!background,
  });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  sessions.set(targetId, sessionId);
  await send('Page.enable', {}, sessionId);
  await send('Runtime.enable', {}, sessionId);
  // Navigation does not wait for the load: a page that cannot get a connection (the case
  // under test) must not hang the bridge. Callers poll with `eval` instead.
  send('Page.navigate', { url }, sessionId).catch(() => {});
  return targetId;
}

async function handle(cmd) {
  switch (cmd.cmd) {
    case 'open':
      return open(cmd.url, cmd.background);
    case 'eval':
      return evaluate(cmd.tab, cmd.js);
    case 'activate':
      await send('Target.activateTarget', { targetId: cmd.tab });
      return null;
    case 'close':
      await send('Target.closeTarget', { targetId: cmd.tab });
      sessions.delete(cmd.tab);
      return null;
    case 'sleep':
      await sleep(cmd.ms);
      return null;
    case 'quit':
      return null;
    default:
      throw new Error('unknown command ' + cmd.cmd);
  }
}

let exiting = false;

// Stop Chrome, then remove its profile once it has exited (it writes until then).
function shutdown(code) {
  if (exiting) return;
  exiting = true;
  let finished = false;
  const done = () => {
    if (finished) return;
    finished = true;
    try {
      rmSync(profile, { recursive: true, force: true, maxRetries: 3 });
    } catch (e) {
      // best effort
    }
    process.exit(code);
  };
  if (chrome.exitCode !== null || chrome.signalCode !== null) {
    done();
    return;
  }
  chrome.once('exit', done);
  setTimeout(done, 3000);
  try {
    chrome.kill('SIGKILL');
  } catch (e) {
    done();
  }
}

chrome.on('exit', () => {
  if (exiting) return;
  process.stdout.write(JSON.stringify({ error: 'chrome exited' }) + '\n');
  shutdown(1);
});

// Wait until the browser answers.
for (let i = 0; ; i++) {
  try {
    await send('Browser.getVersion');
    break;
  } catch (e) {
    if (i > 50) shutdown(1);
    await sleep(100);
  }
}
process.stdout.write(JSON.stringify({ ok: 'ready' }) + '\n');

const rl = createInterface({ input: process.stdin });
for await (const line of rl) {
  if (!line.trim()) continue;
  let cmd;
  try {
    cmd = JSON.parse(line);
  } catch (e) {
    process.stdout.write(JSON.stringify({ error: 'bad JSON: ' + e.message }) + '\n');
    continue;
  }
  try {
    const value = await handle(cmd);
    process.stdout.write(JSON.stringify({ ok: value }) + '\n');
  } catch (e) {
    process.stdout.write(JSON.stringify({ error: String(e && e.message ? e.message : e) }) + '\n');
  }
  if (cmd.cmd === 'quit') break;
}
shutdown(0);
