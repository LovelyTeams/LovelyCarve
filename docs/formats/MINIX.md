# Minix V1 / V2 / V3

LovelyCarve 通过独立纯 Rust `minix-reader` 浏览 Minix 文件系统。它与路由器固件使用的
[MINIFS](MINIFS.md) 是不同格式，节点前缀分别为 `minix:` 和 `minifs:`。

元数据布局改造自 [danifunker/rusty-backup](https://github.com/danifunker/rusty-backup/blob/6e044a314f6523045999058f85a882c1eeb64b50/src/fs/minix.rs)，
固定提交 `6e044a314f6523045999058f85a882c1eeb64b50`，保留作者与 AGPL v3 许可证。
没有引入该项目的写入、修复、格式化或 GUI。主程序复用既有有界源和路径适配层；
不依赖系统挂载、C 库或外部提取程序。

## 支持范围

| 布局 | magic | 文件名字段 | inode / 目录项 |
| --- | --- | --- | --- |
| V1 | `0x137f` / `0x138f` | 14 / 30 字节 | 32 字节 inode、16 位 inode / zone 引用 |
| V2 | `0x2468` / `0x2478` | 14 / 30 字节 | 64 字节 inode、16 位目录 inode 与 32 位 zone 引用 |
| V3 | `0x4d5a` | 60 字节 | 64 字节 inode、32 位 inode / zone 引用 |

- 支持裸文件系统、MBR / GPT 内的分区；小端和大端元数据，分区身份与源扇区几何不变。
- V1 / V2 的 block 为 1024 字节；V3 支持 1024 / 2048 / 4096 字节 block。
  `log_zone_size` 为 0–8；zone 的字节数是 `block_size << log_zone_size`。
- 7 个直接 zone、一级和二级间接寻址；V2 / V3 另支持三级间接寻址。
  一个间接 zone 的指针只占**第一个 block**，不能把整个多 block zone 当成指针表。
- 普通文件、目录、硬链接、符号链接及格式定义的稀疏区。inode / 间接指针为零时读零；
  已分配数据越界、指向空闲 zone、位图容量不足或短读必须失败。
- V1 保留唯一的修改时间，不复制为访问或元数据变更时间；V2 / V3 分别保留三种 UTC 秒时间。
  没有创建时间；Unix epoch 零是合法值。
- V3 `first_data_zone` 为零时按 inode 表末端向 zone 边界取整，支持原生格式的派生规则。
- 文件浏览、预览、范围读、流式读、SHA 哈希、导出、目录快照、索引及既有只读挂载路径共用
  同一读取器。范围读只访问目标 zone 的最多三级映射，不扫描文件前缀。

路径 ID 为 `minix:part:<分区ID>:<UTF-8路径hex>`；原生 inode 号保留在节点上。
同一 inode 的不同硬链接路径各自可定位，符号链接只展示 / 读取目标文字，不跟随宿主路径。
空闲目录项跳过后继续枚举后续现存项，不作为目录结束标记。

## 校验与限制

- 校验超级块、inode 表和数据区不重叠、inode / zone bitmap 容量、源长度、现存 inode 的分配位
  与链接数、数据和间接 zone 的分配位 / 边界、间接祖先循环和数据占用间接元数据。
  这不是全卷 allocation audit，不能证明不存在跨文件重复占用或孤儿 inode。
- 位图使用 Linux / util-linux 的小端 bit indexing，**与元数据端序独立**。未接入历史原生系统
  的大端 16 / 32 / 64 位 word-indexed bitmap 方言；不能仅凭大端 magic 推定这些方言可读。
- 不支持非 UTF-8 文件名的 codepage 参数；无法无损映射到当前路径契约时明确报错。
  V3 未知 disk version 和 mandatory feature flags 明确拒绝。
  不提供已删除 / 孤儿恢复、写入、修复、格式化、设备节点正文、EA 或所有者独立展示。
- inode 缓存最多 4096 项，metadata block 缓存最多 256 块；目录最多 32 MiB / 262144 个
  现存项，符号链接最多 64 KiB。共享适配层另限制路径深度和单次完整分配；超大稀疏文件仍可范围读。

## 验证

`tests/fixtures/util-linux/` 保留官方 util-linux
[`fbfd2ef405d048e8fc01b309b051d2b403b09853`](https://github.com/util-linux/util-linux/tree/fbfd2ef405d048e8fc01b309b051d2b403b09853/tests/expected/minix)
的五种格式、两种元数据端序的 hexdump，以及原测试脚本和许可证。这些转储来自
`mkfs.minix` 创建、`fsck.minix` 返回成功的空文件系统；生成器无损重建其镜像，测试核对几何、
根目录和时间。它们验证真实工具的布局，不等于已经在本机运行 Linux 挂载或 fsck。

独立 Python 编码的 CC0 合成夹具覆盖有内容的目录、两种 zone 大小关系、直接 / 全部间接层级、
两层空洞、硬链接、符号链接和删除槽位。三级稀疏文件用范围读取验证，避免先分配数百 MiB。
负例覆盖位图、zone / inode 越界、未分配引用、错误目录、映射循环、逻辑尺寸和截断。
源码参照使用 MINIX 的 `read_map` 及 Linux v6.12 格式定义，未移植内核 C 实现。

宿主测试核对原始源字节、各版本范围 / 流式 / 导出一致性、双 MBR 分区身份和持久化快照。
CLI smoke 用 V1 / V2 / V3 核对浏览、跨直接 / 间接边界读取、三级范围读、SHA-256 与导出。
尚未完成 Windows GUI / Dokan 实际挂载和 macOS / Linux 桌面运行验证。

来源、SHA-256 与许可见 [ORIGIN.md](../../src-tauri/vendor/minix-reader/ORIGIN.md) 和
[夹具 manifest](../../src-tauri/vendor/minix-reader/tests/fixtures/manifest.json)。

```text
just test-minix
just clippy-minix
just test minix
just test-cli
just generate-minix-fixtures
```
