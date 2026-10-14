import { writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { pluginBundle } from './lib/plugin-bundle.mjs';

const repository = fileURLToPath(new URL('..', import.meta.url));
const { config, plugins } = pluginBundle(repository, text => Bun.TOML.parse(text));
writeFileSync(new URL('../src-tauri/tauri.plugins.generated.conf.json', import.meta.url), `${JSON.stringify(config, null, 2)}\n`);
console.log(`[plugin-bundle] generated resources for ${plugins.length} plugins`);
