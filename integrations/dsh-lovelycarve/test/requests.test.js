import assert from 'node:assert/strict';
import test from 'node:test';
import { usageDetails, retrySmaller } from '../lib/requests.js';
import { createTool, decode } from '../lib/tools.js';

const schema = { targetAlternatives: ['path', 'parentId'], requestBodyFields: [
  { name: 'path', required: false, option: '--path' },
  { name: 'pageSize', required: false, option: '--page-size' },
  { name: 'cursor', required: false, option: '--cursor' },
] };
const processResult = value => ({ exitCode: 0, stdout: { text: JSON.stringify(value) }, stderr: { text: '' } });

test('repairs fields placement and API names without dropping the original page cursor', () => {
  const request = { command: 'files list', session: 'exam', args: { path: '/', 'page-size': 20, fields: 'id,name', cursor: 'page-2' } };
  assert.deepEqual(usageDetails(request, schema).retryRequest, {
    command: 'files list', session: 'exam', args: { path: '/', pageSize: 20, cursor: 'page-2' }, fields: 'id,name',
  });
  assert.equal(usageDetails({ command: 'files search', session: 'exam', args: { path: '/', fields: 'nodeId' } }, {
    targetAlternatives: ['path', 'nodeId'], requestBodyFields: [{ name: 'pattern', required: true }, { name: 'path', required: false }],
  }).retryRequest, undefined);
});

test('case index listing, structured discovery and session close have executable corrections', async () => {
  const details = usageDetails({ command: 'indexes list', session: 'exam', args: { imageId: 'image-1' } }, { requestBodyFields: [] });
  assert.deepEqual(details.allowedFields.args, []);
  assert.deepEqual(details.retryRequest, { command: 'indexes list', session: 'exam', args: {} });
  assert.match(details.scopeNote, /整个当前案件/);
  const tool = createTool({ get: () => { throw new Error('must not execute'); } }, '/unused');
  const help = await tool.execute({ command: 'files list --help', session: 'exam' }, {});
  assert.equal(help.error.code, 'usage');
  assert.deepEqual(help.error.details.retryRequest, { command: 'schema files list' });
  const close = await tool.execute({ command: 'sessions close', session: 'exam' }, {});
  assert.deepEqual(close.error.details.allowedFields.args, ['name']);
  assert.deepEqual(close.error.details.retryRequest, { command: 'sessions close', args: { name: 'exam' } });
});

test('help=true only executes schema without a session or evidence command', async () => {
  const commands = [];
  const ctx = { get: () => undefined, shell: { resolve: value => value, execute: async spec => {
    commands.push(spec.command);
    return { result: async () => processResult(schema) };
  } } };
  const result = await createTool(ctx, '/cli').execute({ command: 'files list', help: true }, { workingDirectory: '/workspace' });
  assert.deepEqual(result.result, schema);
  assert.equal(commands.length, 1);
  assert.match(commands[0], /'schema' 'files' 'list'/);
  assert.doesNotMatch(commands[0], /--session|--with-source/);
});

test('usage discovers legal args and returns a retry, without executing the retry', async () => {
  const commands = [];
  const ctx = { get: () => undefined, shell: { resolve: value => value, execute: async spec => {
    commands.push(spec.command);
    return { result: async () => commands.length === 1
      ? { exitCode: 1, stdout: { text: '' }, stderr: { text: JSON.stringify({ error: { code: 'usage', message: 'unknown fields' } }) } }
      : processResult(schema) };
  } } };
  const result = await createTool(ctx, '/cli').execute({ command: 'files list', session: 'exam', args: { fields: 'id', pageSize: 3 } }, { workingDirectory: '/workspace' });
  assert.deepEqual(result.error.details.allowedFields.args, ['path', 'pageSize', 'cursor']);
  assert.deepEqual(result.error.details.retryRequest, { command: 'files list', session: 'exam', args: { pageSize: 3 }, fields: 'id' });
  assert.equal(commands.length, 2);
  assert.match(commands[1], /'schema' 'files' 'list'/);
});

test('source stays bound to the returned evidence, across warnings, projection and oversize recovery', () => {
  const source = { operationId: 'files.list', imageId: 'image-a', evidencePath: '/evidence/a.E01', volumeKey: 'lvm:2:1', nodeId: 'same-id', path: '/partition/root/etc' };
  const value = { source, result: { items: [{ name: 'x' }], nextCursor: 'next' } };
  const identity = { id: 'r', command: 'files list', session: 'exam' };
  const result = decode(processResult(value), identity);
  assert.deepEqual(result.source, source);
  assert.deepEqual(result.result, value.result);
  const oversized = decode(processResult({ source, result: { items: [{ value: 'x'.repeat(60000) }], nextCursor: 'after-discarded-page' } }), identity,
    { command: 'files list', session: 'exam', args: { cursor: 'original', pageSize: 100 } });
  assert.deepEqual(oversized.source, source);
  assert.deepEqual(oversized.recovery.retryRequest.args, { cursor: 'original', pageSize: 50 });
  assert.equal(oversized.recovery.restartCurrentPage, true);
  assert.equal(oversized.result, undefined);
});

test('recovery never echoes inline credentials or proposes replaying exports', () => {
  const details = usageDetails({ command: 'encrypted-volumes unlock', session: 'exam', args: { options: { password: 'synthetic-secret' }, fields: 'id' } });
  assert.doesNotMatch(JSON.stringify(details), /synthetic-secret/);
  assert.equal(details.retryRequest, undefined);
  assert.ok(details.exampleRequest);
  assert.equal(retrySmaller({ command: 'files export', args: { destination: '/out' } }), undefined);
  assert.deepEqual(retrySmaller({ command: 'files text get', session: 'exam', args: { offset: 2048, maxBytes: 8000 } }).args, { offset: 2048, maxBytes: 4000 });
  assert.equal(retrySmaller({ command: 'files list', session: 'exam', args: { pageSize: 1 } }), undefined);
});
