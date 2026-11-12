// F1/F2 实验台端到端 UI 冒烟：通过 WebView2 的 CDP 调试端口驱动真实 Tauri 窗口。
// 用法（先以 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 启动 Lap.exe）：
//   LAP_VL_API_KEY=... node docs/dev/ui-smoke.mjs [--explore]
// 密钥只经环境变量注入本进程，不写入任何文件。
//
// ⚠ 适用范围：--remote-debugging-port 会杀死所有「运行时创建的子 webview 窗口」（设置窗/
// 实验台独立窗/图片查看器——同 UDF 两个浏览器进程抢同端口，子 webview 起来即死，只留僵尸注册）。
// 本脚本只测主窗口内流程（直接导航 /vl-lab），不涉及子窗口；子窗口入口验证用 UIA
// （UIAutomation 按钮名 Invoke + OS 窗口枚举），勿在开 CDP 的实例上测子窗口。
const BASE = 'http://127.0.0.1:9222';
const GATEWAY_URL = process.env.LAP_VL_GATEWAY_URL || 'http://192.168.199.8:3425';
const sleep = ms => new Promise(r => setTimeout(r, ms));

function log(...a) { console.log('[smoke]', ...a); }
function fail(msg) { console.error('[smoke] FAIL:', msg); process.exitCode = 1; }

async function targets() {
  const res = await fetch(`${BASE}/json/list`);
  return res.json();
}

class Cdp {
  constructor(ws) { this.ws = ws; this.id = 0; this.pending = new Map();
    ws.addEventListener('message', ev => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result);
      }
    });
  }
  static async open(url) {
    const ws = new WebSocket(url);
    await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
    return new Cdp(ws);
  }
  send(method, params = {}) {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
  async eval(expression, awaitPromise = true) {
    const r = await this.send('Runtime.evaluate', {
      expression, awaitPromise, returnByValue: true,
    });
    if (r.exceptionDetails) throw new Error(`page exception: ${r.exceptionDetails.text} ${r.exceptionDetails.exception?.description || ''}`);
    return r.result?.value;
  }
}

async function attach(urlMatch, what) {
  for (let i = 0; i < 40; i++) {
    const list = await targets();
    const hit = list.find(t => t.type === 'page' && urlMatch(t));
    if (hit) { log(`attached: ${what} -> ${hit.url}`); return Cdp.open(hit.webSocketDebuggerUrl); }
    await sleep(500);
  }
  throw new Error(`target not found: ${what}`);
}

const clickByText = (texts) => `(() => {
  for (const want of ${JSON.stringify(texts)}) {
    // 找「自身文本包含目标且子节点不含目标」的最深层元素：点击会冒泡到 Vue 的处理器
    const els = [...document.querySelectorAll('body *')].filter(el => {
      if (el.closest('script,style')) return false;
      const t = (el.textContent || '').trim();
      if (!t || t.length > 60 || !t.includes(want)) return false;
      return [...el.children].every(c => !(c.textContent || '').includes(want));
    });
    if (els.length) { els[0].click(); return 'clicked:' + els[0].textContent.trim().slice(0, 30); }
  }
  return null;
})()`;

const textOf = sel => `(() => {
  const el = document.querySelector(${JSON.stringify(sel)});
  return el ? el.textContent.trim() : null;
})()`;

const setInput = (value) => `(() => {
  const inputs = [...document.querySelectorAll('input[type="text"], input:not([type])')];
  const box = inputs.find(i => (i.placeholder || '').length > 0) || inputs[0];
  if (!box) return null;
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  set.call(box, ${JSON.stringify(value)});
  box.dispatchEvent(new Event('input', { bubbles: true }));
  return 'typed';
})()`;

// ---- 主流程 ----
const explore = process.argv.includes('--explore');

// A. 主窗口：确认 Tauri IPC、配置网关
const main = await attach(t => !/devtools/.test(t.url), 'main window');
const hasTauri = await main.eval(`typeof window.__TAURI_INTERNALS__ === 'object'`);
if (!hasTauri) fail('主窗口没有 __TAURI_INTERNALS__（CDP 附着到了错误目标）');
const cfg = await main.eval(`window.__TAURI_INTERNALS__.invoke('get_vl_config')`);
log('get_vl_config =>', JSON.stringify(cfg));
if (!cfg || !cfg.configured) {
  if (!process.env.LAP_VL_API_KEY) { fail('网关未配置且未提供 LAP_VL_API_KEY'); }
  await main.eval(`window.__TAURI_INTERNALS__.invoke('set_vl_config', { gatewayUrl: ${JSON.stringify(GATEWAY_URL)}, apiKey: ${JSON.stringify(process.env.LAP_VL_API_KEY)}, model: 'auto/deepseek-v4.1-flash' })`);
  const cfg2 = await main.eval(`window.__TAURI_INTERNALS__.invoke('get_vl_config')`);
  log('set_vl_config 后 =>', JSON.stringify(cfg2));
  if (!cfg2?.configured) fail('配置网关后 configured 仍为 false');
}

// B. 入口路径已单独验证（窗口 shell 创建成功、无错误，行为与上游 settings 子窗口一致）。
//    功能验证在主窗口直接导航 /vl-lab（同组件同路由；调试构建的子窗口 webview 不受 CDP 驱动）。
const lab = main;
await lab.eval(`location.assign('/vl-lab')`);
await sleep(2500);

if (explore) {
  const dump = await lab.eval(`(() => ({
    buttons: [...document.querySelectorAll('button')].map(b => b.textContent.trim()).filter(Boolean),
    inputs: [...document.querySelectorAll('input')].map(i => ({ type: i.type, placeholder: i.placeholder })),
    selects: [...document.querySelectorAll('select')].map(s => [...s.options].map(o => o.textContent.trim()).slice(0, 5)),
    bodyStart: document.body.textContent.replace(/\\s+/g, ' ').slice(0, 600),
  }))()`);
  console.log(JSON.stringify(dump, null, 2));
  process.exit(0);
}

// C. 状态条：网关 + 索引一致性
const statusText = await lab.eval(textOf('body'));
const gatewayOk = /已配置|configured/i.test(statusText);
log('状态条(网关):', gatewayOk ? '已配置 ✓' : '未配置 ✗');
if (!gatewayOk) fail('实验台状态条未显示网关已配置');
const m = statusText.match(/索引\s*(\d+)\s*\/\s*文件\s*(\d+)/);
log('状态条(索引):', m ? `${m[1]}/${m[2]}` : '未找到计数');
if (!m) fail('状态条未显示索引计数');
const fts = await lab.eval(`window.__TAURI_INTERNALS__.invoke('fts_status')`);
log('fts_status =>', JSON.stringify(fts));
if (!fts || fts.documents !== fts.files) fail(`索引不一致: ${JSON.stringify(fts)}`);
if (m && String(m[1]) !== String(fts.documents)) fail(`状态条计数(${m[1]})与 fts_status(${fts.documents})不一致`);

// D. F2 精确检索（真库，搜 F1 标注过的词）
log('--- F2 精确检索 ---');
const typed = await lab.eval(setInput('葡萄'));
if (typed !== 'typed') fail('找不到检索输入框');
const searched = await lab.eval(clickByText(['精确搜索', '搜索', 'Search']));
log('点击检索:', searched);
let hitSummary = null;
for (let i = 0; i < 20; i++) {
  await sleep(600);
  const body = await lab.eval(textOf('body'));
  const mm = body.match(/命中\s*(\d+)\s*个?/);
  if (mm) { hitSummary = mm[0]; break; }
}
log('检索结果摘要:', hitSummary);
if (!hitSummary || !/[1-9]/.test(hitSummary)) fail('精确检索无命中（期望至少 1：F1 标注过的照片）');
const body2 = await lab.eval(textOf('body'));
if (!/图片描述|caption/i.test(body2)) fail('结果未显示命中列来源（图片描述/caption 徽标）');
log('F2 检索: 命中≥1 且带命中列徽标 ✓');

// E. F1 单张理解（真网关）
log('--- F1 图片理解 ---');
await sleep(2500); // 等磁贴自动加载
// 选第一个磁贴（F1 选择器在页首，第一个 img 必属它；真实网关调用路径已由 real_library_e2e
// 与探针验证过，这里允许「已跳过」——幂等路径的 UI 验证同样有效）
const picked = await lab.eval(`(() => {
  const tiles = [...document.querySelectorAll('img')].map(i => i.parentElement).filter(Boolean);
  if (!tiles.length) return null;
  tiles[0].click();
  return 'clicked tile, text=' + (tiles[0].textContent || '').trim().slice(0, 30);
})()`);
log('选择磁贴:', picked);
if (!picked) fail('磁贴列表为空（选择器未加载）');
await sleep(600);
const understood = await lab.eval(clickByText(['理解选中', '理解', 'Understand']));
log('点击理解:', understood);
let done = false, lastBody = '';
for (let i = 0; i < 45; i++) {
  await sleep(2000);
  lastBody = await lab.eval(textOf('body'));
  // 「成功 N」或「跳过 N」都算 run 完成（跳过 = 幂等路径的 UI 验证）
  if ((/成功\s*[1-9]/.test(lastBody) || /跳过\s*[1-9]/.test(lastBody)) && !/运行中|执行中/.test(lastBody)) { done = true; break; }
  if (/失败\s*[1-9]/.test(lastBody) && !/运行中/.test(lastBody)) break;
}
log('F1 运行完成:', done);
if (!done) fail(`F1 未在 90 秒内完成。页面片段: ${lastBody.replace(/\s+/g, ' ').slice(0, 400)}`);
const captionOk = await lab.eval(`(() => {
  const t = document.body.textContent || '';
  // 结果行：已标注（新调用成功）或已跳过（幂等）；描述/标签内容段；原始返回溯源入口
  const annotated = /已标注/.test(t);
  const skipped = /已跳过/.test(t);
  const rawEntry = /原始返回/.test(t);
  const descLabel = /描述/.test(t) && /标签/.test(t);
  return { annotated, skipped, rawEntry, descLabel };
})()`);
log('结果区要素:', JSON.stringify(captionOk));
if (!captionOk?.annotated && !captionOk?.skipped) fail('结果区缺少「已标注/已跳过」行');
if (!captionOk?.descLabel) fail('结果区缺少「描述/标签」内容段');
if (!captionOk?.rawEntry) fail('结果区缺少「原始返回」溯源入口');

console.log(process.exitCode ? '\n[smoke] 有失败项' : '\n[smoke] 全部通过');
process.exit(process.exitCode ? 1 : 0); // 不加这句，未关闭的 CDP WebSocket 会把 node 挂住
