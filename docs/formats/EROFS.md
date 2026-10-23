# EROFS

裸 EROFS 镜像、分区和已识别固件分段内的 EROFS 可直接浏览、预览、哈希、导出、建立
快照与索引。使用 vendored `lamfold-erofs` 0.1.3，与 CramFS / ROMFS 共用有界字节源和
路径适配器；压缩在 Rust 内解码，不加载系统 liblzma / libzstd。

## 支持范围

- 512 / 1024 / 2048 / 4096 字节块，compact / extended inode，普通目录、符号链接、
  FLAT_PLAIN 与 FLAT_INLINE；检查声明卷长、分配范围、内联尾部和源边界。
- compressed FULL / COMPACT 索引、big pclusters、LZ4 零前填充 / 右对齐数据、
  DEFLATE、Zstandard、MicroLZMA 和非默认 LZMA 属性；压缩配置跟随超级块 extension slots。
- 4 字节 block-map chunk 文件、chunk 空洞读零。8 字节设备 chunk index 未支持。
- 可选超级块 CRC32C；compact inode 的 mtime 来自超级块，extended inode 使用自身
  mtime，保留 UTC 纳秒。没有 atime / ctime / birth 的格式字段时不合成其他时间角色。
- 范围、流式与完整读取共用逻辑读者；压缩文件首次访问会解压整个文件，缓存一份文件。
  每个压缩文件限 64 MiB、压缩 / 解压块限 16 MiB、LZMA 配置字典限 16 MiB、
  目录限 16 MiB、inode 缓存限 100,000 项。超限明确报错，不发布部分结果。
- 路径 ID 为 `erofs:part:<分区ID>:<UTF-8路径十六进制>`，保留卷和硬链接目录身份，
  不跟随宿主符号链接，不生成压缩文件的虚假物理 runs；源字节、源哈希和 RAW 导出不变。

fragments / all-fragments、ztailpacking、interlaced pclusters、压缩目录、48-bit 布局、
metabox、外部设备及未知 incompat 特性明确拒绝。新式 hole pcluster 不当作普通数据解码。
未接入 fs-verity 根哈希 / 签名验证；超级块 CRC 和解压检查不代表文件来源认证。
xattr 用于确定 inode / 索引布局，但未提供专门的属性展示接口。非 UTF-8、反斜线名称拒绝。
Android sparse 包装应先展开为裸镜像；原生 GUI / Dokan 挂载需单独验证。

## 来源与验证

[lamco-admin/lamfold](https://github.com/lamco-admin/lamfold/tree/fabaad6af0c28b80535307b339b84110f3a3c12e/crates/lamfold-erofs)，
crate 0.1.3，MIT OR Apache-2.0，许可证和 NOTICE 原样保留。本地增加超级块校验和、
声明卷边界、精确 pcluster 源范围、元数据 / 文件限额、单文件缓存、UTF-8 与时间戳保留；
详见 [本地修改](../../src-tauri/vendor/lamfold-erofs/LOVELYCARVE.md)。

保留上游由 erofs-utils 1.8.6 / 1.9.4 `mkfs.erofs` 生成的测试镜像与独立内容公式，覆盖
四种压缩、紧凑索引、大小 pclusters、较小块、扩展槽、chunk 空洞及明确拒绝的格式。
测试源 / 夹具许可、哈希和生成脚本见库的 `tests/fixtures/README.md` 与 `manifest.json`。
宿主另验证目录、时间、完整 / 范围 / 流式读取、导出与原始源保留，CLI 验证打开 / 浏览 / 读 / 哈希。
这些是标准工具生成的独立镜像，不代表已核对所有厂商手机固件或其私有扩展。

```text
just test-erofs
just clippy-erofs
just test flashfs_ --lib
just test-cli
```
