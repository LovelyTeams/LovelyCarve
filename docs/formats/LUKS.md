# LUKS

LUKS 由 Apache-2.0 的 vendored `luks-core` 与 RustCrypto 读取；libluksde 只作行为参照，没有引入其 C 源码或运行时 DLL。
本文以 [libyal/libluksde](https://github.com/libyal/libluksde) 提交
[`5d3b7c997a7a491f981680552a0f136c614641e0`](https://github.com/libyal/libluksde/tree/5d3b7c997a7a491f981680552a0f136c614641e0)（2026-10-12 核对）为参照，
检查范围包括 README、全部 36 个公开 C API、头部 / 密码 / AF / 扇区解密实现、工具和 Python 绑定。

## 格式、算法与凭据

| 能力 | LovelyCarve | libluksde 基准 |
| --- | --- | --- |
| LUKS1 | 支持 | 支持 |
| LUKS2 | 单个动态数据段，校验主头；下列算法同样用于数据段和密钥槽 | 不支持 |
| AES-XTS | null/plain/plain64/benbi，32/64 字节 | 支持 AES-XTS |
| AES-CBC / ECB | 16/24/32 字节密钥 | 支持 |
| Blowfish-CBC / ECB | 4–56 字节密钥，8 字节块 | 支持 |
| Serpent-CBC / ECB | 16–32 字节密钥，16 字节块 | 支持 |
| ARC4-ECB | 5–256 字节密钥；每个扇区独立初始化 | 支持，但流状态依赖调用顺序，见下文 |
| null / plain / plain64 / benbi IV | 支持；plain 在 32 位扇区号处回绕，benbi 根据块大小计算 | 声明支持 |
| ESSIV SHA-256 | CBC 支持，使用完整哈希作为同算法的 IV 密钥 | 声明支持；实现仅 AES，且使用原数据密钥位数 |
| ESSIV SHA-1 | Blowfish / Serpent CBC 支持；AES 拒绝无效的 20 字节哈希密钥 | 声明支持；AES 路径补零/截断哈希，见下文 |
| PBKDF2 / AF SHA-224 | 支持 | 支持 |
| PBKDF2 / AF SHA-1/256/512 | 支持 | 支持 |
| PBKDF2 / AF SHA-384 | 支持 | README 未列出 |
| Argon2i / Argon2id | 支持（LUKS2） | 不支持 LUKS2 |
| UTF-8 密码 | 按原始 UTF-8 字节，不做规范化 | 支持 |
| UTF-16 密码 | reader 提供 `unlock_with_utf16_password`，拒绝非法代理项 | 支持 |
| 二进制密码 / keyfile 内容 | `unlock_with_passphrase(&[u8])` 接受任意字节；桌面没有 keyfile 选择器 | UTF-8/UTF-16 密码 API |
| 已恢复卷主密钥 | LUKS1/2 reader、桌面弹窗和 CLI/IPC 的 `masterKeyHex`；先核验卷密钥摘要 | `set_key`，旧别名 `set_keys` |
| LUKS2 密钥槽与卷密钥长度不同 | 分开保存和推导 `area.key_size` / `key_size` | 不适用 |
| UUID、版本、密码套件、扇区大小 | 原始头部字段及 `VolumeInfo.uuid` | 加密方法、卷标识接口 |

目前没有 anubis、CAST5/6、Twofish、ARC4-CBC、lmk 或 plumb 的解密路径；这些也是基准 README
列出的未支持项。LUKS2 完整重加密、多段、固定大小段、integrity、token 自动凭据和备份头恢复仍不支持；
遇到这些布局明确报错，不选择一个段后伪装成整个卷已成功解密。外置 detached header 也未提供。

### 与参考实现有意保留的差异

- **ESSIV**：遵循 Linux 的同算法 `E_hash(key)(sector)`。AES 用 SHA-256 时，IV 密钥始终是 32 字节，
  即使数据密钥只有 16 字节；SHA-1 的 20 字节不是合法 AES 密钥，明确拒绝。
  基准 `libluksde_encryption_context_set_key` 将 SHA-1 写入零填充的 32 字节缓冲区，
  并按数据密钥位数设置 AES IV 密钥；SHA-256 路径也按数据密钥位数设置。
  不复现这种与标准 ESSIV 不一致的补零/截断行为。
- **benbi**：512 字节扇区的块编号为 `sector * (512 / cipher_block_size) + 1`，放在 IV 末尾，使用大端序。
  Blowfish 是 8 字节块，步长 64；AES/Serpent 是 16 字节块，步长 32。
  基准固定使用步长 32。实现与 [Linux dm-crypt v6.12](https://github.com/torvalds/linux/blob/v6.12/drivers/md/dm-crypt.c) 对齐。
- **ARC4**：扇区独立初始化，随机读取和顺序读取一致。基准为卷复用有状态的 RC4 context，
  没有按扇区重设/定位流状态，导致跳读可能改变输出；不复现依赖缓存命中和调用顺序的结果。
- **密钥验证**：主密钥入口必须匹配头部摘要；LUKS2 必须有绑定到所选数据段的摘要。
  不接受无校验的直接解密，也不把空摘要当作成功。

## 36 个公开 API 的对应关系

以下是语义对照，不是 ABI 兼容层。路径型 C API 在 Rust 中以 `std::fs::File` / `Read + Seek` 替代，
错误和资源生命周期使用 `Result` / 所有权；应用使用已有镜像源、任务中心和会话卸载流程。

<details>
<summary>展开全部 36 个公开函数的对应关系</summary>

| libluksde API（省略 `libluksde_`） | LovelyCarve 对应能力 |
| --- | --- |
| `get_version` | `luks-core` 包版本及 `VolumeInfo.version`（格式版本是不同概念） |
| `get_access_flags_read` | 只读 reader 与证据会话，没有写接口 |
| `get_codepage` | Rust 路径用 `Path` / `OsStr`，无全局代码页 |
| `set_codepage` | 同上；密码固定为调用方提供的字节 / UTF-16 转 UTF-8 |
| `check_volume_signature` | `Luks1Header::parse` / `Luks2Header::parse`、应用 `LuksParser` |
| `check_volume_signature_wide` | 同上，`File::open(Path)` 直接支持 Unicode 路径 |
| `check_volume_signature_file_io_handle` | 同上，输入可为自定义 `Read + Seek` |
| `notify_set_verbose` | 应用 `tracing` 与 `RUST_LOG`；reader 返回错误，不打印成功信息 |
| `notify_set_stream` | 应用 tracing subscriber；不增加 reader 全局输出流 |
| `notify_stream_open` | 应用日志管理 |
| `notify_stream_close` | 应用日志管理和资源释放 |
| `error_free` | 错误值自动释放 |
| `error_fprint` | `LuksError` 的 `Display` / 应用错误链 |
| `error_sprint` | `to_string()` |
| `error_backtrace_fprint` | 应用 anyhow 错误上下文与诊断链 |
| `error_backtrace_sprint` | 同上；不提供 C 风格独立 backtrace 缓冲区 |
| `volume_initialize` | 解析头部 / 构造只读源；无需空的可变全局句柄 |
| `volume_free` | `DecryptedPayload` / 会话 drop，主密钥缓冲区清零 |
| `volume_signal_abort` | 应用 TaskManager 的 CancellationToken 在读/seek 边界检查，KDF 有预算，见限制 |
| `volume_open` | `File::open` + `LuksVolume::unlock_with_passphrase` / `unlock_with_master_key` |
| `volume_open_wide` | 同上，Rust 原生路径类型 |
| `volume_open_file_io_handle` | 通用 `Read + Seek`；应用 EvidenceReader 可包装 RAW/EWF/分区区域 |
| `volume_close` | drop；应用递归卸载父镜像及派生解密会话 |
| `volume_is_locked` | 类型区分解析头部和 `DecryptedPayload`；应用 `unlocked_partitions` |
| `volume_unlock` | 密码 / 主密钥两个解锁方法；错误不返回明文句柄 |
| `volume_read_buffer` | `Read::read` / `read_exact`，按 EOF 返回长度 |
| `volume_read_buffer_at_offset` | `DecryptedPayload::read_at`，非对齐读跨扇区，严格检查边界 |
| `volume_seek_offset` | `Seek::seek`；负位置和 u64 溢出报错 |
| `volume_get_offset` | `Seek::stream_position` |
| `volume_get_size` | `payload_size()` |
| `volume_get_encryption_method` | `info().cipher_name` / `cipher_mode` / `key_bytes` / `sector_size` |
| `volume_get_volume_identifier` | `Luks1Header.uuid` / `VolumeInfo.uuid` |
| `volume_set_key` | `unlock_with_master_key`；桌面/CLI/IPC `masterKeyHex` |
| `volume_set_keys` | 上游已弃用的别名，与上行相同 |
| `volume_set_utf8_password` | `unlock_with_passphrase(password.as_bytes())` |
| `volume_set_utf16_password` | `unlock_with_utf16_password` |

</details>

工具语义：`luksdeinfo` 的头部事实由公共头部模型与 `VolumeInfo` 提供；`luksdemount` 的解密字节读取
由解密镜像和既有文件系统浏览/导出/Windows Dokany 流程提供。没有专门的 LUKS FUSE 程序、Python C 扩展或 C ABI；
它们是平台/语言包装，不需要作为桌面运行时依赖。Rust reader 可安全移交线程，应用通过 Mutex 串行化共享 seek 状态。

## 桌面与 CLI

在镜像信息的「解锁加密容器」或 LUKS 分区行的「解锁」中，选 LUKS1 / LUKS2 后可切换「密码」/「主密钥」。
主密钥输入为十六进制，自动去除空白；非法字符、奇数长度和超过 256 字节会阻止提交。
关闭、切换凭据或格式会清空输入；后台完成/失败或重复任务拒绝也清空传输对象里的凭据。

`encrypted_volume_unlock` 的 `options.password` 与 `options.masterKeyHex` 必须恰好提供一个非空凭据：

```json
{"imageId":"<已加载镜像 ID>","options":{"scheme":"luks","partitionId":0,"masterKeyHex":"<卷主密钥的十六进制>"}}
```

它是卷主密钥，不是 PBKDF2 之后的密码槽密钥；VeraCrypt 不接受该选项。
`target`、源分区/区域选择、任务取消、会话卸载和导出入口沿用现有契约。
结果中的 `volume_key` 是导航标识（例如 `part:0`），不包含密码或加密主密钥。

## 验证与限制

`scripts/make-luks-compatibility-fixtures.py` 用 Python hashlib/OpenSSL、PyCryptodome 和 pyserpent 1.2.0
生成公开测试密钥下的 130 个独立容器和 12 个大扇区 IV 向量。提交的 `vendor/luks/tests/data/compatibility.json`
无需外部镜像或 Python 即可被 `just test-luks` 和应用回归测试使用。它覆盖第二密码槽成功、AF 末块/扇区补齐、
各算法密钥长度、Unicode 密码、主密钥摘要验证、LUKS2 独立 area key size、非对齐随机/顺序读与截断失败。
大扇区向量单独覆盖 2^32 之后的 plain/plain64 和 Blowfish benbi 步长。
`oracle_luks1/2` 的 cryptsetup 大镜像测试由 `LUKS1_ORACLE` / `LUKS2_ORACLE` 控制，未配置时跳过；
独立 Python 样本通过不等于已经运行了这些外部 cryptsetup 对照。

读取器测试使用 `just test-luks` 与 `just clippy-luks`；涉及解锁命令或界面时另跑 `just check`、`just test-cli` 与 `just test-desktop`。

应用严格检查源范围、LUKS1 payload/keyslot 范围和 LUKS2 主头校验/布局。
每次 KDF 预算 30 秒、整个密码解锁预算 90 秒，Argon2 内存上限 4 GiB；预算用于拒绝预测过贵的推导，
RustCrypto 的单次 KDF 内部没有取消点，取消需等当前推导结束。不会为取消创建轮询线程。
解密源关闭、坏区、短读或越界均失败，不合成密文后继续报告成功。
