// Builds the plugin workspace (plugins/Cargo.toml): the lc-plugin-host loader
// and every plugin library. Each library is then copied next to its
// plugin.toml under the platform file name the manifest's `library` resolves
// to (linuxinfo → linuxinfo.dll / liblinuxinfo.so / liblinuxinfo.dylib), and
// `lc-plugin-host --describe` writes the library's analyzers and record
// schemas beside it as schema.json. Development discovery of the repository
// plugins/ directory, the release bundle and .lcpkg packages see the same
// package layout.
//
//   bun scripts/build-plugins.mjs            # debug build
//   bun scripts/build-plugins.mjs --release  # release build (bundles, packs)
//   bun scripts/build-plugins.mjs --plugin linuxinfo # one library, keep an active host running
//
// Always build from the committed plugin lockfile.
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const release = process.argv.includes('--release');
const workspace = fileURLToPath(new URL('../plugins', import.meta.url));
if (!existsSync(join(workspace, 'Cargo.toml'))) {
  console.error('[build-plugins] plugin submodule is missing; run just bootstrap or git submodule update --init --recursive');
  process.exit(1);
}
const cargoArgs = ['build', '--locked', '--manifest-path', join(workspace, 'Cargo.toml')];
const pluginIndex = process.argv.indexOf('--plugin');
const selected = pluginIndex === -1 ? null : process.argv[pluginIndex + 1];
if (pluginIndex !== -1) {
  if (!selected || !/^[a-z][a-z0-9-]*$/.test(selected) || !existsSync(join(workspace, selected, 'plugin.toml'))) {
    console.error('[build-plugins] --plugin requires a bundled plugin directory name');
    process.exit(1);
  }
  const manifest = Bun.TOML.parse(readFileSync(join(workspace, selected, 'Cargo.toml'), 'utf8'));
  if (typeof manifest.package?.name !== 'string') {
    console.error('[build-plugins] selected plugin has no Cargo package name');
    process.exit(1);
  }
  cargoArgs.push('--package', manifest.package.name);
}
if (release) cargoArgs.push('--release');

const build = spawnSync('cargo', cargoArgs, { stdio: 'inherit' });
if (build.status !== 0) process.exit(build.status ?? 1);

const output = join(workspace, 'target', release ? 'release' : 'debug');
const [prefix, suffix] =
  process.platform === 'win32' ? ['', '.dll'] : process.platform === 'darwin' ? ['lib', '.dylib'] : ['lib', '.so'];
// A --plugin build leaves the loader alone (a running app may hold it open),
// so describing uses the one the last full build produced.
const loader = join(output, process.platform === 'win32' ? 'lc-plugin-host.exe' : 'lc-plugin-host');
if (!existsSync(loader)) {
  console.error(`[build-plugins] ${loader} is missing; build the whole workspace once without --plugin`);
  process.exit(1);
}

let staged = 0;
for (const entry of readdirSync(workspace, { withFileTypes: true })) {
  if (selected !== null && entry.name !== selected) continue;
  const manifestPath = join(workspace, entry.name, 'plugin.toml');
  if (!entry.isDirectory() || !existsSync(manifestPath)) continue;
  const manifest = Bun.TOML.parse(readFileSync(manifestPath, 'utf8'));
  const name = manifest.library;
  if (typeof name !== 'string' || /[\\/]|\.(dll|so|dylib)$/i.test(name)) {
    console.error(`[build-plugins] ${manifestPath}: library must be the crate's library name`);
    process.exit(1);
  }
  const fileName = `${prefix}${name}${suffix}`;
  const built = join(output, fileName);
  if (!existsSync(built)) {
    console.error(`[build-plugins] ${built} was not built; is ${entry.name} a workspace member with [lib] name = "${name}"?`);
    process.exit(1);
  }
  const library = join(workspace, entry.name, fileName);
  copyFileSync(built, library);
  const described = spawnSync(loader, ['--describe', '--library', library], {
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
  if (described.status !== 0) {
    console.error(`[build-plugins] describing ${library} failed:\n${described.stderr}`);
    process.exit(1);
  }
  const schema = JSON.parse(described.stdout);
  if (schema.plugin !== manifest.id) {
    console.error(`[build-plugins] ${library} reports plugin ${schema.plugin}, the manifest says ${manifest.id}`);
    process.exit(1);
  }
  writeFileSync(join(workspace, entry.name, 'schema.json'), described.stdout);
  staged += 1;
}
console.log(`[build-plugins] staged ${staged} plugin packages from ${output}`);
