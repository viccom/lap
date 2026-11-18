// Desktop smoke gate (SOP-上游同步升级.md §3.2 / §7-5).
//
// Boots the real desktop binary against a real data directory and asserts,
// over WebView2's CDP, the minimal rendering contract that "builds" cannot
// prove: the grid paints thumbnails, they survive scrolling, and a real
// double-click renders the original in the viewer. Born from the v0.4.0
// incident - that build compiled, launched, and still blanked its grid.
//
//   node scripts/smoke-desktop.mjs
//
// Environment:
//   LAP_SMOKE_EXE       default src-tauri/target/debug/lappp.exe
//   LAP_SMOKE_DATA_DIR  default %LOCALAPPDATA%/com.julyx10.lap.debug
//                       (must match the binary's build flavor: debug builds
//                       read …lap.debug, release builds read …lap - SOP §4)
//   LAP_SMOKE_MIN_WIDTH viewer original must exceed this (default 800; the
//                       grid thumbnails are ~768 wide, so anything above that
//                       proves the full-resolution decode happened)
//   LAP_SMOKE_PORT      fixed CDP port (default: pick a free one)
//
// CDP injection: wry always passes its own browser arguments, and WebView2
// IGNORES the WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS env var in that case
// (verified the hard way). So the script temporarily patches
// `additionalBrowserArgs` into src-tauri/tauri.windows.conf.json, rebuilds
// the launcher (incremental, seconds), runs the assertions, then RESTORES the
// exact original config bytes and rebuilds - the SOP rule "smoke builds must
// never ship the debug port" is enforced by construction, including on crash
// (try/finally). The config is restored byte-identical, so uncommitted user
// edits survive.
//
// Console errors are collected whole and fail the run unless explicitly
// allowlisted below with a reason; the v0.4.0 root cause sat in exactly the
// noise someone would have filtered.
//
// Not part of `npm run test` (rebuilds + spawns a GUI window); run it as the
// sync / release gate per SOP §3 gate 7.
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import os from 'node:os';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..');
const EXE = process.env.LAP_SMOKE_EXE || join(repo, 'src-tauri', 'target', 'debug', 'lappp.exe');
const DATA_DIR = process.env.LAP_SMOKE_DATA_DIR ||
  join(process.env.LOCALAPPDATA || join(os.homedir(), 'AppData', 'Local'), 'com.julyx10.lap.debug');
const MIN_WIDTH = Number(process.env.LAP_SMOKE_MIN_WIDTH || 800);
const OUT_DIR = join(repo, 'build', 'smoke');
const CONF = join(repo, 'src-tauri', 'tauri.windows.conf.json');
// cargo needs cmake, which only exists inside VS BuildTools on this machine
// (machine-profile §3); prepend it when present.
const CMAKE = 'C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin';

// Console noise that is provably harmless. Every entry needs its reason.
const CONSOLE_ALLOWLIST = [
  // (none yet - keep it empty; additions require a justification comment)
];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log('[smoke]', ...a);
const fail = (msg) => { console.error('[smoke] FAIL:', msg); process.exitCode = 1; };

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.listen(0, '127.0.0.1', () => { const port = srv.address().port; srv.close(() => resolve(port)); });
    srv.on('error', reject);
  });
}

function cdpConnect(url, onEvent) {
  const ws = new WebSocket(url);
  let id = 0;
  const pending = new Map();
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) {
      const p = pending.get(m.id); pending.delete(m.id);
      m.error ? p.reject(new Error(JSON.stringify(m.error))) : p.resolve(m.result);
    } else if (m.method) onEvent(m);
  });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const mid = ++id; pending.set(mid, { resolve: res, reject: rej });
    ws.send(JSON.stringify({ id: mid, method, params }));
  });
  return new Promise((res, rej) => {
    ws.addEventListener('open', () => res({ ws, send }), { once: true });
    ws.addEventListener('error', rej, { once: true });
  });
}

function cargoBuild() {
  const env = { ...process.env, PATH: `${CMAKE}${existsSync(CMAKE) ? ';' + process.env.PATH : ''}` };
  const res = spawnSync('cargo', ['build', '-p', 'lappp'], { cwd: join(repo, 'src-tauri'), env, encoding: 'utf8' });
  if (res.status !== 0) {
    console.error((res.stdout || '') + (res.stderr || ''));
    throw new Error('cargo build failed');
  }
}

// Temporarily embed the CDP port, and always restore the exact original bytes.
function withCdpConf(port, body) {
  const original = readFileSync(CONF, 'utf8');
  if (original.includes('remote-debugging-port')) {
    throw new Error('tauri.windows.conf.json already carries a remote-debugging-port - clean it up first (SOP §3.2)');
  }
  const conf = JSON.parse(original);
  conf.app = conf.app || {};
  conf.app.windows = conf.app.windows || [];
  conf.app.windows[0] = conf.app.windows[0] || {};
  conf.app.windows[0].additionalBrowserArgs = `--remote-debugging-port=${port}`;
  writeFileSync(CONF, JSON.stringify(conf, null, 2) + '\n');
  return async (result) => {
    writeFileSync(CONF, original);
    log('config restored, rebuilding a clean binary...');
    try { cargoBuild(); log('clean rebuild done'); } catch (e) { console.error('[smoke] CLEAN REBUILD FAILED:', e.message); }
    return result;
  };
}

async function main() {
  if (!existsSync(EXE)) return fail(`binary not found: ${EXE}`);
  if (!existsSync(DATA_DIR)) return fail(`data dir not found: ${DATA_DIR} (set LAP_SMOKE_DATA_DIR; debug builds read …lap.debug, release …lap)`);

  // Refuse to touch a running instance - killing the user's app is not ours
  // to do, and two instances would fight over the library lock.
  const procs = spawnSync('powershell', ['-NoProfile', '-Command',
    '(Get-Process lappp -ErrorAction SilentlyContinue | Measure-Object).Count'], { encoding: 'utf8' });
  if (Number(procs.stdout.trim()) > 0) return fail('a lappp process is already running - close it first');

  const port = Number(process.env.LAP_SMOKE_PORT) || await freePort();
  mkdirSync(OUT_DIR, { recursive: true });
  const restore = withCdpConf(port);
  let child = null;
  const cleanup = () => { if (child && child.pid) { try { spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F']); } catch {} } };
  process.on('exit', cleanup);

  try {
    log('rebuilding with the CDP port embedded...');
    cargoBuild();
    log(`launching cdp=127.0.0.1:${port} data=${DATA_DIR}`);
    child = spawn(EXE, ['--data-dir', DATA_DIR], { stdio: 'ignore' });

    // Wait for the WebView target.
    let target = null;
    for (let i = 0; i < 60 && !target; i++) {
      await sleep(1000);
      if (child.exitCode !== null) return fail(`app exited early (code ${child.exitCode})`);
      try {
        const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
        target = list.find((t) => t.type === 'page');
      } catch { /* CDP not up yet */ }
    }
    if (!target) return fail('CDP target never appeared within 60s');

  const errors = [];
  const { ws, send } = await cdpConnect(target.webSocketDebuggerUrl, (m) => {
    if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') {
      const text = m.params.args.map((a) => a.value ?? a.description ?? '').join(' ');
      if (!CONSOLE_ALLOWLIST.some((re) => re.test(text))) errors.push(text.slice(0, 300));
    } else if (m.method === 'Runtime.exceptionThrown') {
      const text = m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text || '';
      if (!CONSOLE_ALLOWLIST.some((re) => re.test(text))) errors.push(('EXC: ' + text).slice(0, 300));
    }
  });
  await send('Runtime.enable');
  await send('Page.enable');
  const ev = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    return r.exceptionDetails ? { __err: r.exceptionDetails.exception?.description } : r.result?.value;
  };

  // 1) The grid must paint real thumbnails.
  let thumbs = 0;
  for (let i = 0; i < 90 && thumbs === 0; i++) {
    await sleep(1000);
    thumbs = await ev("document.querySelectorAll('[data-thumbnail-container]').length");
  }
  log(`grid thumbnails: ${thumbs}`);
  if (!thumbs || thumbs < 1) { await shot(send, 'fail-grid'); return fail('no thumbnails rendered'); }

  // 2) Thumbnails must survive scrolling down and back up (the incident's
  //    signature: cells collapse to empty comments after an upward scroll).
  const scroll = async (y) => {
    await ev(`(() => { document.querySelector('.virtual-scroller').scrollTop = ${y}; })()`);
    await sleep(1800);
    return ev("document.querySelectorAll('[data-thumbnail-container]').length");
  };
  const afterDown = await scroll(4000);
  const afterUp = await scroll(0);
  log(`thumbnails after scroll down/up: ${afterDown}/${afterUp}`);
  if (afterDown < 1 || afterUp < 1) { await shot(send, 'fail-scroll'); return fail('thumbnails vanished after scrolling'); }

  // 3) A real double-click must open the viewer and decode the original.
  const rect = await ev("(() => { const el = document.querySelector('[data-thumbnail-container]'); const r = el.getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }; })()");
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: rect.x, y: rect.y });
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: rect.x, y: rect.y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: rect.x, y: rect.y, button: 'left', clickCount: 1 });
  await sleep(150);
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x: rect.x, y: rect.y, button: 'left', clickCount: 2 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: rect.x, y: rect.y, button: 'left', clickCount: 2 });
  await sleep(3500);
  const viewer = await ev(`(() => ({
    toolbar: !!document.querySelector('#responsiveDiv'),
    bigImg: [...document.querySelectorAll('img')].reduce((m, i) => Math.max(m, i.naturalWidth), 0),
  }))()`);
  log(`viewer: ${JSON.stringify(viewer)}`);
  await shot(send, 'viewer');
  if (!viewer || viewer.__err) return fail('viewer probe failed: ' + JSON.stringify(viewer));
  if (!viewer.toolbar) return fail('viewer toolbar did not appear after double-click');
  if (!(viewer.bigImg > MIN_WIDTH)) return fail(`viewer largest image naturalWidth=${viewer.bigImg} <= ${MIN_WIDTH} (original not decoded)`);

  // 4) Console errors fail the gate whole - no keyword filtering.
  if (errors.length) {
    for (const e of errors.slice(0, 10)) console.error('  [console-error]', e);
    return fail(`${errors.length} unclassified console error(s)`);
  }

  console.log('[smoke] OK — grid renders, survives scrolling, viewer decodes the original, console clean');
  } finally {
    cleanup();
    await restore();
  }
}

async function shot(send, name) {
  try {
    const r = await send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(join(OUT_DIR, `${name}.png`), Buffer.from(r.data, 'base64'));
    log(`screenshot build/smoke/${name}.png`);
  } catch { /* screenshots are best-effort evidence */ }
}

main().catch((e) => { console.error('[smoke] FAILED:', e.message); process.exit(1); });
