# gzipf

LovelyCarve 的独立纯 Rust GZIP 只读 crate，参照 RFC 1952 和
[libyal/libgzipf](https://github.com/libyal/libgzipf) 的公开读取能力重写。
没有 C FFI、原生 zlib 或 libgzipf DLL。GZIP 头、成员边界、CRC 和长度
检查由本 crate 实现；DEFLATE 使用 `flate2/rust_backend`（miniz_oxide）。

- 拼接多成员、空成员，以及 stored / fixed / dynamic Huffman DEFLATE。
- FTEXT、FEXTRA、FNAME、FCOMMENT、FHCRC；原始字段与源几何保留。
- 名称与注释按 Latin-1 转 UTF-8 / UTF-16；缺失和显式空字符串可区分。
- 每成员 header CRC、data CRC32、ISIZE modulo 2^32 校验；逻辑长度为 u64。
- `GzipDecoder<R: Read>` 流式读取，`GzipFile<R: Read + Seek>` 完整验证后 seek。
- 输入、输出、成员数量、头与累计元数据限额，以及调用者控制的取消标记。

```rust
use gzipf::{GzipDecoder, GzipFile, Options};
use std::io::{Cursor, Read, Seek, SeekFrom};

# fn example(gzip_bytes: &[u8]) -> std::io::Result<()> {
let mut options = Options::default();
options.max_uncompressed_bytes = 1024 * 1024 * 1024;
let mut file = GzipFile::from_reader_with_options(Cursor::new(gzip_bytes), options)?;
for member in file.members() {
    println!("{:?}: {} bytes", member.name_utf8(), member.uncompressed_size);
}
file.seek(SeekFrom::Start(128))?;
let mut range = [0; 64];
let count = file.read(&mut range)?;

let mut decoder = GzipDecoder::new(gzip_bytes);
std::io::copy(&mut decoder, &mut std::io::sink())?;
assert!(decoder.is_complete());
# Ok(())
# }
```

流式消费必须读到 EOF 或调用 `finish()` 才能声明全文件校验完成。只读
前缀不能验证后续 trailer；任何读取失败都会使该读者失效。错误使用
`std::io::Error`，包含结构/源偏移上下文，不返回看似完整的损坏数据。

`GzipFile` 打开时扫描并验证所有字节，保存成员索引，不保存解压正文。
顺序读取保持 inflater；seek 按成员二分定位，重新从该成员头解压并丢弃
前缀，开销 O(成员内偏移)，没有 DEFLATE checkpoint 索引。`read_at()`
与 libgzipf 一样移动逻辑游标。读取期间源必须保持不变。

默认上限：输入/输出各 1 TiB、单头 1 MiB、累计头 64 MiB、65,536 成员。
可修改 `Options` 的相应字段；输入缓存和单次输出处理各最多 64 KiB，
DEFLATE 自带滑动窗口；元数据内存随已验证成员增加。压缩字节上限按
实际消费计数，底层缓冲可能提前读取最多一个输入缓存。取消通过
`Options::with_cancellation(Arc<AtomicBool>)` 传入，不派生轮询线程。

严格拒绝不支持的压缩方法、保留位、截断、错误 CRC/ISIZE、尾随杂字节
（包括零填充）和超限流。扩展字段作为不透明字节保留，不解释 BGZF
索引或其他应用语义。不实现写入、C ABI、Python binding 或上游命令行。
宿主已接入 tar.gz / tgz、Docker / OCI GZIP 层和小米备份的 GZIP 读取路径，
通过完整流校验后物化到原有 TAR / 层模型，不在浏览时调用 GzipFile seek。

```text
just test-gzipf
just clippy-gzipf -- -D warnings
just test-vendor gzipf
just fmt
```

来源、许可证和 API 对照见 [ORIGIN.md](ORIGIN.md)、
[GZIP 能力说明](../../../docs/formats/GZIP.md)；夹具说明见
[tests/data/README.md](tests/data/README.md)。
