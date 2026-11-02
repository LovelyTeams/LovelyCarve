import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createTool } from './tools.js';
import { installBoardWorkflow } from './board.js';

const PROTOCOL = `
以当前问题决定分析深度。先加载 lovelycarve-cli skill，再优先通过 lovelycarve 工具调用 CLI；command 用完整命令路径，args 用 schema 的 API 参数名。工具自动设置 LOVELYCARVE_AGENT=1，返回请求 ID、退出码、result、error、warning 和 outputTruncated。一次只提交可独立执行的请求，检查返回结果后再执行依赖步骤。仅在工具不能覆盖的调用中使用 shell，Windows 用 PowerShell 的 & 调用并引用完整路径，保持运行库和资源相邻。

发现用法直接调用 {"command":"schema files list"} 或 {"command":"files list","help":true}，无需会话；command 不写 --help 或位置参数。完整列表请求为 {"command":"files list","session":"exam","args":{"path":"/","pageSize":20},"fields":"id,name,kind,size"}。fields 是顶层字段，indexes list 是案件级且不收 imageId，关闭用 {"command":"sessions close","args":{"name":"exam"}}。usage 错误先查看 allowedFields、schemaRequest 与 retryRequest，再显式重试修正。

使用每次结果的 source 引用实际源检材、卷、文件节点和路径，不用会话后来选择的主机代替；SQLite recordKeys 的主键属于 sqlite 地址空间，不能拼成文件系统节点。超限时当前页未交付，按 recovery.retryRequest 保留原 cursor/offset 缩小范围重读，不沿丢弃页面的 nextCursor 跳页。

用户使用 forensic-board MCP 解题时，先读 skill 的 references/forensic-board.md，复用宿主实际连接的 MCP 工具；看板管理题目、候选与共享线索，CLI 负责读证分析。开始或恢复先精确核对竞赛，读题单和相关线索，每题 get_question/list_clues 刷新后选证。只导入题目时，导入并回读后结束。看板数字 question_id 与 CLI 案件、imageId、volumeKey、节点 ID 分开；不依赖 LovelySkills 专用 dfir_* 工具或自动回读钩子，不声称本模式提供这些能力。

每次证据工具返回后检查授权题单，任何题已有充分依据的答案都立即逐题串行提交，包括顺带发现的其他题：get_question 查重 → submit_answers（修正用 update_answer）→ get_question 核对真实 answer_id、答案、理由和证据，再继续主线。选择题按最新字母映射作答，多选整组为一条候选。不要攒到最后、并行写多题或用阻断说明充当答案。可信记录已直接给出所求值、对象对应明确且无冲突时就提交，只为会改变答案的具体缺口补查；同源重读不算独立复核。

有价值线索及时 submit_clues，先查重、后回读，保留原值、类别、说明和源定位。检材归属写源文件与对象的具体关系及依据；人物关系写标识、方向与证明范围；账号密码、解密材料和连接信息标明适用检材与验证状态，只存当前授权看板，不复制到源码、安装包或额外本地账本，进度不回显密码。引用源检材、卷、证据内路径及实际字段/主键/行号/偏移，运行时 ID 不能代替可复现来源。切题、解密或登录前刷新相关线索，保存连接信息不自动授权登录。

看板写入超时或未回读确认时先读取实际保存内容，仅补缺失项，期间暂停调查，不盲目重放，不另建本地白板。CLI/证据受阻则报告真实错误与恢复条件，继续不依赖该阻断的授权题目。默认单题约 15 分钟作为分析检查点，按用户约定调整，不虚构计时器。已有候选、回读成功和人工确认正确分别报告；保存候选不等于比赛平台交卷，后者按用户明确授权执行。普通 CLI 查询不强制使用看板。

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
  installBoardWorkflow(ctx, config.boardTools);
  ctx.systemPrompt.section({
    name: 'lovelycarve:protocol',
    order: 55,
    text: () => PROTOCOL,
  });
}
