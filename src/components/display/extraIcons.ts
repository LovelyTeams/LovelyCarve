// Lucide 里没有的图形：按 Lucide 的 24px 网格、2px 线宽与圆角端点绘制，
// 经 createLucideIcon 生成，用法与 lucide-vue-next 的图标组件相同（:size、class）。
import { createLucideIcon } from 'lucide-vue-next';

/** Android 系统：简化的 Android 机器人头像（CC BY 3.0，署名见 THIRD_PARTY_NOTICES.md）。 */
export const AndroidHead = createLucideIcon('android-head', [
  ['path', { d: 'M2 18a10 10 0 0 1 20 0Z', key: 'head' }],
  ['path', { d: 'm7.5 10-2-3.5', key: 'antenna-left' }],
  ['path', { d: 'm16.5 10 2-3.5', key: 'antenna-right' }],
  ['path', { d: 'M8 14h.01', key: 'eye-left' }],
  ['path', { d: 'M16 14h.01', key: 'eye-right' }],
]);
