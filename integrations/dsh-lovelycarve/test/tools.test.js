import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { randomUUID } from 'node:crypto';
import test from 'node:test';
import { createTool, decode, shellCommand } from '../lib/tools.js';
import { apply, inject } from '../lib/preset-entry.js';

const executeFile = promisify(execFile);
const identity = { id: 'request-1', command: 'files list', session: 'exam' };
const processResult = (value, changes = {}) => ({
  exitCode: 0, stdout: { text: JSON.stringify(value), truncated: false },
  stderr: { text: '', truncated: false }, ...changes,
});

test('preserves pagination, timezone provenance and incomplete warnings', () => {
  const value = { nextCursor: 'next', total: 20, truncated: false, timestampDisplay: { timeZones: [{ zone: 'UTC', source: 'fallback' }] }, items: [{ id: 'node', name: 'entry' }] };
  const warning = { code: 'readFaultsDetected', message: 'synthetic bytes', details: { syntheticBytes: 512 } };
  assert.deepEqual(decode(processResult(value, { exitCode: 2, stderr: { text: JSON.stringify({ warning }) } }), identity), {
    ...identity, exitCode: 2, outputTruncated: false, warning, result: value,
  });
});

test('reports native failures and discards partial bytes after interruption', () => {
  const error = { code: 'notFound', message: 'missing', ipcCommand: 'fs_read' };
  assert.deepEqual(decode(processResult(null, { exitCode: 1, stderr: { text: JSON.stringify({ error }) } }), identity), {
    ...identity, exitCode: 1, outputTruncated: false, error,
  });
  for (const changes of [{ aborted: true, exitCode: null }, { timedOut: true, exitCode: 1 }, { signal: 'SIGTERM', exitCode: null }]) {
    const result = decode(processResult('partial evidence', changes), identity);
    assert.ok(result.error);
    assert.ok(!Object.hasOwn(result, 'result'));
  }
  const invalid = decode(processResult(null, { stdout: { text: '{"items":[' } }), identity);
  assert.equal(invalid.error.code, 'invalidOutput');
});

test('never presents clipped or oversized JSON as a complete result', () => {
  const clipped = decode(processResult(null, { stdout: { text: '{}', truncated: true, spillPath: '/output.log' } }), identity);
  assert.equal(clipped.error.code, 'outputTruncated');
  assert.equal(clipped.outputTruncated, true);
  assert.deepEqual(clipped.error.details.spillPaths, ['/output.log']);
  const oversized = decode(processResult({ nextCursor: 'next', total: 100, truncated: false, items: ['取证'.repeat(50000)] }), identity);
  assert.equal(oversized.error.code, 'outputTooLarge');
  assert.deepEqual(oversized.pagination, { nextCursor: 'next', total: 100, truncated: false });
  assert.ok(!Object.hasOwn(oversized, 'result'));
  assert.ok(Buffer.byteLength(JSON.stringify(oversized)) < 2048);
  const hugeError = decode(processResult(null, { exitCode: 1, stderr: { text: JSON.stringify({ error: { code: 'failed', message: 'large', details: 'x'.repeat(100000) } }) } }), identity);
  assert.equal(hugeError.error.code, 'failed');
  assert.equal(hugeError.error.detailsOmitted, true);
  assert.equal(hugeError.outputTruncated, true);
  assert.ok(Buffer.byteLength(JSON.stringify(hugeError)) < 2048);
});

test('invalid requests cannot reach the shell executor', async () => {
  const ctx = { get: () => { throw new Error('unexpected service access'); } };
  const tool = createTool(ctx, '/unused');
  for (const request of [
    { command: 'files list; injected', session: 'exam' },
    { command: 'images open', args: { path: '/evidence' } },
    { command: 'files list', session: 'exam', timeoutMs: 0 },
    { command: 'files list', session: 'exam', force: 'true' },
    { command: 'files read', session: 'exam' },
    { command: 'sessions attach', args: { name: 'exam' } },
    { command: 'sessions open', session: 'exam' },
    { command: 'system get', unexpected: true },
  ]) {
    assert.equal((await tool.execute(request, {})).error.code, 'usage');
  }
});

test('forwards workspace, cancellation and the standing sandbox policy', async () => {
  const controller = new AbortController();
  const agent = { session: 'dsh-chat' };
  const policy = { mode: 'workspace-write' };
  let received;
  const services = {
    workingDirectory: { ensure: async caller => { assert.equal(caller, agent); return '/workspace'; } },
    sandboxPolicy: { resolve: scope => { assert.deepEqual(scope, { session: agent.session }); return policy; } },
    shellEnv: { collect: () => ({ DSH_TEST: 'current' }) },
  };
  const ctx = {
    get: key => services[key],
    shell: {
      sandboxMode: 'workspace-write',
      resolve: spec => { received = spec; return spec; },
      execute: async spec => { assert.equal(spec, received); return { result: async () => processResult({ ok: true }) }; },
    },
  };
  const result = await createTool(ctx, '/cli').execute({ command: 'system get' }, { agent, signal: controller.signal });
  assert.equal(received.workdir, '/workspace');
  assert.equal(received.signal, controller.signal);
  assert.equal(received.sandboxPolicy, policy);
  assert.deepEqual(received.dshEnv, { DSH_TEST: 'current' });
  assert.match(received.command, /LOVELYCARVE_AGENT/);
  assert.deepEqual(result.result, { ok: true });
  delete services.sandboxPolicy;
  await assert.rejects(createTool(ctx, '/cli').execute({ command: 'system get' }, { agent }), /沙箱策略/);
});

test('preset registers its tool and requires case creation and index verification', () => {
  const tools = [];
  const sections = [];
  apply({ tools: { register: tool => tools.push(tool) }, systemPrompt: { section: section => sections.push(section) } });
  assert.deepEqual(inject, ['systemPrompt', 'tools', 'shell']);
  assert.equal(tools[0].name, 'lovelycarve');
  assert.match(sections[0].text(), /每次调查先建立案件/);
  assert.match(sections[0].text(), /已知路径也遵循此调查流程/);
});

test('POSIX command quoting preserves evidence text literally', async t => {
  if (process.platform === 'win32') return t.skip('POSIX shell integration runs on Unix');
  const directory = await mkdtemp(join(tmpdir(), 'lovelycarve-quoting-'));
  try {
    const executable = join(directory, 'echo-args');
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@"\n', { mode: 0o755 });
    const body = { path: '/volume/a\' $(touch injected) `echo danger` \\ "中"\nfile' };
    const command = shellCommand(executable, { command: 'files get', session: 'exam', args: body }, 'linux');
    const result = await executeFile('/bin/sh', ['-c', command], { cwd: directory });
    const lines = result.stdout.trimEnd().split('\n');
    assert.deepEqual(JSON.parse(lines.at(-1)), body);
    assert.equal(existsSync(join(directory, 'injected')), false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

const cli = resolve(process.env.LOVELYCARVE_TEST_CLI ?? `src-tauri/target/debug/lovelycarve-cli${process.platform === 'win32' ? '.exe' : ''}`);
test('real CLI: named session, case, index, pages, quoted paths, error and export', { skip: !existsSync(cli) && 'build the CLI or set LOVELYCARVE_TEST_CLI', timeout: 180000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'lovelycarve-dsh-'));
  const evidence = join(directory, 'evidence');
  await mkdir(evidence);
  const fileName = 'a\' $name `literal` 中文.txt';
  await writeFile(join(evidence, fileName), 'forensic bytes\n');
  await writeFile(join(evidence, 'b.txt'), 'second\n');
  for (let index = 0; index < 100; index++) await writeFile(join(evidence, `long-${index}-${'x'.repeat(170)}.txt`), 'x');
  const session = `dsh-${randomUUID()}`;
  const environment = { ...process.env, LOVELYCARVE_SESSION_DIR: join(directory, 'sessions') };
  for (const key of ['LOVELYCARVE_SESSION', 'LOVELYCARVE_IMAGE', 'LOVELYCARVE_CASE']) delete environment[key];
  const ctx = {
    get: () => undefined,
    shell: {
      resolve: spec => spec,
      execute: async spec => ({ result: async () => {
        let result;
        try {
          result = await executeFile(process.platform === 'win32' ? 'pwsh' : '/bin/sh', process.platform === 'win32' ? ['-NoProfile', '-Command', spec.command] : ['-c', spec.command], {
            cwd: spec.workdir, env: environment, windowsHide: true, timeout: spec.timeoutMs, maxBuffer: 4 * 1024 * 1024,
          });
        } catch (error) {
          result = error;
        }
        return { exitCode: typeof result.code === 'number' ? result.code : 0, stdout: { text: result.stdout ?? '' }, stderr: { text: result.stderr ?? '' } };
      } }),
    },
  };
  const tool = createTool(ctx, cli);
  const call = (command, args, extra = {}) => tool.execute({ command, ...(args ? { args } : {}), ...extra }, { workingDirectory: directory });
  const query = (command, args, extra = {}) => call(command, args, { session, ...extra });
  const success = async promise => { const result = await promise; assert.equal(result.error, undefined, JSON.stringify(result)); assert.equal(result.exitCode, 0); return result.result; };
  await success(call('sessions open', { name: session }));
  try {
    await success(query('cases create', { dir: join(directory, 'case'), name: 'DSH fixture' }));
    const opened = await query('images open', { path: evidence });
    const image = await success(Promise.resolve(opened));
    assert.equal(opened.source.imageId, image.id);
    assert.equal(opened.source.evidencePath, evidence);
    await success(query('tasks wait'));
    const indexes = await success(query('indexes list'));
    assert.ok(indexes.some(index => index.imageId === image.id && index.indexed && !index.stale && !index.truncated));
    const volumes = await success(query('indexes volumes list'));
    assert.ok(volumes.some(volume => volume.indexes.some(index => index.volumeKey === volume.volumeKey && !index.truncated)));
    const unavailable = await query('indexes files list', { volumeKey: 'missing', filter: { limit: 1 } });
    assert.equal(unavailable.error.details.context.reason, 'indexUnavailable');
    const misplaced = await query('files list', { path: '/evidence', pageSize: 1, fields: 'id,name' });
    assert.equal(misplaced.error.code, 'usage');
    assert.ok(misplaced.error.details.allowedFields.args.includes('pageSize'));
    assert.equal((await tool.execute(misplaced.error.details.retryRequest, { workingDirectory: directory })).error, undefined);
    const caseScope = await query('indexes list', { imageId: image.id });
    assert.deepEqual(caseScope.error.details.allowedFields.args, []);
    assert.equal((await tool.execute(caseScope.error.details.retryRequest, { workingDirectory: directory })).error, undefined);
    const page = await success(query('files list', { path: '/evidence', pageSize: 1 }, { fields: 'id,name,kind' }));
    assert.equal(page.items.length, 1);
    assert.ok(page.nextCursor);
    const next = await success(query('files list', { path: '/evidence', pageSize: 1, cursor: page.nextCursor }, { fields: 'id,name,kind' }));
    assert.notEqual(page.items[0].id, next.items[0].id);
    assert.equal((await success(query('files text get', { path: `/evidence/${fileName}`, maxBytes: 4096 }))).text, 'forensic bytes\n');
    const oversized = await query('files list', { path: '/evidence', pageSize: 100 });
    assert.equal(oversized.error.code, 'outputTooLarge');
    assert.equal(oversized.recovery.retryRequest.args.cursor, undefined);
    const recovered = await tool.execute(oversized.recovery.retryRequest, { workingDirectory: directory });
    assert.equal(recovered.error, undefined);
    assert.equal(recovered.result.items[0].id, page.items[0].id);
    const failure = await query('files get', { path: '/evidence/missing' });
    assert.equal(failure.exitCode, 1);
    assert.equal(failure.error.code, 'notFound');
    const invalid = await query('images list', { pageSize: 30 });
    assert.equal(invalid.exitCode, 1);
    assert.equal(invalid.error.code, 'usage');
    const destination = join(directory, 'copy.txt');
    await success(query('files export', { path: `/evidence/${fileName}`, destination }));
    assert.equal(await readFile(destination, 'utf8'), 'forensic bytes\n');
    assert.equal((await query('files export', { path: `/evidence/${fileName}`, destination })).error.code, 'alreadyExists');
    assert.equal(existsSync(join(directory, 'injected')), false);
    const otherEvidence = join(directory, 'other');
    await mkdir(otherEvidence);
    await writeFile(join(otherEvidence, fileName), 'other evidence\n');
    const other = await success(query('images open', { path: otherEvidence }));
    await success(query('tasks wait'));
    const selected = await query('context use', { imageId: evidence });
    assert.equal(selected.error, undefined);
    assert.equal(selected.source.imageId, image.id);
    assert.equal(selected.source.evidencePath, evidence);
    const nodeId = page.items[0].id;
    const originalRead = await query('files text get', { imageId: image.id, nodeId, maxBytes: 4096 });
    const otherRead = await query('files text get', { imageId: other.id, nodeId, maxBytes: 4096 });
    assert.equal(originalRead.result.text, 'forensic bytes\n');
    assert.equal(otherRead.result.text, 'other evidence\n');
    assert.equal(originalRead.source.nodeId, otherRead.source.nodeId);
    assert.notEqual(originalRead.source.imageId, otherRead.source.imageId);
    assert.equal(originalRead.source.evidencePath, evidence);
    assert.equal(otherRead.source.evidencePath, otherEvidence);
    assert.ok(originalRead.source.path.endsWith(fileName));
  } finally {
    const close = await query('sessions close');
    await success(tool.execute(close.error.details.retryRequest, { workingDirectory: directory }));
    await rm(directory, { recursive: true, force: true });
  }
});
