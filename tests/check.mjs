#!/usr/bin/env node
// Every skill must load in every agent, and every script must at least parse.
//
// Claude Code, Codex, Cursor and Gemini CLI read SKILL.md frontmatter with strict YAML parsers
// and silently skip a skill whose frontmatter fails (an unquoted "word: word" in a description
// is enough), so this parses each one the same way and fails loudly. It also checks the plugin
// manifests point at real skills, every JavaScript file passes `node --check`, and every Rust
// script is non-empty and tracked in Cargo.toml.
//
//   npm install --no-save --no-package-lock yaml@2.6.1   (once; CI does this)
//   node tests/check.mjs
import { readFileSync, readdirSync, existsSync, statSync } from 'node:fs';
import { join, dirname, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

let YAML;
try { YAML = await import('yaml'); } catch {
  console.error('needs the "yaml" package: npm install --no-save --no-package-lock yaml@2.6.1');
  process.exit(2);
}

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SKILLS = join(ROOT, 'skills');
const errors = [];
const fail = (where, msg) => errors.push(`${where}: ${msg}`);
const parse = (text, where) => {
  const doc = YAML.parseDocument(text, { strict: true, uniqueKeys: true, prettyErrors: true });
  for (const e of [...doc.errors, ...doc.warnings]) fail(where, `YAML ${e.name}: ${e.message.split('\n')[0]}`);
  return doc.errors.length ? null : doc.toJS();
};
const walk = (dir) => readdirSync(dir).flatMap((f) => {
  const p = join(dir, f);
  return statSync(p).isDirectory() ? (f === 'node_modules' ? [] : walk(p)) : [p];
});

// skills: frontmatter, names, descriptions, Codex metadata
const names = new Map();
for (const dir of readdirSync(SKILLS)) {
  const file = join(SKILLS, dir, 'SKILL.md');
  if (!statSync(join(SKILLS, dir)).isDirectory()) continue;
  const where = `skills/${dir}/SKILL.md`;
  if (!existsSync(file)) { fail(`skills/${dir}`, 'no SKILL.md'); continue; }
  const src = readFileSync(file, 'utf8');
  const m = src.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n/);
  if (!m) { fail(where, 'no frontmatter block at the top'); continue; }
  const fm = parse(m[1], where);
  if (!fm) continue;
  if (typeof fm.name !== 'string' || !/^[a-z0-9]+(-[a-z0-9]+)*$/.test(fm.name) || fm.name.length > 64) fail(where, `name must be kebab-case, 64 characters at most (got ${JSON.stringify(fm.name)})`);
  if (fm.name !== dir) fail(where, `name "${fm.name}" should match its folder "${dir}" (Gemini CLI and Cursor key skills by folder)`);
  if (typeof fm.description !== 'string' || !fm.description.trim()) fail(where, 'description must be a non-empty string');
  else if (fm.description.length > 1024) fail(where, `description is ${fm.description.length} characters; agents allow 1024 at most`);
  if (names.has(fm.name)) fail(where, `name "${fm.name}" is also used by ${names.get(fm.name)}`); else names.set(fm.name, where);
  if (/—/.test(src)) fail(where, 'contains an em dash');
  // every file the skill mentions under scripts/, assets/ or references/ must exist
  for (const [, ref] of src.matchAll(/`((?:scripts|assets|references)\/[^`\s]+?)`/g)) {
    if (!existsSync(join(SKILLS, dir, ref))) fail(where, `mentions ${ref}, which does not exist`);
  }
  const openai = join(SKILLS, dir, 'agents', 'openai.yaml');
  if (existsSync(openai)) {
    const o = parse(readFileSync(openai, 'utf8'), `skills/${dir}/agents/openai.yaml`);
    if (o && !o.interface?.display_name) fail(`skills/${dir}/agents/openai.yaml`, 'interface.display_name missing');
  }
}

// plugin manifests: valid JSON, names agree, and every skill path they list exists
const manifests = ['.claude-plugin/marketplace.json', '.codex-plugin/plugin.json', '.agents/plugins/marketplace.json', '.cursor-plugin/plugin.json', 'gemini-extension.json'];
const versions = new Set();
for (const rel of manifests) {
  const p = join(ROOT, rel);
  if (!existsSync(p)) { fail(rel, 'missing'); continue; }
  let j; try { j = JSON.parse(readFileSync(p, 'utf8')); } catch (e) { fail(rel, 'invalid JSON: ' + e.message); continue; }
  if (j.name !== 'open-3d-skills') fail(rel, `name is ${JSON.stringify(j.name)}, expected "open-3d-skills"`);
  for (const plugin of j.plugins || []) {
    if (plugin.version) versions.add(plugin.version);
    for (const s of [].concat(plugin.skills || [])) if (!existsSync(join(ROOT, s, 'SKILL.md'))) fail(rel, `lists ${s}, which has no SKILL.md`);
  }
  if (j.version) versions.add(j.version);
  if (typeof j.skills === 'string' && !existsSync(join(ROOT, j.skills))) fail(rel, `skills path ${j.skills} does not exist`);
  if (j.logo && !existsSync(join(ROOT, j.logo))) fail(rel, `logo ${j.logo} does not exist`);
}
if (versions.size > 1) fail('manifests', `versions disagree: ${[...versions].join(', ')}`);
// Claude Code already discovers skills/ under a plugin's source; listing them as well loads
// every skill twice (seen in `claude plugin details`), so the entry must not list them
for (const plugin of JSON.parse(readFileSync(join(ROOT, '.claude-plugin/marketplace.json'), 'utf8')).plugins) {
  if (plugin.source === './' && plugin.skills) fail('.claude-plugin/marketplace.json', `plugin "${plugin.name}" lists skills that skills/ already provides; each would load twice`);
}

// scripts parse
const files = walk(SKILLS);
for (const f of files.filter((f) => f.endsWith('.js') || f.endsWith('.mjs'))) {
  try { execFileSync(process.execPath, ['--check', f], { stdio: 'pipe' }); } catch (e) { fail(relative(ROOT, f), String(e.stderr || e).split('\n').slice(0, 3).join(' ')); }
}
const cargoToml = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
for (const f of files.filter((f) => f.endsWith('.rs'))) {
  const rel = relative(ROOT, f).replace(/\\/g, '/');
  if (!cargoToml.includes(rel)) fail(rel, 'Rust script not registered as [[bin]] in Cargo.toml');
}

if (errors.length) {
  console.error(`not ok - ${errors.length} problem(s):`);
  for (const e of errors) console.error('  ' + e);
  process.exit(1);
}
console.log(`ok - ${names.size} skills load, ${manifests.length} manifests agree, ${files.filter((f) => /\.(m?js|rs)$/.test(f)).length} scripts parse`);
