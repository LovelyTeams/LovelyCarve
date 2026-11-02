import assert from 'node:assert/strict';
import test from 'node:test';
import { BoardWorkflow, installBoardWorkflow } from '../lib/board.js';

const session = { id: 'chat-a' };
const call = (operation, args = {}, owner = session) => ({ name: `mcp__forensic-board__${operation}`, arguments: args, agent: { session: owner } });
const result = value => ({ isError: false, content: [{ type: 'text', text: JSON.stringify(value) }] });
const evidence = owner => ({ name: 'lovelycarve', arguments: { command: 'files text get', session: 'exam', args: { nodeId: 'node' } }, agent: { session: owner ?? session } });
function refresh(workflow, question = 1) {
  workflow.after(call('list_questions', { competition: 'synthetic' }), result({ questions: [{ id: question }] }));
  workflow.after(call('get_question', { question_id: question }), result({ id: question, answers: [] }));
  workflow.after(call('list_clues', { competition: 'synthetic' }), result({ clues: [] }));
}

test('answers require refreshed questions and clues, then real matching readback', () => {
  const workflow = new BoardWorkflow();
  const write = call('submit_answers', { question_id: 1, answers: [{ answer: 'synthetic-answer' }] });
  assert.match(workflow.before(write), /get_question/);
  workflow.after(call('get_question', { question_id: 1 }), result({ id: 1, answers: [] }));
  assert.match(workflow.before(write), /list_clues/);
  refresh(workflow);
  assert.equal(workflow.before(write), undefined);
  workflow.begin(write);
  workflow.after(write, result({ submitted: [{ answer_id: 8, answer: 'synthetic-answer' }] }));
  assert.match(workflow.before(evidence()), /回读/);
  assert.match(workflow.before(write), /回读/); // No blind duplicate submission.
  workflow.after(call('get_question', { question_id: 2 }), result({ answers: [{ answer_id: 8, answer: 'synthetic-answer' }] }));
  assert.match(workflow.before(evidence()), /回读/);
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [{ answer_id: 9, answer: 'synthetic-answer' }] }));
  assert.match(workflow.before(evidence()), /回读/); // Wrong saved identity.
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [{ answer_id: 8, answer: 'synthetic-answer', reason: 'source record', evidence: ['synthetic.E01 /record'] }] }));
  assert.equal(workflow.before(evidence()), undefined);
  assert.match(workflow.before(write), /get_question/); // Refresh again before another write.
});

test('new evidence invalidates pre-analysis question and clue snapshots', () => {
  const workflow = new BoardWorkflow();
  refresh(workflow);
  const write = call('submit_answers', { question_id: 1, answers: [{ answer: 'a' }] });
  workflow.begin(evidence());
  assert.match(workflow.before(write), /get_question/);
  workflow.after(call('get_question', { question_id: 1 }), result({ id: 1, answers: [] }));
  assert.match(workflow.before(write), /list_clues/);
  workflow.after(call('list_clues', { competition: 'synthetic' }), result({ clues: [] }));
  assert.equal(workflow.before(write), undefined);
});

test('timeouts keep writes pending; complete readback permits repairing only missing writes', () => {
  const workflow = new BoardWorkflow();
  refresh(workflow);
  const write = call('submit_answers', { question_id: 1, answers: [{ answer: 'answer' }] });
  workflow.begin(write);
  workflow.after(write, { isError: true, content: [{ type: 'text', text: 'timeout' }] });
  assert.match(workflow.before(write), /回读/);
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [], truncated: true }));
  assert.match(workflow.before(write), /回读/);
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [] }));
  assert.equal(workflow.before(write), undefined);
  assert.match(workflow.before(evidence()), /回读/);
  assert.match(workflow.before(call('submit_answers', { question_id: 2, answers: [{ answer: 'other' }] })), /回读/);
  workflow.begin(write);
  workflow.after(write, result({ submitted: [{ answer_id: 10, answer: 'answer' }] }));
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [{ answer_id: 10, answer: 'answer' }] }));
  assert.equal(workflow.before(evidence()), undefined);
});

test('clues need deduplication and readback, while ordinary chats remain independent', () => {
  const workflow = new BoardWorkflow();
  const write = call('submit_clues', { competition: 'synthetic', clues: [{ content: 'synthetic clue' }] });
  assert.match(workflow.before(write), /list_clues/);
  workflow.after(call('list_clues', { competition: 'synthetic' }), result({ clues: [] }));
  assert.equal(workflow.before(write), undefined);
  workflow.begin(write);
  workflow.after(write, result({ added: [{ id: 5, content: 'synthetic clue' }] }));
  assert.match(workflow.before(evidence()), /回读/);
  assert.equal(workflow.before(evidence({ id: 'ordinary-chat' })), undefined);
  workflow.after(call('list_clues', { competition: 'different' }), result({ clues: [{ id: 5, content: 'synthetic clue' }] }));
  assert.match(workflow.before(evidence()), /回读/);
  workflow.after(call('list_clues', { competition: 'synthetic' }), result({ clues: [{ id: 5, content: 'synthetic clue' }] }));
  assert.equal(workflow.before(evidence()), undefined);
});

test('the installed host hook blocks dispatch, including parallel writes and shell bypass', async () => {
  const hooks = new Map();
  installBoardWorkflow({ on: (name, callback) => hooks.set(name, callback) });
  let executions = 0;
  const dispatch = async (exec, next) => {
    const outcome = await hooks.get('tools/execute')(exec, next);
    hooks.get('tools/result')(exec, outcome);
    return outcome;
  };
  const run = (exec, value) => dispatch(exec, async () => { executions++; return result(value); });
  await run(call('list_questions', { competition: 'synthetic' }), { questions: [{ id: 1 }] });
  await run(call('get_question', { question_id: 1 }), { id: 1, answers: [] });
  await run(call('list_clues', { competition: 'synthetic' }), { clues: [] });
  let release;
  const first = dispatch(call('submit_answers', { question_id: 1, answers: [{ answer: 'a' }] }), () => new Promise(resolve => { release = resolve; }));
  const blocked = await run(evidence(), {});
  assert.equal(blocked.isError, true);
  assert.equal(executions, 3);
  assert.equal((await run(call('submit_answers', { question_id: 1, answers: [{ answer: 'duplicate' }] }), {})).isError, true);
  assert.equal((await run({ name: 'pwsh', arguments: {}, agent: { session } }, {})).isError, true);
  release(result({ submitted: [{ answer_id: 1, answer: 'a' }] }));
  await first;
  const readback = call('get_question', { question_id: 1 });
  await hooks.get('tools/execute')(readback, async () => result({ id: 1, answers: [{ answer_id: 1, answer: 'a' }] }));
  hooks.get('tools/result')(readback, { isError: true, content: [{ type: 'text', text: 'host post-execute rejected this result' }] });
  assert.equal((await run(evidence(), {})).isError, true);
  await run(call('get_question', { question_id: 1 }), { id: 1, answers: [{ answer_id: 1, answer: 'a' }] });
  assert.equal((await run(evidence(), {})).isError, false);
});

test('readback must preserve the submitted reason and evidence before analysis resumes', () => {
  const workflow = new BoardWorkflow();
  refresh(workflow);
  const write = call('submit_answers', { question_id: 1, answers: [{ answer: 'a', reason: 'record field', evidence: ['disk /db tree:5'] }] });
  workflow.begin(write);
  workflow.after(write, result({ submitted: [{ answer_id: 4, answer: 'a' }] }));
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [{ answer_id: 4, answer: 'a' }] }));
  assert.match(workflow.before(evidence()), /回读/);
  workflow.after(call('get_question', { question_id: 1 }), result({ answers: [{ answer_id: 4, answer: 'a', reason: 'record field', evidence: ['disk /db tree:5'] }] }));
  assert.equal(workflow.before(evidence()), undefined);
});
