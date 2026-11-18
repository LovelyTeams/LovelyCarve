import type { Translator } from '../../../i18n/translate';

// BCP codes checked against the GoldenEyes BCP export dictionaries.
const applications: readonly [label: string, icon: string, aliases: string[]][] = [
  ['微信', 'wechat.png', ['1030036', '1420070', '1320008', '1330013', 'WeChat', 'com.tencent.xin', 'com.tencent.mm']],
  ['企业微信', 'wxwork.jpg', ['1030236', 'WeCom', 'com.tencent.wework']],
  ['Telegram', 'telegram.png', ['1030063', 'Telegram（电报）', 'org.telegram.messenger']],
  ['Discord', 'discord.png', ['com.discord']],
  ['QQ邮箱', 'qqmail.jpg', ['1010008', 'QQ 邮箱', 'com.tencent.qqmail']],
  ['Brave', 'brave.png', ['Brave浏览器', 'com.brave.browser']],
  ['Chrome 浏览器', 'chrome.jpg', ['1560019', 'Chrome', 'Chrome浏览器', 'com.android.chrome']],
  ['Edge 浏览器', 'edge.png', ['1560027', 'Edge', 'Edge浏览器', 'Microsoft Edge', 'com.microsoft.emmx']],
  ['Firefox 浏览器', 'firefox.jpg', ['1560006', 'Firefox', 'Firefox浏览器', '火狐浏览器', 'org.mozilla.firefox']],
  ['Safari', 'safari.jpg', ['1560028', 'Safari浏览器', 'com.apple.mobilesafari']],
  ['360安全浏览器', '360browser.jpg', ['1560004', '360浏览器']],
  ['知乎', 'zhihu.jpg', ['1070055', '1190024', 'com.zhihu.android']],
  ['钉钉', 'dingding.jpg', ['1030162', 'DingTalk', 'com.alibaba.android.rimet']],
  ['脉脉', 'maimai.jpg', ['1070109']],
  ['X', 'twitter.png', ['Twitter', 'com.twitter.android']],
  ['腾讯视频', 'tencentvideo.jpg', ['1400020', '1420039', 'com.tencent.qqlive']],
  ['哔哩哔哩', 'bilibili.png', ['1400024', 'Bilibili', 'tv.danmaku.bili']],
  ['快手', 'kuaishou.png', ['1400026', 'com.smile.gifmaker']],
  ['抖音', 'douyin.jpg', ['1400036', '抖音短视频', 'com.ss.android.ugc.aweme']],
  ['携程旅行', 'ctrip.jpg', ['1230005', '1260004', '1460078', '1530001', '1540001', '携程', '携程网', 'ctrip.android.view']],
  ['航旅纵横', 'umetrip.jpg', ['1260010', 'com.umetrip.android.msky.app']],
  ['12306', '12306.png', ['1230017', '1260008', '铁路12306', 'com.MobileTicket']],
  ['高德地图', 'gaodemap.jpg', ['1440003', '1460064', '1510003', 'com.autonavi.minimap']],
  ['百度地图', 'baidumap.jpg', ['1440004', '1510004', 'com.baidu.BaiduMap']],
  ['百度网盘', 'baidunetdisk.png', ['1280015', 'com.baidu.netdisk']],
  ['支付宝', 'alipay.jpg', ['1030203', '1290007', '支付宝聊天', 'com.eg.android.AlipayGphone']],
  ['京东', 'jingdong.png', ['1220005', 'com.jingdong.app.mall']],
  ['淘宝', 'taobao.jpg', ['1220007', '1460059', '淘宝网', 'com.taobao.taobao']],
  ['闲鱼', 'idlefish.jpg', ['1220069', 'com.taobao.idlefish']],
  ['小红书', 'xiaohongshu.jpg', ['1220068', 'com.xingin.xhs']],
  ['拼多多', 'pinduoduo.jpg', ['1220066', 'com.xunmeng.pinduoduo']],
  ['大众点评', 'dazhongdianping.png', ['1220050', '1230029', '1420026', '1460063', '大众点评网']],
  ['美团', 'meituan.jpg', ['1220040', '1230034', '美团网', 'com.sankuai.meituan']],
];

const byAlias = new Map<string, { id: string; label: string; icon: string }>();
for (const [label, icon, aliases] of applications) {
  const app = { id: icon, label, icon };
  for (const alias of [label, ...aliases]) byAlias.set(alias.toLowerCase(), app);
}

export function bcpApplication(raw: string, tr: Translator, recordType: string) {
  if (!raw) return { id: '', label: tr("基本信息"), icon: 'baseinfo.png' };
  const app = byAlias.get(raw.toLowerCase());
  if (app) return app;
  if (recordType === 'bcp.emails' || recordType.startsWith('bcp.email_')) {
    if (raw === '01007' || raw === '01999') return { id: 'other-mail', label: tr("其他邮件"), icon: '' };
  }
  if (raw === '1569999' || raw === '03999' || (raw === '9999999' && recordType.startsWith('bcp.browser_'))) {
    return { id: 'other-browser', label: tr("其他浏览器"), icon: '' };
  }
  if (raw === '1039999' || (raw === '9999999' && [
    'bcp.messaging_accounts', 'bcp.direct_messages', 'bcp.group_messages', 'bcp.groups',
  ].includes(recordType))) {
    return { id: 'other-messaging', label: tr("其他即时通讯"), icon: '' };
  }
  if (raw === '1229999') return { id: raw, label: tr("其他交易平台"), icon: '' };
  if (raw === '1339999') return { id: raw, label: tr("其他社交平台"), icon: '' };
  if (raw === '1429999' && recordType === 'bcp.app_searches') return { id: raw, label: tr("其他应用搜索"), icon: '' };
  if (raw === '1529999') return { id: raw, label: tr("其他出行服务"), icon: '' };
  if (raw === '9999999' || /^[0-9]{3}9999$/.test(raw)) {
    return { id: raw, label: tr("未标明应用"), icon: '' };
  }
  return { id: raw, label: /^\d+$/.test(raw) ? tr("应用编号 {0}", { 0: raw }) : raw, icon: '' };
}
