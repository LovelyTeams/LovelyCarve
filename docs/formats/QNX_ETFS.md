# QNX ETFS

LovelyCarve 的 `qnx-reader` 以纯 Rust 只读打开 QNX Embedded Transaction File System。支持裸镜像、分区中的镜像，以及 `etfsctl` 导出的尾随事务头记录。裸事务导出保留完整源字节长度。

- 识别每个数据簇尾随 16 字节事务头的布局。从文件表和根目录记录核对簇大小与字节序。
- 重建当前文件表。完整事务组按文件 ID、簇号和序号解析，相同序号采用物理偏移较大的页。支持目录、普通文件、符号链接、长文件名扩展项与秒级 POSIX 时间。
- 范围读取直接定位源数据簇，保持逻辑 EOF 与原始符号链接目标。当前版本的必需簇缺失，或事务头记录数据读错时，读取明确报错。

已用原生样本验证 1024 字节数据簇和小端事务头。探测在源头尾各最多 64 KiB 核对文件表锚点。存储厂商自定义 OOB 布局、ECC 修复、已删除文件恢复及历史时点浏览尚未覆盖。原生 GUI 和 Dokan 验证仍待补充。

`just test-qnx`、`just clippy-qnx` 和 CLI 用例覆盖两个原生闪存镜像、独立事务导出、MBR、重写文件范围、链接、哈希与导出。两份镜像分别与 QNX 生成的 TAR 核对了 32 和 22 个条目。事务导出样本是另一轮写入，单独验证目录、文件内容和错误状态。[NFI 样本与生成脚本](https://github.com/NetherlandsForensicInstitute/qnxmount/tree/11c8a7f9ee9b945d584263743f6ea8524e8776d2/tests/qnx_etfs/test_data)
