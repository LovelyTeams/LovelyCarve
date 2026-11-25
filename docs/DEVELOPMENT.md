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
| Rust | stable，最低版本以 `src-tauri/Cargo.toml` 的 `rust-version` 为准，当前为 1.100 |
| Bun | 版本以 `package.json` 的 `packageManager` 为准，当前为 1.4.5；前端工具在 Bun 上运行，不需要 Node.js |
| Windows 桌面 | PowerShell 7、Visual Studio C++ x64 工具与「适用于 Windows 的 C++ Clang 工具」（含 `lld-link`）、Windows SDK、OpenSSL 开发文件、WebView2 |
| macOS | Xcode Command Line Tools；桌面检查另需相应的 Tauri 系统依赖 |
| Linux | 编译工具链、WebKitGTK、OpenSSL 等 Tauri 系统依赖 |
| CLI smoke 测试 | Python 3 |
| Windows 便携打包 | PATH 中的 `upx` 与 `7z` |

默认构建关闭二维码识别与 OCR，不需要 OpenCV 或 CUDA。桌面开发和打包配方只支持 Windows；Rust 检查、前端任务、插件构建和 CLI 构建可以在其他平台运行，但 macOS / Linux 产物需要单独验证。

Windows 桌面程序在开发版和发布版中均嵌入 `asInvoker` 权限声明，启动时不自动请求管理员权限。读取物理磁盘或采集实时内存时，需要用户手动以管理员身份启动程序。

桌面程序启动时，若 `lovelycarve://` 没有注册到当前程序，就为当前用户把它注册过去，开发版也一样：运行 `just dev` 之后点击的链接交给开发版程序（开发服务器未运行时窗口打不开页面），再运行一次安装版或便携版即可改回。运行中的桌面进程在用户数据目录的 `desktop-links/` 中登记接收链接的回环端口，进程退出后由下一个进程清理。

前端的离线预览资源由准备脚本从依赖复制到 `public/file-viewer/`，不提交到 Git；`just dev-fe` 与 `just dev` 会先准备这些资源。只运行前端无法读取证据。Vite 固定监听 `http://127.0.0.1:14200`，更换端口时同时修改 `vite.config.ts` 与 `src-tauri/tauri.conf.json`，并保留 `strictPort`。

仓库根的 `bunfig.toml` 设置 `[run] bun = true`：经 `bun run` 启动的 Vite、Vitest、vue-tsc 与 Tauri CLI 即使在装有 Node.js 的机器上（例如 GitHub Actions 运行器）也在 Bun 运行时中执行，本地与 CI 使用同一运行时。`bunx` 不读取这项设置，这些工具应通过 `package.json` 脚本或 `bun run <工具>` 调用。`scripts/` 下的前端检查、插件打包、预览资源同步和图标脚本用 TypeScript 编写、由 Bun 直接运行，优先使用 Bun 的内置 API（`Bun.Glob`、`Bun.file` / `Bun.write`、`Bun.spawnSync`、`Bun.TOML`、`Bun.Image`）；它们与 `vite.config.ts`、`vitest.config.ts` 同属 `tsconfig.node.json`，由 `just typecheck` 中的 `bun check` 检查，类型来自 `@types/bun`。

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

宿主 Rust 检查、桌面、CLI 与插件配方会初始化 VS x64 环境，默认使用所选 Visual Studio 中的 `lld-link.exe`，Cargo 同时编译 1 个任务，链接器使用 1 个线程，release 使用 Thin LTO。只有 `*-full` 入口额外配置 OpenCV 与 libclang。独立 vendored 读取器的配方沿用 Cargo 自身配置。

以下 Cargo 环境变量可覆盖默认值；脚本不会覆盖调用者已设置的值，也不会向非 LLD 链接器添加 `/threads:1`：

| 环境变量 | Windows 配方默认值 |
| --- | --- |
| `CARGO_BUILD_JOBS` | `1` |
| `CARGO_PROFILE_RELEASE_LTO` | `thin` |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER` | 所选 VS 的 `VC/Tools/Llvm/x64/bin/lld-link.exe`，自动解析完整路径 |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS` | 使用 LLD 时为 `-C link-arg=/threads:1` |

例如可设置 `CARGO_BUILD_JOBS=2` 调整编译并发，或设置 `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=link.exe` 切回 MSVC 链接器。`RUSTFLAGS` / `CARGO_ENCODED_RUSTFLAGS` 的优先级沿用 Cargo 规则，设置它们时需自行包含需要保留的链接线程参数。非 Windows 配方不加载该脚本。

SQLCipher 配方设置 `OPENSSL_NO_VENDOR=1` 和 `OPENSSL_STATIC=1`，使用本机安装的 OpenSSL 开发文件。Windows 配方默认通过 `OPENSSL_LIBS=libssl_static:libcrypto_static` 选择静态库；Shining Light 的 `libssl.lib` 和 `libcrypto.lib` 即使位于 `MT` 目录下也是 DLL 导入库。使用其他 OpenSSL 发行版时，可通过 `OPENSSL_LIBS` 指定其实际静态库名称，并检查二进制的 DLL 依赖。如改为从源码编译 OpenSSL，需要准备 Windows Perl 与 NASM。

实验性的 Windows ARM64 工作流由 GitHub Actions 手动触发，编译桌面端、CLI 与随附插件，并上传供测试的二进制文件。OpenSSL 使用 ARM64 的 `MD` 静态库，以匹配 MSVC 动态运行库。这个构建关闭 QR/OCR、Dokan 挂载与挂载共享，跳过安装包生成；Dokan Rust 依赖当前只支持 x86 和 x64。

## 日常任务

```text
just                          # 按工作流顺序显示任务
just --list --group frontend  # 查看一个分组
just --show build-cli         # 查看本平台的配方内容
just --dry-run pack           # 展开依赖与命令，但不执行
```

| 工作 | 入口 |
| --- | --- |
| 前端 | `dev-fe`、`typecheck`、`test-fe`、`build-fe`、`ipc-check`、`frontend-structure-check`、`i18n-check` |
| 后端 | `fmt`、`check`、`clippy`、`test [名称及 Cargo 参数]`、`check-examples`；原生组件用 `build-components [--release] [--features ocr / ocr-cuda]`、`test-components`、`clippy-components` |
| Windows 桌面 | `dev`、`build`、`build-release`、`pack`、`repack`；含 QR/OCR 的版本使用对应的 `*-full` |
| CLI | `run-cli`、`build-cli`、`build-cli-full`、`test-cli`、`smoke-cli <exe>`、`update-cli-skill`（重新生成代理技能的命令参考）；Windows 另有 `pack-cli`、`pack-cli-full`、`repack-cli`、`stage-cli-skill` |
| Python 读取库 | `develop-python`、`build-python`、`test-python`、`fmt-python`、`clippy-python`；使用 uv 管理解释器，默认 Python 3.15，详见 [Python 文档](../python/README.md) |
| 插件 | `build-plugins [--release]`、`test-plugins`、`clippy-plugins`；Windows 另有 `pack-plugins`（生成 `.lcpkg`）与 `market-index [目录] [标题]`（为插件包目录写出插件市场的 `index.json`） |
| 读取器 | `test-vendor <目录>`、`test-ewf`、`test-vmdk`、`test-vhd`、`test-vhdx`、`test-minix` / `clippy-minix`、`generate-minix-fixtures`、`test-udf` / `clippy-udf`、`generate-udf-fixtures`、`test-qcow2` / `clippy-qcow2`、`generate-qcow-fixtures`、`test-vdi`、`test-phdi` / `clippy-phdi`、`test-smraw` / `clippy-smraw`、`test-gzipf` / `clippy-gzipf`、`clippy-ubi`、`test-bitlocker` / `clippy-bitlocker`、`test-luks` / `clippy-luks`、`test-veracrypt` / `clippy-veracrypt`、`test-filevault` / `clippy-filevault`、`test-vsc` / `clippy-vsc`、`test-refs` / `clippy-refs` / `test-refs-oracle`、`test-ad1`、`test-sqlite-core`、`test-ios-backup`、`test-apfs` / `clippy-apfs`、`clippy-hfsplus` |
| 探针与性能 | `run-example`、`run-example-fast`、`bench-filedb-store`、`probe-refs`、`probe-vsc`、`compare-vsc` |
| 依赖更新 | `update-fe`、`update-rust`、`update-plugins`、`update-tauri`、`update-file-viewer <版本>` |
| 资源维护 | `icons`、`qr-env`、`build-opencv-contrib`、`qr-test <图片>` |

`verify-fe` 用 `bun run --parallel` 同时运行 IPC、结构、中英文词条与工具脚本类型检查、前端测试和生产构建（含 vue-tsc），某项失败时仍跑完其余各项再一并报告；`verify-rust` 汇总格式化、编译、Clippy 和后端测试，并包括原生组件的 Clippy 与测试；`verify-plugins` 汇总格式化、插件 Clippy 和插件测试。按 [贡献指南](../CONTRIBUTING.md#验证) 选择验证范围。

Cargo 默认特性为空。`desktop` 特性引入 Tauri、对话框与文件插件和 tauri-build，桌面二进制 `LovelyCarve` 只在该特性下构建：`tauri dev` / `tauri build` 通过 `tauri.conf.json` 的 `build.features` 开启，`just build` / `build-full` 显式传入。`lovelycarve-cli` 不启用它，是不链接 Tauri、WebView 或 GTK 的独立程序。后端库 `lovelycarve_lib` 只编译 `rlib`：桌面程序、CLI、示例、测试和 Python 绑定都静态链接它，不另生成 Tauri 移动端模板的 `staticlib` / `cdylib`（它们在每次构建时多写一个数 GB 的静态库并链接一份完整 DLL）。桌面包和 CLI 包各自只含一个可执行文件，拆出共享 DLL 不会减小任一安装包，Rust `dylib` 还无法参与发布配置的 LTO。`just check` 和 `just clippy` 依次检查独立 CLI 与桌面两种配置；`just test` 在不含桌面运行时的配置下运行后端测试。Magika 文件类型识别和 OCR 在推理组件 `lovelycarve_inference.dll` 中运行（`src-tauri/components/`，见[架构说明](ARCHITECTURE.md#原生组件)）：它经 C ABI 单独构建，只把 ONNX Runtime 等可选的大依赖移出可执行文件并在首次使用时加载，安装包总体积基本不变。组件构建在 `src-tauri/target/<profile>/`，紧挨主程序和 CLI。需要运行程序的配方会先构建组件；直接用 `cargo run` 时需先运行一次 `just build-components`，否则 CLI 的 `identify` 和 OCR 会报告缺少组件。新注册的后端命令自动可经 CLI 的 `rpc` 调用；常用于命令行的功能再在 `src-tauri/src/cli/cmd/` 增加专门命令，见[架构说明](ARCHITECTURE.md#命令行与桌面只读挂载)。

`fmt` 覆盖主 crate、插件与维护中的读取器。公共读取接口改动后运行 `check-examples`。参数含空格时，需要保留一层传给子 shell 的引号：

```powershell
just run-example probe_partitions '"D:\Evidence Files\disk.E01"'
```

后端探针的用途与参数见 [examples/README.md](../src-tauri/examples/README.md)，各读取器的专项测试、可选外部样本与环境变量见 [格式与读取器](formats/README.md)。

### 读取器验证

独立读取库用各自的测试与 Clippy 配方验证；接入宿主的部分另用 `just test <过滤词> --lib` 运行宿主回归，命令行接入用 `just test-cli`。

- QCOW：`just test-qcow2` / `just clippy-qcow2`，宿主回归用 `just test qcow --lib --jobs 1`。`just generate-qcow-fixtures` 可重建独立 Python zlib/OpenSSL 样本（需 cryptography），能力与边界见 [QCOW](formats/QCOW.md)。
- Parallels PHDI：`just test-phdi` / `just clippy-phdi`，宿主回归用 `just test phdi --lib --jobs 1`，真实命令打开 / 导出用 `just test-cli`。可选外部 oracle、56 API 对照及验证边界见 [PHDI](formats/PHDI.md)。
- 分段 RAW：`just test-smraw` / `just clippy-smraw`，宿主回归用 `just test smraw --lib --jobs 1` 与 `just test-cli`，范围见 [SMRAW](formats/SMRAW.md)。
- GZIP：`just test-gzipf` / `just clippy-gzipf`（也可使用 `just test-vendor gzipf`），宿主回归用 `just test gzip --lib`，CLI 接入用 `just test-cli`，格式范围见 [GZIP](formats/GZIP.md)。
- UBI：`just test-vendor ubidump` / `just clippy-ubi`，宿主回归用 `just test ubi --lib` 与 `just test xiaomi_firmware --lib`，支持范围见 [UBI 固件](formats/UBI.md)。
- QNX4 / QNX6：`just test-qnx` / `just clippy-qnx`，宿主回归用 `just test qnx --lib`，CLI 用 `just test-cli`。`just generate-qnx-fixtures` 重建字段夹具；`just test-qnx-reference <qnxprobe.py路径>` 运行独立 Python 正例 oracle。版本、外部样本与限制见 [QNX](formats/QNX.md)。
- F2FS / EROFS：`just test-vendor f2fs-reader` / `just clippy-f2fs`、`just test-erofs` / `just clippy-erofs`；宿主回归用 `just test flashfs_ --lib`，CLI 用 `just test-cli`。`just generate-f2fs-fixtures` 重建独立字段夹具；真实 mkfs 样本与限制见 [F2FS](formats/F2FS.md) 和 [EROFS](formats/EROFS.md)。
- VeraCrypt / TrueCrypt：`just test-veracrypt` / `just clippy-veracrypt`（VeraCrypt 自检向量与独立兼容样本），宿主回归用 `just test encrypted_image --lib` 与 `just test veracrypt --lib`，CLI 用 `just test-cli`。`python scripts/make-veracrypt-compatibility-fixtures.py` 重建独立样本（需 cryptography 与 twofish）；真实卷 oracle 的环境变量与限制见 [VeraCrypt](formats/VERACRYPT.md)。
- KeePass KDBX：`just test-kdbx` / `just clippy-kdbx`（pykeepass 测试集中的 KeePass / KeePassXC 数据库与独立合成语料），宿主回归用 `just test kdbx`，CLI 用 `just test-cli`。`python scripts/make-kdbx-fixtures.py` 重建合成语料（需 cryptography、argon2-cffi、pycryptodomex 与 twofish）；能力与限制见 [KDBX](formats/KDBX.md)。
- HFS+ 与 FileVault 2：`just test-vendor hfsplus` / `just clippy-hfsplus` 验证 HFS；`just test-filevault` / `just clippy-filevault` 验证 CoreStorage 核心和法证审计器。设置 `FVDE_ORACLE_IMAGE` 为公开测试样本的 raw CoreStorage 区域后运行 `just test-filevault`，执行真实扇区 SHA、HFS 读者与法证审计 oracle；提取范围与哈希见 [FileVault](formats/FILEVAULT.md)。未设置变量不等于通过真实样本验证。

Windows rustc 在大型测试 crate 发生增量编译崩溃时，可通过原配方传入 `--config profile.test.package.lovelycarve.debug=0 --config profile.test.package.lovelycarve.incremental=false`，仅降低宿主测试包调试信息并禁用其增量缓存；不改变发布配置。

## 插件与资源

`plugins/` 是 [LovelyCarve-Plugins](https://github.com/LovelyTeams/LovelyCarve-Plugins) 的 Git 子模块，包含 `sdk`、`sdk-derive`、`host` 与 `linuxinfo`。主仓库固定插件的提交；递归克隆或 `just bootstrap` 会初始化它，已有克隆可运行 `git submodule update --init --recursive`。GitHub 生成的源码压缩包不包含子模块，开发请使用 Git 克隆。

`build-plugins` 按提交的锁文件构建宿主和各插件库，把动态库复制到 `plugin.toml` 旁，并用 `lc-plugin-host --describe` 写出 `schema.json`；`--plugin <目录>` 只构建一个插件库，沿用已构建的加载器。`dev` 使用开发版插件；`build-release` 与 CLI 打包先构建发布版插件。修改插件时先在插件仓库提交并推送，再在主仓库提交子模块指针，不要只提交指针而漏推插件提交。

安装包和便携包共用 `src-tauri/tauri.plugins.conf.json`：主程序旁放置 `lc-plugin-host.exe`，`plugins/linuxinfo/` 包含 `plugin.toml`、DLL、`schema.json` 与 `locales/`，插件不嵌入主程序。`just pack-plugins` 为每个工作区插件生成可在「设置 → 插件」或 CLI `plugin install` 安装的 `.lcpkg`。接口与工件来源见 [插件协议](PLUGIN_PROTOCOL.md) 和 [Linux 基本信息插件](LINUXINFO.md)。

ReFS 使用纯 Rust 读取器；旧的 `librefs-13.dll` 只在显式的 `refs-dll-oracle` 特性下用于对照，不随正式包分发。Dokan 与可选的 OpenCV DLL，以及模型和许可证属于构建 / 运行资源，按打包配置分发；OpenSSL 静态链接进主程序、CLI 和 Python 扩展，不随包分发 DLL；推理组件 `lovelycarve_inference.dll` 由 `src-tauri/tauri.components.conf.json` 声明，从 `src-tauri/target/release` 随包放在主程序旁。`src-tauri/binaries` 中的 `vcruntime140.dll`、`vcruntime140_1.dll`、`msvcp140.dll` 和 `msvcp140_1.dll` 是 Visual Studio 可再发行目录中的 VC++ 运行库，随主程序放在同一目录：桌面主程序静态链接 `vcruntime`，独立 CLI、`lc-plugin-host` 和 `linuxinfo` 动态依赖 `vcruntime140`；含 ONNX Runtime 的推理组件另外依赖 C++ 标准库 `msvcp140` / `msvcp140_1`（它们又依赖 `vcruntime140` / `vcruntime140_1`），以及系统自带的 DirectML、D3D12 与 DXGI。便携包打包时不对这些 VC++ 运行库做 UPX 压缩。Tauri 的 MSI 打包会收入 `src-tauri/target/release` 根目录下的所有 DLL，`build-release` 因此先删除其中未在打包配置里声明的 DLL（例如完整构建留下的 OpenCV 和 ONNX Runtime 执行提供者）。发布前用 `dumpbin /dependents` 核对主程序、CLI、推理组件、插件宿主、插件和随附 DLL 的依赖，除 Windows 自带组件外都应能在包内找到。测试夹具及其来源说明保存在 vendor 与 tests 目录中，不要用真实检材替换。

图标保留透明和带背景两张原图，位于 `src-tauri/icons/source*.png`；修改后执行 `just icons` 同步各平台图标和前端 512px 资源。

## 可选 QR/OCR 与发布构建

Windows 的 `dev-full`、`build-full`、`build-release-full` 和 `build-cli-full` 启用 WeChat 二维码识别和带 CPU 回退的 CUDA OCR：主程序开启 `qrcode-wechat`，推理组件以 `ocr-cuda` 特性构建。带 OCR 的组件构建会把 ONNX Runtime 的 CUDA / TensorRT 执行提供者和 DirectML DLL 链接到 `src-tauri/target/<profile>/`，与组件相邻（组件工作区通过 `[patch.crates-io]` 使用 `src-tauri/vendor/ort-sys`，修正其构建脚本在 Cargo 新构建目录布局下的路径推算）。完整构建的 MSI 随 `target/release` 根目录的 DLL 一并收入它们；便携包和 CLI 包不携带，其中的 OCR 在 CPU 上运行。CUDA 执行提供者还需要系统中可用的 CUDA 运行库，缺失时回退 CPU。需要准备版本匹配的 OpenCV 及 contrib 源码，用 `just build-opencv-contrib` 构建含 `wechat_qrcode` 的 OpenCV；核对安装目录中的 `vc*` 与 world 库名称后设置对应变量。官方的核心 OpenCV 发行包不包含所需模块。

Unix 的 `build-cli-full` 为 CLI 启用 `qrcode-wechat`，推理组件以 `ocr` 特性构建（CPU OCR），需要自行配置系统依赖。CLI 的 `asInvoker` 清单（`src-tauri/cli.manifest`）由构建脚本在 MSVC 链接时嵌入，开发版与发布版相同。

`pack` / `pack-cli` 默认不带 QR/OCR 资源；完整包使用 `pack-full` / `pack-cli-full`，额外资源来自 `src-tauri/tauri.full.conf.json`。`repack` / `repack-full` 复用已有的桌面产物，必须与编译特性一致；`repack-cli` 按 CLI 的实际能力选择资源。便携包输出到 `dist-pack/`，安装包输出到 `src-tauri/target/release/bundle/`。

`stage-cli-skill` 复用现有 Windows release CLI 与插件，按同一打包清单生成 `skills/lovelycarve-cli/bin/`；它不执行编译。运行文件与 DLL 保持相邻，插件与可选模型沿用包内路径，许可证随运行文件复制。该目录的生成文件由其 `.gitignore` 排除，独立安装时需复制完整 skill 目录；具体使用见 [CLI 代理技能](CLI.md#编码代理)。

Windows GitHub Actions 使用 `windows-2025` 执行 `just test-fe`、`just test`、`just test-components` 与 `just test-plugins`；PR 只运行测试，不执行发行构建或上传构建产物。Rust 测试仍需编译测试程序。`v*` 标签在测试成功后自动构建 MSI、CLI ZIP 与插件包并发布到 GitHub Release。云端默认不启用 QR/OCR，macOS/Linux、原生 GUI 与驱动功能仍需单独验证，发布步骤见 [发布指南](RELEASING.md)。

## 性能验证

桌面 `release` profile 以体积优先（`opt-level = "s"`），Windows 配方默认通过环境变量将宿主与插件的 release 链接优化设为 Thin LTO；Cargo 清单保留其他平台的 fat LTO 默认值。哈希、压缩、SQLCipher 与正则等热点依赖在 `src-tauri/Cargo.toml` 中单独设为 `opt-level = 3`；性能比较使用 `release-fast` profile（`just run-example-fast <名称>`）。开发版的 Argon2 / BLAKE2 同样启用优化，以满足高内存 LUKS2 的密钥派生预算；flate2 / miniz_oxide / zlib-rs / simd-adler32 / crc32fast 也启用优化，减少大备份 ZIP 在恢复案件时的 debug 解压开销。完整 QR/OCR 构建与轻量构建的耗时不能混作同一基线。

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

先审查清单约束，再用 `update-fe`、`update-rust` 或 `update-plugins` 更新对应的锁文件。前端依赖可先用 `bun outdated` 查看可更新的包，用 `bun pm diff <包>` 查看已装版本与最新版本之间的改动（会标出新增的安装脚本与 `child_process`、`fs`、`net`、`vm` 引用）；改动打包内容的依赖后，用 `bun pm licenses --prod` 复核许可证。`update-tauri` 同步前后端的 Tauri 包族（含 `tauri-plugin-deep-link`），各包独立发布，补丁版本号不必一致；前端包按精确版本固定，配方以 `@^2` 限定在 Tauri 2 内更新。vendor 的来源与补丁见 [UPSTREAM.md](../src-tauri/vendor/UPSTREAM.md)，更新后用对应读取器的配方验证。

Vue 类型检查通过 `typescript` 的 npm 别名接入 `typescript-native-bridge@6.0.3-bridge.18.tsgo.7.0.2`，以兼容 `vue-tsc` 需要的经典编译器 API；别名与 override 固定为同一完整版本，不要直接替换为 `typescript@7`。`just typecheck` 会显示 `TNB ACTIVE`，随后用 Bun 内置的 `bun check` 检查 `tsconfig.node.json` 中的脚本与 Vite 配置；`bun check` 不读取 `.vue` 文件，应用代码仍由 vue-tsc 检查。普通组件测试默认关闭动效，动效专用测试单独验证开关与收尾。

前端生产构建使用 Vite 8 自带的 Oxc 压缩器，不需要额外安装 esbuild，也不要恢复 `build.minify: 'esbuild'`。非 Windows 平台保持 Safari 13 构建目标，但地图渲染器依赖 BigInt，map / shp 预览需要 Safari 14 或更新的 WebKit。

升级后按改动范围执行前端、Rust、插件与读取器验证。不要把本机路径、依赖目录、案件、缓存、导出结果和签名凭据提交到仓库；`.gitignore` 为回归样本保留了例外。
