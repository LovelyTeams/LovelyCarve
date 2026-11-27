# QNX EFS

LovelyCarve 的 `qnx-reader` 以纯 Rust 只读打开 QNX Embedded File System 的 F3S 3.0 闪存卷。支持裸卷和分区内的卷，浏览时保留镜像的原始分区位置。

- 校验 boot 记录、擦除单元几何和 32 字节 extent 头。从逻辑单元标识建立映射，适配物理单元因磨损均衡而重排的卷。被替换的 extent 依据格式标记跟到当前记录。
- 支持目录、常规文件和符号链接。按完整 extent 链生成文件的逻辑大小与源字节范围，核对边界、循环和冗余状态。读取 POSIX 模式、修改时间和状态变更时间。
- 单元缺失、镜像截断、状态副本不一致或 extent 链损坏时明确报错。

原生验证覆盖小端、128 KiB 擦除单元、一个备用单元与实际镜像中的已替换 extent。另将原生样本中的物理单元互换，验证逻辑目录和文件仍一致。版本 3.0 以外的卷、ECC 修复、已删除文件恢复及无分区表长镜像中的内部卷发现尚未覆盖。

`just test-qnx` 和 `just clippy-qnx` 验证读取器。QNX 独立生成的 TAR 核对 31 项的路径、类型、模式、mtime、文件和链接字节。CLI 覆盖裸卷、MBR 内的重排布局、范围读取、哈希与导出。[NFI 原生样本与生成脚本](https://github.com/NetherlandsForensicInstitute/qnxmount/tree/11c8a7f9ee9b945d584263743f6ea8524e8776d2/tests/qnx_efs/test_data)
