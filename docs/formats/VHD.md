# VHD / VHDX

VHD 使用 [vhd-forensic](https://github.com/SecurityRonin/vhd-forensic) 的 `vhd-core` 0.1.4，VHDX 使用 [vhdx-forensic](https://github.com/SecurityRonin/vhdx-forensic) 的 `vhdx-core` 0.4.1。桌面端与 CLI 共用这些 Rust 读取器，运行和打包均不需要 `libvhdi.dll`。固定的上游提交、本地改动与回归样本来源见 [`src-tauri/vendor/vhd/LOVELYCARVE.md`](../../src-tauri/vendor/vhd/LOVELYCARVE.md) 与 [`src-tauri/vendor/vhdx/LOVELYCARVE.md`](../../src-tauri/vendor/vhdx/LOVELYCARVE.md)。

## 支持范围

- 固定盘、动态盘、随机读取与 RAW 导出；按签名识别改名后的镜像。
- VHD 使用 footer 的 CurrentSize（当前容量）；校验和错误报错，不再通过 RAW 回退绕过。本地补丁只读取 footer、动态头和 BAT（分配表上限 64 MiB），不为打开大镜像读入整个文件；截断的已分配数据块报错。
- VHDX 按需读取，脏日志在内存覆盖层中重放，不修改检材。

## 限制

VHD 上游不支持差分盘；VHDX 上游虽有显式父盘 API，现有命令尚无父盘绑定参数。因此差分 VHD / VHDX 均明确拒绝，不会把缺失父盘的数据伪装为零。

## 验证

```powershell
just test-vhd
just test-vhdx
just test-cli
```

`just test-cli` 导出各样本的完整逻辑字节，与样本目录 `LOVELYCARVE_ORACLE.json` 中记录的大小和 SHA-256 比较。这些参考值来自 libvhdi 对公开样本的独立读取，测试本身不依赖该 DLL。
