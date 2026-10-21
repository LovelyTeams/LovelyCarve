# UBI 与小米 HDR1 固件

LovelyCarve 通过本地改造的 [rust_ubidump](https://github.com/aspnmy/rust_ubidump)
UBI 只读库解析 Flash 逻辑卷，通过现有 `backhand` 读取其中的 SquashFS。
来源、许可证及补丁见 [vendor/ubidump/LOVELYCARVE.md](../../src-tauri/vendor/ubidump/LOVELYCARVE.md)。

## 打开与浏览

可以直接添加 UBI 镜像，也可以添加含 UBI 分段的小米 `HDR1` `.bin` 固件。
HDR1 支持旧版 8 槽和扩展到 32 槽的分段表，按最早的分段位置界定表尾；
检查分段魔数、长度、重复偏移、重叠和签名区边界。非文件系统段保留在原始镜像中。
固件寻址单位为字节，不强制将载荷对齐到磁盘扇区。

展开 UBI 分区时，每个命名卷均显示一个 `<卷名>.img` 文件，提供去除 UBI
头部、按逻辑块重组的卷字节，可预览、哈希和导出。卷以 SquashFS 开头时，另有
同名目录，可浏览其中的目录、普通文件和符号链接，复用现有预览与文件导出。
卷号属于节点身份，不同分区、不同卷的读取缓存相互隔离。

源镜像的大小、哈希、字节视图与源镜像导出始终对应原始固件，逻辑卷的重组不会
替换检材字节。UBI 头与其他固件分段仍可通过原始视图检查。

## 读取完整性与限制

- 支持 UBI v1、自动识别 4 KiB 至 16 MiB 范围内二次幂 PEB、大端 EC / VID / VTBL。
  至少需要前两个 PEB 具有有效 EC 头来确定几何，来源须由完整 PEB 组成。
- 同一卷的 LEB 按逻辑块号映射；重复块选择序号较高者，序号相同则拒绝。
  校验所有非擦除 EC / VID 头，静态数据和复制块的数据 CRC。
- 静态卷检查完整块集合与末块长度；动态卷按卷表保留容量读取，未映射 LEB 返回
  擦除值 `0xff`。已映射块的来源短读、越界或损坏明确失败。
- 两份有效卷表不同、更新未完成、混合镜像序列、不支持的内部卷（包括 fastmap）
  明确拒绝；不尝试猜测电源中断后的状态。一个卷表副本 CRC 失败时可使用另一个。
- 当前自动展开对象是 UBI 中的 SquashFS；UBIFS、内核及其他载荷提供原始逻辑卷文件，
  不提供 UBIFS 目录、日志重放、删除恢复或 NAND OOB / ECC 解码。
- 压缩文件没有伪造的连续物理范围；SquashFS 范围读取复用流式回退，成本为 O(offset)。
  UBI 原始逻辑卷提供直接范围读取。

## 验证

```text
just test-vendor ubidump
just clippy-ubi
just test ubi --lib
just test xiaomi_firmware --lib
just test-cli
```

独立库测试用逐字段合成镜像和独立 CRC 算法验证乱序 LEB、动态空洞、静态末块、
重复序号、卷表主备、损坏和短读。宿主合成测试把可读 SquashFS 分布到乱序 PEB，
核对普通读取、流式读取、范围读取、节点父链与源字节。

可选外部固件回归通过 `LOVELYCARVE_UBI_IMAGE=<镜像路径>` 后运行
`just test ubi_real_firmware --lib -- --ignored`，要求其中存在含 `/etc/openwrt_release`
的 SquashFS 卷；不把外部检材纳入仓库。
