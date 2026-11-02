/** Repair suggestions are returned for review; they are never executed automatically. */
export const REQUEST_FIELDS = ['command', 'args', 'session', 'fields', 'timezone', 'force', 'dryRun', 'timeoutMs', 'id', 'help'];
export const EXAMPLES = [
  { command: 'schema files list' },
  { command: 'files list', help: true },
  { command: 'sessions open', args: { name: 'exam' } },
  { command: 'cases create', session: 'exam', args: { dir: '<新案件绝对目录>', name: '<案件名>' } },
  { command: 'files list', session: 'exam', args: { path: '/', pageSize: 20 }, fields: 'id,name,kind,size' },
  { command: 'indexes list', session: 'exam' },
  { command: 'sessions close', args: { name: 'exam' } },
];

export function helpRequest(request) {
  if (request?.help === true && typeof request.command === 'string') {
    return { command: request.command.startsWith('schema ') ? request.command : `schema ${request.command}` };
  }
  const match = typeof request?.command === 'string' && /^([a-z][a-z0-9-]*(?: [a-z][a-z0-9-]*)*) --help$/.exec(request.command);
  return match ? { command: `schema ${match[1]}` } : null;
}

function redact(value, sensitive = false) {
  if (sensitive) return '<重新通过 @凭据文件提供>';
  if (Array.isArray(value)) return value.map(entry => redact(entry));
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key,
    redact(entry, /password|passphrase|secret|token|(?:^|_)(?:fvek|vmk)|keyHex|rawHexKey|recoveryKey|startupKey/i.test(key))]));
}

export function usageDetails(request, schema) {
  const discovered = schema?.requestBodyFields;
  const allowedArgs = discovered?.map(field => field.name);
  const details = {
    allowedFields: { request: REQUEST_FIELDS, ...(allowedArgs ? { args: allowedArgs } : {}) },
    schemaRequest: { command: `schema ${String(request?.command ?? '').replace(/ --help$/, '')}` },
    examples: EXAMPLES,
    ...(schema?.toolRequestExample ? { exampleRequest: schema.toolRequestExample } : {}),
  };
  if (!request || typeof request !== 'object' || Array.isArray(request)) return details;
  const help = helpRequest(request);
  if (help) return { ...details, retryRequest: help };
  if (typeof request.command !== 'string' || !/^[a-z][a-z0-9-]*(?: [a-z][a-z0-9-]*)*$/.test(request.command)) return details;
  const retry = Object.fromEntries(Object.entries(request).filter(([key]) => REQUEST_FIELDS.includes(key)));
  retry.args = { ...(request.args && typeof request.args === 'object' && !Array.isArray(request.args) ? request.args : {}) };
  for (const key of ['fields', 'timezone', 'force', 'dryRun', 'timeoutMs']) {
    if (retry.args[key] !== undefined && retry[key] === undefined) {
      retry[key] = retry.args[key];
      delete retry.args[key];
    }
  }
  if (request.command.startsWith('sessions ') && retry.session) {
    if (retry.args.name && retry.args.name !== retry.session) return details;
    retry.args.name ??= retry.session;
    delete retry.session;
  }
  if (discovered) {
    if (/^cases (?:open|create)$/.test(request.command) && Object.hasOwn(retry.args, 'path') && !Object.hasOwn(retry.args, 'dir')) {
      retry.args.dir = retry.args.path;
      delete retry.args.path;
    }
    if (request.command === 'images open' && typeof retry.args.imageId === 'string' && !retry.args.path) {
      return { ...details, scopeNote: '选择已加载检材用 context use；images open 接收源路径。',
        retryRequest: redact({ command: 'context use', session: request.session, args: { imageId: retry.args.imageId } }) };
    }
    for (const key of Object.keys(retry.args)) {
      if (allowedArgs.includes(key)) continue;
      const parameter = discovered.find(field => field.option === `--${key}` || field.option === `<${key}>`);
      if (parameter && !Object.hasOwn(retry.args, parameter.name)) {
        retry.args[parameter.name] = retry.args[key];
        delete retry.args[key];
      }
    }
    if (request.command === 'indexes list' && Object.hasOwn(retry.args, 'imageId')) {
      delete retry.args.imageId;
      details.scopeNote = 'indexes list 列出整个当前案件的索引；按返回记录的 imageId 筛选。单个检材的卷映射用 indexes volumes list。';
    }
    if (Object.keys(retry.args).some(key => !allowedArgs.includes(key))) return details;
    if (discovered.some(field => field.required && !Object.hasOwn(retry.args, field.name)
      && !((schema.targetAlternatives ?? []).includes(field.name)
        && schema.targetAlternatives.some(key => Object.hasOwn(retry.args, key))))) return details;
    for (const field of discovered.filter(field => field.sensitive)) {
      if (Object.hasOwn(retry.args, field.name)) retry.args[field.name] = '<重新通过 @凭据文件提供>';
    }
  }
  delete retry.id;
  delete retry.help;
  const safe = redact(retry);
  if (JSON.stringify(safe).includes('<重新通过 @凭据文件提供>')) details.exampleRequest = safe;
  else if (JSON.stringify(safe) !== JSON.stringify(request)) details.retryRequest = safe;
  return details;
}

export function retrySmaller(request) {
  // Retrying a mutation, export or task could repeat side effects.
  if (!request || !/(?: list| search|files text get)$/.test(request.command)) return undefined;
  const retry = { ...request, args: { ...request.args } };
  delete retry.id;
  if (request.command === 'files text get') {
    if (retry.args.maxBytes === 1) return undefined;
    retry.args.maxBytes = Math.max(1, Math.floor((retry.args.maxBytes ?? 4096) / 2));
  } else if (request.command === 'files list') {
    if (retry.args.pageSize === 1) return undefined;
    retry.args.pageSize = Math.max(1, Math.floor((retry.args.pageSize ?? 50) / 2));
  } else if (retry.args.filter && typeof retry.args.filter === 'object') {
    if (retry.args.filter.limit === 1) return undefined;
    retry.args.filter = { ...retry.args.filter, limit: Math.max(1, Math.floor((retry.args.filter.limit ?? 50) / 2)) };
  } else if (retry.args.limit !== undefined) {
    if (retry.args.limit === 1) return undefined;
    retry.args.limit = Math.max(1, Math.floor(retry.args.limit / 2));
  } else return undefined;
  const safe = redact(retry);
  return JSON.stringify(safe).includes('<重新通过 @凭据文件提供>') ? undefined : safe;
}
