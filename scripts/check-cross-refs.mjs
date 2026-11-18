// Cross-file contract reconciliation (SOP-上游同步升级.md §3.1).
//
// One-stop sync-day reconciler for the three registries a merge can silently
// break (the v0.4.0 sync shipped a dangling shortcut id exactly this way -
// no textual conflict, contract violated):
//
//   1. Tauri commands: every frontend `invoke('literal')` must be registered
//      in a generate_handler![...] block (src-tauri/src/main.rs; both feature
//      variants are unioned). The thin-client/server bridge parity is pinned
//      separately by lappp-server's bridge_whitelist test.
//   2. i18n keys: every literal `t('a.b')` / `$t('a.b')` call must resolve in
//      src/locales/en.json (vue-i18n fallbackLocale: 'en'). Template calls
//      with a static prefix (`t(`ns.${x}`)`) are namespace-checked instead.
//      Keys behind runtime fallbacks (menuLabel(['k','fallback']) etc.) never
//      reach a literal t() call and are out of scope here.
//   3. Shortcut action ids: every literal referenced through the shortcut API
//      must be registered in SHORTCUTS (mirrors tests/shortcutActionIdType
//      + shortcutsLayout, kept here so this script alone covers a sync).
//
// Exit 1 on any dangling reference. Run via `npm run test:crossrefs`
// (part of the aggregate test gate).
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative } from 'node:path';

const repo = join(dirname(fileURLToPath(import.meta.url)), '..');
const viteRoot = join(repo, 'src-vite');
const rel = (p) => relative(repo, p).replace(/\\/g, '/');
// typescript lives in src-vite/node_modules (pnpm); resolve it from there so
// this script runs from the repo root without a node_modules there.
const ts = createRequire(join(viteRoot, 'package.json'))('typescript');

function walk(dir, exts) {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      if (/node_modules|dist$/.test(name)) return [];
      return walk(full, exts);
    }
    return exts.some((e) => name.endsWith(e)) ? [full] : [];
  });
}

const failures = [];
const fail = (kind, where, message) => failures.push(`${kind}  ${where}  ${message}`);

// ---------------------------------------------------------------------------
// 1. Tauri command registry vs frontend invoke literals
// ---------------------------------------------------------------------------
const rustMain = readFileSync(join(repo, 'src-tauri', 'src', 'main.rs'), 'utf8');
const registered = new Set();
for (const m of rustMain.matchAll(/generate_handler!\[([\s\S]*?)\]/g)) {
  for (const entry of m[1].matchAll(/(?:\w+::)?([a-zA-Z_]\w*)\s*,/g)) registered.add(entry[1]);
}
if (registered.size < 100) {
  console.error('[cross-refs] registry extraction looks wrong: only ' + registered.size + ' commands found');
  process.exit(2);
}

const frontendFiles = walk(join(viteRoot, 'src'), ['.ts', '.js', '.vue']);
const invoked = [];
for (const file of frontendFiles) {
  const text = readFileSync(file, 'utf8');
  for (const m of text.matchAll(/invoke(?:<[^>]*>)?\(\s*['"]([a-zA-Z_]\w*)['"]/g)) {
    const line = text.slice(0, m.index).split(/\r?\n/).length;
    invoked.push({ file, line, name: m[1] });
  }
}
for (const call of invoked) {
  if (!registered.has(call.name)) {
    fail('invoke ', `${rel(call.file)}:${call.line}`, `command '${call.name}' is not registered in generate_handler`);
  }
}

// ---------------------------------------------------------------------------
// 2. i18n literal keys vs en.json
// ---------------------------------------------------------------------------
const en = JSON.parse(readFileSync(join(viteRoot, 'src', 'locales', 'en.json'), 'utf8'));
const flatKeys = new Set();
(function flatten(node, prefix) {
  for (const [k, v] of Object.entries(node)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object' && !Array.isArray(v)) flatten(v, key);
    else flatKeys.add(key);
  }
})(en, '');

// Keys that resolve through runtime fallbacks rather than the locale file.
// Each entry must name the mechanism that covers it.
const I18N_ALLOWLIST = new Map();

for (const file of frontendFiles) {
  const text = readFileSync(file, 'utf8');
  for (const m of text.matchAll(/\W(?:\$t|t|tc|te)\(\s*(['"])((?:(?!\1)[^\\${])*)\1/g)) {
    const key = m[2];
    if (!key) continue;
    const line = text.slice(0, m.index).split(/\r?\n/).length;
    if (/^\s*\+/.test(text.slice(m.index + m[0].length))) {
      // `t('prefix.' + x)` / `t('prefix_' + x)`: dynamic concat - require at
      // least one real key carrying that static prefix.
      if (![...flatKeys].some((k) => k.startsWith(key))) {
        fail('i18n   ', `${rel(file)}:${line}`, `dynamic prefix '${key}' matches no key in locales/en.json`);
      }
      continue;
    }
    if (flatKeys.has(key) || I18N_ALLOWLIST.has(key)) continue;
    const note = I18N_ALLOWLIST.get(key) ? ` (allowlisted: ${I18N_ALLOWLIST.get(key)})` : '';
    fail('i18n   ', `${rel(file)}:${line}`, `key '${key}' missing from locales/en.json${note}`);
  }
  // Template calls: same prefix rule for the static part before ${...}.
  for (const m of text.matchAll(/\W(?:\$t|t)\(\s*`([^`$]*)\$\{/g)) {
    const prefix = m[1];
    if (!prefix) continue;
    if ([...flatKeys].some((k) => k.startsWith(prefix))) continue;
    const line = text.slice(0, m.index).split(/\r?\n/).length;
    fail('i18n   ', `${rel(file)}:${line}`, `template prefix '${prefix}…' matches no key in locales/en.json`);
  }
}

// ---------------------------------------------------------------------------
// 3. Shortcut action ids vs the SHORTCUTS registry
// ---------------------------------------------------------------------------
const shortcutsPath = join(viteRoot, 'src', 'common', 'shortcuts.ts');
const registryExports = {};
new Function('exports', ts.transpile(readFileSync(shortcutsPath, 'utf8'), {
  target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
}))(registryExports);
const shortcutIds = new Set(registryExports.SHORTCUTS.map((s) => s.id));

const SHORTCUT_CALL = /\W(?:shortcut|getShortcutLabel|getShortcutLabels|getShortcut|matchesActionShortcut)\(\s*['"]([a-zA-Z][\w.]*)['"]/g;
let shortcutRefs = 0;
for (const file of frontendFiles) {
  if (file.endsWith('shortcuts.ts')) continue;
  const text = readFileSync(file, 'utf8');
  for (const m of text.matchAll(SHORTCUT_CALL)) {
    shortcutRefs++;
    if (shortcutIds.has(m[1])) continue;
    const line = text.slice(0, m.index).split(/\r?\n/).length;
    fail('shortct', `${rel(file)}:${line}`, `action id '${m[1]}' not registered in SHORTCUTS`);
  }
}

// ---------------------------------------------------------------------------
console.log(`[cross-refs] registry: ${registered.size} commands, ${flatKeys.size} i18n keys, ${shortcutIds.size} shortcut ids`);
console.log(`[cross-refs] references: ${invoked.length} invoke, ${shortcutRefs} shortcut`);

if (failures.length) {
  console.error(`[cross-refs] FAIL — ${failures.length} dangling reference(s):`);
  for (const f of failures.slice(0, 40)) console.error('  ' + f);
  if (failures.length > 40) console.error(`  ... and ${failures.length - 40} more`);
  process.exit(1);
}
console.log('[cross-refs] OK — every referenced command, key and action id resolves');
