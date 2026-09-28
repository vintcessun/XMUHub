#!/usr/bin/env node
// Static checks for the no-build web frontend (run by CI, and locally: node scripts/check-web.mjs).
//
//  1. Every first-party script (web/assets/**, worker/) parses (node --check).
//  2. Every relative import resolves to a file, and every name imported from it is exported
//     by it. A page importing a function its app.js lacks breaks the whole page.
//  3. Pages reference only /assets/… and /vendor/… files that exist, have a <main> and one
//     page module script (the soft-navigation router needs both), and so do scripts that
//     name /assets/… files in strings.
//
// No dependencies: plain Node (22+).

import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';

const root = resolve(process.argv[2] || join(dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')), '..'));
const web = join(root, 'web');
const errors = [];
const rel = (p) => relative(root, p).split(sep).join('/');
const fail = (file, msg) => errors.push(`${rel(file)}: ${msg}`);

function walk(dir, keep, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, keep, out);
    else if (keep(p)) out.push(p);
  }
  return out;
}

const scripts = [
  ...walk(join(web, 'assets'), (p) => /\.m?js$/.test(p)),
  ...(existsSync(join(root, 'worker')) ? walk(join(root, 'worker'), (p) => /\.m?js$/.test(p)) : []),
];
const pages = readdirSync(web).filter((n) => n.endsWith('.html')).map((n) => join(web, n));

// ---------------------------------------------------------------- 1. syntax

// Checked as .mjs copies: for a .js file Node guesses the module type, and with that guess
// `node --check` lets a module with a syntax error through (seen on Node 24).
const tmp = mkdtempSync(join(tmpdir(), 'xmuhub-check-'));
scripts.forEach((f, i) => {
  const copy = join(tmp, `${i}.mjs`);
  writeFileSync(copy, readFileSync(f));
  const r = spawnSync(process.execPath, ['--check', copy], { encoding: 'utf8' });
  if (r.status !== 0) fail(f, `syntax error\n${(r.stderr || '').trim().split('\n').slice(1, 6).join('\n')}`);
});
rmSync(tmp, { recursive: true, force: true });

// ---------------------------------------------------------------- 2. imports and exports

const stripComments = (src) => src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:\\'"`])\/\/.*$/gm, '$1');

function exportsOf(file) {
  const src = stripComments(readFileSync(file, 'utf8'));
  const names = new Set();
  for (const m of src.matchAll(/\bexport\s+(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)/g)) names.add(m[1]);
  for (const m of src.matchAll(/\bexport\s+(?:const|let|var|class)\s+([A-Za-z_$][\w$]*)/g)) names.add(m[1]);
  for (const m of src.matchAll(/\bexport\s+default\b/g)) names.add('default');
  for (const m of src.matchAll(/\bexport\s*\{([^}]*)\}/g)) {
    for (const part of m[1].split(',').map((s) => s.trim()).filter(Boolean)) names.add(part.split(/\s+as\s+/).pop().trim());
  }
  return names;
}

const exportCache = new Map();
const exported = (f) => exportCache.get(f) ?? exportCache.set(f, exportsOf(f)).get(f);

for (const f of scripts) {
  const src = stripComments(readFileSync(f, 'utf8'));
  // import { a, b as c } from './x.js' / import d from … / import * as n from … / export { a } from …
  for (const m of src.matchAll(/\b(import|export)\s*([^'";]*?)\s*from\s*['"]([^'"]+)['"]/g)) {
    const [, kind, clause, spec] = m;
    if (!spec.startsWith('.')) continue;
    const target = resolve(dirname(f), spec.split('?')[0]);
    if (!existsSync(target)) { fail(f, `imports ${spec}, which does not exist`); continue; }
    const names = [];
    const braces = /\{([^}]*)\}/.exec(clause);
    if (braces) for (const part of braces[1].split(',').map((s) => s.trim()).filter(Boolean)) names.push(part.split(/\s+as\s+/)[0].trim());
    const def = kind === 'import' && /^([A-Za-z_$][\w$]*)\s*(,|$)/.exec(clause.trim());
    if (def) names.push('default');
    const have = exported(target);
    for (const n of names) if (!have.has(n)) fail(f, `imports { ${n} } from ${spec}, but ${rel(target)} does not export it`);
  }
  // import('./x.js') with a literal path
  for (const m of src.matchAll(/\bimport\s*\(\s*['"](\.[^'"]+)['"]\s*\)/g)) {
    if (!existsSync(resolve(dirname(f), m[1].split('?')[0]))) fail(f, `import('${m[1]}') points at a missing file`);
  }
}

// ---------------------------------------------------------------- 3. pages and asset paths

const exists = (urlPath) => existsSync(join(web, urlPath.split('?')[0]));

for (const f of pages) {
  const html = readFileSync(f, 'utf8');
  for (const m of html.matchAll(/\b(?:src|href)\s*=\s*["'](\/(?:assets|vendor)\/[^"'#]+)["']/g)) {
    if (!exists(m[1])) fail(f, `references ${m[1]}, which does not exist`);
  }
  const modules = [...html.matchAll(/<script\b[^>]*\btype\s*=\s*["']module["'][^>]*\bsrc\s*=\s*["']([^"']+)["']/g)];
  if (modules.length !== 1) fail(f, `needs exactly one page module <script type="module" src=…> (found ${modules.length})`);
  if (!/<main[\s>]/.test(html)) fail(f, 'has no <main> (the soft-navigation router swaps it)');
  if (!/<title>[^<]+<\/title>/.test(html)) fail(f, 'has no <title>');
}

for (const f of scripts.filter((p) => p.startsWith(join(web, 'assets')))) {
  const src = readFileSync(f, 'utf8');
  for (const m of src.matchAll(/["'`(](\/assets\/[A-Za-z0-9_./-]+\.[A-Za-z0-9]+)["'`)]/g)) {
    if (!exists(m[1])) fail(f, `names ${m[1]}, which does not exist`);
  }
}

// ---------------------------------------------------------------- result

if (errors.length) {
  console.error(`web checks failed (${errors.length}):\n\n${errors.map((e) => `- ${e}`).join('\n')}`);
  process.exit(1);
}
console.log(`web checks passed: ${scripts.length} scripts, ${pages.length} pages`);
