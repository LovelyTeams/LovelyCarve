import { randomUUID } from 'node:crypto';

const MAX_OUTPUT_BYTES = 48 * 1024;
const MAX_DIAGNOSTICS = 2048;
const OPTIONAL_STRINGS = ['id', 'session', 'fields', 'timezone'];
const REQUEST_KEYS = new Set(['command', 'args', 'force', 'dryRun', 'timeoutMs', ...OPTIONAL_STRINGS]);

function validate(request) {
  if (!request || typeof request !== 'object' || Array.isArray(request)) return '请求必须是对象';
  if (Object.keys(request).some(key => !REQUEST_KEYS.has(key))) return '请求含有未知字段';
  if (typeof request.command !== 'string' || !/^[a-z][a-z0-9-]*(?: [a-z][a-z0-9-]*)*$/.test(request.command)) {
    return 'command 必须是命令目录中的完整命令路径，不含选项或 shell 代码';
  }
  for (const key of OPTIONAL_STRINGS) {
    if (request[key] !== undefined && (typeof request[key] !== 'string' || !request[key].trim())) return `${key} 必须是非空字符串`;
  }
  if ((request.id?.length ?? 0) > 128 || (request.session?.length ?? 0) > 64 || request.command.length > 256 || (request.fields?.length ?? 0) > 4096) return '请求标识或字段选择过长';
  if (request.args !== undefined && (!request.args || typeof request.args !== 'object' || Array.isArray(request.args))) return 'args 必须是 API 参数对象';
  for (const key of ['force', 'dryRun']) {
    if (request[key] !== undefined && typeof request[key] !== 'boolean') return `${key} 必须是布尔值`;
  }
  if (request.timeoutMs !== undefined && (!Number.isInteger(request.timeoutMs) || request.timeoutMs < 1 || request.timeoutMs > 600000)) {
    return 'timeoutMs 必须是 1–600000 的整数';
  }
  if (request.command === 'session' || request.command === 'shell' || request.command.startsWith('sessions attach') || request.command.startsWith('sessions serve')) {
    return '交互进程不能通过此工具运行；用 sessions open 和 session 字段复用命名会话';
  }
  if (request.command === 'files read') return '原始字节请用 files export 导出，文本请用 files text get 读取';
  const lifecycle = request.command.startsWith('sessions ');
  if (lifecycle && request.session) return 'sessions 命令用 args.name 指定名称，不传 session';
  if (!lifecycle && !request.command.startsWith('schema') && request.command !== 'system get' && !request.session) {
    return '调查命令必须指定 session；先用 sessions open 创建唯一命名会话';
  }
  return null;
}

// Quote every argument, including JSON, as literal data in the host executor.
// No user-provided text is interpolated as executable shell syntax.
export function shellCommand(executable, request, platform = process.platform) {
  const arguments_ = ['-q'];
  if (request.session) arguments_.push('--session', request.session);
  if (request.timezone) arguments_.push('--timezone', request.timezone);
  arguments_.push(...request.command.split(' '));
  if (request.args && Object.keys(request.args).length) arguments_.push('--body', JSON.stringify(request.args));
  if (request.fields) arguments_.push('--fields', request.fields);
  if (request.force) arguments_.push('--force');
  if (request.dryRun) arguments_.push('--dry-run');
  if (platform === 'win32') {
    const quote = value => `'${value.replaceAll("'", "''")}'`;
    return `$env:LOVELYCARVE_AGENT = '1'\n& ${[executable, ...arguments_].map(quote).join(' ')}\nexit $LASTEXITCODE`;
  }
  const quote = value => `'${value.replaceAll("'", "'\"'\"'")}'`;
  return `LOVELYCARVE_AGENT=1 ${[executable, ...arguments_].map(quote).join(' ')}`;
}

function decodeResult(result, identity) {
  const envelope = { ...identity, exitCode: result.exitCode ?? null, outputTruncated: false };
  const fail = (code, message, details) => ({ ...envelope, error: { code, message, ...(details ? { details } : {}) } });
  if (result.sandbox?.runnerFailed || result.sandbox?.denied) {
    return fail('sandboxDenied', 'DSH 执行器未允许此调用；命令结果不可用', result.sandbox);
  }
  if (result.aborted || result.timedOut || result.signal || result.exitCode === null || result.exitCode === undefined) {
    return fail(result.timedOut ? 'timeout' : 'interrupted', '调用未正常结束；已丢弃部分输出。命名会话中的工作可能仍在运行，请检查任务状态');
  }
  if (result.stdout?.truncated || result.stderr?.truncated) {
    return {
      ...fail('outputTruncated', '执行器截断了输出，无法验证完整结果；请缩小页大小、选择 fields 或检查完整输出文件', {
        spillPaths: [result.stdout?.spillPath, result.stderr?.spillPath].filter(Boolean),
      }),
      outputTruncated: true,
    };
  }
  const stdout = result.stdout?.text ?? '';
  const stderr = result.stderr?.text ?? '';
  let error;
  let warning;
  const diagnostics = [];
  for (const line of stderr.split(/\r?\n/).filter(Boolean)) {
    try {
      const entry = JSON.parse(line);
      if (entry.error) error = entry.error;
      else if (entry.warning) warning = entry.warning;
      else diagnostics.push(line);
    } catch {
      diagnostics.push(line);
    }
  }
  if (error || (result.exitCode !== 0 && result.exitCode !== 2)) {
    return { ...envelope, error: error ?? { code: 'cliFailed', message: stderr.slice(0, MAX_DIAGNOSTICS) || `CLI 退出码 ${result.exitCode}` } };
  }
  let value;
  try {
    value = stdout.trim() ? JSON.parse(stdout) : null;
  } catch {
    return fail('invalidOutput', 'CLI 未返回完整 JSON；结果不可用，请检查命令和运行库版本');
  }
  if (warning || result.exitCode === 2) envelope.warning = warning ?? { code: 'incompleteResult', message: 'CLI 返回不完整结果' };
  if (diagnostics.length) {
    const text = diagnostics.join('\n');
    envelope.diagnostics = text.slice(0, MAX_DIAGNOSTICS);
    envelope.diagnosticsTruncated = text.length > MAX_DIAGNOSTICS;
  }
  const answer = { ...envelope, result: value };
  if (Buffer.byteLength(JSON.stringify(answer), 'utf8') <= MAX_OUTPUT_BYTES) return answer;
  // Keep cursors and coverage visible without returning a clipped JSON value.
  const pagination = {};
  for (const key of ['nextCursor', 'nextOffset', 'total', 'truncated']) {
    if (value && Object.hasOwn(value, key)) pagination[key] = value[key];
  }
  return {
    ...fail('outputTooLarge', '结果超过工具输出上限；请缩小页大小、选择 fields 或缩小读取范围', { maxBytes: MAX_OUTPUT_BYTES }),
    ...(envelope.warning ? { warning: envelope.warning } : {}),
    outputTruncated: true,
    pagination,
  };
}

export function decode(result, identity) {
  const answer = decodeResult(result, identity);
  if (Buffer.byteLength(JSON.stringify(answer), 'utf8') <= MAX_OUTPUT_BYTES) return answer;
  // Backend error/warning details can be large too. Preserve their identity,
  // explicitly report omitted detail and never leak an unbounded tool result.
  const compact = { ...identity, exitCode: answer.exitCode, outputTruncated: true };
  for (const key of ['error', 'warning']) {
    if (answer[key]) compact[key] = {
      code: String(answer[key].code ?? key).slice(0, 128),
      message: String(answer[key].message ?? '').slice(0, MAX_DIAGNOSTICS),
      detailsOmitted: true,
    };
  }
  if (!compact.error) compact.error = { code: 'outputTooLarge', message: '工具响应超过输出上限；请缩小查询范围' };
  return compact;
}

/** Registry-ready DSH tool; uses the existing shell service for policy and cancellation. */
export function createTool(ctx, executable) {
  return {
    name: 'lovelycarve',
    description: '以结构化请求调用 LovelyCarve CLI，复用命名会话，返回 ID、退出码、JSON 结果、错误和不完整警告。先加载 lovelycarve-cli skill；调查先建案、等待索引并核验。command 是完整命令路径，args 用 schema 中的 API 参数名。错误后检查结果再提交依赖请求；大结果用小页和 fields。',
    timeoutMs: 660000,
    parameters: {
      type: 'object', additionalProperties: false, required: ['command'],
      properties: {
        id: { type: 'string', description: '可选请求 ID；省略时生成唯一 ID' },
        command: { type: 'string', description: '例如 sessions open、cases create、files list、files text get' },
        args: { type: 'object', additionalProperties: true, description: 'API 参数对象，例如 {"path":"/<卷>/etc/os-release","maxBytes":4096}；不使用 shell 选项名' },
        session: { type: 'string', description: '已有命名会话；sessions 生命周期命令省略此字段，用 args.name 指定名称' },
        fields: { type: 'string', description: 'list/search 的条目字段投影，例如 name,id,kind,size；保留分页与覆盖信息' },
        timezone: { type: 'string', description: '本次请求的显示时区覆盖' },
        force: { type: 'boolean', description: '仅在用户授权破坏性操作并检查 dryRun 后设为 true' },
        dryRun: { type: 'boolean', description: '检查解析后的请求，不执行命令' },
        timeoutMs: { type: 'integer', description: '执行超时，1–600000 毫秒，默认 120000；超时后检查命名会话中的任务' },
      },
    },
    output: {
      schema: { type: 'object', additionalProperties: true },
      render: (_args, value) => [{ type: 'text', text: JSON.stringify(value) }],
    },
    async execute(request, exec) {
      const identity = { id: typeof request?.id === 'string' ? request.id : randomUUID(), command: request?.command ?? null, session: request?.session ?? null };
      const problem = validate(request);
      if (problem) return { ...identity, exitCode: null, outputTruncated: false, error: { code: 'usage', message: problem } };
      const workingDirectory = ctx.get('workingDirectory');
      const workdir = exec.agent ? await workingDirectory?.ensure(exec.agent, exec.signal) : exec.workingDirectory;
      if (!workdir) throw new Error('当前会话没有工作区，请先在 DSH 选择工作区');
      const sandboxPolicy = ctx.get('sandboxPolicy');
      if (ctx.shell.sandboxMode !== undefined && !sandboxPolicy) throw new Error('DSH 沙箱策略服务不可用，未执行命令');
      const policy = sandboxPolicy?.resolve(exec.agent ? { session: exec.agent.session } : {});
      const spec = ctx.shell.resolve({
        command: shellCommand(executable, request), workdir, timeoutMs: request.timeoutMs ?? 120000,
        signal: exec.signal,
        ...(policy ? { sandboxPolicy: policy } : {}),
        ...(ctx.get('shellEnv') ? { dshEnv: ctx.get('shellEnv').collect(exec) } : {}),
      });
      const process = await ctx.shell.execute(spec);
      return decode(await process.result(), identity);
    },
  };
}
