// Builds the plugin workspace (plugins/Cargo.toml): the lc-plugin-host loader
// and every plugin library. Each library is then copied next to its
// plugin.toml under the platform file name the manifest's `library` resolves
// to (linuxinfo → linuxinfo.dll / liblinuxinfo.so / liblinuxinfo.dylib), and
// `lc-plugin-host --describe` writes the library's analyzers and record
// schemas beside it as schema.json. Development discovery of the repository
// plugins/ directory, the release bundle and .lcpkg packages see the same
// package layout.
//
//   bun scripts/build-plugins.ts            # debug build
//   bun scripts/build-plugins.ts --release  # release build (bundles, packs)
//   bun scripts/build-plugins.ts --plugin linuxinfo # one library, keep an active host running
//
// Always build from the committed plugin lockfile.
import { join } from 'node:path';
import { globFiles } from './lib/files.ts';

function fail(message: string): never {
  console.error(`[build-plugins] ${message}`);
  process.exit(1);
}

const release = process.argv.includes('--release');
const workspace = join(import.meta.dir, '..', 'plugins');
if (!await Bun.file(join(workspace, 'Cargo.toml')).exists()) {
  fail('plugin submodule is missing; run just bootstrap or git submodule update --init --recursive');
}
const cargoArgs = ['build', '--locked', '--manifest-path', join(workspace, 'Cargo.toml')];
const pluginIndex = process.argv.indexOf('--plugin');
const selected = pluginIndex === -1 ? null : process.argv[pluginIndex + 1];
if (selected !== null) {
  if (!selected || !/^[a-z][a-z0-9-]*$/.test(selected) || !await Bun.file(join(workspace, selected, 'plugin.toml')).exists()) {
    fail('--plugin requires a bundled plugin directory name');
  }
  const manifest = Bun.TOML.parse(await Bun.file(join(workspace, selected, 'Cargo.toml')).text()) as { package?: { name?: unknown } };
  if (typeof manifest.package?.name !== 'string') fail('selected plugin has no Cargo package name');
  cargoArgs.push('--package', manifest.package.name);
}
if (release) cargoArgs.push('--release');

const build = Bun.spawnSync(['cargo', ...cargoArgs], { stdio: ['inherit', 'inherit', 'inherit'] });
if (!build.success) process.exit(build.exitCode ?? 1);

const output = join(workspace, 'target', release ? 'release' : 'debug');
const [prefix, suffix] =
  process.platform === 'win32' ? ['', '.dll'] : process.platform === 'darwin' ? ['lib', '.dylib'] : ['lib', '.so'];
// A --plugin build leaves the loader alone (a running app may hold it open),
// so describing uses the one the last full build produced.
const loader = join(output, process.platform === 'win32' ? 'lc-plugin-host.exe' : 'lc-plugin-host');
if (!await Bun.file(loader).exists()) fail(`${loader} is missing; build the whole workspace once without --plugin`);

let staged = 0;
for (const manifestFile of await globFiles('*/plugin.toml', workspace)) {
  const directory = manifestFile.slice(0, -'/plugin.toml'.length);
  if (selected !== null && directory !== selected) continue;
  const manifestPath = join(workspace, manifestFile);
  const manifest = Bun.TOML.parse(await Bun.file(manifestPath).text()) as { id?: unknown; library?: unknown };
  const name = manifest.library;
  if (typeof name !== 'string' || /[\\/]|\.(dll|so|dylib)$/i.test(name)) fail(`${manifestPath}: library must be the crate's library name`);
  const fileName = `${prefix}${name}${suffix}`;
  const built = join(output, fileName);
  if (!await Bun.file(built).exists()) {
    fail(`${built} was not built; is ${directory} a workspace member with [lib] name = "${name}"?`);
  }
  const library = join(workspace, directory, fileName);
  await Bun.write(library, Bun.file(built));
  const described = Bun.spawnSync([loader, '--describe', '--library', library], { stderr: 'pipe' });
  if (!described.success) fail(`describing ${library} failed:\n${described.stderr.toString()}`);
  const schema = JSON.parse(described.stdout.toString()) as { plugin?: unknown };
  if (schema.plugin !== manifest.id) fail(`${library} reports plugin ${schema.plugin}, the manifest says ${manifest.id}`);
  await Bun.write(join(workspace, directory, 'schema.json'), described.stdout);
  staged += 1;
}
console.log(`[build-plugins] staged ${staged} plugin packages from ${output}`);
