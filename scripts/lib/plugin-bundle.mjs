// One manifest-derived resource map for installers, portable builds and CLI packages.
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from 'node:fs';
import { isAbsolute, join, relative, sep } from 'node:path';

function relativeFile(directory, path) {
  if (typeof path !== 'string' || !path || isAbsolute(path) || path.includes('\\')
    || path.split('/').some(part => !part || part === '.' || part === '..' || part.includes(':'))) {
    throw new Error(`Invalid plugin resource path: ${path}`);
  }
  const file = join(directory, path);
  if (!statSync(file).isFile()) throw new Error(`Not a plugin file: ${file}`);
  const contained = relative(realpathSync(directory), realpathSync(file));
  if (isAbsolute(contained) || contained === '..' || contained.startsWith(`..${sep}`)) {
    throw new Error(`Plugin resource escapes its directory: ${file}`);
  }
  return file;
}

export function pluginBundle(repository, parseToml, platform = process.platform) {
  const workspace = join(repository, 'plugins');
  const tauri = join(repository, 'src-tauri');
  const [prefix, suffix] = platform === 'win32' ? ['', '.dll'] : platform === 'darwin' ? ['lib', '.dylib'] : ['lib', '.so'];
  const hostName = `lc-plugin-host${platform === 'win32' ? '.exe' : ''}`;
  const host = join(workspace, 'target', 'release', hostName);
  if (!existsSync(host)) throw new Error(`Missing ${host}; run just build-plugins --release`);
  const resources = { [relative(tauri, host).split(sep).join('/')]: hostName };
  const plugins = [];
  const ids = new Set();
  for (const entry of readdirSync(workspace, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isDirectory()) continue;
    const directory = join(workspace, entry.name);
    const manifestPath = join(directory, 'plugin.toml');
    if (!existsSync(manifestPath)) continue;
    const manifest = parseToml(readFileSync(manifestPath, 'utf8'));
    if (typeof manifest.id !== 'string' || !/^[A-Za-z0-9._-]+$/.test(manifest.id) || ids.has(manifest.id)) {
      throw new Error(`Invalid or duplicate plugin ID in ${manifestPath}`);
    }
    ids.add(manifest.id);
    const declared = manifest.library?.path;
    if (typeof declared !== 'string' || !declared) throw new Error(`Missing library.path in ${manifestPath}`);
    const parts = declared.split('/');
    if (!/\.(dll|so|dylib)$/i.test(parts.at(-1))) parts[parts.length - 1] = `${prefix}${parts.at(-1)}${suffix}`;
    const assets = manifest.library.resources ?? [];
    if (!Array.isArray(assets) || assets.some(asset => typeof asset !== 'string')) throw new Error(`Invalid library.resources in ${manifestPath}`);
    for (const file of new Set(['plugin.toml', parts.join('/'), ...assets])) {
      const source = relativeFile(directory, file);
      resources[relative(tauri, source).split(sep).join('/')] = `plugins/${entry.name}/${file}`;
    }
    plugins.push({ id: manifest.id, directory: entry.name, version: manifest.version });
  }
  return { config: { $schema: 'https://schema.tauri.app/config/2', bundle: { resources } }, plugins };
}
