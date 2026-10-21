<p align="center">
  <img src="src/assets/app-icon.png" width="120" alt="LovelyCarve 图标">
</p>

<h1 align="center">LovelyCarve</h1>

<p align="center">
  <b>a forensic carver with a soft heart</b><br>
  基于 Rust、Tauri 2 与 Vue 3 的桌面磁盘取证与数据恢复工具
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: AGPL-3.0-only" src="https://img.shields.io/badge/license-AGPL--3.0--only-blue"></a>
  <img alt="Platform: Windows x64" src="https://img.shields.io/badge/platform-Windows%20x64-0078d4">
  <img alt="Rust 1.99+" src="https://img.shields.io/badge/rust-1.99%2B-dea584">
  <img alt="Tauri 2" src="https://img.shields.io/badge/tauri-2-24c8db">
  <img alt="Vue 3" src="https://img.shields.io/badge/vue-3-42b883">
</p>

<p align="center">
  <a href="#功能">功能</a> ·
  <a href="#界面">界面</a> ·
  <a href="#支持的格式">支持的格式</a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="#从源码构建">从源码构建</a> ·
  <a href="docs/README.md">文档</a>
</p>

LovelyCarve 面向电子数据取证、应急响应和 CTF 取证题：加载磁盘镜像、逻辑检材或物理磁盘，识别分区与文件系统，浏览、预览、检索、校验并导出检材中的数据，再用时间线、文件恢复和插件分析补全线索。镜像、文件系统与加密容器的读取器均为纯 Rust 实现，检材始终只读打开；界面以中文为主，提供浅色、深色和樱花色三套主题。

<p align="center">
  <img src="docs/images/file-browser.png" alt="文件浏览：目录树、文件列表与原类型预览">
</p>

## 功能

- **案件管理**：一个目录就是一个案件，登记多份检材，记录标签和带哈希链的操作审计；卷快照与文件索引落盘复用，源镜像暂时缺失时仍可浏览已索引的目录和元数据。
- **检材接入**：磁盘镜像、文件夹、归档、AD1 逻辑镜像、Docker/OCI 镜像、iOS 本地备份、Windows 物理磁盘，以及由成员盘重组的 RAID。
- **分区、卷与加密**：GPT、MBR/EBR、APM、BSD 分区与 LVM2 逻辑卷；mdadm（元数据 0.90 / 1.x）linear 与 RAID0/1/4/5/6/10 重组，含降级重建、RAID6 双盘恢复与 md 上的 LVM，Intel RST / VROC（IMSM）与 PERC、MegaRAID 等控制器（SNIA DDF）的固件 RAID 卷按元数据重组，无元数据成员盘按 RAID0/RAID5 推定；解锁 BitLocker、LUKS、VeraCrypt、FileVault 2 和加密的 iOS 备份，浏览 NTFS 卷影副本。解密视图按需读取，不生成整卷明文。
- **文件浏览**：目录树与分页文件列表，排序、列筛选和递归“全显”；按文件系统能力展示删除项、未分配空间、命名流、符号链接与元数据节点。
- **原类型预览**：文本、十六进制、图片、音视频、PDF、Office 文档和归档；SQLite（含 WAL 回放与删除记录恢复）、Realm、PE / ELF / Mach-O / GGUF 等结构化视图；预览可拆出为独立窗口。
- **校验与导出**：MD5、SHA-1、SHA-256、SM3；E01 采集哈希核对；导出文件、目录、ZIP、RAW 镜像与解密镜像。读取遇到坏区时报告补零字节，短读不会被当作完整结果。
- **全局搜索**：关键词与正则，覆盖 ASCII、UTF-8、UTF-16LE/BE、GBK，可包含删除文件与未分配空间；命中可回到源文件并定位字节偏移，支持去重与 CSV 导出。
- **时间线**：投影文件修改时间、插件事件与 NTFS USN 日志，提供年度活动热力图、趋势钻取、筛选和导出。
- **文件恢复**：读取 FAT、NTFS、exFAT 的删除记录并重建丢失分区，同时按签名雕刻 JPEG、PNG、GIF、PDF、ZIP/Office、SQLite 等 14 种格式；结果像文件列表一样按目录浏览，扫描可暂停、继续和取消，恢复文件时生成 SHA-256 与来源报告。
- **插件分析**：基于 C ABI 的 LPP v2 动态库插件，每个任务在独立宿主进程中运行；插件在识别出的操作系统（按 fstab、盘符组装的卷）、整个检材或选中的文件上分析，输出带类型的记录，结果可排序、检索、回溯来源并查看运行审计；`.lcpkg` 插件包可在设置中安装。内置 `linuxinfo`，提取 Linux 系统、用户、登录、网络、服务、软件、定时任务与命令历史等信息。
- **命令行与挂载**：独立的 `lovelycarve-cli` 不依赖桌面运行时，以 `images open`、`files read` 这样的命令覆盖桌面端全部后端功能，支持交互 shell 和 JSONL 自动化；Windows 下可借助 Dokany 把卷只读挂载为盘符或文件夹。

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

| 类别 | 格式 |
| --- | --- |
| 镜像容器 | RAW / DD / IMG / ISO、E01 / Ex01、AD1、VMDK、VHD / VHDX、QCOW2、VDI、Parallels PHDI、DMG、DiskGenius PMFX、雷电模拟器 LDBK、光盘 BIN / CUE（多文件、多轨、Mode 1 / XA Form 1）、小米 HDR1 固件、UBI（原始逻辑卷与内嵌 SquashFS） |
| 逻辑检材 | 文件夹（自动展开其中的小米 `.bak` 备份）、TAR / TAR.GZ、ZIP、7z、Docker / OCI 镜像归档、磁盘内的 Docker 容器、iOS 本地备份、BCP 交换数据 |
| 分区与卷 | GPT、MBR / EBR、APM、BSD 标签、LVM2 线性卷（含跨 PV / 跨阵列）、mdadm RAID 成员（0.90 / 1.x）、Intel IMSM 与 SNIA DDF 固件 RAID、linear / RAID0 / 1 / 4 / 5 / 6 / 10 重组 |
| 文件系统 | NTFS、ext2/3/4、FAT12/16/32、exFAT、APFS、HFS+ / HFSX、Btrfs、ZFS、XFS（v1–v5）、ReFS、UFS / UFS2、SquashFS、ISO 9660 |
| 加密与快照 | BitLocker（密码、恢复密钥、BEK、明文密钥、FVEK / VMK）、LUKS1 / LUKS2、VeraCrypt、FileVault 2（CoreStorage）、加密 iOS 备份、NTFS 卷影副本（VSS） |

支持程度取决于格式变体、文件系统特性和镜像完整性。各格式的实现范围、与 libyal 系列库的能力对照和已知限制见 [格式与读取器](docs/formats/README.md)。

## 快速开始

1. 获取程序：Windows x64 预编译包通过 [GitHub Releases](https://github.com/LovelyTeams/LovelyCarve/releases) 发布，也可以按下文[从源码构建](#从源码构建)。运行需要 WebView2（Windows 10/11 通常已内置）；只读挂载另需 [Dokany](https://github.com/dokan-dev/dokany) 驱动，读取物理磁盘需要管理员权限。
2. 在起始页或「案件管理」中新建案件，选择一个空目录。
3. 「添加检材」选择镜像文件、文件夹、BCP 数据、RAID 成员或物理磁盘。分段镜像选第一段，其余分段放在同一目录。
4. 检材加入后会自动识别平台并在后台建立卷快照和文件索引，进度在右上角「任务中心」查看。
5. 在「镜像信息」查看分区、解锁加密卷、计算哈希；在「文件列表」浏览和预览；在「工具」中使用全局搜索、时间线和文件恢复；在「任务中心 → 快速分析」运行插件，结果出现在「分析」中。

完整操作说明见 [使用指南](docs/USAGE.md)。

## 命令行

`lovelycarve-cli` 是独立的命令行工具，不链接 Tauri 或 WebView，直接调用与桌面端相同的后端。命令采用 `<集合> <动词>` 结构，结果以 JSON 或原始字节写到标准输出：

```powershell
lovelycarve-cli images open "D:\evidence\disk.E01"
lovelycarve-cli --image "D:\evidence\disk.E01" files tree list --path / --depth 2
lovelycarve-cli --image "D:\evidence\disk.E01" files read "/<卷名称>/Users/alice/report.txt" > report.txt
lovelycarve-cli cli search "recover deleted files"
lovelycarve-cli docs search "BitLocker 恢复密码"
```

桌面端的每个后端命令都有对应的 CLI 命令；`cli search` 按意图查找命令，`docs search` 离线搜索随程序附带的使用文档与格式专题。`shell` 在一个进程中逐行执行命令，JSONL 会话适合脚本与代理编排一系列分析：

```powershell
Get-Content .\workflow.jsonl | lovelycarve-cli session --events
```

详见 [CLI 手册](docs/CLI.md)。

## 插件

插件是导出 `lovelycarve_plugin_v1` C ABI 的动态库，连同清单、记录结构与语言包打成插件包。插件通过 LPP v2（LovelyCarve Plugin Protocol）经只读视图访问检材，按声明的记录结构输出带类型的记录，由宿主保存、投影时间线并保留来源。SDK、隔离宿主 `lc-plugin-host` 与内置的 `linuxinfo` 由子模块 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 维护。已发现的插件被视为可信代码，进程隔离用于故障隔离，不是权限沙箱。

协议与开发说明见 [插件协议](docs/PLUGIN_PROTOCOL.md) 和 [Linux 基本信息插件](docs/LINUXINFO.md)。

## 从源码构建

桌面开发与发布配方面向 Windows x64。默认构建不包含二维码识别和 OCR，不需要 OpenCV 或 CUDA。

| 依赖 | 版本或说明 |
| --- | --- |
| Rust | stable，最低 1.99 |
| [Bun](https://bun.sh/) | 版本见 `package.json` 的 `packageManager` |
| [just](https://github.com/casey/just) | 1.53 或更新 |
| PowerShell | 7 或更新 |
| Visual Studio | C++ x64 生成工具与 Windows SDK |
| OpenSSL | Windows 开发文件，默认位置 `C:\Program Files\OpenSSL-Win64` |
| WebView2 | 运行时 |

```powershell
git clone --recurse-submodules https://github.com/LovelyTeams/LovelyCarve.git
cd LovelyCarve
just bootstrap   # 初始化插件子模块并安装前端依赖
just dev         # 启动桌面开发版
```

| 目的 | 命令 |
| --- | --- |
| 前端开发 / 生产构建 | `just dev-fe` / `just build-fe` |
| 桌面安装包 / 便携包 | `just build-release` / `just pack` |
| CLI 开发 / 发布 / Windows 压缩包 | `just run-cli` / `just build-cli` / `just pack-cli` |
| 含二维码识别与 OCR 的版本 | `just build-release-full` / `just pack-full` / `just pack-cli-full` |
| 全部检查 | `just verify-fe` / `just verify-rust` / `just verify-plugins` |

GitHub 自动生成的源码压缩包不含插件子模块，请使用 `--recurse-submodules` 克隆，已有克隆可运行 `git submodule update --init --recursive`。Visual Studio、OpenSSL 路径、可选 QR/OCR 依赖和跨平台 CLI 构建见 [开发指南](docs/DEVELOPMENT.md)；`just --list` 列出全部配方。

## 文档

| 主题 | 文档 |
| --- | --- |
| 使用 | [使用指南](docs/USAGE.md)、[CLI 手册](docs/CLI.md)、[iOS 备份](docs/IOS_BACKUPS.md) |
| 格式与读取器 | [格式总览与维护要求](docs/formats/README.md)，以及 NTFS、ext、FAT、XFS、ReFS、APFS、HFS+、GPT、MBR、LVM2、LUKS、BitLocker、FileVault 2、VSS 专题 |
| 插件 | [LPP 协议与 SDK](docs/PLUGIN_PROTOCOL.md)、[Linux 基本信息插件](docs/LINUXINFO.md) |
| 开发 | [开发指南](docs/DEVELOPMENT.md)、[架构说明](docs/ARCHITECTURE.md)、[前端规范](docs/FRONTEND.md)、[前端模块与生命周期](docs/FRONTEND_STRUCTURE.md)、[发布指南](docs/RELEASING.md) |

完整列表见 [文档导航](docs/README.md)。

## 局限

- 删除项和雕刻结果不保证内容完整：雕刻受块对齐、碎片化和格式覆盖范围限制，未命中不代表文件不存在。
- 独立的孤儿文件扫描和丢失分区扫描尚未接入；文件时间线目前只投影修改时间。
- 桌面开发与发布只在 Windows 上验证；macOS 和 Linux 需要自行构建和验证。
- 当前案件格式不兼容早期开发版的 `.carve` 案件。

## 参与贡献

欢迎提交问题报告、格式样本线索、文档修正和代码。开始之前请阅读 [贡献指南](CONTRIBUTING.md) 和 [仓库约定](AGENTS.md)。报告读取问题时附版本、复现步骤和可公开的最小合成样本，**不要上传真实检材、案件目录、个人数据或凭据**。安全漏洞请按 [安全策略](SECURITY.md) 私下报告。

## 许可与致谢

LovelyCarve 以 [AGPL-3.0-only](LICENSE) 许可发布。Copyright (C) 2026 LovelyCarve contributors.

项目使用并修改了许多开源读取器与解析库，包括 SecurityRonin 的 ewf / vmdk / vhd / vhdx / bitlocker / luks / veracrypt / vsc / ad1 / sqlite / ios-backup forensic 系列，以及 ntfs、ext4-view、rust-fatfs、exfat、xfuse、dpp（APFS / HFS+ / DMG）、backhand（SquashFS）、refsprogs 和 Google Magika 等；格式行为参照了 [libyal](https://github.com/libyal) 系列库与 [dfVFS](https://github.com/log2timeline/dfvfs) 的公开文档和样本，富文档预览使用 Flyfish File Viewer。各组件的来源、许可证与本地修改见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 和 [src-tauri/vendor/UPSTREAM.md](src-tauri/vendor/UPSTREAM.md)。
