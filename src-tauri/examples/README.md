# 后端探针与基准

源码按职责分类；Cargo.toml 显式登记全部 46 个 example，目标名称保持稳定。
使用仓库根 Justfile：`just run-example <name> [args]`，编译验证使用 `just check-examples`。

仅读取源字节、分区或 RAID 的探针使用 `evidence::image::DiskImage`；调用文件树、预览、索引、插件浏览或挂载的探针使用 `filesystem::evidence_session::EvidenceSession`。RAID 的成员仍为字节读者，选择或组装目标后再包装浏览会话。

## evidence

`probe_partitions`, `probe_raid`, `probe_raid_nometa`, `probe_physical_disk`, `probe_evidence_names`, `probe_evidence_path`, `probe_lvm_meta`

`just run-example probe_partitions <image> --mbr` 额外输出 MBR/EBR 原始四槽、磁盘签名、启动标志、CHS、记录与槽位偏移，以及受损表恢复告警。格式范围和 libvsmbr 完整对照见 [MBR/EBR](../../docs/formats/MBR.md)。

`just run-example probe_partitions <image> --gpt` 输出 GPT 主备头字段与 CRC、选用副本、原始槽位/偏移、GUID、属性、名称和范围有效性，以及恢复告警。完整 libvsgpt 对照见 [GPT](../../docs/formats/GPT.md)。

## filesystem

`fs_dispatch_smoke`, `probe_access_bitmap`, `probe_apfs`, `probe_btrfs`, `probe_cdfs`, `probe_docker`, `probe_docker_fs`, `probe_exfat`, `probe_ext4`, `probe_fat_deleted`, `probe_hfsplus`, `probe_lvm_active`, `probe_refs`, `probe_squashfs`, `probe_ufs`, `probe_xfs`, `probe_xfs_magic`, `probe_zfs`

## catalog

`benchmark_filedb_store`, `benchmark_flat_page`, `probe_btrfs_snapshot`, `probe_filedb`, `probe_refs_snapshot`, `probe_snapshot`

## analysis

`probe_ntfs_timeline`, `probe_report_sources`

## carving

`probe_carve`, `probe_carve_truth`

## search

`bench_global_search`, `keyword_scan`

## preview

`probe_pe`, `probe_vim_swap`, `qr_test`, `verify_csv_recovery`

## extensions

`probe_plugin`, `probe_plugin_sources`

`just run-example probe_plugin <image> <plugin-id> <analyzer> <output-json> [--system <system id> | --file <root> <path>]` 在临时案件中走完整的任务流程：启动 `lc-plugin-host`、打开系统或检材视图、解析声明的输入、写入并发布 `records.db`，输出已发布的记录与运行审计，并打印识别出的系统。`probe_plugin_sources <image> <output-json>...` 检查这些输出中每条来源都能定位到镜像中的文件。`probe_evidence_path`、`probe_report_sources` 与 `probe_access_bitmap` 以插件看到的检材视图（每个卷是以卷键命名的目录）读取路径。

## platform

`probe_dokan_docker`, `probe_dokan_mount`, `probe_dokan_slice`
