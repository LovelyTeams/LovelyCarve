# 贡献指南

欢迎提交问题、文档修正、格式支持与功能改进。开始开发前阅读 [架构说明](docs/ARCHITECTURE.md) 与仓库约定 [AGENTS.md](AGENTS.md)。修改界面时同时阅读 [前端规范](docs/FRONTEND.md) 和 `src/styles/` 的主题、基元与动效样式；修改读取器时先读 [格式与读取器](docs/formats/README.md) 及对应的格式专题。

## 环境

按照 [开发文档](docs/DEVELOPMENT.md) 安装依赖，并在仓库根目录执行：

```text
just bootstrap
just --list
```

任务统一通过 `just` 执行。前端开发用 `just dev-fe`，Windows 桌面开发用 `just dev`，命令行开发用 `just run-cli`。默认关闭 QR/OCR，需要这些功能时使用 `*-full` 入口并配置额外依赖。

## 提交问题

在 [GitHub Issues](https://github.com/LovelyTeams/LovelyCarve/issues) 选择问题或功能建议模板。说明操作系统、应用版本或提交号、证据格式、操作步骤、实际结果与预期结果。读取问题尽量附可公开的最小合成样本、错误信息和相关偏移。性能问题附数据规模、耗时与机器配置。安全漏洞按 [SECURITY.md](SECURITY.md) 报告。

不要提交真实检材、个人数据、密钥或案件目录。公开测试数据需记录来源、许可证和校验值；外部样本应采用可选测试参数接入，不依赖开发者的固定本机路径。

## 修改约定

- 沿用现有模块和接口，保持改动聚焦，保留与本次工作无关的变更。
- Rust 核心逻辑放在 `src-tauri/src/core/` 的所属领域，命令包装放在 `commands/`，跨域编排放在 `app/workflows/`，长期状态由 `AppState` 管理；核心不依赖应用层或 Tauri。
- Vue 使用 Composition API；业务状态放在 `src/features/<功能>/composables/`，通用能力放在 `src/composables/`，跨功能接线由 `src/app/` 持有。Tauri 调用和事件只通过 `src/lib/ipc/`。
- 跨 IPC 功能同时更新核心实现、命令、注册表、TypeScript 类型和调用方。
- 长任务接入取消、进度和错误反馈。整盘哈希与导出检查完整长度，失败或取消清理部分输出。
- 界面文案通过 `tr()` 提供中英文版本，默认中文；新增文案同步维护词典并运行 `just i18n-check`。使用已有主题 token、`ui-*` 控件和 lucide 图标，验证三套主题、两种语言与窄窗口。
- 用户可见变化按 [Keep a Changelog 0.3.0](https://keepachangelog.com/zh-CN/0.3.0/) 更新 `CHANGELOG.md` 的 `[Unreleased]`，使用英文变更分类并概括用户影响；行为、配置或流程变化同步更新对应文档（文档分工见 [文档导航](docs/README.md)）。
- 新增第三方代码、数据或资源时保留版权、许可证和来源声明。

## 验证

| 改动 | 最低验证 |
| --- | --- |
| 前端 | `just typecheck`、`just test-fe`、`just build-fe`；结构改动另跑 `just frontend-structure-check`、`just ipc-check` |
| Rust | `just fmt`、`just check`、`just clippy`、`just test`；模块迁移或公共读取接口改动另跑 `just check-examples` |
| IPC | 前端与 Rust 检查，以及 `just ipc-check` |
| 插件工作区 | `just fmt`、`just clippy-plugins`、`just test-plugins`；SDK 改动另跑 Rust 检查 |
| Vendored 读取器 | `just fmt`、对应 `just test-*`；主机集成变化另跑 Rust 检查 |
| 仅文档 | 检查相对链接、路径、命令名称和描述与当前实现一致 |

针对解析器、寻址、取消和数据完整性提供有意义的回归样本。不要用进程环境变量作为测试间共享状态。无法完成某项验证时，在提交说明中写明原因。

## 拉取请求

Fork 仓库后创建工作分支，在本地完成改动与对应验证，再向主仓库提交 Pull Request。较大的格式支持、架构变化或新依赖先通过 issue 说明范围，便于确认接口和回归样本。

提交标题使用 Conventional Commits，例如 `fix(ntfs): ...`、`feat(plugin): ...`、`docs: ...`。

描述应写明问题、最终行为、相关 issue 和实际运行的 `just` 配方。界面变化附截图或录屏，性能变化附样本说明与测量结果。在 PR 中报告实际完成的本地验证。

仓库不配置 GitHub Actions，贡献者需完成上表的本地验证。维护与发布见 [发布指南](docs/RELEASING.md)。

除明确另有声明的第三方内容外，提交到本项目的贡献采用 [AGPL-3.0-only](LICENSE)。请确认你有权提交相关代码、文档与资源，并保留第三方内容原有的版权和许可证声明。
