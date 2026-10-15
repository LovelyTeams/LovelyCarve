import { readdir, readFile } from 'node:fs/promises';
import ts from 'typescript';
import { parse } from '@vue/compiler-sfc';
import { baseParse, NodeTypes } from '@vue/compiler-dom';

const catalog = JSON.parse(await readFile('src/i18n/en.json', 'utf8'));
const chinese = JSON.parse(await readFile('src/i18n/zh-CN.json', 'utf8'));
const errors = [];
const keys = new Set();
const parameters = text => [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();
for (const [source, english] of Object.entries(catalog)) {
  if (typeof english !== 'string') errors.push(`Invalid translation: ${source}`);
  else if (JSON.stringify(parameters(source)) !== JSON.stringify(parameters(english))) {
    errors.push(`Interpolation parameters differ: ${source}`);
  }
}
for (const [key, message] of Object.entries(chinese)) {
  if (!Object.hasOwn(catalog, key)) errors.push(`Missing English context key: ${key}`);
  else if (JSON.stringify(parameters(message)) !== JSON.stringify(parameters(catalog[key]))) {
    errors.push(`Context interpolation parameters differ: ${key}`);
  }
}

function checkScript(source, file) {
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  function visit(node) {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'tr'
      && node.arguments[0] && ts.isStringLiteral(node.arguments[0])) {
      const message = node.arguments[0].text;
      keys.add(message);
      if (!Object.hasOwn(catalog, message)) errors.push(`${file}: missing translation: ${message}`);
    }
    ts.forEachChild(node, visit);
  }
  visit(ast);
}

async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = `${directory}/${entry.name}`;
    if (entry.isDirectory()) await walk(file);
    else if (/\.(vue|ts)$/.test(file) && !/\.test\.|\.d\.ts$/.test(file)) {
      const source = await readFile(file, 'utf8');
      if (!file.endsWith('.vue')) { checkScript(source, file); continue; }
      const { descriptor } = parse(source, { filename: file });
      for (const block of [descriptor.script, descriptor.scriptSetup].filter(Boolean)) checkScript(block.content, file);
      if (!descriptor.template) continue;
      function visit(node) {
        if (node.type === NodeTypes.TEXT && /\p{Script=Han}/u.test(node.content)) {
          errors.push(`${file}: untranslated template text: ${node.content.trim()}`);
        }
        if (node.type === NodeTypes.INTERPOLATION) checkScript(node.content.content, file);
        if (node.type === NodeTypes.ELEMENT) for (const prop of node.props) {
          if (prop.type === NodeTypes.DIRECTIVE && prop.exp) checkScript(prop.exp.content, file);
          if (prop.type === NodeTypes.ATTRIBUTE && prop.value && /\p{Script=Han}/u.test(prop.value.content)) {
            errors.push(`${file}: untranslated ${prop.name}: ${prop.value.content}`);
          }
        }
        if (node.children) node.children.forEach(visit);
      }
      visit(baseParse(descriptor.template.content));
    }
  }
}
await walk('src');
if (errors.length) {
  console.error(errors.join('\n'));
  process.exitCode = 1;
} else console.log(`i18n OK: ${keys.size} literal messages; ${Object.keys(catalog).length} English translations.`);
