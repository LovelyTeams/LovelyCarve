import { afterEach, expect, test } from 'bun:test';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pluginBundle } from './plugin-bundle.mjs';

const directories = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });
function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'lc-plugin-bundle-'));
  directories.push(root);
  mkdirSync(join(root, 'plugins/target/release'), { recursive: true });
  mkdirSync(join(root, 'src-tauri'));
  writeFileSync(join(root, 'plugins/target/release/lc-plugin-host.exe'), 'host');
  return root;
}
function install(root, directory, id, extra = '') {
  const target = join(root, 'plugins', directory);
  mkdirSync(target);
  writeFileSync(join(target, 'plugin.toml'), `id = "${id}"\nversion = "1"\n[library]\npath = "custom"\n${extra}`);
  writeFileSync(join(target, 'custom.dll'), 'dll');
  return target;
}
test('discovers arbitrary plugin names and includes declared dependencies without source files', () => {
  const root = fixture();
  install(root, 'first', 'org.test.first');
  const second = install(root, 'unrelated-name', 'org.test.second', 'resources = ["helper.dll", "data/map.json"]');
  mkdirSync(join(second, 'data'));
  writeFileSync(join(second, 'helper.dll'), 'dependency');
  writeFileSync(join(second, 'data/map.json'), '{}');
  writeFileSync(join(second, 'source.rs'), 'source');
  const bundle = pluginBundle(root, text => Bun.TOML.parse(text), 'win32');
  expect(bundle.plugins.map(plugin => plugin.id)).toEqual(['org.test.first', 'org.test.second']);
  expect(Object.values(bundle.config.bundle.resources)).toContain('plugins/unrelated-name/data/map.json');
  expect(Object.values(bundle.config.bundle.resources)).not.toContain('plugins/unrelated-name/source.rs');
});
test('rejects duplicate IDs, missing libraries and escaping resource paths', () => {
  const root = fixture();
  const first = install(root, 'one', 'org.test.one');
  install(root, 'two', 'org.test.one');
  expect(() => pluginBundle(root, text => Bun.TOML.parse(text), 'win32')).toThrow('duplicate');
  rmSync(join(root, 'plugins/two'), { recursive: true });
  rmSync(join(first, 'custom.dll'));
  expect(() => pluginBundle(root, text => Bun.TOML.parse(text), 'win32')).toThrow();
  writeFileSync(join(first, 'custom.dll'), 'dll');
  writeFileSync(join(first, 'plugin.toml'), 'id = "one"\n[library]\npath = "custom"\nresources = ["../outside.dll"]');
  expect(() => pluginBundle(root, text => Bun.TOML.parse(text), 'win32')).toThrow('Invalid plugin resource path');
});
test('resolves platform library names and explicit paths', () => {
  const root = fixture();
  const directory = install(root, 'one', 'one');
  writeFileSync(join(root, 'plugins/target/release/lc-plugin-host'), 'host');
  writeFileSync(join(directory, 'libcustom.so'), 'library');
  expect(Object.values(pluginBundle(root, text => Bun.TOML.parse(text), 'linux').config.bundle.resources)).toContain('plugins/one/libcustom.so');
  writeFileSync(join(directory, 'plugin.toml'), 'id = "one"\n[library]\npath = "custom.dll"');
  expect(Object.values(pluginBundle(root, text => Bun.TOML.parse(text), 'win32').config.bundle.resources)).toContain('plugins/one/custom.dll');
});
