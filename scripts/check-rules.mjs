#!/usr/bin/env node
// Guards for the hard rules in AGENTS.md that a compiler can't see (run by CI, and locally:
// node scripts/check-rules.mjs).
//
//  - §1: code that fetches or streams request/response bodies lives only in the files listed
//    in scripts/allowed-network-code.txt; anything new needs the owner's review.
//  - §4: the storage backend trait has no delete (material files are never deleted).
//  - §5: nothing under .secrets/ is tracked by git.

import { execFileSync } from 'node:child_process';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';

const root = resolve(dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')), '..');
const errors = [];
const rel = (p) => relative(root, p).split(sep).join('/');

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) { if (name !== 'target') walk(p, out); } else if (p.endsWith('.rs')) out.push(p);
  }
  return out;
}

// §1 network code only where reviewed
const FETCH = /reqwest::|bytes_stream|\.bytes\(\)|Body::from_stream|StreamBody|\.chunk\(\)/;
const allowed = new Set(readFileSync(join(root, 'scripts/allowed-network-code.txt'), 'utf8')
  .split('\n').map((l) => l.replace(/#.*/, '').trim()).filter(Boolean));
for (const f of walk(join(root, 'crates'))) {
  const r = rel(f);
  const lines = readFileSync(f, 'utf8').split('\n');
  const hit = lines.findIndex((l) => FETCH.test(l.replace(/\/\/.*$/, '')));
  if (hit >= 0 && !allowed.has(r)) {
    errors.push(`${r}:${hit + 1}: uses network / body-streaming APIs (${lines[hit].trim().slice(0, 80)}).\n  Material files must never pass through the server (AGENTS.md §1). If this is metadata only, ask the owner to add the file to scripts/allowed-network-code.txt.`);
  }
}

// §4 no delete in the storage trait
const storage = readFileSync(join(root, 'crates/xmuhub-core/src/storage/mod.rs'), 'utf8');
const trait = /pub trait StorageBackend[\s\S]*?\n\}/.exec(storage)?.[0] || '';
if (/\bfn\s+(delete|remove|purge)\w*\s*\(/.test(trait)) {
  errors.push('crates/xmuhub-core/src/storage/mod.rs: StorageBackend has a delete method. Material files are never deleted through the system (AGENTS.md §4).');
}

// §5 secrets never tracked
const tracked = execFileSync('git', ['ls-files', '--', '.secrets'], { cwd: root, encoding: 'utf8' }).trim();
if (tracked) errors.push(`files under .secrets/ are tracked by git (AGENTS.md §5):\n  ${tracked.split('\n').join('\n  ')}`);

if (errors.length) {
  console.error(`rule checks failed (${errors.length}):\n\n${errors.map((e) => `- ${e}`).join('\n')}`);
  process.exit(1);
}
console.log('rule checks passed');
