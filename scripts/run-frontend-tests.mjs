// Frontend aggregate test runner (SOP-上游同步升级.md §7-③).
//
// Runs every `test:*` domain script from src-vite/package.json in sequence,
// collects results, and fails if any domain fails. `test:quarantine` is
// excluded on purpose: it lists legacy harnesses that are known-red against
// refactored source (stale extraction anchors, not product bugs) and is
// tracked for repair in the SOP backlog. tests/orphanGuard.test.mjs keeps
// that honest: every tests/*.test.mjs must be reachable from some script.
//
// The domains are spawned as `node <args>` DIRECTLY, never through npm:
// spawning `npm_execpath` (npm-cli.js) through a shell on Windows does not
// execute the .js file at all - cmd.exe hands it to the file association, the
// command exits 0 and every domain reports PASS without running a single
// test. That false green is exactly what the runner exists to prevent, so
// the spawn path is covered by tests/aggregateRunner.test.mjs.
//
// Environment:
//   LAP_TEST_PKG   package.json to read the domains from (self-test)
//   LAP_TEST_ROOT  working directory for the domains (self-test)
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const viteRoot = join(here, "..", "src-vite");
const pkgPath = process.env.LAP_TEST_PKG || join(viteRoot, "package.json");
const workDir = process.env.LAP_TEST_ROOT || viteRoot;
const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));

const domains = Object.keys(pkg.scripts)
  .filter((k) => /^test:/.test(k) && k !== "test:quarantine")
  .sort();

/** Split a domain command into an executable and its arguments. */
function parseCommand(command) {
  const parts = command.trim().split(/\s+/).filter(Boolean);
  if (parts[0] === "node") {
    return { command: process.execPath, args: parts.slice(1) };
  }
  return null;
}

const results = [];
for (const name of domains) {
  const parsed = parseCommand(pkg.scripts[name]);
  let res;
  if (parsed) {
    res = spawnSync(parsed.command, parsed.args, {
      cwd: workDir,
      stdio: ["ignore", "inherit", "inherit"],
      env: process.env,
    });
  } else {
    // A domain that is not a plain `node ...` invocation: fall back to npm.
    const manager = process.platform === "win32" ? "npm.cmd" : "npm";
    res = spawnSync(manager, ["run", name], {
      cwd: workDir,
      stdio: ["ignore", "inherit", "inherit"],
      shell: process.platform === "win32",
      env: process.env,
    });
  }
  if (res.error) {
    console.error(`[aggregate] ${name} could not be started:`, res.error.message);
  }
  results.push({ name, ok: res.status === 0 });
}

const failed = results.filter((r) => !r.ok);
console.log("");
for (const r of results) console.log(`${r.ok ? "PASS" : "FAIL"}  ${r.name}`);
console.log(`\n${results.length - failed.length}/${results.length} test domains passed` +
  (failed.length ? `; failed: ${failed.map((f) => f.name).join(", ")}` : ""));
process.exit(failed.length ? 1 : 0);
