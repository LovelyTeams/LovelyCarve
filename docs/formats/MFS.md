# Macintosh File System（MFS）

LovelyCarve 使用纯 Rust 只读打开 Macintosh MFS。可直接打开包含 MFS 的裸卷、Apple Partition Map 分区和 Disk Copy 4.2 镜像。

## 支持范围

- 已验证 400K 和 800K 的裸卷与 Disk Copy 4.2 镜像，以及 APM 中的 `Apple_MFS` 分区。Disk Copy 4.2 会校验数据区和扇区 tags 的长度及校验和。原始镜像仍保留封装头和 tags，文件读取从封装内的卷地址开始。
- MFS 的磁盘目录是扁平的。可按 MacRoman 名称查询文件，查询遵循经典 Macintosh 大小写规则。同名的不同版本分开显示，非零版本带 `⟨vN⟩` 后缀。创建和修改时间保留为无时区偏移的 Macintosh 本地时间。
- 数据叉支持预览、范围读取、哈希和导出。资源叉以 `com.apple.ResourceFork` 命名流呈现，可单独预览和导出。数据叉为空的文件仍可读取其资源叉。
- 分配区内的目录必须占用标记为保留的块。400K 和 800K 卷的主 MDB 无效时，可尝试使用卷尾几何有效的备用 MDB 与分配表。文件读取验证叉的逻辑长度、物理分配长度及完整分配链。

## CLI 示例

```text
lovelycarve-cli --image floppy.image ls /floppy/part:0
lovelycarve-cli --image floppy.image streams /floppy/part:0/Document
lovelycarve-cli --image floppy.image export /floppy/part:0/Document --stream com.apple.ResourceFork -o Document.rsrc
```

## 限制与验证

- 当前不重建 Finder 的虚拟文件夹，不恢复已删除的目录项。GCR / MFM 轨道流和 flux 捕获需先转换为逻辑扇区镜像。传统 HFS 卷使用[传统 HFS 读取器](CLASSIC_HFS.md)。
- `just test-vendor mfs-reader` 覆盖裸卷、Disk Copy 4.2、双叉读取、损坏校验和、损坏分配链、备用 MDB 与同名版本。CLI 端到端测试另覆盖 APM 和原始 Apple 样本。要验证 Apple 样本，解压 [Apple MFSLives](https://developer.apple.com/library/archive/samplecode/MFSLives/MFSLives.zip)，将 `LOVELYCARVE_MFS_APPLE_ORACLE` 指向其中的 `Sample.img`，再运行 `just test-cli`。样本及预期哈希见[读取器来源记录](../../src-tauri/vendor/mfs-reader/ORIGIN.md)。
