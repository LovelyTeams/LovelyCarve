# LovelyCarve DSH 取证模式

为 DeepSeek Harness 增加可选择的 **LovelyCarve 取证模式**，随包提供 Windows CLI、运行库、插件宿主、Linux 信息插件及 lovelycarve-cli skill。

每次调查先创建或打开对应案件，加载检材后等待任务并核验所需卷索引，再开展浏览、读取与分析；无法建索引的卷明确记录覆盖限制。流程按问题确定范围：版本查询读取系统盘上的权威文件，无校验需求时不自动计算整盘哈希；克隆主机可按成员盘限定 RAID/LVM 检测。区分已安装版本、引导配置和有证据支持的运行态。

模式提供 `lovelycarve` 结构化工具。例如先调用 `{"command":"sessions open","args":{"name":"<唯一会话名>"}}`，再用 `{"session":"<唯一会话名>","command":"cases create","args":{"dir":"<新案件绝对目录>","name":"<案件名>"}}` 建案。参数名以 `schema <命令>` 为准；后续请求显式带 `session`，复用同一进程中的检材与任务。

工具通过 DSH 执行器运行 CLI，保留沙箱策略和取消信号，自动设置 `LOVELYCARVE_AGENT=1`，返回请求 `id`、`exitCode`、`result`、`error`、`warning` 和 `outputTruncated`。结果完整保留分页与时区元数据；超过 48 KiB 时返回明确错误和分页信息，不截断 JSON 冒充完整结果。使用小页、`fields` 和分段文本读取；原始字节用 `files export` 导出。超时或中断后检查命名会话中的任务，不能假定服务端工作已停止。工作完成后关闭自己创建的会话。

源文件位于主仓库 integrations/dsh-lovelycarve/，技能复用 skills/lovelycarve-cli/。在仓库根运行 just stage-dsh，从现有 release 产物生成完整插件；修改 CLI 后先运行 just build-cli。此目录的源码不含运行文件，安装应使用生成包，参见 INSTALL.md。

开发验证使用 `just test-dsh`，包含工具错误、警告、截断、取消、命令引用和实际 CLI 会话测试；集成测试需要已构建的 debug CLI，也可通过 `LOVELYCARVE_TEST_CLI` 指定其他二进制。
