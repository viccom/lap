// Orphan guard: every tests/*.test.mjs must be reachable from a package.json
// script, either a `test:*` domain (runs in the aggregate) or the explicit
// `test:quarantine` list (known-red legacy harnesses, tracked in
// docs/dev/SOP-上游同步升级.md). Invisible tests are how the v0.4.0 sync
// shipped with green gates while 26 test files never ran anywhere.
//
// The guard also refuses *.test.ts files: node cannot execute them on the
// CI's node 20, so a TypeScript test would silently never run. The two that
// existed (imageNavigator, montageLayout) were ported to .mjs and load their
// subject through tests/helpers/ts-module.mjs.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const testsDir = dirname(fileURLToPath(import.meta.url));
const viteRoot = dirname(testsDir);
const pkg = JSON.parse(await readFile(join(viteRoot, "package.json"), "utf8"));

const referenced = new Set();
for (const cmd of Object.values(pkg.scripts)) {
  for (const m of cmd.matchAll(/tests\/[\w.-]+\.mjs/g)) referenced.add(m[0].replace(/^tests\//, ""));
}

const entries = await readdir(testsDir);
const files = entries.filter((f) => f.endsWith(".test.mjs"));
const orphans = files.filter((f) => !referenced.has(f));
const typescriptTests = entries.filter((f) => f.endsWith(".test.ts"));

const quarantineCmd = pkg.scripts["test:quarantine"] || "";
const quarantined = new Set(
  [...quarantineCmd.matchAll(/tests\/([\w.-]+\.mjs)/g)].map((m) => m[1]),
);
const staleQuarantine = [...quarantined].filter((f) => !files.includes(f));

test("every test file is wired into a script (no silent orphans)", () => {
  assert.deepEqual(orphans, [], [
    "test files not referenced by any package.json script:",
    orphans.join(", "),
    "- add them to a test:* domain, or to test:quarantine if known-red",
  ].join(" "));
});

test("no TypeScript test files: node 20 cannot run them, so they would never run", () => {
  assert.deepEqual(typescriptTests, [], [
    "TypeScript test files found (they would never execute on CI):",
    typescriptTests.join(", "),
    "- port them to .mjs and load the module through tests/helpers/ts-module.mjs",
  ].join(" "));
});

test("quarantine list only names files that exist and are known-red", () => {
  assert.deepEqual(staleQuarantine, [], "test:quarantine references missing files: " + staleQuarantine.join(", "));
  for (const f of quarantined) {
    assert.ok(referenced.has(f), `quarantined file must appear only via test:quarantine: ${f}`);
  }
});
