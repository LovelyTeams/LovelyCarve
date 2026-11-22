# EWF（E01 / Ex01）

E01 / Ex01 使用 [ewf-forensic](https://github.com/SecurityRonin/ewf-forensic) 仓库中的 `ewf` 0.4.10 纯 Rust 读取器，桌面端、CLI 与挂载入口共用，不需要 `libewf.dll`。源码版本与本地兼容补丁见 [`src-tauri/vendor/ewf/LOVELYCARVE.md`](../../src-tauri/vendor/ewf/LOVELYCARVE.md)，读取路径见 [架构说明的 EWF 读取器](../ARCHITECTURE.md#ewf-读取器)。

## 支持范围

- 多分卷、随机读取、zlib 压缩，以及 Ex01 的 bzip2 与模式填充块。
- 存储的哈希与采集元数据（`evidence show` 的 `ewf`）。
- 打开时检查分卷序号和块表覆盖，缺失分卷报错，不以零补齐。
- 坏块通过读取故障机制报告：容错读取以零填充并记录坏区域，哈希与导出据此报告 `readFaultsDetected`。

## 限制

- 加密的 EWF2 容器明确返回不支持。BitLocker 等卷加密是独立功能，见 [BitLocker](BITLOCKER.md)。

## 验证

```powershell
just test-ewf
just test-cli
```

`just test-ewf` 运行读取器及其样本语料测试；`just test-cli` 对 `ntfs_sample.E01` 与 `zeros_128s_compressed.Ex01` 计算整份检材的 MD5 / SHA-1 并导出 RAW，与已知值核对。
