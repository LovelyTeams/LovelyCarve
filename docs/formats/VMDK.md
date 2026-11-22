# VMDK

VMDK 使用 [vmdk-forensic](https://github.com/SecurityRonin/vmdk-forensic) 中的 `vmdk-core` 0.8.4 纯 Rust 读取器，桌面端、CLI 与挂载共用，不需要 `libvmdk.dll`。源码版本、补丁与测试素材见 [`src-tauri/vendor/vmdk/LOVELYCARVE.md`](../../src-tauri/vendor/vmdk/LOVELYCARVE.md)。

## 支持范围

- monolithicFlat、monolithicSparse、streamOptimized 压缩镜像，分卷 flat / sparse，以及读取器支持的 COWD / seSparse 格式。
- 打开描述符时需保留相邻分卷，缺失文件报错；直接打开 `*-flat.vmdk` 时优先寻找描述符，没有描述符时按原始磁盘读取。
- 单文件稀疏快照及 seSparse 描述符按 `parentFileNameHint` 读取父盘，检查 CID、容量和循环；父盘需位于当前镜像目录或其子目录。所有父盘、描述符和分卷都纳入源变化检测。
- 损坏块通过读取故障机制报告。
- LDPlayer 备份内部的内存 VMDK 保留原有纯 Rust 读取路径。

## 限制

- 多分卷差异快照（含 vmfsSparse / vmfsThin / custom 差异层）暂不支持，须先合并为完整镜像。
- 缺少父盘时报错，不会把缺失的父盘数据当成空洞补零。

## 验证

```powershell
just test-vmdk
just test-cli
```

`just test-cli` 导出压缩流、稀疏和 flat 样本的 RAW 并与已知字节比较，另以截断的样本检查读故障报告。
