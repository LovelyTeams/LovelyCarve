# KeePass KDBX

KeePass 2.x 与 KeePassXC 的密码库（`.kdbx`）由 vendored `kdbx-reader`（`src-tauri/vendor/kdbx`，GPL-3.0-only）读取。
它是 [pykeepass](https://github.com/libkeepass/pykeepass) 4.2.0（提交 `aec256659e7b`，GPL-3.0）读取部分的 Rust 改写：
外部头部、密钥派生、载荷解密与校验、受保护值和 XML 数据模型，以及 pykeepass 的只读查询（按 UUID / 路径查找、回收站、
字段引用、附件数据、明文 XML）。没有保存、种子轮换或任何写入路径。格式细节另按 KeePass 2.x 的读取行为核对，
差异见下文。

## 格式、算法与凭据

| 能力 | LovelyCarve | pykeepass 4.2.0 |
| --- | --- | --- |
| 版本 | KDBX 2.x / 3.0 / 3.1 按 KDBX 3 布局，4.0 / 4.1 按 KDBX 4 布局；KeePass 2 预发布签名 `0xB54BFB66`；KeePass 1.x（KDB）只识别、不读取 | 3.x 与 4.x |
| 载荷加密 | AES-256-CBC、Twofish-CBC（PKCS#7 填充）、ChaCha20（96 位 nonce） | 相同 |
| 压缩 | 无、GZip | 相同 |
| 密钥派生 | AES-KDF（KDBX 3 与 4）；Argon2d、Argon2id，版本 0x10 / 0x13，支持密钥参数 `K` 与不超过 32 字节的关联数据 `A`；种子不是 32 字节时按 KeePass 先取 SHA-256 | 忽略 `K` 与 `A` |
| 内部流 | Salsa20（KDBX 3）、ChaCha20（KDBX 4）、无；ArcFour 变体不实现，受保护值保留密文并告警 | ArcFour 变体报错 |
| 凭据 | 主密码（UTF-8 的 SHA-256）；密钥文件：XML 1.0（base64）、XML 2.0（十六进制并校验 `Hash`）、正好 32 字节、正好 64 个十六进制字符，其余文件取 SHA-256；转换密钥（KDF 输出）。空密码与没有密码是不同的密钥成分 | 相同；转换密钥同 |
| 受保护值 | 所有 `Protected="True"` 元素按文档顺序消耗内部流，包括 KDBX 3 `Meta/Binaries` 中受保护的附件 | 只处理 `Value` 元素，受保护附件之后的值会错位 |
| 完整性 | KDBX 3：流起始字节校验密钥，每个数据块校验 SHA-256，`Meta/HeaderHash` 与头部比对；KDBX 4：解密前校验头部 SHA-256，头部 HMAC 校验密钥，每个数据块校验 HMAC-SHA-256 | 不比对 `Meta/HeaderHash` |
| 时间 | KDBX 4 为 0001-01-01 UTC 起的秒数（base64）；KDBX 3 为 ISO 8601 文本，带 `Z` 或偏移量时换算为 UTC，没有时区的保留原文并标为未标明时区 | 一律当作 UTC |
| 数据模型 | 群组、条目及全部字段（含受保护标记）、标签、时间、附件（附件池或 KDBX 2.x 条目内嵌数据）、自动输入、历史版本、自定义数据、自定义图标、已删除对象；KDBX 4.1 的群组标签、原所在群组、密码质量检查、图标名称与修改时间 | 相同，经 XPath 读取 |
| 字段引用 | `{REF:T/U/P/A/N/I@T/U/P/A/N/I/O:…}`，最多嵌套 12 层；`O` 在自定义字段中查找 | 不支持 `O`，嵌套不设上限 |

头部报告无需密钥：版本、加密算法与 UUID、压缩、密钥派生及参数（轮数，或内存、迭代次数、并行度与算法版本）、盐或转换种子、
主种子、加密 IV、KDBX 3 的内部流与其密钥和流起始字节、注释、公开自定义数据、头部长度与 SHA-256（KDBX 4 同时判断存储值是否一致），
以及每个头部字段的 ID、名称、偏移和长度。

## 与参考实现有意保留的差异

- `Meta/HeaderHash` 与头部不符时 KeePass 拒绝打开；这里照常解密并报告不符，供检验人判断头部是否被改动。
- XML 密钥文件声明未知版本或 `Hash` 校验失败时按 KeePass 报错；pykeepass 会把它当作普通文件取哈希。
- 布尔值按 KeePassXC 接受 `True` / `true` / `1`；`Protected` 属性只认 KeePass 写的 `True`，与 KeePass 消耗内部流的方式一致。
- 多于一个根群组时读取第一个并告警；读取器的明文 XML（`Database::xml`）仍包含全部内容。
- 受保护值不是 UTF-8 时按替换字符解码，不像 pykeepass 那样剔除 XML 不允许的控制字符。

## 读取上限

一个文件可以选择任意大的密钥派生代价，读取器在计算前拒绝超出上限的参数：AES-KDF 至多 2³² 轮，Argon2 内存至多 4 GiB 且
内存与迭代次数之积至多 256 GiB；解密并解压后的载荷与单个附件至多 1 GiB，XML 至多 256 层、400 万个元素。Argon2 内存按需分配，
分配失败报错而不是终止程序。应用另限制整库读取 256 MiB、密钥文件 64 MiB、单次附件预览 1 MiB。

## 桌面与 CLI

`.kdbx` 文件，或任何以 KDBX 签名开头的文件，在原类型视图中先显示无需密钥的外部头部和解锁表单：主密码、密钥文件（同一目录的
检材文件，或主机上的文件）与转换密钥。密码框为空且选了密钥文件时，先只用密钥文件，失败后再用空密码加密钥文件，与 KeePass 区分
「无密码」与「空密码」的方式一致。解密后左侧是群组树（含回收站）、数据库信息与已删除对象，右侧是条目表与条目详情：受保护字段默认
遮蔽、可逐项显示和复制，字段引用显示解析结果，历史版本标出每次修改的字段，附件按文本或十六进制预览。解密后的读取改用转换密钥，
不再保留密码；锁定后丢弃数据库。

CLI 用 `inspect` 读取，按签名或 `.kdbx` 扩展名识别，也可用 `--as kdbx` 指定。不给凭据时只输出外部头部；
`--password @文件`（或 `-`、终端输入）、`--keyfile <主机文件>`、`--transformed-key @文件` 解密并输出完整数据库报告：

```powershell
lovelycarve-cli -C "<案件目录>" inspect /<别名>/<卷>/Users/alice/Documents/Database.kdbx --password @pw.txt --keyfile "<密钥文件路径>"
```

后端命令 `fs_kdbx_header`、`fs_kdbx_open`、`fs_kdbx_attachment` 也可经 `rpc` 调用。凭据错误的错误码为 `invalidInput`，损坏为
`corruptData`，不支持的算法或版本为 `unsupportedFormat`，超出上限为 `resourceLimit`。

## 验证与限制

- `tests/data/pykeepass/`：pykeepass 测试集里由 KeePass 2.x 与 KeePassXC 写出的 16 个数据库及其密钥文件（来源、许可证与 SHA-256 见该目录
  README）。`tests/pykeepass_corpus.rs` 逐一核对加密算法、密钥派生、版本与压缩，用两份转换密钥打开，并复现 pykeepass 测试断言的
  条目、群组、UUID、标签、自定义字段、自动输入、OTP、历史版本数量、字段引用、回收站与附件；另验证错误密码、错误与无效密钥文件、
  头部 SHA-256、数据块与 `Meta/HeaderHash` 篡改。
- `tests/data/synthetic.json`：`scripts/make-kdbx-fixtures.py` 按格式规则独立写出的 7 个数据库（Python cryptography、argon2-cffi、
  PyCryptodome、twofish，不经 Rust 读取器），覆盖 KDBX 3.0 / 3.1 / 4.0 / 4.1、三种加密算法、AES-KDF / Argon2d v0x10 与 v0x13 / 带密钥参数和
  关联数据的 Argon2id、三种内部流、四种密钥文件、空密码、多数据块、受保护与压缩附件、历史版本、已删除对象、公开自定义数据与未知头部字段。
- 单元测试：AES-KDF 与 Argon2 对照 cryptography / argon2-cffi 的结果，Salsa20 密钥流对照 PyCryptodome，并覆盖头部、VariantDictionary、
  密钥文件、数据块、时间与 XML 的边界和错误。
- 不支持：KeePass 1.x（KDB）、插件提供的加密算法与密钥派生、ArcFour 变体内部流、密钥提供程序（Windows 用户账户、YubiKey
  挑战应答等）、超过 32 字节的 Argon2 关联数据，以及任何写入。KeePass 2.0 测试版写出的 KDBX 2.x 与预发布签名文件没有真实样本，
  只按格式规则实现。

`just test-kdbx` 运行上述测试，`just clippy-kdbx` 检查读取器；修改语料后用 `python scripts/make-kdbx-fixtures.py` 重新生成，并更新
`tests/data/README.md` 中的 SHA-256。
