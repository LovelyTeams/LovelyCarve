# smraw

独立、无第三方运行依赖的纯 Rust 单文件 / 分段 RAW I/O，参照
[libyal/libsmraw](https://github.com/libyal/libsmraw) 的公开接口与格式行为重写。
无 C 源码、FFI、DLL 或 C ABI 兼容层，证据读取始终只读。

```rust
use std::io::{Read, Seek, SeekFrom};
use smraw::RawImage;

# fn example() -> std::io::Result<()> {
let image = RawImage::open("disk.raw.001")?;
image.require_media_size(1024 * 1024)?; // 独立记录的预期长度
let mut buffer = [0; 512];
image.read_at(500, &mut buffer)?;       // 支持跨段，无共享游标
let mut cursor = image.cursor();
cursor.seek(SeekFrom::Start(500))?;
cursor.read_exact(&mut buffer)?;
# Ok(())
# }
```

- `discover`：现存路径或 basename；数字后缀 `.0/.1`、`.00/.01`、
  `.000/.001`、`.raw.001`，以及无点的 `00/000`、`aa/aaa`；识别编号进位。
  接受任意成员，但返回从首段开始的列表；应用侧要求选择第一段。
- `RawImage::open_segments`：调用者提供准确顺序，支持不等长段。
  记录路径、长度、时间及逻辑偏移；拒绝空列表、重复路径、非文件和多段中的空段。
- `read_at` / `RawCursor`：并发 positioned I/O、独立 `Read + Seek` 游标、
  严格短读/源变更错误、可取消、默认最多缓存 64 个文件句柄。
- `discover_contiguous` / `RawImage::open_forced` / `read_at_forced`：显式的 RAW
  连续前缀兼容模式，不向前补片；短读返回已读数与诊断，完全越界主动清零并以
  `ForcedRead.synthetic` 标识。调用宿主必须审计合成字节；`read_at` 与源稳定性检查仍严格。
- `RawWriter`：只用 `create_new` 创建新输出，支持顺序、seek 和随机回写；
  单段 `<basename>.raw`，多段 `<basename>.raw.001` 起。`finish` 核对完整长度并
  sync 后才保留输出，未完成输出在 Drop 中清理；清理失败会打印错误。
- `MediaMetadata` / `MetadataValues`：调用者声明的几何、介质属性、UTF-8/UTF-16
  信息与哈希值，保留未知值及插入顺序。它们不来自 RAW 字节，也不自动验证哈希。

RAW 没有分段总数、固有签名或校验值。能发现已有后续段之前的编号空缺，
不能仅凭文件名证明最后一段未丢失或打开前未截断。
有独立长度时必须调用 `require_media_size`；未知/自定义命名使用显式分段列表。
不读写 libsmraw 的 `.info` sidecar，不提供原位修改检材或 libbfio 池接口。
更详细的接口对照与边界见 [SMRAW](../../../docs/formats/SMRAW.md)。

验证：仓库根运行 `just test-smraw`、`just clippy-smraw`、`just fmt`。
源代码与测试采用 AGPL-3.0-only；来源记录见 [ORIGIN.md](ORIGIN.md)。
