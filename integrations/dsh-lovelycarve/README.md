# LovelyCarve DSH 取证模式

为 DeepSeek Harness 增加可选择的 **LovelyCarve 取证模式**，随包提供 Windows CLI、运行库、插件宿主、Linux 信息插件及 lovelycarve-cli skill。

流程按问题确定范围：版本查询先读取系统盘上的权威文件，无校验需求时不自动计算整盘哈希；克隆主机可按成员盘限定 RAID/LVM 检测。已知路径读取无需等待完整目录索引，索引查询仍核验目标卷的发布状态。每次 CLI 调用检查退出码和完整性，区分已安装版本、引导配置和有证据支持的运行态。

源文件位于主仓库 integrations/dsh-lovelycarve/，技能复用 skills/lovelycarve-cli/。在仓库根运行 just stage-dsh，从现有 release 产物生成完整插件；修改 CLI 后先运行 just build-cli。此目录的源码不含运行文件，安装应使用生成包，参见 INSTALL.md。
