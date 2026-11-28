# 发布指南

本文面向维护者。开发环境与构建参数见 [开发指南](DEVELOPMENT.md)，项目许可证见根目录的 [LICENSE](../LICENSE)。

## 发布前检查

- 在干净的递归克隆中执行 `just bootstrap`，然后按 [贡献指南](../CONTRIBUTING.md#验证) 完成前端、Rust、IPC 与结构检查；改动了插件或读取器时执行其专用配方，涉及 example 的公共接口另跑 `just check-examples`。
- 确认 `plugins/` 固定的提交已推送到公开的 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins)。GitHub 自动生成的主仓库源码 ZIP / tar.gz 不包含子模块，发行说明中应给出 `git clone --recurse-submodules` 与初始化步骤。
- 确认仓库中没有检材、案件、凭据、本机配置和个人数据。`.gitignore` 只影响未跟踪文件，不会清除已提交的历史；`fixtures/` 中的外部镜像、构建输出和本机 agent 配置不作为项目文件提交。
- 保留 vendor 目录的许可证、`ORIGIN.md` / `UPSTREAM.md` / `LOVELYCARVE.md` 说明及合成测试数据的来源；[ReFS](formats/REFS.md) 等格式专题中记录的公开镜像来源和哈希需要继续维护。
- 仓库设置中保持私密漏洞报告开启，使 [SECURITY.md](../SECURITY.md) 的报告入口可用，操作见 [GitHub 文档](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository)。

发布含 QR/OCR 的完整版本时，还需在目标 Windows 环境中检查模型加载、CPU 回退、离线文件预览、插件宿主和 DLL 加载。安装包与便携包分别做启动、建案、添加合成证据、浏览、哈希、导出及关闭的验证；挂载功能需要 Dokany 驱动并做实际挂载测试（x64 与 ARM64 各自安装对应架构的 Dokany）。macOS / Linux 产物由 CI 检查架构、包内文件、插件描述和 CLI 能力；图形界面仍需在对应平台上启动验证。

## 版本与构建

1. 同步 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 与 `src-tauri/components/Cargo.toml`（原生组件）中的应用版本，以及 `python/pyproject.toml`、`python/Cargo.toml` 和 Python 包的 `__version__`。依赖锁文件一并提交，插件版本按其独立的兼容要求维护。
2. 按 [Keep a Changelog 0.3.0](https://keepachangelog.com/zh-CN/0.3.0/) 把 CHANGELOG 中实际纳入本次发布的 `[Unreleased]` 条目整理为 `## [版本号] - YYYY-MM-DD`，并记录格式或案件兼容性变化；未发布的内容保留在 `[Unreleased]`，没有剩余条目时保留空的 `[Unreleased]` 标题。
3. 根据功能组合选择本地构建与打包入口：

| 产物 | 默认（不含 QR/OCR） | 含 QR/OCR |
| --- | --- | --- |
| Windows 桌面安装包 | `just build-release` | `just build-release-full` |
| Windows 桌面便携包 | `just pack` | `just pack-full` |
| Windows CLI 包 | `just pack-cli` | `just pack-cli-full` |
| Python wheel（各平台） | `just build-python` | — |
| Windows ARM64 桌面安装程序 / CLI 包 | `just build-release --bundles nsis --config src-tauri/tauri.arm64.conf.json` / `just pack-cli -Arm64` | — |
| macOS 桌面 DMG（Apple Silicon / Intel） | `just build-macos` | — |
| Linux 桌面 AppImage（x64 / ARM64） | `just build-linux` | — |
| 插件独立包 | `just pack-plugins` | — |

`repack` / `repack-full` 复用已有的桌面产物，必须与编译特性一致；`repack-cli` 按 CLI 的实际能力选择资源。运行库、原生组件、模型与插件清单由打包配置决定，不要按文件后缀猜测所需资源。

CHANGELOG 只使用实际有内容的英文分类（`Added`、`Changed`、`Deprecated`、`Removed`、`Fixed`、`Security`），版本按时间倒序排列，不保留空分类，也不直接粘贴提交记录。版本号使用程序的完整语义化版本，Git 标签为 `v<版本号>`（例如 `1.0.0` 对应 `v1.0.0`），版本链接指向对应的比较页或 Release 页，`[Unreleased]` 的比较起点为最新的发布标签。

## 第三方声明与源码

发布前按实际产物核对 [第三方声明](../THIRD_PARTY_NOTICES.md)、vendor 许可证、文件预览器资源、字体、Dokan、OpenSSL、OpenCV、ONNX Runtime 以及 QR/OCR 模型的来源、版本和许可文本。锁文件记录了包版本，二进制与模型还需核对实际来源；根目录声明不是全部传递依赖的自动许可证清单。

打包配方会随附 LICENSE 与 THIRD_PARTY_NOTICES。发行前仍需检查产物内容完整，保留组件自带的声明与模型许可证，并确保下载者能获得与二进制对应的项目源码、vendor 改动、锁文件、构建脚本和必要的第三方来源说明。Quicksand 字体的 OFL 全文同时保存在字体目录和第三方声明中。

## 标签与发行产物

把 `v<版本号>` 标签推送到对应的源码提交后，`Release` 工作流（`windows-release.yml`）并行运行 `Windows tests` 并构建各平台的发行包。所有任务成功后，在同一个 GitHub Release 上传以下 18 个产物和一份覆盖全部下载项的 `SHA256SUMS.txt`。每个平台另用 `just build-python` 生成 Python wheel（abi3，CPython 3.10+），安装到新的虚拟环境中按本机架构导入一次后随其他产物上传。

- Windows x64 通过 `just build-release --bundles msi`、`just pack-cli` 和 `just build-python` 生成 MSI、CLI ZIP 和 Python wheel。
- Windows ARM64（`windows-arm64.yml`）生成 NSIS 安装程序和 CLI ZIP，静默安装后检查全部可执行文件和 DLL 都是 ARM64，并确认包内没有 x64 的 `dokan2.dll`。
- macOS（`macos.yml`）在 Apple Silicon 与 Intel 运行器上通过 `just build-macos` 和 `just build-cli` 生成桌面 DMG 和 CLI 压缩包。工作流从 DMG 读取应用并检查架构、签名、版本与插件，在解包后的 CLI 中检查推理组件能否加载；Intel 组件应报告不支持文件类型识别。
- Linux（`linux.yml`）在 Ubuntu 24.04 的 x64 与 ARM64 运行器上通过 `just build-linux` 和 `just build-cli` 生成 AppImage 和 CLI 压缩包，解开 AppImage 检查架构与插件，并检查 CLI 能否加载推理组件。

推送 `release-check/<名称>` 分支会运行同一个工作流：构建、检查全部产物并写出校验和，但不创建 GitHub Release，也不上传 PyPI。打标签前可以用它验证全部平台。

例如，版本同步为 `1.5.0` 并提交后，将标签推送到公开仓库：

```powershell
git tag v1.5.0
git push <公开仓库远端名> v1.5.0
```

标签的基础版本必须与应用和 Python 包清单一致。`v1.3.0-rc.1` 这类带后缀的标签发布为预发行版，`v1.3.0` 发布为正式版。已有标签不会因添加工作流而重新触发，请勿移动已发布的标签。发布任务通过仓库内置的 `GITHUB_TOKEN` 和 `contents: write` 权限上传 GitHub Release 产物。

`LovelyTeams/LovelyCarve` 的正式标签在 GitHub Release 发布成功后，会把 Windows x64 / ARM64、macOS Apple Silicon / Intel 与 Linux x64 / ARM64 的六个 wheel 一并上传到 PyPI。预发行标签沿用清单中的正式版本号，Python wheel 只随 GitHub 预发行版提供。PyPI 上传失败时，可以在 GitHub Actions 重跑失败的任务，复用同次运行的 wheel。

首次启用时，在 GitHub 仓库的 **Settings → Environments** 创建 `pypi` 环境，将部署来源限定为 `v*` 标签，并在 [PyPI 项目的 Publishing 设置](https://pypi.org/manage/project/lovelycarve/settings/publishing/)中添加 GitHub Trusted Publisher：

| 字段 | 值 |
| --- | --- |
| Owner | `LovelyTeams` |
| Repository name | `LovelyCarve` |
| Workflow name | `windows-release.yml` |
| Environment name | `pypi` |

发布任务通过 GitHub OIDC 向 PyPI 换取短期上传凭据，无需配置 PyPI API token。配置步骤见 [PyPI Trusted Publishing 文档](https://docs.pypi.org/trusted-publishers/adding-a-publisher/)。

自动构建默认关闭 QR/OCR。Windows 二进制未签名。macOS 应用使用 ad-hoc 签名，未经 Apple 公证，首次打开时可能需要在「系统设置 → 隐私与安全性」中确认。Linux AppImage 需要 FUSE 2 和 glibc 2.39 或更新版本：ort 的 Linux 预编译 ONNX Runtime 引用 glibc 2.38 的符号，Linux 包因此在 Ubuntu 24.04 上构建。原生 GUI、安装及 Dokany 驱动仍需人工验证。完整功能构建和桌面便携包继续使用上表的本地配方。

自动 Release 说明包含源码提交、构建范围和外部驱动要求；维护者应补充版本变化与已知限制。本地发行时，从该标签创建 GitHub Release，并上传实际构建的产物与 SHA-256：

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath 'dist-pack/LovelyCarve-v<版本号>-win-x64.7z'
```

源码标签、发行说明和产物应对应同一提交；说明签名状态、已验证的平台范围和外部驱动要求。
