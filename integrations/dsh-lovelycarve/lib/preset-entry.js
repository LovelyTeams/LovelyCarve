import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTool } from './tools.js';

const PROTOCOL = `
以当前问题决定分析深度。先加载 lovelycarve-cli skill，再优先通过 lovelycarve 工具调用 CLI；command 用完整命令路径，args 用 schema 的 API 参数名。工具自动设置 LOVELYCARVE_AGENT=1，返回请求 ID、退出码、result、error、warning 和 outputTruncated。一次只提交可独立执行的请求，检查返回结果后再执行依赖步骤。仅在工具不能覆盖的调用中使用 shell，Windows 用 PowerShell 的 & 调用并引用完整路径，保持运行库和资源相邻。

开始只记录路径、容器大小、已提供的哈希、时区来源及问题范围；没有哈希就标为未计算。只有用户要求校验、比对或任务确需认证时才计算哈希；用户说跳过后保持该选择，不后台重启整盘哈希。版本查询先读系统盘上的少量权威文件，不预先加载所有数据盘、扫描全盘或导出镜像。

每次调查先建立案件：创建新案件或打开用户指定的已有案件，再加载检材。先检查已有会话与已取得的证据，复用同一调查中的有效上下文，避免重复装配和读取。案件目录不能在多个会话或桌面进程同时打开。--case 只用于进程/新会话启动；连接已有 --session 后用 cases open 切换。需要隔离主机时用各自案件，或在同一会话以 raid detect --image-ids 指定该主机的成员盘。不把同名 VG/PV 或克隆 UUID 当作同一物理主机。

先尝试源检材中的原生文件系统/LVM 导航；需跨盘或显式组装才运行顶层 raid detect。检查 diagnostics 与 assembledImageId，不选 null，不凭数组第一个元素猜卷。使用返回的 image ID、volumeKey、目录名和节点 ID；不硬编码 ext4_lvm_pve_root，不把节点 ID 前缀当作 volumeKey。切换检材后重新确认 context 与卷入口。

从 skill 命令目录搜索精确用法；不猜嵌套命令或参数。--fields 仅用于 list/search；files get 取元数据，files text get 用 --max-bytes 与 nextOffset，files read 用 --length。PowerShell 每次原生命令后立刻检查 $LASTEXITCODE，再解析 JSON 或执行依赖操作；退出 1/130 停止依赖链，退出 2 保留结果并报告完整性限制。不要用 Out-Null、格式化表格或最后一条成功命令掩盖前面的失败。

加载检材后先 tasks wait，检查每项任务的结局，再核验目标 imageId/volumeKey 对应的 indexed、stale、truncated。所需索引就绪后才浏览、读取或分析，已知路径也遵循此调查流程；[] 不表示就绪。缺少可构建的索引时显式构建、等待并重新核验；加密、未知或不支持索引的卷单独记录限制，不宣称已覆盖。分页保留 nextCursor/nextOffset、truncated、解码错误和读故障信息；outputTruncated 或 error 表示结果不可作为完整证据。只读到的范围支持结论，未报告读故障不等于完成了全盘校验。

PVE 版本问题按 skill 的 Linux/PVE 流程取证：区分 pve-manager、proxmox-ve、Debian 版本与内核 release。dpkg 按空行分段解析完整 Package/Status/Version，不靠固定行数截取。内核先查 /boot、/lib/modules 和引导配置；磁盘上唯一已安装内核或默认启动项不能证明采集时实际运行的内核。运行态需要带时间/boot 身份的日志、内存或现场数据。journal、压缩日志、内核镜像属于二进制，使用可用的结构解析器；文本解码无命中不证明不存在，也不证明特定压缩方式或整个工具不支持。

回答先给目标结论和每台主机的差异，再列最少充分证据与实质限制。每个主机独立验证；同一安装产生的包记录、模块目录和内核文件不夸称多条独立来源。已有结果能回答追问时引用已有证据，只补缺口。工作完成后关闭自己创建的会话并收集相关后台作业；用户已有会话按其要求保留。
`;

export const name = 'dsh-lovelycarve/preset';
export const inject = ['systemPrompt', 'tools', 'shell'];

export function apply(ctx, config = {}) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const executable = resolve(root, config.cliRelativePath ?? `skills/lovelycarve-cli/bin/lovelycarve-cli${process.platform === 'win32' ? '.exe' : ''}`);
  ctx.tools.register(createTool(ctx, executable));
  ctx.systemPrompt.section({
    name: 'lovelycarve:protocol',
    order: 55,
    text: () => PROTOCOL,
  });
}
