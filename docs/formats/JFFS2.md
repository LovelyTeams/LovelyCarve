# JFFS2 文件系统

LovelyCarve 可直接浏览裸 `.jffs2` 与受支持固件分段中的新格式 JFFS2。
读取器先评估并复用 [jffs2-rs 0.2.1](https://github.com/xmoezzz/jffs2-rs/tree/52e1234ce66b0e099b35afabff3fc3e3078a2c0e)
的 Rust 节点字段与 RTIME 算法，再改为有界只读库；不包含上游 C LZO / Rubin、
宿主目录解包代码或 FFI。来源见 [vendor 说明](../../src-tauri/vendor/jffs2/LOVELYCARVE.md)。

## 支持范围

- 新格式 magic `0x1985`，小端和大端；所有数值字段统一使用镜像字节序。
  裸镜像使用字节几何，保留短镜像和非扇区对齐尾部。
- 校验 common header、inode、dirent、名称 CRC。NOR 清除 ACCURATE 位的废弃节点
  先还原该位验证原头，再跳过，不复活为最新版本。
- 以 `(父 inode, 名称)` 选择最高版本目录记录，`ino=0` 作为删除标记；硬链接保留
  各个路径。相同版本的 GC 副本可复用，字段或数据 CRC 冲突明确拒绝。
- inode 选择最高版本元数据。数据按版本覆盖重叠片段，按最新 `isize` 裁剪；缺失
  片段与 ZERO 节点按 JFFS2 语义读零。完整、流式与范围读使用同一片段索引。
  实际读取的数据节点校验压缩数据 CRC，短读、越界与解码失败不转换为空洞。
- 读取 NONE / COPY、ZERO、RTIME、zlib 与 LZO。zlib 使用带头与 Adler32 的流，
  检查精确输入 / 输出长度和结束；LZO 使用纯 Rust `lzokay`，RTIME 检查回引用和尾随字节。
- 支持格式允许的隐式根 inode；其他现存目录项引用缺失 inode 则报错。检查可达
  目录的父链、类型、循环、目录硬链接与深度。未链接的历史子树不投影为现存文件。
- 保留 inode 号、模式、UID/GID 与秒级 Unix atime、mtime、ctime。目录中的较新
  dirent mctime 更新目录的 mtime / ctime；ctime 不充当出生时间。符号链接展示目标，
  不跟随到宿主路径。设备、FIFO、socket 保留库类型，界面以零长度节点展示，正文读取拒绝。
- summary 与 xattr / xref 检查长度和 CRC；summary 不替代全卷扫描，扩展属性不投影
  为文件。原始字节、哈希与 RAW 导出仍对应源文件，固件解密仅作为文件系统读取视图。

## 限制

- 不支持 magic `0x1984` 的旧格式、Rubin / DynRubin、厂商 LZMA 编号或未知编码；
  不自动调用外部解包工具。旧版 ZERO 节点 csize/dsize 对调和零 isize 符号链接暂不兼容。
- 不解析 NAND OOB / ECC，不推断擦除块几何或校验节点是否跨擦除块；输入须为连续
  主数据区。没有损坏节点跳过 / 重同步、断电恢复、孤儿或删除文件恢复。
  损坏 CRC 明确失败，不能把此读取器当作 Linux 的容错挂载实现。
- 名称和符号链接目标须为 UTF-8；含反斜杠的名称不投影为路径。当前宿主文件树不展示
  UID/GID、原始 dirent 版本、xattr 或完整设备属性，不生成文件系统未分配空间节点。
- 节点扫描上限 1,000,000，片段上限 2,000,000，元数据估算预算 512 MiB；
  单个压缩 / 解压数据节点最多 1 MiB，目录深度 256，单次范围读取 256 MiB。
  数据节点延迟校验，因此成功列目录不等于全部文件内容已经校验。

## 验证与来源

```text
just test-vendor jffs2
just clippy-jffs2
just test jffs2_host --lib
just test-cli
```

独立 Python 夹具覆盖大小端、四种数据编码、ZERO、版本乱序、片段重叠、最终裁剪、
空洞、GC 副本、NOR 废弃节点、删除标记、硬链接、中文目录和符号链接；损坏测试
在重新计算 CRC 后检查结构、版本冲突、类型、源短读和解码边界。宿主另验证
Buffalo ENC / TRX 内嵌视图、原始字节保留、范围读取与流式读取。

Unblob 的公开 fruits 新格式样本与其独立记录的 `apple.txt`、`banana.txt`、
`cherry.txt` 输出比较字节和哈希；输入与输出均核对 Git LFS 实际对象，来源、长度、
哈希与读取状态见[公开样本清单](firmware-samples.json)。这些样本不代表所有厂商固件。

```powershell
$env:JFFS2_EXTERNAL_SAMPLES = '<公开镜像1>;<公开镜像2>'
just test-vendor jffs2 external_fruits -- --ignored
$env:LOVELYCARVE_JFFS2_IMAGES = '<公开镜像1>;<公开镜像2>'
just test jffs2_host_external --lib -- --ignored
```

格式交叉依据为 Linux v6.12 的 [jffs2.h](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/jffs2.h)、
[scan.c](https://github.com/torvalds/linux/blob/v6.12/fs/jffs2/scan.c)、
[readinode.c](https://github.com/torvalds/linux/blob/v6.12/fs/jffs2/readinode.c) 与
[compr_zlib.c](https://github.com/torvalds/linux/blob/v6.12/fs/jffs2/compr_zlib.c)。
它们用于核对磁盘布局与语义，没有编译或运行 Linux 文件系统代码。
