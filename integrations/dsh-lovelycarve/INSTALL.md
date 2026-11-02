# 安装与更新

1. 完全退出 DSH。
2. 将生成的完整插件目录复制到目标 profile，例如 C:\Users\<you>\.dsh\profiles\desktop\plugins\dsh-lovelycarve。
3. 在该 profile 的 package.json 中配置本地依赖：`"dsh-lovelycarve": "link:C:/Users/<you>/.dsh/profiles/desktop/plugins/dsh-lovelycarve"`。
4. 在同一文件的 dsh.profile.bundles 中加入 `"dsh-lovelycarve"`，通过 DSH 的 profile 包管理流程安装依赖。
5. 重启 DSH，创建新对话，选择 **LovelyCarve 取证模式**。

开发环境也可链接仓库中的 dist-pack/dsh-lovelycarve。just stage-dsh 会刷新此目录，旧包保留在 dist-pack/dsh-previous-*/；重启并使用新对话加载新的提示词与技能。

CLI 位于 skills/lovelycarve-cli/bin/，复制时保留 DLL、plugins、resources 与许可证。使用带 --image-ids 的范围检测需同时更新 CLI、技能与模式；不要只替换提示词。已运行的 CLI 会话使用旧进程，完成工作后关闭，由新对话创建新会话。
