# Android Backup `.ab`

参照 [nelenkov/android-backup-extractor](https://github.com/nelenkov/android-backup-extractor)
的 `AndroidBackup.java` 和 ADB 备份头约定，实现只读 Android Backup 文件树。
只对照格式，不引入 Java 运行时或外部解包程序，不复制上游代码。

## 入口与能力

- 「添加检材 → 镜像文件」可选 `.ab` 与小米 `.bak`；拖入文件和 CLI `images open`
  共用同一后端。标准 `ANDROID BACKUP\n` 与 `MIUI BACKUP\n` 魔数也可识别改名文件。
- 版本支持 1–5，头字段是魔数、版本、压缩标志和加密方式。支持 `compression=0`
  的原始 TAR，以及 `compression=1` 的 zlib；沿用小米兼容路径的 GZIP 载荷支持。
- ZIP / 本机目录中的 `.ab` 与 `.bak` 也自动展开；详见 [小米备份](MIUI_BAK.md)。
- 应用数据保留 `apps/<包名>/…` 层级，可使用现有文件预览、哈希、导出、搜索和卷快照。
  不执行备份恢复，不重打包，不解析业务数据库内容。

## 身份、完整性与限制

`ImageFormat::AndroidBackup` 对外序列化为 `android_backup`。字节源仍是原文件
mmap，镜像大小、整镜像哈希与十六进制读取包含原始备份头及压缩数据。
解码后的 TAR 保存在自动清理的临时文件，文件树复用 `TarImage`；不把该临时文件
中的偏移暴露为原 `.ab` 的物理数据段。备份本身无分区表，使用合成根卷。

头行长度上限 4096 字节；版本、压缩标志、换行终止与 UTF-8 均校验。
zlib 要求有效结束标记及 Adler-32，GZIP 校验所有成员的 CRC / ISIZE，
完整解码后才发布目录；TAR 的路径、链接和读取边界沿用既有读取器。
临时磁盘占用约为解码后的 TAR 总大小。

当前仅支持 `encryption=none`，没有 AES-256 / PBKDF2 密码解锁流程。
加密 `.ab` 明确报错；ZIP / 文件夹内的加密备份保留原始文件并记录日志，
不会误报为可浏览的明文备份。ABE 的 pack / pack-kk、密码解密功能不在当前实现范围。
损坏或不支持的独立 `.ab` 不静默回退为 RAW。

## 验证

```powershell
just test android_backup --lib
just test mi_bak --lib
just test zip_image --lib
just test folder_image --lib
```

合成样本覆盖全部五个版本、未压缩 / zlib、MIUI 前缀和改名文件、原始字节保持、
文件范围读取、物理偏移隔离、损坏压缩流、无效头及加密备份拒绝。
`src-tauri/tests/fixtures/android-backup/` 包含由 ABE 实际 `pack` / `pack-kk`
生成的版本 1 / 2 独立夹具，始终参与测试；来源、工具哈希和重建方法见该目录 README。
仅包含虚构路径与 `payload` 文本，不提交真实检材或下载的 ABE 程序。
