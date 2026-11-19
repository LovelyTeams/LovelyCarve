# VHD / VHDX

VHD 使用 [vhd-forensic](https://github.com/SecurityRonin/vhd-forensic) 的 `vhd-core` 0.1.4，VHDX 使用 [vhdx-forensic](https://github.com/SecurityRonin/vhdx-forensic) 的 `vhdx-core` 0.4.1。桌面端与 CLI 共用这些 Rust 读取器，运行和打包均不需要 `libvhdi.dll`。固定的上游提交、本地改动与回归样本来源见 [`src-tauri/vendor/vhd/LOVELYCARVE.md`](../../src-tauri/vendor/vhd/LOVELYCARVE.md) 与 [`src-tauri/vendor/vhdx/LOVELYCARVE.md`](../../src-tauri/vendor/vhdx/LOVELYCARVE.md)。

## 支持范围

- 固定盘、动态盘、随机读取与 RAW 导出；按签名识别改名后的镜像。
- VHD 使用 footer 的 CurrentSize（当前容量）；校验和错误报错，不再通过 RAW 回退绕过。本地补丁只读取 footer、动态头和 BAT（分配表上限 64 MiB），不为打开大镜像读入整个文件；截断的已分配数据块报错。
- VHDX 按需读取，脏日志在内存覆盖层中重放，不修改检材。
- VHDX 差分盘按 `relative_path` 定位父盘，最多 32 层（含子盘）。打开时校验父盘的 `DataWriteGuid`、容量和逻辑扇区大小。每层按扇区分配状态读取本层数据、父盘数据或格式定义的零。源变化检测、检材指纹和磁盘占用统计涵盖整条链。

## 限制

差分 VHD 明确拒绝。VHDX 父盘须保留在当前层所在目录或其子目录，父链中的路径须解析到顶层检材所在目录之内。缺失父盘、标识或几何不符、循环及超限均会报错。Windows 绝对路径和卷路径定位器不参与自动解析。

## 验证

```powershell
just test-vhd
just test-vhdx
just test-cli
```

`just test-cli` 导出各样本的完整逻辑字节，与样本目录 `LOVELYCARVE_ORACLE.json` 中记录的大小和 SHA-256 比较。这些参考值来自 libvhdi 对公开样本的独立读取，测试本身不依赖该 DLL。
