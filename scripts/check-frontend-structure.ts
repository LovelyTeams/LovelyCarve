import { globFiles } from './lib/files.ts';
import { auditModule, runtimeCycles } from './lib/frontend-structure.ts';

// Every file below src/, scanned once: imports and scoped styles resolve against this set.
// The audited modules are its TypeScript and Vue files except tests (a Bun.Glob extglob).
const sourceFiles = await globFiles('src/**');
const known = new Set(sourceFiles);
const modules = new Bun.Glob('src/**/!(*.test|*.spec).@(ts|vue)');
const isFile = async (path: string) => path.startsWith('src/') ? known.has(path) : Bun.file(path).exists();

const violations: string[] = [];
const graph = new Map<string, string[]>();
let runtimeImports = 0;
let typeImports = 0;
for (const file of sourceFiles.filter(path => modules.match(path))) {
  const result = auditModule(file, await Bun.file(file).text());
  const edges: string[] = [];
  for (const violation of result.violations) violations.push(`${file}:${violation.line}: ${violation.message}`);
  for (const entry of result.imports) {
    if (entry.typeOnly) typeImports++; else runtimeImports++;
    if (!entry.target) continue;
    const candidates = [entry.target, `${entry.target}.ts`, `${entry.target}.vue`, `${entry.target}/index.ts`];
    const found = await Promise.all(candidates.map(isFile));
    const resolved = candidates.find((_, index) => found[index]);
    if (!resolved) violations.push(`${file}: unresolved import ${entry.specifier}`);
    else if (!entry.typeOnly && /\.(ts|vue)$/.test(resolved)) edges.push(resolved);
  }
  graph.set(file, edges);
  for (const style of result.styles) if (!style || !await isFile(style)) violations.push(`${file}: unresolved scoped style ${style}`);
}
for (const cycle of runtimeCycles(graph)) violations.push(`Runtime import cycle: ${cycle.join(' <-> ')}`);
if (violations.length) {
  console.error(violations.join('\n'));
  process.exitCode = 1;
} else console.log(`Frontend structure OK: ${graph.size} modules; ${runtimeImports} runtime and ${typeImports} type-only imports.`);
