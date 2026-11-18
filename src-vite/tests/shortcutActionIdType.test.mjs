// Type-contract test for the shortcut registry, runnable under plain
// `node --test` (no vue-tsc needed): it builds a real TypeScript program over
// src/common/shortcuts.ts plus an in-memory probe file, and pins the
// ShortcutActionId <-> SHORTCUTS relationship in BOTH directions.
//
// Background (2026-11-18 incident): the v0.4.0 sync kept a dangling
// `shortcut('file.moveToFolder')` reference after upstream deleted the action.
// Nothing in the pipeline type-checked the file, and the throw happened inside
// a render, blanking the grid and the media viewer (Vue replaces a component
// root with a comment node when its render throws).
//
// What this guards:
//   1. shortcuts.ts type-checks cleanly (0 diagnostics) — registry refactors
//      cannot silently break assignability;
//   2. the in-memory probe compiles: registered ids are referenceable,
//      unregistered ids ('file.moveToFolder', any typo) are rejected —
//      enforced via @ts-expect-error, independent of diagnostic message text;
//   3. the ShortcutActionId union equals the runtime registry ids exactly —
//      no drift, and no collapse to plain `string` (which would make every
//      typo legal).
// (.vue consumers still need the full vue-tsc gate; SOP-上游同步升级.md §7-②.)
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import ts from "typescript";

const testsDir = dirname(fileURLToPath(import.meta.url));
const viteRoot = dirname(testsDir);
const shortcutsPath = join(viteRoot, "src", "common", "shortcuts.ts");
// A real-looking path in the same directory so './shortcuts' resolves; the
// file itself never exists on disk (served from memory by the host below).
const probePath = join(viteRoot, "src", "common", "shortcuts-contract-probe.ts");

const PROBE_SOURCE = `import { getShortcutLabel, SHORTCUTS, type ShortcutActionId } from './shortcuts';

// Every registered id must be referenceable (registry \\u2286 type).
const all: ShortcutActionId[] = SHORTCUTS.map((s) => s.id);

// Unregistered ids must be rejected — the v0.4.0 sync shipped a dangling
// 'file.moveToFolder' and nothing type-checked it.
// @ts-expect-error incident id must stay rejected
getShortcutLabel('file.moveToFolder');
// @ts-expect-error an arbitrary typo must fail
getShortcutLabel('file.bogus');

// A registered id must compile without complaint.
getShortcutLabel('file.copy');

export { all };
`;

function buildProgram() {
  const cfg = ts.readConfigFile(join(viteRoot, "tsconfig.json"), ts.sys.readFile);
  assert.ok(!cfg.error, "tsconfig.json must parse");
  const parsed = ts.parseJsonConfigFileContent(cfg.config, ts.sys, viteRoot);
  const host = ts.createCompilerHost(parsed.options);
  const isProbe = (fileName) => fileName.replace(/\\/g, "/").endsWith("shortcuts-contract-probe.ts");
  const baseGetSourceFile = host.getSourceFile;
  host.getSourceFile = (fileName, versionOrOptions, onError, shouldCreate) => {
    if (isProbe(fileName)) {
      return ts.createSourceFile(fileName, PROBE_SOURCE, ts.ScriptTarget.ES2022, true);
    }
    return baseGetSourceFile(fileName, versionOrOptions, onError, shouldCreate);
  };
  const baseFileExists = host.fileExists;
  host.fileExists = (fileName) => isProbe(fileName) || baseFileExists(fileName);
  return ts.createProgram(
    [shortcutsPath, probePath],
    { ...parsed.options, noEmit: true },
    host,
  );
}

function diagnostics(program) {
  return ts.getPreEmitDiagnostics(program).map((d) => {
    const file = d.file
      ? (d.file.fileName.split(/[\\/]src-vite[\\/]/)[1] || d.file.fileName)
      : "?";
    const line =
      d.file && typeof d.start === "number"
        ? d.file.getLineAndCharacterOfPosition(d.start).line + 1
        : "?";
    return `${file}:${line} TS${d.code}: ${ts.flattenDiagnosticMessageText(d.messageText, " ")}`;
  });
}

function typeAliasLiteralIds(program, filePath, aliasName) {
  const checker = program.getTypeChecker();
  const source = program.getSourceFile(filePath);
  assert.ok(source, `source file not in program: ${filePath}`);
  let found = null;
  const visit = (node) => {
    if (ts.isTypeAliasDeclaration(node) && node.name.text === aliasName) {
      const type = checker.getTypeAtLocation(node.name);
      const parts = type.isUnion() ? type.types : [type];
      found = parts.filter((p) => p.isStringLiteral()).map((p) => p.value);
    } else {
      ts.forEachChild(node, visit);
    }
  };
  visit(source);
  return found;
}

const registrySource = await readFile(shortcutsPath, "utf8");
const transpiled = ts.transpile(registrySource, {
  target: ts.ScriptTarget.ES2022,
  module: ts.ModuleKind.CommonJS,
});
const moduleExports = {};
new Function("exports", transpiled)(moduleExports);
const registryIds = moduleExports.SHORTCUTS.map((s) => s.id);

test("shortcut registry ids are unique", () => {
  assert.equal(new Set(registryIds).size, registryIds.length, "SHORTCUTS contains duplicate ids");
});

test("shortcuts.ts and the assignability probe type-check cleanly", () => {
  const diags = diagnostics(buildProgram());
  assert.deepEqual(diags, [], [
    "shortcuts.ts must be clean and the probe must prove:",
    "- registered ids are referenceable,",
    "- 'file.moveToFolder' / typo ids are rejected (@ts-expect-error),",
    "otherwise the sync contract is broken.",
  ].join(" "));
});

test("ShortcutActionId equals the runtime registry in both directions", () => {
  const program = buildProgram();
  const typeIds = typeAliasLiteralIds(program, shortcutsPath, "ShortcutActionId");
  assert.ok(Array.isArray(typeIds), "ShortcutActionId must resolve to literal union members");
  assert.deepEqual(
    [...new Set(typeIds)].sort(),
    [...new Set(registryIds)].sort(),
    "ShortcutActionId must be exactly the SHORTCUTS ids (drift or collapse to `string` breaks the sync contract)",
  );
  assert.ok(!typeIds.includes("file.moveToFolder"), "the incident id must not be a legal action id");
});
