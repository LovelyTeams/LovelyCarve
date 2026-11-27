# ZFS

LovelyCarve 通过 vendored `zfs-rs` 以纯 Rust 浏览单设备及完整多设备池的文件系统数据集。整盘裸卷和分区内 ZFS 使用同一套标签识别规则，文件支持预览、哈希、范围读取和流式导出。

## 卷识别与事务根

- 检查 vdev 开头与尾部共四份标签。用 vdev 内偏移验证 XDR 配置与 uberblock 的嵌入式 SHA-256，支持 1、2、4、8 KiB 槽及中间留空的环。
- 按池 GUID 和设备 GUID 汇总同一叶设备的有效槽，从校验通过的 uberblock 中按事务组和时间选取最新根。跨不同池或不同叶设备的标签各自保留身份。
- 大端标签可识别。目前对象树和文件读取支持小端。

## 多设备池

- 支持多个顶层磁盘 vdev、镜像、RAIDZ1/2/3，以及使用磁盘或镜像的 `special` 分配类。按 XDR 标签记录的池 GUID、顶层 vdev ID 和叶 GUID 绑定成员。选取与完整拓扑 GUID 总和匹配的最新事务根。
- 文件的 DVA 按顶层 vdev 寻址。镜像按块校验后可尝试其他副本。RAIDZ 按各列的实际大小与扇区位置拼回数据，并依据块指针校验。
- 多张镜像先登记进同一案件，再运行 `raid detect`，从结果 `assembled[].path` 下的第一个成员分区 `part:0` 浏览池。同一张镜像包含完整成员分区时，从分区表中第一个同池叶设备直接打开。池 GUID 相同的其余分区列为成员设备，不单独浏览、建索引、搜索或挂载；池的 `$未分配簇` 覆盖每个成员设备。

## 目录与文件

- 卷根可浏览根数据集文件和子数据集。对象编号仍以 512 字节槽为单位，可读取同一块内混用的普通和大 dnode，范围为 512 字节至 16 KiB。
- 从 bonus 和 spill 读取系统属性，用于文件大小、模式、时间及范围裁剪。按逻辑块号查找直接和间接数据指针，稀疏块返回零，已分配数据短读会报错。
- 内容读取支持未压缩和 LZ4 数据，以及 Fletcher2、Fletcher4、SHA-256 校验。全量、范围与导出使用同一个可寻址流。

缺盘降级、RAIDZ 校验列重建、扩容后的 RAIDZ、dRAID 和已移除设备的间接映射仍待接入。原生加密、ZVOL、ZIL 重放和历史快照浏览也暂无支持。

## 验证

`just test-vendor zfs` 覆盖跨槽 dnode、下一对象、spill、池拓扑匹配、RAIDZ 列转序和分配边界。`just test zfs` 覆盖离散槽、标签偏移校验、备用标签的识别，以及同池叶设备按池 GUID 归入第一个成员分区。

原生验证使用 FreeBSD 14.4 的 OpenZFS，生成四组对应的 `ashift=9/11/12/13` 与 `dnodesize=1k/2k/4k/16k` 检材。从 CLI 浏览根数据集及其目录，核对稀疏文件和 LZ4 文件的读取范围与导出 SHA-256。另以仅保留尾部标签的副本和嵌入 MBR 分区的副本验证两种入口。

多设备样本使用相同版本的 OpenZFS，分别生成 stripe、mirror、RAIDZ1/2/3、普通磁盘加 special mirror。由 CLI 读取池中的稀疏文件与 LZ4 文件，并与 FreeBSD 的 SHA-256 交叉核对。同一 MBR 镜像中的双分区读取也通过。缺少成员或重复成员会被拒绝。损坏一份镜像副本时可读取校验通过的另一份，两份都损坏时读取和导出会报错。

格式参见 [OpenZFS dnode 布局](https://github.com/openzfs/zfs/blob/master/include/sys/dnode.h)、[vdev 标签和 uberblock 槽](https://github.com/openzfs/zfs/blob/master/include/sys/vdev_impl.h) 与 [嵌入式标签校验](https://github.com/openzfs/zfs/blob/master/module/zfs/zio_checksum.c)。

多设备寻址参见 [OpenZFS vdev 布局](https://openzfs.github.io/openzfs-docs/man/7/zpoolconcepts.7.html)、[标签拓扑选取](https://github.com/openzfs/zfs/blob/master/lib/libzutil/zutil_import.c) 与 [RAIDZ 列布局](https://github.com/openzfs/zfs/blob/master/module/zfs/vdev_raidz.c)。
