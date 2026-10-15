# phdi

LovelyCarve 的只读纯 Rust Parallels Hard Disk Image 读取器，独立 Cargo
工作区，也作为宿主 path dependency。没有 libphdi / libbfio / DLL 运行依赖。

```rust,no_run
use phdi::PhdiImage;
let image = PhdiImage::open("disk.hdd/DiskDescriptor.xml")?;
let mut bytes = vec![0; 4096];
let count = image.read_at(0, &mut bytes)?;
# Ok::<(), std::io::Error>(())
```

支持 UTF-8 XML 描述符、Plain、v2 `WithoutFreeSpace` HDS、分段、差分父链，
元数据枚举和 `Read` / `Seek` / 并发 `read_at`。`OpenOptions` 支持显式快照 GUID、
extent 目录以及描述符 / BAT / 句柄 / 父链限额；可取消接口直接轮询调用者闭包。
分段中未变化的子 extent 可以继承父 extent；根 extent 缺失则报错。

`Compressed` 是 Parallels 对稀疏 HDS 的类型名称，不代表压缩编码。
XML 中的磁盘、extent、block 单位都是 512 字节，与逻辑扇区大小独立。

独立 HDS 无法提供父链身份；在差分场景必须打开描述符。
不支持 v1、扩展 `WithouFreSpacExt`、Parallels 加密、写入、非 UTF-8 XML、
描述符自动备份恢复或 C ABI。完整范围、56 API 对照与验证见
[PHDI](../../../docs/formats/PHDI.md)，来源见 [ORIGIN.md](ORIGIN.md)。

在仓库根运行 `just test-phdi` 和 `just clippy-phdi`。
外部 oracle 需先提供独立 RAW 输出，再设置 `PHDI_ORACLE_DESCRIPTOR` /
`PHDI_ORACLE_RAW`，运行 `just test-phdi -- --include-ignored`；测试未运行不代表已验证。
