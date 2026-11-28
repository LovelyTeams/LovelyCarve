<p align="center">
  <img src="src/assets/app-icon.png" width="120" alt="LovelyCarve 图标">
</p>

<h1 align="center">LovelyCarve</h1>

<p align="center">
  <b>a forensic carver with a soft heart</b><br>
  磁盘取证与数据恢复工具，提供桌面版、独立 CLI 和 Python 读取库
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: AGPL-3.0-only" src="https://img.shields.io/badge/license-AGPL--3.0--only-blue"></a>
  <img alt="Platform: Windows x64 / ARM64" src="https://img.shields.io/badge/platform-Windows%20x64%20%7C%20ARM64-0078d4">
  <img alt="Platform: macOS Apple Silicon / Intel" src="https://img.shields.io/badge/platform-macOS%20Apple%20Silicon%20%7C%20Intel-333333">
  <img alt="Platform: Linux x64 / ARM64" src="https://img.shields.io/badge/platform-Linux%20x64%20%7C%20ARM64-555555">
  <img alt="Rust 1.100+" src="https://img.shields.io/badge/rust-1.100%2B-dea584">
  <img alt="Tauri 2" src="https://img.shields.io/badge/tauri-2-24c8db">
  <img alt="Vue 3" src="https://img.shields.io/badge/vue-3-42b883">
</p>

<p align="center">
  <a href="#下载">下载</a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="#功能">功能</a> ·
  <a href="#界面">界面</a> ·
  <a href="#支持的格式">支持的格式</a> ·
  <a href="#命令行">命令行</a> ·
  <a href="#从源码构建">从源码构建</a> ·
  <a href="docs/README.md">文档</a>
</p>

LovelyCarve 可以在同一个案件里检查多份检材。打开磁盘镜像、物理磁盘或逻辑备份后，可以沿分区和目录浏览文件、全文检索、查看时间线，也可以校验哈希、导出证据或恢复删除的文件。读取器使用纯 Rust 实现，以只读方式打开源检材。桌面界面提供简体中文和英文，支持浅色与深色主题。

<p align="center">
  <img src="docs/images/file-browser.png" alt="文件浏览：目录树、文件列表与原类型预览">
</p>

## 下载

从 [GitHub Releases](https://github.com/LovelyTeams/LovelyCarve/releases/latest) 下载对应系统和架构的发行包。下载后可用发布页的 `SHA256SUMS.txt` 核对文件。

| 平台 | 桌面版 | 独立 CLI | 运行要求 |
| --- | --- | --- | --- |
| Windows x64 | MSI | ZIP | 桌面版需要 WebView2 |
| Windows ARM64 | 安装程序 | ZIP | 桌面版需要 WebView2 |
| macOS（Apple Silicon / Intel） | DMG | tar.gz | macOS 15 或更新版本 |
| Linux（x64 / ARM64） | AppImage | tar.gz | glibc 2.39 或更新版本（如 Ubuntu 24.04），运行 AppImage 还需要 FUSE 2 |

Windows 发行包未签名。读取物理磁盘时请以管理员身份运行。Windows x64 和 ARM64 都可以借助 Dokany 2 将卷只读挂载为盘符或文件夹。[Dokany 2 下载](https://github.com/dokan-dev/dokany/releases/latest)

macOS 应用使用 ad-hoc 签名，首次打开时可能需要在「系统设置 → 隐私与安全性」中确认。Intel 版的 Magika 文件类型识别不可用。各平台的标准发行包均关闭二维码识别和 OCR。

## 快速开始

1. 打开桌面版，在起始页新建案件，选择一个空目录保存案件数据。
2. 点击「添加检材」，选择镜像、文件夹、备份或物理磁盘。分段镜像选择第一段，其余分段放在同一目录。
3. 打开「镜像信息」查看分区、解锁加密卷或核对哈希。卷索引和平台识别的进度可在「任务中心」查看。
4. 在「文件列表」浏览和预览文件，在「工具」中运行全局搜索、时间线或文件恢复。插件分析从「任务中心 → 快速分析」启动，结果可在侧栏「分析」中查看。

[桌面版使用指南](docs/USAGE.md)

## 功能

- **管理案件与审计记录**。在一个案件中登记多份检材，保存卷索引和带哈希链的操作记录。源镜像暂时缺失时，仍可浏览已索引的目录与元数据。
- **浏览卷与文件**。识别分区、组装阵列、解锁加密卷并查看支持的快照。按文件系统能力浏览删除项、未分配空间、命名流和元数据。
- **预览文件**。同时保留多个预览标签，也可将预览弹出为独立窗口。支持文本、十六进制、图片、音视频、PDF 和 Office 文档，以及 SQLite、Realm、KeePass KDBX、Vim 交换文件和 PE / ELF / Mach-O 等结构化视图。SQLite 可回放 WAL、查询表并查看可恢复的删除记录。
- **检索文件和事件**。按关键词或正则跨卷搜索，按需包含删除文件和未分配空间。时间线汇集文件修改时间、NTFS USN 日志和插件事件。搜索命中和分析来源可复制为 `lovelycarve://` 链接，在本机重新定位到案件里的文件或命中字节。
- **校验并导出证据**。核对 E01 采集哈希，计算 MD5、SHA-1、SHA-256 和 SM3，导出文件、目录、ZIP、RAW 镜像或支持的解密镜像。读取结果会报告坏区造成的补零，遇到短读时明确标记失败。
- **恢复删除文件**。从 FAT、NTFS 和 exFAT 的删除记录尝试恢复文件，查找丢失分区，并按签名雕刻图片、文档、归档和数据库等 14 种格式。导出恢复文件时可获得 SHA-256 与来源报告。
- **运行分析插件**。按操作系统、整个检材或所选文件运行分析器，筛选结果、导出 CSV，并从记录回到源文件。内置 `linuxinfo`，可提取 Linux 的用户、登录、网络、服务、软件和命令历史等信息。

## 界面

以下截图来自 2026 精武杯服务器取证检材（Ubuntu 22.04，E01，58 GiB）。

| 镜像信息 | SQLite 浏览与删除记录 |
| :---: | :---: |
| <img src="docs/images/image-info.png" alt="镜像信息：分区布局、平台识别与完整性校验"> | <img src="docs/images/sqlite-viewer.png" alt="SQLite 浏览：表清单、行数与 WAL 回放"> |
| 分区布局、平台识别、完整性校验与采集信息 | 浏览器历史库的表与记录，自动回放 `-wal` |
| **全局搜索** | **时间线** |
| <img src="docs/images/global-search.png" alt="全局搜索：命中列表与上下文"> | <img src="docs/images/timeline.png" alt="时间线：活动热力图与事件列表"> |
| 13 万处命中按文件去重，查看偏移、编码与上下文 | 59 万条事件的年度热力图与明细 |
| **文件雕刻** | **插件分析** |
| <img src="docs/images/file-carving.png" alt="文件雕刻：结果列表与预览"> | <img src="docs/images/plugin-results.png" alt="插件分析：Linux 登录记录"> |
| 从分区扫描恢复文件，按类型筛选并预览 | `linuxinfo` 解析 wtmp 登录记录并回溯源文件 |

## 支持的格式

| 类别 | 常见格式与用法 |
| --- | --- |
| 磁盘镜像 | RAW / 分段 RAW、E01 / Ex01、AD1、VMDK、VHD / VHDX、QCOW2、VDI、DMG、PHDI，以及多轨 BIN / CUE 和部分路由器固件 |
| 逻辑检材 | 文件夹、ZIP、7z、TAR、Docker / OCI 镜像、iOS 本地备份、小米备份和 BCP 交换数据 |
| 分区与多盘卷 | GPT、MBR / EBR、APM、BSD 标签和 LVM2。按元数据组装 Linux md、Intel IMSM 和 SNIA DDF 阵列。载入全部成员后可组装多盘 Btrfs、bcachefs、ZFS、APFS Fusion 和跨盘 VMFS |
| 文件系统 | NTFS、FAT / exFAT、ext2 / 3 / 4、APFS、HFS+、XFS、ReFS、Btrfs、ZFS、VMFS、ISO 9660、UDF，以及 HPFS、HAMMER / HAMMER2、QNX、LittleFS 和 Xbox FATX / XTAF 等 |
| 加密与快照 | BitLocker、LUKS、VeraCrypt / TrueCrypt、FileVault 2、APFS 软件加密和加密的 iOS 备份。支持浏览 NTFS 卷影副本和 APFS 历史快照 |

不同格式支持的版本、布局、读取方式与恢复范围见 [格式总览](docs/formats/README.md)。

## 命令行

`lovelycarve-cli` 与桌面版共用读取器，可独立运行。命令使用 `/<检材别名>/<卷>/<路径>` 定位检材。下面的 PowerShell 示例创建案件、登记镜像并浏览第一个分区。请用列表返回的 `items[].path` 替换示例中的卷地址。

```powershell
$caseDir = 'D:\cases\demo'
lovelycarve-cli case new $caseDir --open
lovelycarve-cli -C $caseDir evidence add 'D:\evidence\disk.E01' --as disk --wait
lovelycarve-cli -C $caseDir ls /disk --json
lovelycarve-cli -C $caseDir tree /disk/part:0 --depth 2
lovelycarve-cli -C $caseDir case close
```

案件后台进程在多条命令间复用已打开的检材、解锁卷和已组装的阵列。交互操作用 `shell`，自动化用 `batch` 处理 JSONL 请求。单条命令支持 `--json` 输出和 `--jq` 筛选结果。[CLI 手册](docs/CLI.md) · [编码代理技能](skills/lovelycarve-cli/SKILL.md)

## Python 读取库

`lovelycarve` 用于在 Python 脚本中只读打开镜像和归档、遍历卷与目录、查询元数据和按范围读取文件。发行版为上述六种平台提供 CPython 3.10+ wheel，Linux wheel 需要 glibc 2.34 或更新版本。[Python 读取库文档](python/README.md) · [PyPI](https://pypi.org/project/lovelycarve/)

```sh
python -m pip install lovelycarve
```

## 插件

在「设置 → 插件市场」浏览官方插件和版本，或下载 `.lcpkg` 后在桌面版检视并安装。[官方插件发布页](https://lovelyteams.github.io/LovelyCarve-Plugin-Site/)

插件通过 LPP v2 读取检材，输出带来源的结构化记录，宿主可将其中的事件加入时间线。独立宿主进程用于隔离故障，插件以当前用户的权限运行。安装时可先核对发布者、权限和 SHA-256。SDK、宿主和 `linuxinfo` 源码由插件子模块维护。[LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) · [插件协议与 SDK](docs/PLUGIN_PROTOCOL.md) · [linuxinfo 使用说明](docs/LINUXINFO.md)

## 从源码构建

项目采用 Rust、Tauri 2 和 Vue 3。先安装 Rust、Bun、just 和目标平台的系统依赖，再递归克隆仓库。下面以 Windows 桌面开发为例。[各平台环境与工具链](docs/DEVELOPMENT.md)

```powershell
git clone --recurse-submodules https://github.com/LovelyTeams/LovelyCarve.git
cd LovelyCarve
just bootstrap
just dev
```

| 任务 | 命令 |
| --- | --- |
| 运行独立 CLI | `just run-cli` |
| 构建 CLI | `just build-cli` |
| 构建 Windows 安装包 / 便携包 | `just build-release` / `just pack` |
| 构建 macOS DMG / Linux AppImage | `just build-macos` / `just build-linux` |
| 验证前端、Rust 和插件 | `just verify-fe` / `just verify-rust` / `just verify-plugins` |

运行 `just --list` 查看其他配方。需要二维码识别和 OCR 时，请按开发指南配置完整构建的依赖。已有克隆可用 `git submodule update --init --recursive` 补齐子模块。[发布与打包指南](docs/RELEASING.md)

## 文档

- 使用与格式：[桌面版使用指南](docs/USAGE.md) · [CLI 手册](docs/CLI.md) · [Python 读取库](python/README.md) · [格式总览](docs/formats/README.md) · [iOS 备份](docs/IOS_BACKUPS.md)
- 开发与维护：[开发指南](docs/DEVELOPMENT.md) · [架构说明](docs/ARCHITECTURE.md) · [前端规范](docs/FRONTEND.md) · [前端模块与生命周期](docs/FRONTEND_STRUCTURE.md) · [更新日志](CHANGELOG.md)

[完整文档导航](docs/README.md)

## 局限

- 删除项或雕刻结果可能缺少部分内容。块对齐、碎片化程度和格式支持范围都会影响扫描结果，即使没有命中，也无法据此判定文件是否存在。
- 丢失分区查找依赖 FAT、NTFS、exFAT 的引导扇区或残留 `$MFT`，目前没有独立的孤儿文件扫描。文件系统时间线目前只采用文件修改时间。
- 早期开发版的 `.carve` 案件与当前案件格式不兼容。

## 参与贡献

欢迎提交问题报告、格式样本线索、文档改进和代码。报告读取问题时，请附版本、复现步骤和可公开的最小合成样本，切勿上传真实检材、案件目录、个人数据或凭据。[贡献指南](CONTRIBUTING.md) · [仓库约定](AGENTS.md) · [安全漏洞报告](SECURITY.md)

## 许可与致谢

LovelyCarve 采用 AGPL-3.0-only 许可证。[许可证全文](LICENSE)

Copyright (C) 2026 LovelyCarve contributors

感谢提供读取器和解析库的上游项目、公开格式资料与样本的维护者，以及 Flyfish File Viewer 的作者。依赖来源、许可证和本地补丁见 [第三方声明](THIRD_PARTY_NOTICES.md) 和 [内置依赖与补丁记录](src-tauri/vendor/UPSTREAM.md)。
