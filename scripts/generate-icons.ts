// Regenerates the platform icons from src-tauri/icons/source.png with `tauri icon`, then the
// frontend's 512 px brand images: the transparent one from that icon set, the one with a
// background resized from source-background.png by Bun.Image.
import { join } from 'node:path';

const projectRoot = join(import.meta.dir, '..');
const iconsDirectory = join(projectRoot, 'src-tauri', 'icons');
const assetsDirectory = join(projectRoot, 'src', 'assets');

const source = join(iconsDirectory, 'source.png');
const result = Bun.spawnSync([process.execPath, 'run', 'tauri', 'icon', source], { cwd: projectRoot, stdio: ['inherit', 'inherit', 'inherit'] });
if (!result.success) throw new Error(`图标生成失败：${source}（${result.signalCode ?? result.exitCode}）`);
await Bun.write(join(assetsDirectory, 'app-icon.png'), Bun.file(join(iconsDirectory, 'icon.png')));

await Bun.file(join(iconsDirectory, 'source-background.png')).image()
  .resize(512, 512)
  .png({ compressionLevel: 9 })
  .write(join(assetsDirectory, 'app-icon-background.png'));
