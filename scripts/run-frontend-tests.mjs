// Frontend aggregate test runner (SOP-上游同步升级.md §7-③).
//
// Runs every `test:*` domain script from src-vite/package.json in sequence,
// collects results, and fails if any domain fails. `test:quarantine` is
// excluded on purpose: it lists legacy harnesses that are known-red against
// refactored source (stale extraction anchors, not product bugs) and is
// tracked for repair in the SOP backlog. tests/orphanGuard.test.mjs keeps
// that honest: every tests/*.test.mjs must be reachable from some script.
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const viteRoot = join(here, "..", "src-vite");
const pkg = JSON.parse(readFileSync(join(viteRoot, "package.json"), "utf8"));

const domains = Object.keys(pkg.scripts)
  .filter((k) => /^test:/.test(k) && k !== "test:quarantine")
  .sort();

const manager = process.env.npm_execpath || "npm";
const results = [];
for (const name of domains) {
  const res = spawnSync(manager, ["run", name], {
    cwd: viteRoot,
    stdio: ["ignore", "inherit", "inherit"],
    shell: process.platform === "win32",
    env: process.env,
  });
  results.push({ name, ok: res.status === 0 });
}

const failed = results.filter((r) => !r.ok);
console.log("");
for (const r of results) console.log(`${r.ok ? "PASS" : "FAIL"}  ${r.name}`);
console.log(`\n${results.length - failed.length}/${results.length} test domains passed` +
  (failed.length ? `; failed: ${failed.map((f) => f.name).join(", ")}` : ""));
process.exit(failed.length ? 1 : 0);
