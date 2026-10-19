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

完成本地验证与打包后，把 `v<版本号>` 标签指向对应的源码提交。仓库没有 GitHub Actions，推送标签不会触发云端构建，发行产物来自上表的本地配方。

从该标签创建 GitHub Release，写明版本变化、功能组合、平台要求、已知限制，并上传实际构建的产物与 SHA-256：

```powershell
Get-FileHash -Algorithm SHA256 -LiteralPath 'dist-pack/LovelyCarve-v<版本号>-win-x64.7z'
```

### 应用内更新

桌面安装包启用了 Tauri updater。发布前需要把签名私钥放在本机环境中，私钥不要提交到仓库：

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = 'C:\path\to\lovelycarve-updater.key'
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '<private-key-password-if-used>'
just build-release
```

首次启用 updater 时，维护者还需要生成并保管一对 minisign 密钥，把项目公钥写入
`src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`，并把私钥交给发布环境；
合并此功能前必须将当前分支的引导公钥替换为项目维护者公钥，匹配私钥只能通过受控渠道交接，不能提交到仓库。
`requireSignedVersion` 保持开启，未配置有效项目公钥的构建不能发布可更新安装包。

构建会在 `src-tauri/target/release/bundle/` 生成安装包及其 `.sig` 签名。
Tauri CLI 不会自动生成 `latest.json`，发布者需要根据实际产物编写静态更新清单，例如：

```powershell
$releaseVersion = (Get-Content -Raw src-tauri/tauri.conf.json | ConvertFrom-Json).version
$installerName = "LovelyCarve_${releaseVersion}_x64-setup.exe"
$signature = (Get-Content -Raw "src-tauri/target/release/bundle/nsis/$installerName.sig").Trim()
@{
  version = $releaseVersion
  platforms = @{
    'windows-x86_64-nsis' = @{
      signature = $signature
      url = "https://github.com/LovelyTeams/LovelyCarve/releases/download/v$releaseVersion/$installerName"
    }
  }
} | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 latest.json
```

将 `latest.json`、Windows x64 的 `*-setup.exe` 与对应 `.sig` 一起上传到同一个 GitHub Release；
更新器按 `windows-x86_64-nsis` 目标选择安装包，校验签名后以静默模式安装并重启应用。
Windows x64 的 NSIS 安装版和便携包统一下载此安装器，显式指定当前程序目录就地更新。
便携目录会增加 NSIS 的卸载器及安装登记。MSI 安装版和其他平台只检测新版本并显示暂不支持应用内安装，
避免用 NSIS 静默覆盖 MSI 管理的文件；后续可独立扩展原格式升级。
丢失私钥后无法发布可被现有安装识别的签名更新。

启动时先通过 GitHub Releases API 比较最新稳定版与当前版本，相同或更旧时直接显示已是最新版本。
旧 Release 没有 `latest.json` 时，Windows x64 可使用同一 Release 的
`LovelyCarve_<版本号>_x64-setup.exe`；下载后必须同时匹配 GitHub API 返回的文件大小与 `sha256:` digest，
通过后以 NSIS 的 `/S /UPDATE /R /D=<当前程序目录>` 参数静默安装并重启。不弹出下载页；检查失败可以重试。
这个兼容通道依赖 HTTPS 与 GitHub 提供的摘要，不等同于维护者私钥签名。发布 `latest.json` 后会使用签名通道，
签名或元数据校验失败时不会回退执行未签名安装器。没有兼容安装器的新版会显示暂不支持应用内安装。

源码标签、发行说明和产物应对应同一提交；说明签名状态、已验证的平台范围和外部驱动要求。
