import { sourceText, type Translator } from '../../../i18n/translate';
import type { BcpDataset } from '../../../lib/ipc/bcp';

export const BCP_CATEGORIES = [
  { prefix: '01', label: '设备与通信', icon: 'Phone' },
  { prefix: '02', label: '即时通信', icon: 'Send' },
  { prefix: '03', label: '社交动态', icon: 'Share2' },
  { prefix: '05', label: '浏览器记录', icon: 'Globe' },
  { prefix: '07', label: '交易与支付', icon: 'Database' },
  { prefix: '08', label: '网络位置', icon: 'Wifi' },
  { prefix: '09', label: '应用与媒体', icon: 'Package' },
  { prefix: '10', label: '文件清单', icon: 'Files' },
  { prefix: '16', label: '检材信息', icon: 'Info' },
] as const;

const DATASET_NAMES: Record<string, string> = {
  '010100': '采集信息', '010200': '设备信息', '010201': '设备标识', '010202': '操作系统',
  '010300': 'SIM 卡', '010400': '联系人', '010500': '联系人字段', '010600': '通话记录',
  '010700': '短信', '010800': '彩信', '010900': '日程', '011000': '同步账号',
  '020100': '即时通信账号', '020200': '好友', '020300': '群组', '020400': '群成员',
  '020500': '私聊消息', '020600': '群聊消息', '020700': '动态消息', '020800': '搜索记录',
  '030100': '社交账号', '030200': '关注与好友', '030300': '发布内容', '030400': '私信',
  '030500': '搜索记录', '050200': '书签', '050300': '访问历史', '050400': 'Cookie',
  '050500': '网站账号', '050600': '浏览器搜索', '070100': '支付账号', '070400': '交易记录',
  '070500': '支付记录', '070800': '红包记录', '080100': 'Wi-Fi 记录',
  '090100': '应用账号', '090200': '媒体关联', '090300': '已装应用',
  '090400': '媒体文件', '090600': '文件哈希', '109999': '关联文件', '160100': '检材详情',
};

export function bcpSuffix(code: string): string {
  return code.match(/(\d{6})$/)?.[1] ?? code;
}

export function bcpDatasetLabel(dataset: BcpDataset, tr: Translator = sourceText): string {
  const suffix = bcpSuffix(dataset.code);
  return (DATASET_NAMES[suffix] ? tr(DATASET_NAMES[suffix]) : undefined) ?? tr("{0} 数据", { 0: suffix });
}

export function bcpCategory(code: string): string {
  const prefix = bcpSuffix(code).slice(0, 2);
  return BCP_CATEGORIES.some(category => category.prefix === prefix) ? prefix : 'other';
}
