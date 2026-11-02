# LovelyCarve DSH 取证模式

为 DeepSeek Harness 增加可选择的 **LovelyCarve 取证模式**，随包提供 Windows CLI、运行库、插件宿主、Linux 信息插件及 lovelycarve-cli skill。

每次调查先创建或打开对应案件，加载检材后等待任务并核验所需卷索引，再开展浏览、读取与分析；无法建索引的卷明确记录覆盖限制。流程按问题确定范围：版本查询读取系统盘上的权威文件，无校验需求时不自动计算整盘哈希；克隆主机可按成员盘限定 RAID/LVM 检测。区分已安装版本、引导配置和有证据支持的运行态。

使用 forensic-board MCP 解题时，复用宿主已有连接，按 skill 的 `references/forensic-board.md` 管理题单、候选和线索。每题先读取最新原题与线索，有依据的答案立即逐题提交并回读，包括顺带发现的其他题；检材归属、账号密码、人物关系与连接信息按实际来源及时保存、跨题复用。超时先读取保存状态再补缺失项，不创建本地答案白板。模式复用宿主 MCP 连接，不创建连接、计时器或自动提交。宿主提供 `tools/execute` 钩子时，模式跟踪同一聊天实际发生的看板调用：答案提交前要求刷新该题和相关线索；线索写入前要求查重；写入后（包括超时）先回读，再继续调查。回读核对真实答案 ID、值及提交的理由和证据；确认缺失后仅允许修复原题或原竞赛的写入。工具默认识别 `forensic-board` / `forensic_board` MCP 名称，自定义名称可在 preset 的 `boardTools` 中将操作名映射到实际工具名。状态只在内存中跟踪流程，不另存答案白板，也不会判断证据是否足以解题。

模式提供 `lovelycarve` 结构化工具。例如先调用 `{"command":"sessions open","args":{"name":"<唯一会话名>"}}`，再用 `{"session":"<唯一会话名>","command":"cases create","args":{"dir":"<新案件绝对目录>","name":"<案件名>"}}` 建案。参数名以 `schema <命令>` 为准；后续请求显式带 `session`，复用同一进程中的检材与任务。

工具通过 DSH 执行器运行 CLI，保留沙箱策略和取消信号，自动设置 `LOVELYCARVE_AGENT=1`，返回请求 `id`、`exitCode`、`result`、`error`、`warning` 和 `outputTruncated`。结果完整保留分页与时区元数据；超过 48 KiB 时返回明确错误和分页信息，不截断 JSON 冒充完整结果。使用小页、`fields` 和分段文本读取；原始字节用 `files export` 导出。超时或中断后检查命名会话中的任务，不能假定服务端工作已停止。工作完成后关闭自己创建的会话。

源文件位于主仓库 integrations/dsh-lovelycarve/，技能复用 skills/lovelycarve-cli/。在仓库根运行 just stage-dsh，从现有 release 产物生成完整插件；修改 CLI 后先运行 just build-cli。此目录的源码不含运行文件，安装应使用生成包，参见 INSTALL.md。

开发验证使用 `just test-dsh`，包含工具错误、警告、截断、取消、命令引用和实际 CLI 会话测试；集成测试需要已构建的 debug CLI，也可通过 `LOVELYCARVE_TEST_CLI` 指定其他二进制。

结构化发现不需要会话：`{"command":"schema files list"}` 或 `{"command":"files list","help":true}`。不要把 `--help` 写进 `command`。`usage` 错误的 `details.allowedFields` 列出合法顶层字段和 `args` 字段，`schemaRequest` 给出发现请求；可确定修正时返回 `retryRequest`，由调用方显式重试。涉及凭据的示例会隐藏内联秘密，不把占位符作为可执行重试。

完整请求示例：

```json
{"command":"files list","session":"exam","args":{"path":"/","pageSize":20},"fields":"id,name,kind,size"}
{"command":"indexes list","session":"exam"}
{"command":"sessions close","args":{"name":"exam"}}
```

`fields` 位于顶层；`indexes list` 是案件级命令，不接收 `imageId`。卷映射与可用索引键用 `indexes volumes list`，其中 `indexes[].pathPrefix` 限定父索引中的逻辑卷子树；没有索引或索引被截断时，索引文件查询明确失败。

调查结果的 `source` 来自 CLI 同次执行中解析后的检材和节点，包含源路径、实际 imageId、文件系统节点及可解析的证据内路径；切换检材或相同节点 ID 不会改变已返回结果的来源。SQLite 表记录额外附带数据库节点、表名与各行 `recordKeys`，其中主键属于 `sqlite` 地址空间，不得拼成文件系统节点 ID。没有可用主键或无法解析路径时明确保留未知。

输出超限时 `recovery.restartCurrentPage` 为真，安全的读取返回缩小范围后的 `recovery.retryRequest`；它保留原 cursor/offset。当前页已经被丢弃，不能沿 `pagination.nextCursor` 跳过它。导出、变更和任务命令不提供重放建议。

升级包含来源封装的 CLI 后，使用原版本结束旧常驻会话，再用新版本创建会话；常驻传输协议已更新，JSONL 默认响应形状保持不变，来源封装按请求启用。
