# dokan-rust 本地修改

来源：crates.io 发布的 `dokan` / `dokan-sys` `0.3.1+dokan206`，对应 [dokan-dev/dokan-rust](https://github.com/dokan-dev/dokan-rust) 提交 `d88cbf15d681b37b4cb5cd5e79f0f90d877219b5`。许可证为 MIT，见同目录 `LICENSE`。绑定沿用 Dokany 2.0.6 头文件的 API（`DOKAN_VERSION` 206，最低兼容 200）。

## 运行时加载 Dokany

上游 `dokan-sys` 的构建脚本用随包的 Dokany C 源码编译 `dokan2.dll` 并在链接期导入，程序缺少该 DLL 时无法启动；脚本也只接受 x86 / x64 目标。本副本改为与 MemProcFS 相同的运行时加载：

- 删除构建脚本、版本生成程序和随包的 Dokany C 源码，`DOKAN_VERSION` 等常量改为手写，`links` 一并去掉；程序不再链接 `dokan2.dll`。
- 新增 `library` 模块：`load()` 依次从 System32（Dokany 安装的、与驱动同一版本的库）、可执行文件所在目录和调用方给出的备用文件加载 `dokan2.dll`，用 `LOAD_LIBRARY_SEARCH_*` 限定搜索范围，当前目录和 `PATH` 不参与；成功后解析全部导出函数，整个进程只加载一次且不卸载。失败不缓存，安装 Dokany 后再次调用即可找到。
- 原 `extern` 块中的每个函数改为同名同签名的转发函数：已加载时调用库中的函数；无法加载时返回失败值（`DOKAN_ERROR`、`FALSE`、`WAIT_FAILED`、`INVALID_HANDLE_VALUE`、空指针、`STATUS_DLL_NOT_FOUND`，或什么也不做），不会引发崩溃。没有显式调用 `load()` 时，转发函数按默认顺序自动加载，上游测试因此无需改动。
- `Library::driver_version()` 在没有驱动应答时返回 0，调用方据此区分「库缺失」与「驱动未安装」。

## 调用约定

两个 crate 的回调、函数指针和函数表全部从 `extern "stdcall"` 改为 `extern "system"`。在 x86 上两者相同；在 x64 和 ARM64 上 `system` 即 C 调用约定，而 `stdcall` 只靠编译器已标记为将要移除的兼容回退才被接受。

## 未带入

上游的 `memfs` 示例未带入。`usage_tests.rs` 与各模块内的测试保留，它们会挂载真实盘符，需要已安装 Dokany 2 驱动。
