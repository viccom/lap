// Locale union resolver: for each conflicted locale file, take upstream (v0.4.0)
// as the ordered base, deep-append fork-only keys, write back with the file's
// original CRLF convention. Reports per-file key deltas.
//
// Usage: node scripts/merge-locales.mjs   (run from repo root, during a merge)
import { execSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';

const LOCALES = ['cs', 'de', 'en', 'es', 'fr', 'hu', 'it', 'ja', 'nl', 'pl', 'pt', 'ru', 'uk', 'vi', 'zh-CN', 'zh-TW'];

function stage(side, path) {
  return execSync(`git show :${side}:${path}`, { maxBuffer: 64 * 1024 * 1024 }).toString('utf8');
}

function countKeys(obj) {
  let n = 0;
  for (const v of Object.values(obj)) {
    if (v && typeof v === 'object') n += countKeys(v);
    else n += 1;
  }
  return n;
}

// Merge fork-only keys onto the upstream base. Objects recurse; leaves append.
function union(base, forkSide) {
  let added = 0;
  for (const [k, v] of Object.entries(forkSide)) {
    if (!(k in base)) {
      base[k] = v;
      added += countKeys(v);
    } else if (v && typeof v === 'object' && base[k] && typeof base[k] === 'object') {
      added += union(base[k], v);
    }
    // Key exists on both sides with a leaf value: keep upstream's (upstream is
    // the canonical translator; fork rarely edits existing leaf strings).
  }
  return added;
}

let report = [];
for (const loc of LOCALES) {
  const path = `src-vite/src/locales/${loc}.json`;
  const ours = JSON.parse(stage(2, path));   // fork (develop)
  const theirs = JSON.parse(stage(3, path)); // upstream v0.4.0
  const baseCount = countKeys(theirs);
  const added = union(theirs, ours);
  // 2-space indent, CRLF line endings to match the repo convention.
  const out = JSON.stringify(theirs, null, 2).replace(/\n/g, '\r\n') + '\r\n';
  writeFileSync(path, out, 'utf8');
  report.push(`${loc}: upstream ${baseCount} keys + fork-only ${added} appended`);
}
console.log(report.join('\n'));
console.log('DONE');
