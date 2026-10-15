# 文档导航

功能概览、截图、获取方式与快速开始见根目录的 [README](../README.md)。本目录的文档按读者分为三类；构建命令以根目录 `justfile` 为准，命令注册以 `src-tauri/src/lib.rs` 为准。

## 使用

| 文档 | 内容 |
| --- | --- |
| [使用指南](USAGE.md) | 案件与检材、镜像信息、文件浏览与预览、校验与导出、全局搜索、时间线、文件雕刻、插件分析、BCP 数据与只读挂载 |
| [CLI 手册](CLI.md) | `lovelycarve-cli` 的交互浏览、全部命令入口、JSONL 自动化会话，以及镜像与加密卷的命令行用法 |
| [iOS 备份](IOS_BACKUPS.md) | iTunes / Finder 本地备份的浏览、加密备份解锁与异常检查 |
| [Linux 基本信息插件](LINUXINFO.md) | 内置 `linuxinfo` 插件的分析范围、构建部署与验证 |

## 格式与读取器

[格式总览与维护要求](formats/README.md) 汇总读取器的通用约束，并链接各格式专题：

[NTFS](formats/NTFS.md) · [ext2/3/4](formats/EXT.md) · [FAT / exFAT](formats/FAT.md) · [XFS](formats/XFS.md) · [ReFS](formats/REFS.md) · [APFS](formats/APFS.md) · [HFS+ / HFSX](formats/HFS.md) · [GPT](formats/GPT.md) · [MBR / EBR](formats/MBR.md) · [LVM2](formats/LVM.md) · [LUKS](formats/LUKS.md) · [BitLocker](formats/BITLOCKER.md) · [FileVault 2](formats/FILEVAULT.md) · [卷影副本](formats/VSS.md)

vendor [GZIP 读取器](formats/GZIP.md) 参照 libgzipf 实现多成员、元数据、校验和 seek，已接入 tar.gz / tgz、Docker / OCI 压缩层与小米备份。

vendor [RAW / 分段 RAW](formats/SMRAW.md) 参照 libsmraw 实现分段读写与完整性检查，读取接入桌面和 CLI。

vendor [QCOW / QCOW2](formats/QCOW.md) 对照 libqcow 全部 42 API，支持 v1/2/3、DEFLATE、差分父链和 legacy AES 凭据读取。

vendor [Parallels PHDI](formats/PHDI.md) 对照 libphdi 56 API，读取 XML / .hdd bundle、v2 Plain / Sparse / Split 和差分父链；接入桌面与 CLI。

vendored 读取器的上游版本与本地补丁记录在 [vendor/UPSTREAM.md](../src-tauri/vendor/UPSTREAM.md)，后端探针程序见 [examples/README.md](../src-tauri/examples/README.md)。

## 开发

| 任务 | 文档 |
| --- | --- |
| 配置环境、构建、打包与性能验证 | [开发指南](DEVELOPMENT.md) |
| 提交修改、选择验证范围 | [贡献指南](../CONTRIBUTING.md)、[仓库约定](../AGENTS.md) |
| 理解后端、证据会话、任务、案件存储与索引 | [架构说明](ARCHITECTURE.md) |
| 修改界面与前端状态 | [前端规范](FRONTEND.md)、[前端模块与生命周期](FRONTEND_STRUCTURE.md) |
| 增加命令或事件 | [架构说明 · 功能与 IPC 对照](ARCHITECTURE.md#功能与-ipc-对照)、[扩展流程](ARCHITECTURE.md#扩展流程) |
| 开发插件 | [LPP 协议与 SDK](PLUGIN_PROTOCOL.md) |
| 发布版本 | [发布指南](RELEASING.md)、[第三方声明](../THIRD_PARTY_NOTICES.md) |
| 报告安全问题 | [安全策略](../SECURITY.md) |

新增或修改功能时同步更新所属文档：用户可见的变化写入 [CHANGELOG](../CHANGELOG.md) 的 `[Unreleased]`，格式能力写入对应的格式专题。测试样本的来源、许可与哈希保存在样本目录，第三方许可证与来源说明随代码保留。
