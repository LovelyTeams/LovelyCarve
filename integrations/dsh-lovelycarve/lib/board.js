/** Observe real host tool calls; no MCP connection, board writes or local answer ledger. */
const OPERATIONS = ['list_questions', 'get_question', 'list_clues', 'submit_answers', 'update_answer', 'submit_clues', 'update_clue'];

function operation(name, mapping) {
  const configured = Object.entries(mapping).find(([, tool]) => tool === name)?.[0];
  if (configured) return configured;
  if (!/forensic[-_]board/.test(name ?? '')) return undefined;
  return OPERATIONS.find(operation => name.endsWith(`__${operation}`));
}

function objects(value) {
  if (!value || typeof value !== 'object') return [];
  return [value, ...Object.values(value).flatMap(objects)];
}

function payload(result) {
  if (result?.isError) return undefined;
  let value = result?.value;
  if (value === undefined) {
    try { value = JSON.parse((result?.content ?? []).filter(block => block.type === 'text').map(block => block.text).join('\n')); }
    catch { return undefined; }
  }
  // MCP content may itself be wrapped by a typed host tool.
  if (Array.isArray(value?.content)) return payload(value);
  if (!value || typeof value !== 'object') return undefined;
  if (objects(value).some(object => object.error || object.isError || object.outputTruncated || object.truncated === true || object.nextCursor)) return undefined;
  return value;
}

export class BoardWorkflow {
  constructor(mapping = {}) { this.mapping = mapping; this.states = new Map(); }
  state(exec) {
    const session = exec.agent?.session;
    if (!session) return undefined;
    const key = session.id ?? session;
    if (!this.states.has(key)) this.states.set(key, { questions: new Set(), answerQuestions: new Map(), clues: new Set(), pending: null, competition: null });
    return this.states.get(key);
  }
  before(exec) {
    const state = this.state(exec);
    if (!state) return undefined;
    const action = operation(exec.name, this.mapping);
    const args = exec.arguments ?? {};
    const question = args.question_id ?? args.questionId ?? state.answerQuestions.get(String(args.answer_id));
    if (state.pending) {
      if (['get_question', 'list_clues', 'list_questions'].includes(action)) return undefined;
      if (exec.name === 'lovelycarve' && (args.help || /^(?:schema(?: |$)|tasks (?:get|list|wait|cancel)$|sessions (?:get|list|close)$)/.test(args.command ?? ''))) return undefined;
      const repair = state.pending.missing && (state.pending.kind === 'answer'
        ? ['submit_answers', 'update_answer'].includes(action) && String(question) === state.pending.question
        : ['submit_clues', 'update_clue'].includes(action) && (args.competition ?? state.competition) === state.pending.competition);
      if (!repair) return `看板写入尚未核对保存；先调用 ${state.pending.kind === 'answer' ? 'get_question' : 'list_clues'} 回读。已确认缺失时仅修复原题/原竞赛的写入，再回读；期间暂停后续调查。`;
    }
    if (['submit_answers', 'update_answer'].includes(action)) {
      if (!state.questions.has(String(question))) return '提交答案前先 get_question 刷新该题；使用看板 question_id，不使用 CLI 节点 ID。';
      if (!state.clues.has(String(question)) && !state.clues.has(String(state.competition))) return '提交答案前先 list_clues 读取该题或同一竞赛的共享线索。';
    }
    if (['submit_clues', 'update_clue'].includes(action) && !state.clues.has(String(args.competition ?? state.competition))) return '保存线索前先 list_clues 查重。';
    return undefined;
  }
  begin(exec) {
    const state = this.state(exec);
    if (!state) return;
    const action = operation(exec.name, this.mapping);
    const args = exec.arguments ?? {};
    const evidence = ['pwsh', 'bash', 'read', 'grep'].includes(exec.name)
      || exec.name === 'lovelycarve' && !args.help && !/^(?:schema(?: |$)|system get$|sessions |cases |context |tasks |indexes (?:list|volumes list)$)/.test(args.command ?? '');
    if (evidence) { state.questions.clear(); state.clues.clear(); }
    if (['submit_answers', 'update_answer', 'submit_clues', 'update_clue'].includes(action)) {
      const kind = action.includes('answer') ? 'answer' : 'clue';
      const values = kind === 'answer' ? (args.answers ?? [args]) : (args.clues ?? [args]);
      state.pending = { kind, question: String(args.question_id ?? args.questionId ?? state.answerQuestions.get(String(args.answer_id))), competition: args.competition ?? state.competition,
        expected: values.map(value => ({ value: kind === 'answer' ? value.answer : value.content, id: value.answer_id ?? value.clue_id,
          reason: value.reason, evidence: value.evidence })).filter(entry => typeof entry.value === 'string'), ids: [] };
    }
  }
  after(exec, result) {
    const state = this.state(exec);
    if (!state) return;
    const action = operation(exec.name, this.mapping);
    const args = exec.arguments ?? {};
    const value = payload(result);
    if (value === undefined) return; // A timeout cannot prove a write failed.
    if (action === 'list_questions' && args.competition !== undefined) {
      if (state.competition !== args.competition) { state.questions.clear(); state.clues.clear(); }
      state.competition = args.competition;
    }
    if (action === 'get_question') {
      const question = String(args.question_id ?? args.questionId);
      state.questions.add(question);
      for (const row of objects(value)) {
        if ((row.answer ?? row['答案']) !== undefined && (row.answer_id ?? row.id) !== undefined) state.answerQuestions.set(String(row.answer_id ?? row.id), question);
      }
    }
    if (action === 'list_clues') state.clues.add(String(args.question_id ?? args.questionId ?? args.competition ?? state.competition));
    const pending = state.pending;
    if (!pending) return;
    if (['submit_answers', 'update_answer', 'submit_clues', 'update_clue'].includes(action)) {
      const rows = objects(value);
      pending.ids = rows.flatMap(object => object.answer_id !== undefined ? [object.answer_id] : object.clue_id !== undefined ? [object.clue_id] : []);
      for (const expected of pending.expected) {
        const saved = rows.find(row => (row.answer ?? row.content ?? row['答案'] ?? row['内容']) === expected.value
          && (row.answer_id ?? row.clue_id ?? row.id) !== undefined);
        if (saved) expected.id ??= saved.answer_id ?? saved.clue_id ?? saved.id;
      }
      return;
    }
    const correctRead = pending.kind === 'answer'
      ? action === 'get_question' && String(args.question_id ?? args.questionId) === pending.question
      : action === 'list_clues' && (args.competition ?? state.competition) === pending.competition;
    if (!correctRead) return;
    const rows = objects(value);
    const found = pending.expected.length > 0 && pending.expected.every(expected => rows.some(row => {
      const id = row.answer_id ?? row.clue_id ?? row.id;
      return (expected.id === undefined || String(expected.id) === String(id))
        && (pending.ids.length === 0 || pending.ids.some(known => String(known) === String(id)))
        && (row.answer ?? row.content ?? row['答案'] ?? row['内容']) === expected.value
        && (expected.reason === undefined || (row.reason ?? row['理由']) === expected.reason)
        && (expected.evidence === undefined || JSON.stringify(row.evidence ?? row['证据']) === JSON.stringify(expected.evidence));
    }));
    if (found) {
      state.pending = null;
      state.questions.clear();
      state.clues.clear();
    } else {
      // A full read proves the expected write is missing. Re-read/deduplicate
      // before explicitly repairing it; never automatically replay a write.
      pending.missing = true;
    }
  }
}

export function installBoardWorkflow(ctx, mapping) {
  if (typeof ctx.on !== 'function') return;
  const workflow = new BoardWorkflow(mapping);
  ctx.on('tools/execute', async (exec, next) => {
    const reason = workflow.before(exec);
    if (reason) return { isError: true, content: [{ type: 'text', text: reason }], error: { message: reason, info: { code: 'boardWorkflow' } } };
    workflow.begin(exec);
    return next();
  });
  // Observe the final, committed tool result after host post-execute policy.
  ctx.on('tools/result', (exec, result) => workflow.after(exec, result));
  if (typeof ctx.effect === 'function') ctx.effect(() => () => workflow.states.clear());
}
