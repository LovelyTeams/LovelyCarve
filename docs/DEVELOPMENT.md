# 开发指南

所有任务都在仓库根目录通过 [just](https://github.com/casey/just) 执行（建议 1.53 或更新版本）。首次克隆后运行 `just bootstrap` 初始化插件子模块；之后用 `just --list` 查看全部入口。

```powershell
git clone --recurse-submodules https://github.com/LovelyTeams/LovelyCarve.git
cd LovelyCarve
just bootstrap
just dev
```

## 环境

| 组件 | 要求 |
| --- | --- |
| Rust | stable，最低版本以根目录 `Cargo.toml` 的 `rust-version` 为准，当前为 1.99 |
| Bun | 运行 `scripts/build-plugins.mjs`（`just build-plugins`、`dev`、`build-release` 与 CLI 打包），CI 固定 1.4.2 |
| Windows | PowerShell 7、Visual Studio C++ x64 工具与「适用于 Windows 的 C++ Clang 工具」（含 `lld-link`）、Windows SDK、OpenSSL 开发文件 |
| macOS | Xcode Command Line Tools；桌面应用另需 GPUI 在该平台的系统依赖 |
| Linux | 编译工具链与 OpenSSL 开发文件；桌面应用另需 GPUI 在该平台的系统依赖 |
| CLI smoke 测试 | Python 3 |
| DSH 集成测试 | Node.js |
| Windows 便携打包 | PATH 中的 `7z`；默认还需要 `upx`（`just pack -NoUpx` 可跳过） |

默认构建关闭二维码识别与 OCR，不需要 OpenCV 或 CUDA。桌面开发和打包配方只支持 Windows；Rust 检查、插件构建和 CLI 构建可以在其他平台运行，但 macOS / Linux 产物需要单独验证。

桌面程序与 CLI 都以调用者权限运行（`asInvoker`），启动时不自动请求管理员权限：CLI 的清单由构建脚本嵌入，桌面程序沿用 GPUI 自带的清单。读取物理磁盘或采集实时内存时，需要用户手动以管理员身份启动程序。

## 工作区

根目录的 `Cargo.toml` 是 Cargo 工作区，成员为 `crates/core`（取证引擎）、`crates/backend`（进程内后端）、`crates/cli`（独立 CLI）和 `crates/desktop`（GPUI Kit 桌面应用，可执行文件 `LovelyCarve`）。共享依赖版本、Clippy lint、构建 profile 与 `notatin` 补丁都在根清单中；`vendor/`、`plugins/` 与 `python/` 不属于工作区，保留各自的清单和锁文件。构建输出位于根目录的 `target/`。

GPUI 会为 serde_json 开启 `preserve_order`，与 CLI 在同一次 Cargo 调用中编译时会改变 CLI 动态 JSON 输出的键顺序。因此 `default-members` 不含桌面应用，`cargo build` / `cargo test` 不带 `-p` 时只构建引擎、后端与 CLI；`just check`、`just clippy` 分两次调用分别检查这两组，桌面测试使用单独的 `just test-desktop`。

后端构建脚本在 Windows 上把 `binaries/` 中的 Dokan、OpenSSL 与 VC++ 运行库复制到当前 profile 目录（如 `target/debug/`），因此 `just dev`、`just run-cli`、测试与直接双击开发版都能找到这些 DLL。

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

工作区 Rust 检查、桌面、CLI 与插件配方会初始化 VS x64 环境，默认使用所选 Visual Studio 中的 `lld-link.exe`，Cargo 同时编译 1 个任务，链接器使用 1 个线程，release 使用 Thin LTO。只有 `*-full` 入口额外配置 OpenCV 与 libclang。独立 vendored 读取器的配方沿用 Cargo 自身配置。

以下 Cargo 环境变量可覆盖默认值；脚本不会覆盖调用者已设置的值，也不会向非 LLD 链接器添加 `/threads:1`：

| 环境变量 | Windows 配方默认值 |
| --- | --- |
| `CARGO_BUILD_JOBS` | `1` |
| `CARGO_PROFILE_RELEASE_LTO` | `thin` |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER` | 所选 VS 的 `VC/Tools/Llvm/x64/bin/lld-link.exe`，自动解析完整路径 |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS` | 使用 LLD 时为 `-C link-arg=/threads:1` |

例如可设置 `CARGO_BUILD_JOBS=2` 调整编译并发，或设置 `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=link.exe` 切回 MSVC 链接器。`RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` 的优先级沿用 Cargo 规则，设置它们时需自行包含需要保留的链接线程参数。非 Windows 配方不加载该脚本。

SQLCipher 配方设置 `OPENSSL_NO_VENDOR=1` 和 `OPENSSL_STATIC=1`，使用本机安装的 OpenSSL 开发文件；如改为从源码编译 OpenSSL，需要准备 Windows Perl 与 NASM。静态链接标记不保证没有 DLL 依赖，调整 `OPENSSL_LIBS` 或分发 DLL 前请检查二进制的实际依赖。

## 日常任务

```text
just                          # 按工作流顺序显示任务
just --list --group desktop   # 查看一个分组
just --show build-cli         # 查看本平台的配方内容
just --dry-run pack           # 展开依赖与命令，但不执行
```

| 工作 | 入口 |
| --- | --- |
| 工作区 | `fmt`、`check`、`clippy`、`test [名称及 Cargo 参数]`、`check-examples` |
| 桌面应用 | `dev`、`build [--release]`、`check-desktop`、`test-desktop`；Windows 另有 `build-release`、`pack`、`repack` |
| CLI | `run-cli`、`build-cli`、`build-cli-full`、`test-cli`、`smoke-cli <exe>`、`update-cli-skill`（重新生成代理技能的命令参考）；Windows 另有 `pack-cli`、`pack-cli-full`、`repack-cli`、`stage-cli-skill` |
| Python 读取库 | `develop-python`、`build-python`、`test-python`、`fmt-python`、`clippy-python`；使用 uv 管理解释器，默认 Python 3.15，详见 [Python 文档](../python/README.md) |
| 插件 | `build-plugins [--release]`、`test-plugins`、`clippy-plugins`；Windows 另有 `pack-plugins`（生成 `.lcpkg`） |
| 读取器 | `test-vendor <目录>`、`test-ewf`、`test-vmdk`、`test-vhd`、`test-vhdx`、`test-minix` / `clippy-minix`、`generate-minix-fixtures`、`test-udf` / `clippy-udf`、`generate-udf-fixtures`、`test-qcow2` / `clippy-qcow2`、`generate-qcow-fixtures`、`test-vdi`、`test-phdi` / `clippy-phdi`、`test-smraw` / `clippy-smraw`、`test-gzipf` / `clippy-gzipf`、`clippy-ubi`、`test-bitlocker` / `clippy-bitlocker`、`test-luks` / `clippy-luks`、`test-veracrypt`、`test-filevault` / `clippy-filevault`、`test-vsc` / `clippy-vsc`、`test-refs` / `clippy-refs` / `test-refs-oracle`、`test-ad1`、`test-sqlite-core`、`test-ios-backup`、`test-apfs` / `clippy-apfs`、`clippy-hfsplus` |
| 探针与性能 | `run-example`、`run-example-fast`、`bench-filedb-store`、`probe-refs`、`probe-vsc`、`compare-vsc` |
| 依赖更新 | `update-rust`、`update-plugins` |
| QR/OCR 资源 | `qr-env`、`build-opencv-contrib`、`qr-test <图片>` |

`verify-rust` 汇总格式化、编译、Clippy、引擎 / 后端 / CLI 测试与桌面测试；`verify-plugins` 汇总格式化、插件 Clippy 和插件测试。按 [贡献指南](../CONTRIBUTING.md#验证) 选择验证范围。

各 crate 的默认特性为空。新增后端命令要在 `crates/backend/src/lib.rs` 的 `command_registry!` 中注册，并在 `crates/cli/src/commands.toml` 中映射为 CLI 命令，命名规则不满足时构建脚本报错，规则见[架构说明](ARCHITECTURE.md#命令行与桌面只读挂载)。

桌面应用的程序参数写在 `--` 之后，例如 `just dev -- --case <案件目录> <检材路径>`：`--case` 打开该目录的案件，目录中还没有案件时以目录名新建；其余参数作为检材路径加载。`just dev` 先构建开发版插件，使应用能发现仓库中的插件包。桌面应用目前不提供文件夹大小树图、时间线的事件类型筛选与热力图、插件任务的暂停与重新分析、应用日志页、案件管理中的卷索引重建，以及 SQLite、Realm、ZIP、CSV 等结构化原类型视图和二维码 / OCR；这些后端能力可通过 CLI 使用。

桌面应用的源码预览按文件名或扩展名识别，通过 GPUI Kit 只读编辑器显示行号、缩进线、查找和 Tree-sitter 语法高亮。无对应语法时显示纯文本，单次预览读取文件开头最多 1 MiB，并提示截断及解码错误。

Windows x64 的其他文件类型由外部安装的 Oracle Outside In Viewer 8.5.7 预览。缺少运行库时，通过「选择 Viewer 目录」保存包含 `sccvw.dll`、`oswin64.dll` 及其余运行库和资源的完整目录。未保存目录时，依次查找 `LOVELYCARVE_OUTSIDE_IN_DIR` 和可执行文件旁的 `viewer` 目录。Viewer 由独立进程承载，通过 Win32 子窗口嵌入 GPUI，读取后端准备的检材临时副本，运行库由使用者提供。

`fmt` 覆盖工作区 crate、插件与维护中的读取器。公共读取接口改动后运行 `check-examples`。参数含空格时，需要保留一层传给子 shell 的引号：

```powershell
just run-example probe_partitions '"D:\Evidence Files\disk.E01"'
```

后端探针的用途与参数见 [examples/README.md](../crates/core/examples/README.md)，各读取器的专项测试、可选外部样本与环境变量见 [格式与读取器](formats/README.md)。

### 读取器验证

独立读取库用各自的测试与 Clippy 配方验证；接入宿主的部分另用 `just test <过滤词> --lib` 运行宿主回归，命令行接入用 `just test-cli`。

- QCOW：`just test-qcow2` / `just clippy-qcow2`，宿主回归用 `just test qcow --lib --jobs 1`。`just generate-qcow-fixtures` 可重建独立 Python zlib/OpenSSL 样本（需 cryptography），能力与边界见 [QCOW](formats/QCOW.md)。
- Parallels PHDI：`just test-phdi` / `just clippy-phdi`，宿主回归用 `just test phdi --lib --jobs 1`，真实命令打开 / 导出用 `just test-cli`。可选外部 oracle、56 API 对照及验证边界见 [PHDI](formats/PHDI.md)。
- 分段 RAW：`just test-smraw` / `just clippy-smraw`，宿主回归用 `just test smraw --lib --jobs 1` 与 `just test-cli`，范围见 [SMRAW](formats/SMRAW.md)。
- GZIP：`just test-gzipf` / `just clippy-gzipf`（也可使用 `just test-vendor gzipf`），宿主回归用 `just test gzip --lib`，CLI 接入用 `just test-cli`，格式范围见 [GZIP](formats/GZIP.md)。
- UBI：`just test-vendor ubidump` / `just clippy-ubi`，宿主回归用 `just test ubi --lib` 与 `just test xiaomi_firmware --lib`，支持范围见 [UBI 固件](formats/UBI.md)。
- QNX4 / QNX6：`just test-qnx` / `just clippy-qnx`，宿主回归用 `just test qnx --lib`，CLI 用 `just test-cli`。`just generate-qnx-fixtures` 重建字段夹具；`just test-qnx-reference <qnxprobe.py路径>` 运行独立 Python 正例 oracle。版本、外部样本与限制见 [QNX](formats/QNX.md)。
- F2FS / EROFS：`just test-vendor f2fs-reader` / `just clippy-f2fs`、`just test-erofs` / `just clippy-erofs`；宿主回归用 `just test flashfs_ --lib`，CLI 用 `just test-cli`。`just generate-f2fs-fixtures` 重建独立字段夹具；真实 mkfs 样本与限制见 [F2FS](formats/F2FS.md) 和 [EROFS](formats/EROFS.md)。
- HFS+ 与 FileVault 2：`just test-vendor hfsplus` / `just clippy-hfsplus` 验证 HFS；`just test-filevault` / `just clippy-filevault` 验证 CoreStorage 核心和法证审计器。设置 `FVDE_ORACLE_IMAGE` 为公开测试样本的 raw CoreStorage 区域后运行 `just test-filevault`，执行真实扇区 SHA、HFS 读者与法证审计 oracle；提取范围与哈希见 [FileVault](formats/FILEVAULT.md)。未设置变量不等于通过真实样本验证。

Windows rustc 在大型测试 crate 发生增量编译崩溃时，可通过原配方传入 `--config profile.test.package.lovelycarve-core.debug=0 --config profile.test.package.lovelycarve-core.incremental=false`，仅降低引擎测试包调试信息并禁用其增量缓存；不改变发布配置。

## 插件与资源

`plugins/` 是 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 的 Git 子模块，包含 `sdk`、`sdk-derive`、`host` 与 `linuxinfo`。主仓库固定插件的提交；递归克隆或 `just bootstrap` 会初始化它，已有克隆可运行 `git submodule update --init --recursive`。GitHub 生成的源码压缩包不包含子模块，开发请使用 Git 克隆。

`build-plugins` 按提交的锁文件构建宿主和各插件库，把动态库复制到 `plugin.toml` 旁，并用 `lc-plugin-host --describe` 写出 `schema.json`；`--plugin <目录>` 只构建一个插件库，沿用已构建的加载器。`dev` 使用开发版插件；`build-release` 与 CLI 打包先构建发布版插件。修改插件时先在插件仓库提交并推送，再在主仓库提交子模块指针，不要只提交指针而漏推插件提交。

桌面便携包与 CLI 包由 `scripts/lib/release-bundle.ps1` 放置同一套插件运行文件：主程序旁放 `lc-plugin-host.exe`，每个插件包目录 `plugins/<名称>/` 包含 `plugin.toml`、动态库、`schema.json` 与 `locales/`，插件不嵌入主程序。`just pack-plugins` 为每个工作区插件生成可在「设置 → 插件」或 `plugins create` 安装的 `.lcpkg`。接口与工件来源见 [插件协议](PLUGIN_PROTOCOL.md) 和 [Linux 基本信息插件](LINUXINFO.md)。

ReFS 使用纯 Rust 读取器；旧的 `librefs-13.dll` 只在 `lovelycarve-core` 显式的 `refs-dll-oracle` 特性下用于对照，不随正式包分发。Dokan、OpenSSL 与可选的 OpenCV DLL，以及模型和许可证属于构建 / 运行资源，由打包脚本放到可执行文件旁。`binaries/` 中的 `vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll` 和 `msvcp140_1.dll` 是 Visual Studio 可再发行目录中的 VC++ 运行库，随主程序放在同一目录：主程序和 CLI 静态链接 `vcruntime`，但动态依赖 C++ 标准库 `msvcp140`（它又依赖 `vcruntime140` / `vcruntime140_1`），`lc-plugin-host`、`linuxinfo` 和 OpenSSL 的 `libcrypto` 动态依赖 `vcruntime140`；便携包打包时不对这些 DLL 做 UPX 压缩。发布前用 `dumpbin /dependents` 核对主程序、CLI、插件宿主、插件和随附 DLL 的依赖，除 Windows 自带组件外都应能在包内找到。测试夹具及其来源说明保存在 vendor 与 tests 目录中，不要用真实检材替换。

品牌资源位于 `crates/desktop/assets/`：桌面应用内嵌的 `app-icon.png` / `app-icon-background.png` 与 Quicksand 字体，以及 `icons/` 中透明和带背景的原图与 Windows 图标 `icon.ico`（16、24、32、48、64、256px）。修改原图后需要同步导出 512px 的两张品牌 PNG 和 `icon.ico`。桌面应用的构建脚本在 Windows 上把 `icons/app.rc` 编译为 `LovelyCarve.exe` 的 1 号图标资源：资源管理器显示它，GPUI 把它注册为窗口类图标，任务栏和 Alt+Tab 也随之显示。应用清单由 GPUI Kit 嵌入，`app.rc` 只含图标。

文件列表的文件类型、文件夹和展开箭头图标位于 `crates/desktop/assets/file_icons/`，是第三方 SVG 的原样副本，来源提交与许可证见该目录的 `README.md`、`LICENSE-GPL` 和 `THIRD_PARTY_NOTICES.md`。`crates/desktop/src/assets.rs` 的 `file_icons!` 列出打包的文件，`crates/desktop/src/files/icons/file_types.rs` 维护文件名、后缀到图标的对应关系；测试保证目录中的 SVG、打包清单与对应表三者一致，增删图标时要同步修改。界面其余图标是 GPUI Kit 的 Lucide 图标，登记在同一文件的 `icon_assets!` 中，另一项测试扫描界面源码，用到却未登记或登记了却没用到时失败。

## 可选 QR/OCR 与发布构建

Windows 的 `build-cli-full` 启用 WeChat 二维码识别和带 CPU 回退的 CUDA OCR。需要准备版本匹配的 OpenCV 及 contrib 源码，用 `just build-opencv-contrib` 构建含 `wechat_qrcode` 的 OpenCV；核对安装目录中的 `vc*` 与 world 库名称后设置对应变量。官方的核心 OpenCV 发行包不包含所需模块。桌面应用没有 QR/OCR 界面，始终不启用这两个特性。

Unix 的 `build-cli-full` 启用 `qrcode-wechat ocr`（CPU OCR），需要自行配置系统依赖。CLI 的 `asInvoker` 清单（`crates/cli/cli.manifest`）由构建脚本在 MSVC 链接时嵌入，开发版与发布版相同。

`pack` 生成便携桌面包 `dist-pack/LovelyCarve-v<版本>-win-x64.7z`；`repack` 复用已有的桌面产物。`pack-cli` 默认不带 QR/OCR 资源，完整包使用 `pack-cli-full`；`repack-cli` 按 CLI 的实际能力选择资源。

`stage-cli-skill` 复用现有 Windows release CLI 与插件，按同一打包清单生成 `skills/lovelycarve-cli/bin/`；它不执行编译。运行文件与 DLL 保持相邻，插件与可选模型沿用包内路径，许可证随运行文件复制。该目录的生成文件由其 `.gitignore` 排除，独立安装时需复制完整 skill 目录；具体使用见 [CLI 代理技能](CLI.md#编码代理与小输出)。

DSH 模式源码位于 `integrations/dsh-lovelycarve/`，与主仓库的 `skills/lovelycarve-cli/` 共用技能。`just stage-dsh` 先暂存现有 release 运行库，再生成 `dist-pack/dsh-lovelycarve/`，旧包保留在 `dist-pack/dsh-previous-*/`。它不执行编译；CLI 修改后先运行 `just build-cli`，插件运行库需已由 `just build-plugins --release` 生成。安装见包内 `INSTALL.md`；使用本地链接的 DSH 在重启并创建新对话后加载更新的模式与技能。

`just test-dsh` 验证结构化工具的退出码、错误 / 警告、输出上限、取消与沙箱策略传递，并用合成检材覆盖命名会话、建案、索引、分页、特殊字符路径和导出不覆盖。集成测试默认使用已构建的 debug CLI（`target/debug/lovelycarve-cli`）；可设置 `LOVELYCARVE_TEST_CLI` 指向其他二进制。此配方使用 Node.js 内置测试运行器，不需要安装 DSH 或额外包。

Windows GitHub Actions 使用 `windows-2025` 执行 `just test`、`just test-desktop` 与 `just test-plugins`；PR 只运行测试，不执行发行构建或上传构建产物。Rust 测试仍需编译测试程序。`v*` 标签在测试成功后自动构建便携桌面包（不做 UPX 压缩）、CLI ZIP 与插件包并发布到 GitHub Release。云端默认不启用 QR/OCR，macOS/Linux、原生 GUI 与驱动功能仍需单独验证，发布步骤见 [发布指南](RELEASING.md)。

## 性能验证

`release` profile 以体积优先（`opt-level = "s"`），Windows 配方默认通过环境变量将宿主与插件的 release 链接优化设为 Thin LTO；Cargo 清单保留其他平台的 fat LTO 默认值。哈希、压缩、SQLCipher 与正则等热点依赖在根目录 `Cargo.toml` 中单独设为 `opt-level = 3`；性能比较使用 `release-fast` profile（`just run-example-fast <名称>`）。开发版的 Argon2 / BLAKE2 同样启用优化，以满足高内存 LUKS2 的密钥派生预算；flate2 / miniz_oxide / zlib-rs / simd-adler32 / crc32fast 也启用优化，减少大备份 ZIP 在恢复案件时的 debug 解压开销。GPUI、GPUI Kit 组件库、taffy 布局、AccessKit 与 SVG 光栅化等界面依赖在开发版同样启用优化：GPUI 的每个动画帧都重绘整窗，未优化时满屏表格页面跟不上显示器刷新率；桌面应用自身的 crate 仍按开发配置编译，改界面后的增量构建不受影响，只有这些依赖重新编译时更慢。完整 QR/OCR 构建与轻量构建的耗时不能混作同一基线。

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
| 插件分析 | 多任务调度、进度写入、记录批量写入与发布、排序分页、视图读取与输入解析 |
| 关闭案件与卸载镜像 | 任务退出、句柄释放和临时文件清理 |

```text
just bench-filedb-store
just run-example-fast probe_partitions <image-path>
just run-example probe_carve <image-path>
```

优化前后核对结果、长度和校验值：分页需要稳定顺序，并行扫描需要稳定合并，缓存需要有界容量和明确的清理点。增加并发时同时测量目录浏览等前台交互，不能只提高后台吞吐。失败、取消、短读和 SQLite 忙等待同样属于性能验证范围。

源盘 MD5 的可选全量对比测试使用 `EWF_HASH_IMAGE=<镜像路径>` 和 `just test image_hash_external_benchmark --lib --release -- --ignored --nocapture`。测试在相同读取器上交替运行当前 `fast-md5` 命令流水线与 RustCrypto 对照流水线，核对完整逻辑长度、证据稳定性、严格读取及内嵌采集 MD5；未提供样本时不会执行。它不清除系统文件缓存，应把缓存状态和构建 profile 一并记录。

## 更新依赖

先审查清单约束，再用 `update-rust` 或 `update-plugins` 更新对应的锁文件。工作区 crate 共用的依赖版本写在根目录 `Cargo.toml` 的 `[workspace.dependencies]` 中，各 crate 专用的依赖留在其自身清单里。vendor 的来源与补丁见 [UPSTREAM.md](../vendor/UPSTREAM.md)，更新后用对应读取器的配方验证。GPUI Kit 的升级需要同时运行 `just test-desktop` 并在 `just dev` 中实际检查界面。

升级后按改动范围执行 Rust、插件与读取器验证。不要把本机路径、依赖目录、案件、缓存、导出结果和签名凭据提交到仓库；`.gitignore` 为回归样本保留了例外。
