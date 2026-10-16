
import { tr } from '../../../composables/useLocale';
import type { Component } from 'vue';
import { HardDrive, Layers, Grid2X2, Ruler, FileImage, FileText, FileArchive, Database, FileAudio, FileVideo } from 'lucide-vue-next';
import type { CarveCategory, CarveScope } from '../../../lib/ipc/carve';
export const scopeOptions: { value: CarveScope; label: string; icon: Component }[] = [
  { value: 'whole', get label() { return tr("整个镜像"); }, icon: HardDrive },
  { value: 'partition', get label() { return tr("指定分区"); }, icon: Layers },
  { value: 'unallocated', get label() { return tr("分区间隙"); }, icon: Grid2X2 },
  { value: 'range', get label() { return tr("扇区范围"); }, icon: Ruler },
];
export function categoryIcon(category: CarveCategory): Component {
  return {
    image: FileImage,
    document: FileText,
    archive: FileArchive,
    database: Database,
    audio: FileAudio,
    video: FileVideo,
  }[category];
}
