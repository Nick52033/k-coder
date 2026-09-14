import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { initialize, resolve } from './plugin-artifact-loader.mjs';
import { ensureArtifactToolWorkspace } from '../.k-coder/plugins/presentations/skills/presentations/scripts/artifact_tool_utils.mjs';

const root = process.cwd();
const temporary = fs.mkdtempSync(path.join(root, 'docs', 'plugin-artifact-test-'));
const relative = path.relative(root, temporary);
const invoke = (...args) => spawnSync('powershell', ['-NoProfile', '-File', 'scripts/plugin-artifact-runtime.ps1', ...args], { cwd: root, encoding: 'utf8' });
try {
  test('info reports versioned installed-runtime capability without secrets', () => {
    const result = invoke('-Action', 'info');
    assert.equal(result.status, 0, result.stderr);
    const info = JSON.parse(result.stdout);
    assert.equal(info.schemaVersion, 1);
    assert.equal(info.workspace.toLowerCase(), root.toLowerCase());
    assert(info.node && info.python && info.artifactToolVersion);
    assert.deepEqual(Object.keys(info).sort(), ['artifactToolVersion', 'libreOffice', 'node', 'python', 'schemaVersion', 'workspace']);
  });
  test('launcher rejects absolute paths, parent traversal and alternate streams', () => {
    for (const script of [path.join(root, 'scripts/plugin-artifact-smoke.py'), '../external.py', 'scripts/plugin-artifact-smoke.py:stream']) {
      const result = invoke('-Action', 'python', '-Script', script);
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /workspace-relative|traversal|alternate streams/);
    }
  });
  test('launcher rejects directory junctions before executing their files', () => {
    const target = path.join(temporary, 'real');
    const link = path.join(temporary, 'link');
    fs.mkdirSync(target);
    fs.writeFileSync(path.join(target, 'forbidden.py'), 'raise RuntimeError("MUST_NOT_RUN")');
    fs.symlinkSync(target, link, 'junction');
    const result = invoke('-Action', 'python', '-Script', path.join(relative, 'link/forbidden.py'));
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /junctions are forbidden/);
    assert.doesNotMatch(result.stderr, /MUST_NOT_RUN/);
    fs.unlinkSync(link);
  });
  test('launcher preserves child failure exit status', () => {
    fs.writeFileSync(path.join(temporary, 'failure.py'), 'raise SystemExit(7)');
    const result = invoke('-Action', 'python', '-Script', path.join(relative, 'failure.py'));
    assert.equal(result.status, 7, result.stderr);
  });
  test('launcher maps public ESM imports without workspace dependency links', () => {
    fs.writeFileSync(path.join(temporary, 'import.mjs'), "import { Workbook } from '@oai/artifact-tool'; import * as jsx from '@oai/artifact-tool/presentation-jsx'; if (!Workbook || !Object.keys(jsx).length) process.exit(8);");
    const result = invoke('-Action', 'node', '-Script', path.join(relative, 'import.mjs'));
    assert.equal(result.status, 0, result.stderr);
    assert(!fs.existsSync(path.join(temporary, 'node_modules')));
  });
  test('module adapter rejects missing runtime and forwards unrelated imports', async () => {
    assert.throws(() => initialize({ modules: null }), /missing/);
    assert.equal(await resolve('node:fs', {}, async specifier => specifier), 'node:fs');
  });
  test('presentation helper validates output workspace and creates no junctions', async () => {
    await assert.rejects(ensureArtifactToolWorkspace(path.resolve(root, '../artifact-outside')), /inside the current project/);
    const directory = path.join(temporary, 'deck');
    const result = await ensureArtifactToolWorkspace(directory);
    assert.equal(result.workspaceDir, directory);
    assert(!fs.existsSync(path.join(directory, 'node_modules')));
    const link = path.join(temporary, 'deck-link');
    fs.symlinkSync(directory, link, 'junction');
    await assert.rejects(ensureArtifactToolWorkspace(path.join(link, 'child')), /links and junctions/);
    fs.unlinkSync(link);
  });
} finally {
  // Deferred until node:test's queued cases finish. Only this generated directory
  // is removed, with any junction removed explicitly inside its test.
  process.on('exit', () => {
    assert(temporary.startsWith(path.join(root, 'docs', 'plugin-artifact-test-')));
    fs.rmSync(temporary, { recursive: true, force: true });
  });
}
