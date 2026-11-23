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
const featureOf = (path: string) => path.match(/^src\/features\/([^/]+)\//)?.[1];
const modelOf = (path: string) => /^src\/features\/[^/]+\/model\//.test(path) || /^src\/(utils|lib\/models)\//.test(path);

/** One import, re-export, dynamic import or require of a module; `target` is set for local specifiers. */
export interface ModuleImport {
  specifier: string;
  target: string | null;
  typeOnly: boolean;
}

/** What `auditModule` found in one module; violation lines are 1-based lines of the whole file. */
export interface ModuleAudit {
  violations: { line: number; message: string }[];
  imports: ModuleImport[];
  styles: (string | null)[];
}

/** Resolve the existing @ alias and relative imports without executing modules. */
export function importTarget(file: string, specifier: string): string | null {
  const path = specifier.split('?')[0];
  if (path.startsWith('@/')) return posix.normalize(`src/${path.slice(2)}`);
  if (path.startsWith('.')) return posix.normalize(posix.join(posix.dirname(file), path));
  return null;
}

/** Runtime edges include lazy imports; type-only cycles are reported separately by import counts. */
export function runtimeCycles(graph: ReadonlyMap<string, readonly string[]>): string[][] {
  let index = 0;
  const indices = new Map<string, number>();
  const lows = new Map<string, number>();
  const stack: string[] = [];
  const active = new Set<string>();
  const cycles: string[][] = [];
  function visit(file: string): void {
    indices.set(file, index); lows.set(file, index++); stack.push(file); active.add(file);
    for (const target of graph.get(file) ?? []) {
      if (!indices.has(target)) { visit(target); lows.set(file, Math.min(lows.get(file)!, lows.get(target)!)); }
      else if (active.has(target)) lows.set(file, Math.min(lows.get(file)!, indices.get(target)!));
    }
    if (lows.get(file) !== indices.get(file)) return;
    const group: string[] = [];
    let member: string;
    do { member = stack.pop()!; active.delete(member); group.push(member); } while (member !== file);
    if (group.length > 1 || graph.get(file)?.includes(file)) cycles.push(group.sort());
  }
  for (const file of graph.keys()) if (!indices.has(file)) visit(file);
  return cycles;
}

function typeOnly(node: ts.Node): boolean {
  if (ts.isImportTypeNode(node)) return true;
  if (ts.isImportDeclaration(node)) {
    const clause = node.importClause;
    if (clause?.isTypeOnly) return true;
    const bindings = clause?.namedBindings;
    return !clause?.name && !!bindings && ts.isNamedImports(bindings)
      && bindings.elements.length > 0 && bindings.elements.every(item => item.isTypeOnly);
  }
  return ts.isExportDeclaration(node) && (node.isTypeOnly || !!node.exportClause && ts.isNamedExports(node.exportClause)
    && node.exportClause.elements.length > 0 && node.exportClause.elements.every(item => item.isTypeOnly));
}

/** Analyze TS and both Vue script blocks, including dynamic and type-only imports. */
export function auditModule(file: string, source: string): ModuleAudit {
  const violations: ModuleAudit['violations'] = [];
  const imports: ModuleImport[] = [];
  const styles: (string | null)[] = [];
  let blocks = [{ content: source, firstLine: 1 }];
  if (file.endsWith('.vue')) {
    const { descriptor, errors } = parse(source, { filename: file });
    for (const error of errors) violations.push({ line: 1, message: `Vue parse: ${String(error)}` });
    blocks = [descriptor.script, descriptor.scriptSetup].filter(block => block !== null)
      .map(block => ({ content: block.content, firstLine: block.loc.start.line }));
    for (const style of descriptor.styles) if (style.src) styles.push(importTarget(file, style.src));
  }
  const owner = featureOf(file);
  const pure = modelOf(file);
  for (const block of blocks) {
    const parsed = ts.createSourceFile(file.replace(/\.vue$/, '.ts'), block.content, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    const report = (node: ts.Node, message: string) => {
      violations.push({ line: block.firstLine + parsed.getLineAndCharacterOfPosition(node.getStart(parsed)).line, message });
    };
    const visit = (node: ts.Node): void => {
      let literal: ts.StringLiteral | undefined;
      if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) literal = node.moduleSpecifier;
      if (ts.isImportTypeNode(node) && ts.isLiteralTypeNode(node.argument) && ts.isStringLiteral(node.argument.literal)) literal = node.argument.literal;
      if (ts.isCallExpression(node) && (node.expression.kind === ts.SyntaxKind.ImportKeyword || ts.isIdentifier(node.expression) && node.expression.text === 'require')) {
        const [argument] = node.arguments;
        if (argument && ts.isStringLiteral(argument)) literal = argument;
      }
      if (literal) {
        const specifier = literal.text;
        const target = importTarget(file, specifier);
        const onlyType = typeOnly(node);
        imports.push({ specifier, target, typeOnly: onlyType });
        const other = target && featureOf(`${target}.ts`);
        if (target?.startsWith('src/app/') && owner) report(node, 'A feature must not depend on app/.');
        if (owner && target && other && other !== owner) {
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
    };
    visit(parsed);
  }
  return { violations, imports, styles };
}
