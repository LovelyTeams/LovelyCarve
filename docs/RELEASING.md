# 发布指南

本文面向维护者。开发环境与构建参数见 [开发指南](DEVELOPMENT.md)，项目许可证见根目录的 [LICENSE](../LICENSE)。

## 发布前检查

- 在干净的递归克隆中执行 `just bootstrap`，然后按 [贡献指南](../CONTRIBUTING.md#验证) 完成前端、Rust、IPC 与结构检查；改动了插件或读取器时执行其专用配方，涉及 example 的公共接口另跑 `just check-examples`。
- 确认 `plugins/` 固定的提交已推送到公开的 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins)。GitHub 自动生成的主仓库源码 ZIP / tar.gz 不包含子模块，发行说明中应给出 `git clone --recurse-submodules` 与初始化步骤。
- 确认仓库中没有检材、案件、凭据、本机配置和个人数据。`.gitignore` 只影响未跟踪文件，不会清除已提交的历史；`fixtures/` 中的外部镜像、构建输出和本机 agent 配置不作为项目文件提交。
- 保留 vendor 目录的许可证、`ORIGIN.md` / `UPSTREAM.md` / `LOVELYCARVE.md` 说明及合成测试数据的来源；[ReFS](formats/REFS.md) 等格式专题中记录的公开镜像来源和哈希需要继续维护。
- 仓库设置中保持私密漏洞报告开启，使 [SECURITY.md](../SECURITY.md) 的报告入口可用，操作见 [GitHub 文档](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository)。

发布含 QR/OCR 的完整版本时，还需在目标 Windows 环境中检查模型加载、CPU 回退、离线文件预览、插件宿主和 DLL 加载。安装包与便携包分别做启动、建案、添加合成证据、浏览、哈希、导出及关闭的验证；挂载功能需要 Dokany 驱动并做实际挂载测试。macOS / Linux 只有完成对应平台的验证后才提供产物。

## 版本与构建

1. 同步 `package.json`、`src-tauri/Cargo.toml` 与 `src-tauri/tauri.conf.json` 中的应用版本，依赖锁文件一并提交；插件版本按其独立的兼容要求维护。
2. 按 [Keep a Changelog 0.3.0](https://keepachangelog.com/zh-CN/0.3.0/) 把 CHANGELOG 中实际纳入本次发布的 `[Unreleased]` 条目整理为 `## [版本号] - YYYY-MM-DD`，并记录格式或案件兼容性变化；未发布的内容保留在 `[Unreleased]`，没有剩余条目时保留空的 `[Unreleased]` 标题。
3. 根据功能组合选择本地构建与打包入口：

| 产物 | 默认（不含 QR/OCR） | 含 QR/OCR |
| --- | --- | --- |
| Windows 桌面安装包 | `just build-release` | `just build-release-full` |
| Windows 桌面便携包 | `just pack` | `just pack-full` |
| Windows CLI 包 | `just pack-cli` | `just pack-cli-full` |
| 插件独立包 | `just pack-plugins` | — |

`repack` / `repack-full` 复用已有的桌面产物，必须与编译特性一致；`repack-cli` 按 CLI 的实际能力选择资源。运行库、模型与插件清单由打包配置决定，不要按文件后缀猜测所需资源。

CHANGELOG 只使用实际有内容的英文分类（`Added`、`Changed`、`Deprecated`、`Removed`、`Fixed`、`Security`），版本按时间倒序排列，不保留空分类，也不直接粘贴提交记录。版本号使用程序的完整语义化版本，Git 标签为 `v<版本号>`（例如 `1.0.0` 对应 `v1.0.0`），版本链接指向对应的比较页或 Release 页，`[Unreleased]` 的比较起点为最新的发布标签。

## 第三方声明与源码

发布前按实际产物核对 [第三方声明](../THIRD_PARTY_NOTICES.md)、vendor 许可证、文件预览器资源、字体、Dokan、OpenSSL、OpenCV、ONNX Runtime 以及 QR/OCR 模型的来源、版本和许可文本。锁文件记录了包版本，二进制与模型还需核对实际来源；根目录声明不是全部传递依赖的自动许可证清单。

打包配方会随附 LICENSE 与 THIRD_PARTY_NOTICES。发行前仍需检查产物内容完整，保留组件自带的声明与模型许可证，并确保下载者能获得与二进制对应的项目源码、vendor 改动、锁文件、构建脚本和必要的第三方来源说明。Quicksand 字体的 OFL 全文同时保存在字体目录和第三方声明中。

## 标签与发行产物

把 `v<版本号>` 标签推送到对应的源码提交后，`Windows release` 工作流会先运行与 PR 相同的前端、Rust 和插件测试，再执行 `just build-release --bundles msi`、`just pack-cli`、`just smoke-cli` 与 `just pack-plugins`，上传 Windows x64 MSI、CLI ZIP、插件 `.lcpkg` 和 `SHA256SUMS.txt`，自动创建 GitHub Release。例如，应用版本同步为 `1.2.1` 并提交后：

```powershell
git tag v1.2.1
git push origin v1.2.1
```

标签的基础版本必须与三个应用清单一致；`v1.2.1-rc.1` 这类带后缀的标签发布为预发行版，`v1.2.1` 发布为正式版。已有标签不会因添加工作流而重新触发，请勿移动已发布的标签。工作流使用仓库内置的 `GITHUB_TOKEN`，发布 job 具有 `contents: write` 权限，无需另设 PAT 或个人凭据。

自动构建默认不含 QR/OCR，二进制未做代码签名；原生 GUI、安装及 Dokany 驱动仍需人工验证。完整功能构建和桌面便携包继续使用上表的本地配方。

自动 Release 说明包含源码提交、构建范围和外部驱动要求；维护者应补充版本变化与已知限制。本地发行时，从该标签创建 GitHub Release，并上传实际构建的产物与 SHA-256：

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath 'dist-pack/LovelyCarve-v<版本号>-win-x64.7z'
```

源码标签、发行说明和产物应对应同一提交；说明签名状态、已验证的平台范围和外部驱动要求。
