import ts from 'typescript';
import { parse } from '@vue/compiler-sfc';
import { posix } from 'node:path';

const componentEdges = new Set([
  'files>preview/components/DiskFileDetail.vue',
  'files>preview/components/FilePreviewDock.vue',
  'files>tasks/components/PluginTaskDialog.vue',
  'evidence>raid/components/RaidStackPanel.vue',
]);
const apiEdges = new Set([
  'files>preview', 'files>cases', 'evidence>tasks', 'evidence>cases',
  'settings>cases', 'cases>raid', 'raid>evidence', 'analysis>tasks', 'settings>tasks',
]);
const modelEdges = new Set([
  'search>files/model/tags',
  'tasks>evidence/model/autodetectLabels',
]);
const pureGlobals = new Set(['window', 'document', 'localStorage', 'sessionStorage', 'navigator', 'fetch']);
const featureOf = path => path.match(/^src\/features\/([^/]+)\//)?.[1];
const modelOf = path => /^src\/features\/[^/]+\/model\//.test(path) || /^src\/(utils|lib\/models)\//.test(path);

/** Resolve the existing @ alias and relative imports without executing modules. */
export function importTarget(file, specifier) {
  const path = specifier.split('?')[0];
  if (path.startsWith('@/')) return posix.normalize(`src/${path.slice(2)}`);
  if (path.startsWith('.')) return posix.normalize(posix.join(posix.dirname(file), path));
  return null;
}

/** Runtime edges include lazy imports; type-only cycles are reported separately by import counts. */
export function runtimeCycles(graph) {
  let index = 0;
  const indices = new Map();
  const lows = new Map();
  const stack = [];
  const active = new Set();
  const cycles = [];
  function visit(file) {
    indices.set(file, index); lows.set(file, index++); stack.push(file); active.add(file);
    for (const target of graph.get(file) ?? []) {
      if (!indices.has(target)) { visit(target); lows.set(file, Math.min(lows.get(file), lows.get(target))); }
      else if (active.has(target)) lows.set(file, Math.min(lows.get(file), indices.get(target)));
    }
    if (lows.get(file) !== indices.get(file)) return;
    const group = [];
    let member;
    do { member = stack.pop(); active.delete(member); group.push(member); } while (member !== file);
    if (group.length > 1 || graph.get(file)?.includes(file)) cycles.push(group.sort());
  }
  for (const file of graph.keys()) if (!indices.has(file)) visit(file);
  return cycles;
}

function typeOnly(node) {
  if (ts.isImportTypeNode(node)) return true;
  if (ts.isImportDeclaration(node)) {
    const clause = node.importClause;
    if (clause?.isTypeOnly) return true;
    return !clause?.name && clause?.namedBindings && ts.isNamedImports(clause.namedBindings)
      && clause.namedBindings.elements.length > 0 && clause.namedBindings.elements.every(item => item.isTypeOnly);
  }
  return ts.isExportDeclaration(node) && (node.isTypeOnly || node.exportClause && ts.isNamedExports(node.exportClause)
    && node.exportClause.elements.length > 0 && node.exportClause.elements.every(item => item.isTypeOnly));
}

/** Analyze TS and both Vue script blocks, including dynamic and type-only imports. */
export function auditModule(file, source) {
  const violations = [];
  const imports = [];
  const styles = [];
  let blocks = [{ content: source, firstLine: 1 }];
  if (file.endsWith('.vue')) {
    const { descriptor, errors } = parse(source, { filename: file });
    for (const error of errors) violations.push({ line: 1, message: `Vue parse: ${String(error)}` });
    blocks = [descriptor.script, descriptor.scriptSetup].filter(Boolean)
      .map(block => ({ content: block.content, firstLine: block.loc.start.line }));
    for (const style of descriptor.styles) if (style.src) styles.push(importTarget(file, style.src));
  }
  const owner = featureOf(file);
  const pure = modelOf(file);
  for (const block of blocks) {
    const parsed = ts.createSourceFile(file.replace(/\.vue$/, '.ts'), block.content, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    function report(node, message) {
      violations.push({ line: block.firstLine + parsed.getLineAndCharacterOfPosition(node.getStart(parsed)).line, message });
    }
    function visit(node) {
      let literal;
      if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) literal = node.moduleSpecifier;
      if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument) && ts.isStringLiteral(node.argument.literal)) literal = node.argument.literal;
      if (ts.isCallExpression(node) && (node.expression.kind === ts.SyntaxKind.ImportKeyword || ts.isIdentifier(node.expression) && node.expression.text === 'require') && ts.isStringLiteral(node.arguments[0])) literal = node.arguments[0];
      if (literal) {
        const specifier = literal.text;
        const target = importTarget(file, specifier);
        const onlyType = Boolean(typeOnly(node));
        imports.push({ specifier, target, typeOnly: onlyType });
        const other = target && featureOf(`${target}.ts`);
        if (target?.startsWith('src/app/') && owner) report(node, 'A feature must not depend on app/.');
        if (owner && other && other !== owner) {
          const edge = `${owner}>${other}`;
          const component = `${owner}>${target.slice('src/features/'.length)}`;
          const model = component.replace(/\.ts$/, '');
          if (target !== `src/features/${other}/api` && !componentEdges.has(component) && !modelEdges.has(model)
            && !(onlyType && target.includes('/model/'))) report(node, 'Cross-feature imports must use an explicit API, component entry, or pure contract.');
          if (target === `src/features/${other}/api` && !apiEdges.has(edge)) report(node, `Unapproved feature dependency: ${edge}.`);
        }
        if (owner && target === `src/features/${owner}/api`) report(node, 'Feature internals must not import their own API barrel.');
        if (/^src\/(components|composables)\//.test(file) && target && /src\/(app|features)\//.test(target)) report(node, 'Shared UI/state must not depend on business modules.');
        if (/^src\/(components|composables)\//.test(file) && !onlyType && target?.startsWith('src/lib/ipc/')) report(node, 'Shared UI/state must not invoke business IPC.');
        if (pure && !onlyType && (specifier === 'vue' || specifier.startsWith('@tauri-apps/') || target && /\/(composables|components|app|ipc|platform)\//.test(target))) report(node, 'Pure models must not import state, UI, host APIs, or IPC commands.');
        if (file.startsWith('src/lib/ipc/') && (specifier === 'vue' || target && /src\/(features|app|components|composables)\//.test(target))) report(node, 'IPC must not depend on Vue or business state.');
      }
      if (pure && ts.isIdentifier(node) && pureGlobals.has(node.text)) {
        const parent = node.parent;
        const isName = (ts.isPropertyAssignment(parent) || ts.isPropertySignature(parent) || ts.isVariableDeclaration(parent)
          || ts.isParameter(parent) || ts.isPropertyAccessExpression(parent)) && parent.name === node;
        if (!isName) report(node, `Pure model uses host global ${node.text}.`);
      }
      ts.forEachChild(node, visit);
    }
    visit(parsed);
  }
  return { violations, imports, styles };
}
