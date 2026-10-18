import { sourceText, type Translator } from '../../../i18n/translate';
import type { DetectRoute, EvidencePlatform } from '../../../lib/ipc/autodetect';


// ── 展示映射(中文优先) ─────────────────────────────────────────────────────

export const PLATFORM_LABELS: Record<EvidencePlatform, string> = {
  android: 'Android',
  ios: 'iOS',
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
  harmony: '鸿蒙 HarmonyOS',
  huaweihmos: '鸿蒙 HMOS',
  othermobile: '其他移动设备',
};

export function platformLabel(platform: EvidencePlatform | null | undefined, tr: Translator = sourceText): string {
  if (!platform) return tr("未识别");
  return tr(PLATFORM_LABELS[platform] ?? platform);
}

export const ROUTE_LABELS: Record<DetectRoute, string> = {
  disk_image: '磁盘镜像路',
  archive: '归档/备份文件路',
  phone_image: '手机镜像路',
};

export function routeLabel(route: DetectRoute, tr: Translator = sourceText): string {
  return tr(ROUTE_LABELS[route] ?? route);
}

/** `evidence.type` 词 → 展示文字;备份枚举词原样返回(它们本身就是标识)。 */
export function evidenceTypeLabel(evidenceType: string, tr: Translator = sourceText): string {
  switch (evidenceType) {
    case 'disk':
      return tr("磁盘镜像");
    case 'compressfile':
      return tr("归档 / 逻辑检材");
    case 'backup':
      return tr("备份文件");
    default:
      return evidenceType;
  }
}

/** 厂商关键词首字母大写。 */
export function manufactorLabel(manufactor: string | null | undefined): string | null {
  if (!manufactor) return null;
  return manufactor.charAt(0).toUpperCase() + manufactor.slice(1);
}
