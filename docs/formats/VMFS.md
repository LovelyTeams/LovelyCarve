# VMware VMFS 5 / 6

LovelyCarve 用独立的纯 Rust `vmfs-reader` 只读解析 VMFS 5、VMFS 6 和对应的 VMFS-L 布局。读取器识别 VMware LVM 成员，也支持从逻辑卷开头提取、直接包含 VMFS 文件系统描述符的镜像。

## 打开和浏览

单设备镜像直接打开，在识别为 VMFS 的分区下浏览。跨镜像数据存储需先加载所有成员，然后在桌面端执行阵列检测，或在 CLI 的案件后台中执行 `raid detect`，从结果 `assembled[].path` 下的第一个成员分区 `part:0` 进入。同一镜像内完整的 extent 成员从分区表中的第一个成员浏览。其余 extent 成员列为成员设备，不单独浏览、建索引、搜索或挂载。组装要求卷 UUID、快照标识、代次和版本一致，且物理 extent 覆盖完整。对缺盘、重复、旧代次和映射冲突的集合，检测结果会给出诊断。

目录枚举覆盖 VMFS 5 的平面记录和 VMFS 6 的分配图，包括哈希冲突目录项。支持常规文件、驻留数据、直接和间接块、符号链接原始内容，以及文件块未分配产生的稀疏空洞。预览、哈希和导出共用逻辑文件字节，范围读取按块寻址。目录时间保留秒精度的修改、访问和元数据变更时间。

## 限制

- 读取现存元数据。日志重放、删除项恢复和历史视图浏览未实现。
- RDM 原始设备映射可以列出，其引用的外部设备内容无法通过该映射读取。虚拟机文件作为普通文件读取，VMDK 快照链属于独立的镜像容器功能。
- 支持未分配的稀疏块和已有明确整块置零标志的块。VMFS 6 的部分置零位图返回明确错误，避免把未确定的区间作为取证内容。
- 底层映射保留原始成员及字节范围。应用的逐文件 `dataRuns` 目前留空。

## 验证

测试使用 Fox-IT 发布的真实 VMFS 5/6 语料，固定上游提交并核对 Git LFS 指针的文件大小和 SHA-256。将解压后的 `.bin` 文件和 `span/` 子目录放在 `LOVELYCARVE_VMFS_ORACLE` 指定的目录。测试包含 VMFS-L、普通与资源优化布局、无 VMware LVM 外层的卷、乱序双成员、目录哈希冲突、驻留数据，以及 42 TiB 逻辑文件中的实际写入块。测试数据和来源见 [读取器来源](../../src-tauri/vendor/vmfs-reader/ORIGIN.md)。

```powershell
just test-vendor vmfs-reader -- --ignored
just clippy-vmfs
just run-cli system
python scripts/test-cli.py src-tauri/target/debug/lovelycarve-cli.exe vmfs
```
