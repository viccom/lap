// Load a TypeScript module of src/common into a plain exports object.
//
// The pure-logic modules are .ts with extensionless imports, and node's ESM
// resolver cannot follow either the extensionless specifier or the '@/'
// alias. The established convention in this repo (see tests/tagTree.test.mjs)
// is: read the source, transpile it to CommonJS and evaluate it. This helper
// adds the missing piece - a `require` shim that does the same for relative
// imports, so a module that imports a sibling .ts module is testable too.
//
// Plain JavaScript on purpose: it runs on every node version the CI uses
// (node 20 cannot execute .ts files at all).
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, resolve as resolvePath } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';

const nodeRequire = createRequire(import.meta.url);
const cache = new Map();

/** Resolve an extensionless specifier to a real file. */
function resolveModulePath(base) {
  for (const candidate of [`${base}.ts`, `${base}.js`, `${base}.mjs`, `${base}/index.ts`]) {
    try {
      readFileSync(candidate);
      return candidate;
    } catch {
      // try the next candidate
    }
  }
  throw new Error(`cannot resolve module: ${base}`);
}

/**
 * Load a TypeScript module and return its exports.
 * @param {string | URL} target an absolute path or a file URL of the module
 * @returns {Record<string, any>} the module exports
 */
export function loadTsModule(target) {
  const filePath = target instanceof URL ? fileURLToPath(target) : resolvePath(target);
  const cached = cache.get(filePath);
  if (cached) return cached;

  const source = readFileSync(filePath, 'utf8');
  const compiled = ts.transpile(source, {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.CommonJS,
  });

  const module = { exports: {} };
  const dir = dirname(filePath);
  const localRequire = (specifier) => {
    if (!specifier.startsWith('.') && !specifier.startsWith('/')) return nodeRequire(specifier);
    return loadTsModule(resolveModulePath(resolvePath(dir, specifier)));
  };

  const evaluate = new Function('exports', 'require', 'module', '__filename', '__dirname', compiled);
  evaluate(module.exports, localRequire, module, filePath, dir);
  cache.set(filePath, module.exports);
  return module.exports;
}
