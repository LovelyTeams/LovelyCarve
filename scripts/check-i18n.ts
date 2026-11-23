import ts from 'typescript';
import { parse } from '@vue/compiler-sfc';
import { baseParse, NodeTypes, type RootNode, type TemplateChildNode } from '@vue/compiler-dom';
import { globFiles } from './lib/files.ts';

const catalog: Record<string, unknown> = await Bun.file('src/i18n/en.json').json();
const chinese: Record<string, unknown> = await Bun.file('src/i18n/zh-CN.json').json();
const errors: string[] = [];
const keys = new Set<string>();
const parameters = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();
for (const [source, english] of Object.entries(catalog)) {
  if (typeof english !== 'string') errors.push(`Invalid translation: ${source}`);
  else if (!Bun.deepEquals(parameters(source), parameters(english))) errors.push(`Interpolation parameters differ: ${source}`);
}
for (const [key, message] of Object.entries(chinese)) {
  const english = catalog[key];
  if (!Object.hasOwn(catalog, key)) errors.push(`Missing English context key: ${key}`);
  else if (typeof message !== 'string' || typeof english !== 'string' || !Bun.deepEquals(parameters(message), parameters(english))) {
    errors.push(`Context interpolation parameters differ: ${key}`);
  }
}

function checkScript(source: string, file: string): void {
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'tr') {
      const [message] = node.arguments;
      if (message && ts.isStringLiteral(message)) {
        keys.add(message.text);
        if (!Object.hasOwn(catalog, message.text)) errors.push(`${file}: missing translation: ${message.text}`);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true));
}

function checkTemplate(node: RootNode | TemplateChildNode, file: string): void {
  if (node.type === NodeTypes.TEXT && /\p{Script=Han}/u.test(node.content)) {
    errors.push(`${file}: untranslated template text: ${node.content.trim()}`);
  }
  if (node.type === NodeTypes.INTERPOLATION && node.content.type === NodeTypes.SIMPLE_EXPRESSION) checkScript(node.content.content, file);
  if (node.type === NodeTypes.ELEMENT) for (const prop of node.props) {
    if (prop.type === NodeTypes.DIRECTIVE && prop.exp?.type === NodeTypes.SIMPLE_EXPRESSION) checkScript(prop.exp.content, file);
    if (prop.type === NodeTypes.ATTRIBUTE && prop.value && /\p{Script=Han}/u.test(prop.value.content)) {
      errors.push(`${file}: untranslated ${prop.name}: ${prop.value.content}`);
    }
  }
  if (node.type === NodeTypes.ROOT || node.type === NodeTypes.ELEMENT) for (const child of node.children) checkTemplate(child, file);
}

// Application sources: TypeScript and Vue files except tests and declarations (a Bun.Glob extglob).
for (const file of await globFiles('src/**/!(*.test|*.d).@(ts|vue)')) {
  const source = await Bun.file(file).text();
  if (!file.endsWith('.vue')) { checkScript(source, file); continue; }
  const { descriptor } = parse(source, { filename: file });
  for (const block of [descriptor.script, descriptor.scriptSetup]) if (block) checkScript(block.content, file);
  if (descriptor.template) checkTemplate(baseParse(descriptor.template.content), file);
}
if (errors.length) {
  console.error(errors.join('\n'));
  process.exitCode = 1;
} else console.log(`i18n OK: ${keys.size} literal messages; ${Object.keys(catalog).length} English translations.`);
