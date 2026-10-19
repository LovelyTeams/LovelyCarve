# 安全报告

## 报告渠道

安全问题请使用本仓库 [Security Advisories](https://github.com/LovelyTeams/LovelyCarve/security/advisories) 中的 **Report a vulnerability** 私密报告入口。如果未显示该入口，请先提交一个只询问私密联系方式的 issue，不公开漏洞细节、利用样本或真实检材。入口需要维护者在 GitHub 开启，操作见 [GitHub 私密报告说明](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately)。

报告请包含受影响的版本或提交号、操作系统、触发功能、预期与实际行为、影响范围，以及可公开的最小合成样本或复现脚本。普通读取失败、功能建议和使用问题通过 [Issues](https://github.com/LovelyTeams/LovelyCarve/issues) 反馈。

项目目前处于开发阶段；修复在当前开发分支和后续发布版本中提供，暂不维护独立的长期支持分支。报告时请标明版本，不假定旧版本已获得补丁。

## 执行边界

- 镜像与文件内容来自外部证据，解析器、文档预览和模型推理应按不可信输入处理。越界读取、任意文件写入、非预期代码执行和凭据泄露均属于安全问题。
- 已发现或安装的 LPP 插件包会被视为可信并可执行；进程隔离用于承载和故障隔离，不提供操作系统权限沙箱。只安装来源可信的插件包，插件接口和宿主行为见 [插件协议](docs/PLUGIN_PROTOCOL.md)。
- 只读证据访问不代表程序不会写入主机：案件、索引、缓存、临时预览和导出都有各自的输出目录。
- 日志、截图和案件数据库可能包含路径、文件名、账户与解锁信息。提交报告前移除真实个人数据、案件内容和密钥。
