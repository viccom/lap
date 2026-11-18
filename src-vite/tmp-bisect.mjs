import ts from 'typescript';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
const here = dirname(fileURLToPath(import.meta.url));
const src = fs.readFileSync(join(here, 'src/stores/configStore.js'), 'utf8');
const cfg = ts.readConfigFile(join(here, 'tsconfig.json'), ts.sys.readFile);
const parsed = ts.parseJsonConfigFileContent(cfg.config, ts.sys, here);

// split top-level getters/actions member blocks by 4-space indent at line start
const lines = src.split(/\r?\n/);
const memberStart = [];
for (let i = 0; i < lines.length; i++) {
  if (/^    [A-Za-z_$][\w$]*\s*[(:]/.test(lines[i])) memberStart.push(i);
}
function variant(dropIdxSet) {
  // drop member blocks whose start-line index is in the set (block = from its start line to next member start)
  const kill = new Set();
  for (const idx of dropIdxSet) {
    const pos = memberStart.indexOf(idx);
    const from = idx;
    const to = pos + 1 < memberStart.length ? memberStart[pos + 1] : lines.length;
    for (let i = from; i < to; i++) kill.add(i);
  }
  return lines.filter((_, i) => !kill.has(i)).join('\n');
}
function storeKeys(jsSource) {
  const storePath = join(here, 'src/stores/__cs_probe__.js');
  const probePath = join(here, 'src/stores/__cs_probe_test__.ts');
  fs.writeFileSync(storePath, jsSource.replace("defineStore('configStore'", "defineStore('csProbe'"));
  fs.writeFileSync(probePath, "import { useProbe } from './__cs_probe__';\nexport declare const w: keyof ReturnType<typeof useProbe>;\n".replace('useProbe', 'useConfigStore'));
  // rename export to useConfigStore? original file exports useConfigStore — keep name
  const program = ts.createProgram([probePath], { ...parsed.options, noEmit: true });
  const checker = program.getTypeChecker();
  let keys = null;
  const visit = (node) => {
    if (ts.isVariableStatement(node)) {
      for (const d of node.declarationList.declarations) {
        const t = checker.getTypeAtLocation(d.name);
        keys = t.isUnion() ? t.types.filter((p) => p.isStringLiteral()).map((p) => p.value) : [];
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(program.getSourceFile(probePath));
  fs.rmSync(storePath); fs.rmSync(probePath);
  return keys || [];
}

const target = (k) => k.includes('double') ? k : null; // unused
const full = storeKeys(src);
console.log('FULL keys sample:', full.filter((k) => !k.startsWith('$') && !['main','content','leftPanel','collectionTray','rightPanel','dedup','infoPanel','search','calendar','camera','mediaViewer','video','imageEditor','montage','imageViewer','libraryChangedVersion','systemLocale','settings','_customProperties'].includes(k)).slice(0, 10));
// bisect: find minimal subset of members whose presence breaks inference
// start: test with only getters (drop all action blocks) and only actions (drop getters)
const gettersBlock = memberStart.filter((i) => lines[i].trim().startsWith('externalAppsFor') || ['imageSearchThresholds','similarImageSearchThreshold','smartTagSearchThreshold','similarPhotoGroupingThresholds','faceClusterThresholds','defaultExternalApp'].some(n => lines[i].trim().startsWith(n + ':')));
const actionsBlock = memberStart.filter((i) => !gettersBlock.includes(i));
console.log('getters members:', gettersBlock.length, 'action members:', actionsBlock.length);
console.log('only-getters:', storeKeys(variant(new Set(actionsBlock))).filter(k => !k.startsWith('$')).slice(0, 8));
console.log('only-actions:', storeKeys(variant(new Set(gettersBlock))).filter(k => !k.startsWith('$')).slice(0, 8));
