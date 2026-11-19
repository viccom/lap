// Self-test for the aggregate runner (scripts/run-frontend-tests.mjs).
//
// The runner once spawned `npm_execpath` (npm-cli.js) through a shell on
// Windows: cmd.exe did not execute the .js file, every domain exited 0 and
// the gate reported "35/35 test domains passed" while not a single test had
// run. A gate that cannot fail is worse than no gate, so the spawn path is
// pinned here: a failing domain MUST make the runner exit 1, and a passing
// domain MUST run its script.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const runner = fileURLToPath(new URL('../../scripts/run-frontend-tests.mjs', import.meta.url));

/** A throwaway package with one passing and one failing domain. */
function fixture() {
  const dir = mkdtempSync(join(tmpdir(), 'lappp-aggregate-'));
  writeFileSync(join(dir, 'ok.mjs'), 'console.log("ran-ok");\n');
  writeFileSync(join(dir, 'bad.mjs'), 'console.log("ran-bad");\nprocess.exit(3);\n');
  writeFileSync(join(dir, 'package.json'), JSON.stringify({
    name: 'aggregate-fixture',
    private: true,
    scripts: {
      'test:ok': 'node ok.mjs',
      'test:bad': 'node bad.mjs',
    },
  }, null, 2));
  return dir;
}

function runAggregate(dir) {
  const res = spawnSync(process.execPath, [runner], {
    cwd: dir,
    encoding: 'utf8',
    env: { ...process.env, LAP_TEST_PKG: join(dir, 'package.json'), LAP_TEST_ROOT: dir },
  });
  return { status: res.status, stdout: res.stdout || '', stderr: res.stderr || '' };
}

test('a failing domain makes the aggregate runner exit non-zero', () => {
  const dir = fixture();
  const res = runAggregate(dir);
  assert.equal(res.status, 1, `expected exit 1, got ${res.status}\n${res.stdout}\n${res.stderr}`);
  assert.match(res.stdout, /FAIL {2}test:bad/);
  assert.match(res.stdout, /1\/2 test domains passed/);
  assert.match(res.stdout, /failed: test:bad/);
});

test('the aggregate runner actually executes the domain scripts', () => {
  const dir = fixture();
  const res = runAggregate(dir);
  // Both scripts print; if the runner had swallowed the spawn, neither
  // marker would appear in the output.
  assert.match(res.stdout, /ran-ok/);
  assert.match(res.stdout, /ran-bad/);
});

test('an all-green aggregate exits zero and reports every domain', () => {
  const dir = mkdtempSync(join(tmpdir(), 'lappp-aggregate-'));
  writeFileSync(join(dir, 'ok.mjs'), 'console.log("ran-ok");\n');
  writeFileSync(join(dir, 'package.json'), JSON.stringify({
    name: 'aggregate-fixture',
    private: true,
    scripts: { 'test:ok': 'node ok.mjs' },
  }, null, 2));
  const res = runAggregate(dir);
  assert.equal(res.status, 0, res.stdout + res.stderr);
  assert.match(res.stdout, /PASS {2}test:ok/);
  assert.match(res.stdout, /1\/1 test domains passed/);
});
