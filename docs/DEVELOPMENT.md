# 开发指南

所有任务都在仓库根目录通过 [just](https://github.com/casey/just) 执行（建议 1.53 或更新版本）。首次克隆后运行 `just bootstrap`，它会初始化插件子模块并执行 `bun install --frozen-lockfile`；之后用 `just --list` 查看全部入口。

```powershell
git clone --recurse-submodules https://github.com/LovelyTeams/LovelyCarve.git
cd LovelyCarve
just bootstrap
just dev
```

## 环境

| 组件 | 要求 |
| --- | --- |
| Rust | stable，最低版本以 `src-tauri/Cargo.toml` 的 `rust-version` 为准，当前为 1.99 |
| Bun | 版本以 `package.json` 的 `packageManager` 为准 |
| Windows 桌面 | PowerShell 7、Visual Studio C++ x64 工具、Windows SDK、OpenSSL 开发文件、WebView2 |
| macOS | Xcode Command Line Tools；桌面检查另需相应的 Tauri 系统依赖 |
| Linux | 编译工具链、WebKitGTK、OpenSSL 等 Tauri 系统依赖 |
| CLI smoke 测试 | Python 3 |
| Windows 便携打包 | PATH 中的 `upx` 与 `7z` |

默认构建关闭二维码识别与 OCR，不需要 OpenCV 或 CUDA。桌面开发和打包配方只支持 Windows；Rust 检查、前端任务、插件构建和 CLI 构建可以在其他平台运行，但 macOS / Linux 产物需要单独验证。

前端的离线预览资源由准备脚本从依赖复制到 `public/file-viewer/`，不提交到 Git；`just dev-fe` 与 `just dev` 会先准备这些资源。只运行前端无法读取证据。Vite 固定监听 `http://127.0.0.1:14200`，更换端口时同时修改 `vite.config.ts` 与 `src-tauri/tauri.conf.json`，并保留 `strictPort`。

## Windows 配置

配方在当前命令的 PowerShell 中加载 `scripts/windows-env.ps1`。Visual Studio 默认通过 `vswhere` 查找已安装 C++ x64 工具的最新实例；其他位置可以用环境变量或 `just name=value <任务>` 指定：

```powershell
$env:VS_PATH = 'C:\Program Files\Microsoft Visual Studio\2022\BuildTools'
$env:OPENSSL_DIR = 'C:\Program Files\OpenSSL-Win64'
$env:OPENSSL_LIB = "$env:OPENSSL_DIR\lib\VC\x64\MT"
just dev
```

| just 变量 | 环境变量 | 默认行为 |
| --- | --- | --- |
| `vs_path` | `VS_PATH` | 空值，由 `vswhere` 查找 |
| `openssl_dir` | `OPENSSL_DIR` | `C:\Program Files\OpenSSL-Win64` |
| `openssl_lib` | `OPENSSL_LIB` | OpenSSL 目录下的 `lib\VC\x64\MT` |
| `perl_bin` | `PERL_BIN` | 空值，不修改 Perl 搜索路径 |
| `opencv_dir` | `OPENCV_DIR` | 仓库下的 `opencv-contrib-install` |
| `opencv_lib` | `OPENCV_LIB` | `opencv_world4130` |
| `opencv_msvc` | `OPENCV_MSVC` | `vc18`，按实际安装修改 |
| `opencv_src` | `OPENCV_SRC` | 仓库下的 `opencv\sources` |
| `opencv_contrib` | `OPENCV_CONTRIB` | 仓库下的 `opencv_contrib-4.x\opencv_contrib-4.x\modules` |
| `opencv_install` | `OPENCV_INSTALL` | 与 `opencv_dir` 相同 |

轻量的 Rust 检查只配置 OpenSSL；桌面与发布 CLI 构建会初始化 VS x64 环境并使用 MSVC 默认链接器。只有 `*-full` 入口配置 OpenCV 与 libclang，需要另装 Visual Studio 的 LLVM 工具组件。Cargo 并发由工具链默认值决定，内存不足时可通过 `CARGO_BUILD_JOBS=1` 或 `2` 限制。

SQLCipher 配方设置 `OPENSSL_NO_VENDOR=1` 和 `OPENSSL_STATIC=1`，使用本机安装的 OpenSSL 开发文件；如改为从源码编译 OpenSSL，需要准备 Windows Perl 与 NASM。静态链接标记不保证没有 DLL 依赖，调整 `OPENSSL_LIBS` 或分发 DLL 前请检查二进制的实际依赖。

## 日常任务

```text
just                          # 按工作流顺序显示任务
just --list --group frontend  # 查看一个分组
just --show build-cli         # 查看本平台的配方内容
just --dry-run pack           # 展开依赖与命令，但不执行
```

| 工作 | 入口 |
| --- | --- |
| 前端 | `dev-fe`、`typecheck`、`test-fe`、`build-fe`、`ipc-check`、`frontend-structure-check` |
| 后端 | `fmt`、`check`、`clippy`、`test [名称及 Cargo 参数]`、`check-examples` |
| Windows 桌面 | `dev`、`build`、`build-release`、`pack`、`repack`；含 QR/OCR 的版本使用对应的 `*-full` |
| CLI | `run-cli`、`build-cli`、`build-cli-full`、`test-cli`、`smoke-cli <exe>`；Windows 另有 `pack-cli`、`pack-cli-full`、`repack-cli` |
| 插件 | `build-plugins [--release]`、`test-plugins`、`clippy-plugins`；Windows 另有 `pack-plugins` |
| 读取器 | `test-vendor <目录>`、`test-ewf`、`test-vmdk`、`test-vhd`、`test-vhdx`、`test-qcow2`、`test-vdi`、`test-bitlocker` / `clippy-bitlocker`、`test-luks` / `clippy-luks`、`test-veracrypt`、`test-vsc` / `clippy-vsc`、`test-refs` / `clippy-refs`、`test-ad1`、`test-sqlite-core`、`test-ios-backup`、`test-apfs` / `clippy-apfs` |
| 探针与性能 | `run-example`、`run-example-fast`、`bench-filedb-store`、`probe-refs`、`probe-vsc`、`compare-vsc` |
| 依赖更新 | `update-fe`、`update-rust`、`update-plugins`、`update-tauri`、`update-file-viewer <版本>` |
| 资源维护 | `icons`、`qr-env`、`build-opencv-contrib`、`qr-test <图片>` |

`verify-fe` 汇总 IPC、结构、类型检查、前端测试和生产构建；`verify-rust` 汇总格式化、编译、Clippy 和后端测试；`verify-plugins` 汇总格式化、插件 Clippy 和插件测试。按 [贡献指南](../CONTRIBUTING.md#验证) 选择验证范围。

QCOW 使用 `just test-qcow2` / `just clippy-qcow2`；宿主回归用 `just test qcow --lib --jobs 1`，CLI 用 `just test-cli`。`just generate-qcow-fixtures` 可重建独立 Python zlib/OpenSSL 样本（需 cryptography），能力与边界见 [QCOW](formats/QCOW.md)。

Parallels PHDI 使用 `just test-phdi` / `just clippy-phdi`；宿主回归用 `just test phdi --lib --jobs 1`，真实命令打开 / 导出用 `just test-cli`。可选外部 oracle、56 API 对照及验证边界见 [PHDI](formats/PHDI.md)。

`fmt` 覆盖主 crate、插件与维护中的读取器。独立 GZIP 库的验证入口为 `just test-gzipf` / `just clippy-gzipf`，也可使用 `just test-vendor gzipf`；宿主 GZIP 回归用 `just test gzip --lib`，CLI 接入用 `just test-cli`，格式范围见 [GZIP](formats/GZIP.md)。公共读取接口改动后运行 `check-examples`。参数含空格时，需要保留一层传给子 shell 的引号：

```powershell
just run-example probe_partitions '"D:\Evidence Files\disk.E01"'
```

后端探针的用途与参数见 [examples/README.md](../src-tauri/examples/README.md)，各读取器的专项测试、可选外部样本与环境变量见 [格式与读取器](formats/README.md)。

分段 RAW 独立库使用 `just test-smraw` / `just clippy-smraw`，宿主回归使用
`just test smraw --lib --jobs 1` 与 `just test-cli`。Windows rustc 在大型测试 crate
发生增量编译崩溃时，可通过原配方传入
`--config profile.test.package.lovelycarve.debug=0 --config profile.test.package.lovelycarve.incremental=false`，
仅降低宿主测试包调试信息并禁用其增量缓存；不改变发布配置。范围见 [SMRAW](formats/SMRAW.md)。

## 插件与资源

`plugins/` 是 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 的 Git 子模块，只包含 `sdk`、`host` 与 `linuxinfo`。主仓库固定插件的提交；递归克隆或 `just bootstrap` 会初始化它，已有克隆可运行 `git submodule update --init --recursive`。GitHub 生成的源码压缩包不包含子模块，开发请使用 Git 克隆。

`build-plugins` 按提交的锁文件构建宿主和 linuxinfo，并把动态库复制到 `plugin.toml` 旁。`dev` 使用开发版插件；`build-release` 与 CLI 打包先构建发布版插件。修改插件时先在插件仓库提交并推送，再在主仓库提交子模块指针，不要只提交指针而漏推插件提交。

安装包和便携包共用 `src-tauri/tauri.plugins.generated.conf.json`：主程序旁放置 `lc-plugin-host.exe`，插件目录包含 `plugins/linuxinfo/plugin.toml` 和 DLL，插件不嵌入主程序。单独部署插件使用 `just pack-plugins`；新增插件目录后在设置页手动“检查插件目录”即可发现；更换已有插件文件前退出应用，重启后生效。`just plugin-bundle` 从全部清单生成资源配置，声明的 `[library] resources` 也随包分发；生成配置不提交到 Git。资源生成回归用 `just test-plugin-bundle`。接口与工件来源见 [插件协议](PLUGIN_PROTOCOL.md) 和 [Linux 基本信息插件](LINUXINFO.md)。

ReFS 使用纯 Rust 读取器；旧的 `librefs-13.dll` 只在显式的 `refs-dll-oracle` 特性下用于对照，不随正式包分发。Dokan、OpenSSL 与可选的 OpenCV DLL，以及模型和许可证属于构建 / 运行资源，按打包配置分发。`src-tauri/binaries` 中的 `vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll` 和 `msvcp140_1.dll` 是 Visual Studio 可再发行目录中的 VC++ 运行库，随主程序放在同一目录：主程序和 CLI 静态链接 `vcruntime`，但动态依赖 C++ 标准库 `msvcp140`（它又依赖 `vcruntime140` / `vcruntime140_1`），`lc-plugin-host`、`linuxinfo` 和 OpenSSL 的 `libcrypto` 动态依赖 `vcruntime140`；便携包打包时不对这些 DLL 做 UPX 压缩。Tauri 的 MSI 打包会收入 `src-tauri/target/release` 根目录下的所有 DLL，`build-release` 因此先删除其中未在打包配置里声明的 DLL（例如完整构建留下的 OpenCV 和 ONNX Runtime 执行提供者）。发布前用 `dumpbin /dependents` 核对主程序、CLI、插件宿主、插件和随附 DLL 的依赖，除 Windows 自带组件外都应能在包内找到。测试夹具及其来源说明保存在 vendor 与 tests 目录中，不要用真实检材替换。

图标保留透明和带背景两张原图，位于 `src-tauri/icons/source*.png`；修改后执行 `just icons` 同步各平台图标和前端 512px 资源。

## 可选 QR/OCR 与发布构建

Windows 的 `dev-full`、`build-full`、`build-release-full` 和 `build-cli-full` 启用 WeChat 二维码识别和带 CPU 回退的 CUDA OCR。需要准备版本匹配的 OpenCV 及 contrib 源码，用 `just build-opencv-contrib` 构建含 `wechat_qrcode` 的 OpenCV；核对安装目录中的 `vc*` 与 world 库名称后设置对应变量。官方的核心 OpenCV 发行包不包含所需模块。

Unix 的 `build-cli-full` 启用 `qrcode-wechat ocr`（CPU OCR），需要自行配置系统依赖。Windows 发布 CLI 使用 `mt.exe` 嵌入清单，Cargo 构建失败时不会执行这一步。

`pack` / `pack-cli` 默认不带 QR/OCR 资源；完整包使用 `pack-full` / `pack-cli-full`，额外资源来自 `src-tauri/tauri.full.conf.json`。`repack` / `repack-full` 复用已有的桌面产物，必须与编译特性一致；`repack-cli` 按 CLI 的实际能力选择资源。便携包输出到 `dist-pack/`，安装包输出到 `src-tauri/target/release/bundle/`。

仓库不提供 GitHub Actions 工作流，验证与打包都在本地完成，发布步骤见 [发布指南](RELEASING.md)。

## 性能验证

桌面 `release` profile 以体积优先（`opt-level = "s"`、fat LTO），哈希、压缩、SQLCipher 与正则等热点依赖在 `src-tauri/Cargo.toml` 中单独设为 `opt-level = 3`；性能比较使用 `release-fast` profile（`just run-example-fast <名称>`）。开发版的 Argon2 / BLAKE2 同样启用优化，以满足高内存 LUKS2 的密钥派生预算。完整 QR/OCR 构建与轻量构建的耗时不能混作同一基线。

性能结论应基于当前提交的可复现场景。记录提交号、操作系统、CPU / 内存、存储介质、构建 profile、特性组合、镜像格式与逻辑大小；文件系统操作另记录目录项规模、文件大小分布和卷类型，私有检材只记录可公开的规模描述。分别测量冷启动与缓存命中，报告重复次数与耗时分布，并观察吞吐、峰值内存、临时磁盘占用、读取次数、取消延迟和交互延迟。

| 场景 | 观察目标 |
| --- | --- |
| 顺序整盘哈希与 RAW 导出 | 完整长度、吞吐、缓冲复用、坏区反馈 |
| 大目录与百万行 filedb | 首屏、翻页、排序、过滤、计数与内存 |
| 多卷快照与索引导入 | 构建时间、事务等待、取消、旧索引可读性 |
| 全局搜索 | 编码与正则匹配、分段结果一致性、命中落库、交互响应 |
| 文件雕刻 | 单 / 多线程结果一致性、解析读取量、暂停与取消 |
| 预览与并发操作 | 媒体缓存预算、引用释放、哈希期间的目录浏览 |
| Dokan 挂载读取 | 随机寻址、每句柄窗口命中、短读错误与只读保护 |
| 插件分析 | 多任务调度、进度写入、结果上限与源路径读取 |
| 关闭案件与卸载镜像 | 任务退出、句柄释放和临时文件清理 |

```text
just bench-filedb-store
just run-example-fast probe_partitions <image-path>
just run-example probe_carve <image-path>
```

优化前后核对结果、长度和校验值：分页需要稳定顺序，并行扫描需要稳定合并，缓存需要有界容量和明确的清理点。增加并发时同时测量目录浏览等前台交互，不能只提高后台吞吐。失败、取消、短读和 SQLite 忙等待同样属于性能验证范围。

## 更新依赖

先审查清单约束，再用 `update-fe`、`update-rust` 或 `update-plugins` 更新对应的锁文件。`update-tauri` 同步前后端的 Tauri 包族，各包独立发布，补丁版本号不必一致。vendor 的来源与补丁见 [UPSTREAM.md](../src-tauri/vendor/UPSTREAM.md)，更新后用对应读取器的配方验证。

Vue 类型检查通过 `typescript` 的 npm 别名接入 `typescript-native-bridge@6.0.3-bridge.18.tsgo.7.0.2`，以兼容 `vue-tsc` 需要的经典编译器 API；别名与 override 固定为同一完整版本，不要直接替换为 `typescript@7`。`just typecheck` 会显示 `TNB ACTIVE`。普通组件测试默认关闭动效，动效专用测试单独验证开关与收尾。

前端生产构建使用 Vite 8 自带的 Oxc 压缩器，不需要额外安装 esbuild，也不要恢复 `build.minify: 'esbuild'`。非 Windows 平台保持 Safari 13 构建目标，但地图渲染器依赖 BigInt，map / shp 预览需要 Safari 14 或更新的 WebKit。

升级后按改动范围执行前端、Rust、插件与读取器验证。不要把本机路径、依赖目录、案件、缓存、导出结果和签名凭据提交到仓库；`.gitignore` 为回归样本保留了例外。

### HFS 与 FileVault 读者

`just test-vendor hfsplus` / `just clippy-hfsplus` 验证 HFS；`just test-filevault` / `just clippy-filevault` 验证 CoreStorage 核心和法证审计器。设置 `FVDE_ORACLE_IMAGE` 为公开测试样本的 raw CoreStorage 区域后运行 `just test-filevault`，执行真实扇区 SHA、HFS 读者与法证审计 oracle；提取范围与哈希见 [FileVault](formats/FILEVAULT.md)。未设置变量不等于通过真实样本验证。
