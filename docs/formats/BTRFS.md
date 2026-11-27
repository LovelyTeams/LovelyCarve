# Btrfs

LovelyCarve 用同一个纯 Rust 读取器浏览 Btrfs 目录、子卷与快照，并读取、预览和流式导出文件。可以直接打开单设备分区或线性 LVM 逻辑卷。跨独立镜像的卷先加载所有成员，再运行 RAID 自动检测（CLI 为 `raid detect`）。成员可以是整盘镜像或镜像内的 Btrfs 分区。多设备卷只从第一个成员分区浏览：组装检材的 `part:0`，或同一检材内完整成员集在分区表中的第一个成员。其余成员分区列为成员设备，不单独浏览、建索引、搜索或挂载；卷的 `$未分配簇` 覆盖每个成员设备。

## 多设备读取

- 先按文件系统 UUID 分组，再检查设备 ID、设备 UUID、事务代际和树根。多设备卷验证每个主超级块的 CRC32C 或 SHA-256，拒绝缺失、重复、过期或截短的成员。合成检材保留各成员原始范围。
- 系统 chunk 表用于打开 chunk 树。每个 chunk 独立映射 SINGLE、DUP、RAID0、RAID1、RAID10、RAID1C3、RAID1C4、健康 RAID5 和健康 RAID6 的数据区。一个卷可以对系统、元数据和文件数据使用不同布局。
- 目录与文件共用同一映射。文件的全量、范围和流式读取按条带边界拆分，返回逻辑空洞的零填充。物理 runs 记录实际读取的条带，合成检材的偏移可用成员范围回溯到原始镜像。

## 限制

目前使用主超级块和已提交树。暂不重放 tree log，也不使用备用超级块、镜像副本重试或 RAID5/6 校验重建。树节点及文件数据的内容校验未接入读取路径。多设备源暂不组装跨多个独立 LVM 逻辑卷的成员，或使用 xxHash64 / BLAKE2b 超级块校验的成员。需要 RAID stripe tree 或 remap tree 的布局会报告不支持。

## 验证

`just test btrfs` 包含两设备 RAID0 镜像测试，覆盖非首盘上的元数据、跨条带的文件、稀疏范围、导出流和物理来源，以及组装检材只有一个卷、第二个设备列为成员设备、整个卷只建一个索引。

原生交叉验证使用 `mkfs.btrfs` 5.16.2，生成元数据为 RAID1、数据分别为 RAID0、RAID10、RAID5、RAID6、RAID1C3、RAID1C4 的镜像。RAID6 卷使用 SHA-256 超级块校验，其余使用 CRC32C。在 Linux 写入稀疏文件与 Zstandard 压缩文件后，从 CLI 的 `raid detect` 结果浏览卷，核对范围读取和导出文件的 SHA-256。同样验证了成员分别位于不同 MBR 检材内，以及成员同处一个 MBR 检材时直接打开的路径。

格式与条带规则参见 [Btrfs 磁盘格式](https://btrfs.readthedocs.io/en/stable/dev/On-disk-format.html) 和 [Linux Btrfs 逻辑地址映射](https://github.com/torvalds/linux/blob/master/fs/btrfs/volumes.c)。
