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
- 跨 IPC 功能同时更新核心实现、命令、注册表、`src-tauri/src/cli/commands.toml` 中的 CLI 命令、TypeScript 类型和调用方。
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

### 代码风格与范围

- 以仓库约定、所属模块和相邻代码的写法为准。接收外部 PR 时，先确认问题在当前 `main` 上仍然存在，再整理实现、命名、导入、注释与测试，使其融入现有代码。
- 优先复用已有模块、类型、IPC 包装、任务生命周期和 UI 基元。新增抽象应有明确职责；避免夹带无关重构、整文件格式化或词典重排。
- 保留既有的节点 ID、案件／检材身份、持久化值与接口语义。响应式状态、订阅与取消沿用现有所有权和代际检查，不能让旧任务清理或取消新任务。
- 新增用户文案同时维护中英文；项目文档和 changelog 条目使用中文，变更分类沿用英文。用户行为、格式能力或验证边界有变化时，更新对应文档。

### 标题与描述

PR 标题与提交标题使用 Conventional Commits，例如 `fix(export): release finished image export jobs`、`feat(plugin): ...`、`docs: ...`，明确最终改动的领域和行为。范围变化后同步改写标题与描述。

描述面向没有参与讨论的审阅者，先写具体问题和最终行为，再补充必要的实现选择与验证依据。简单改动用一两段即可，较复杂的改动可按以下内容组织：

- **问题与结果**：触发条件、原有表现、修复后的行为，以及相关 issue。
- **实现与兼容性**：关键实现选择、保留的接口或身份约束、用户需要知道的限制。
- **验证**：实际运行的 `just` 配方、结果、跳过项和未验证范围。界面变化附截图或录屏，性能变化附样本说明与测量结果。

PR 描述反映最终实现，不记录讨论经过、已放弃的方案或与审阅无关的操作流水。每次验证的通过数量、工具限制和本机诊断写在 PR 中，不写入长期项目文档。

### 回归与验证

按上方[验证矩阵](#验证)选择本地配方。修复缺陷时补充能复现原问题的回归测试，并验证修复后的用户行为；解析、寻址与完整性问题使用独立构造的最小样本和已知结果。

异步状态变化应覆盖与改动相关的成功、失败、取消、重试、案件切换及迟到结果；不要只检查实现细节。涉及共享导出流程时，同时检查普通导出和解密导出等实际调用入口。

源码对照、mock IPC、组件测试、原生桌面操作和外部样本／oracle 验证应分别说明，不能相互替代。仓库不配置 GitHub Actions，贡献者需完成本地验证；未执行的检查、缺失样本及保留的构建警告应如实说明。

### 合并要求

1. 同步目标分支 `main`，解决冲突时保留主分支已有功能、翻译和文档；检查最终差异没有带入无关改动。
2. 完成代码风格整理、回归测试、对应文档与 `[Unreleased]` 更新，再运行所需验证。验证后继续修改代码或解决冲突时，重新执行受影响的检查。
3. 将最终改动推到 PR 分支，核对待合并的提交与已验证的提交一致，并更新 PR 标题和说明。整理外部贡献时保留原作者归属。
4. 默认使用 Squash 合并，合并提交标题遵循 Conventional Commits，且必须以实际 PR 编号 `(#<编号>)` 结尾，例如 `feat(plugin): persist plugin logs beside the executable (#3)`。使用 `gh pr merge --subject` 自定义标题时也必须显式保留该后缀；合并后核对远端合并提交的标题与编号，再将本地 `main` 快进同步到远端。
5. 合并提交保留贡献者原有的 Author 姓名和邮箱，Committer 记录实际执行合并的维护者（目前为 `b3nguang`）。外部贡献在 GitHub 上应显示为“原作者 authored，维护者 committed”。合并后使用 `git show --format=fuller <合并提交>` 检查两种身份，并核对远端提交关联的 GitHub 账号；不要将 Author 改成维护者，也不要为了显示头像虚构 `Co-authored-by`。

维护与发布见 [发布指南](docs/RELEASING.md)。

除明确另有声明的第三方内容外，提交到本项目的贡献采用 [AGPL-3.0-only](LICENSE)。请确认你有权提交相关代码、文档与资源，并保留第三方内容原有的版权和许可证声明。
