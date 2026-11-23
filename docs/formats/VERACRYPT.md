# VeraCrypt / TrueCrypt

VeraCrypt 与 TrueCrypt 卷由 vendored `veracrypt-core`（Apache-2.0，源自 veracrypt-forensic，已改写为当前结构）用 RustCrypto 读取，
不加载 VeraCrypt 驱动，也不生成整卷明文。格式行为以 VeraCrypt 源码 master 分支（1.26.29 之后的 1.27.0 开发版，2026-11-24 核对）为参照：
`src/Common/Volumes.c`（`ReadVolumeHeader`）、`Pkcs5.c`、`Crypto.c`、`Keyfiles.c`、`Tests.c`，
以及 `src/Volume/VolumeLayout.cpp` / `Volume.cpp`；cryptsetup 的 `lib/tcrypt/tcrypt.c` 作为另一份独立实现对照
TrueCrypt 迭代次数、旧版布局和系统加密偏移。

## 格式、算法与凭据

| 能力 | LovelyCarve | VeraCrypt 1.26 |
| --- | --- | --- |
| PBKDF2 PRF | SHA-512、SHA-256、BLAKE2s-256、Whirlpool、Streebog、RIPEMD-160 | 前五种；RIPEMD-160 已于 1.26 移除 |
| Argon2id | 一条 lane、版本 0x13、64 字节盐、192 字节输出；PIM 0 视为 12，内存 `64 + (PIM−1)·32` MiB（上限 1 GiB），轮数 PIM ≤ 31 时 `3 + (PIM−1)/3`，之后每 PIM 加一 | 1.26.29 起，仅非系统卷 |
| 迭代次数（卷） | PIM 0：500000（RIPEMD-160 为 655331）；PIM > 0：`15000 + 1000·PIM`，RIPEMD-160 同样适用 | 相同 |
| 迭代次数（系统加密） | SHA-256、BLAKE2s、Streebog：200000 或 `2048·PIM`；SHA-512、Whirlpool 同卷；RIPEMD-160：327661 或 `2048·PIM` | 相同，见下文 Streebog |
| TrueCrypt | `TRUE` 卷头，RIPEMD-160 2000（系统加密 1000）、SHA-512 1000、Whirlpool 1000；无 PIM | 1.26 起不再支持 TrueCrypt 模式 |
| 密码 | 原样使用 UTF-8 字节，最长 128 字节 | 相同 |
| PIM | 卷 0–2147468，系统加密 0–65535 | 相同 |
| 密钥文件 | 文件与文件夹（不递归，跳过隐藏文件与子文件夹），每个只取前 1 MiB；密码不超过 64 字节时 64 字节池，否则 128 字节池；可只用密钥文件 | 相同；安全令牌与 EMV 密钥文件不支持 |
| 加密算法 | AES、Serpent、Twofish、Camellia、Kuznyechik，以及 AES-Twofish、AES-Twofish-Serpent、Serpent-AES、Serpent-Twofish-AES、Twofish-Serpent、Camellia-Kuznyechik、Camellia-Serpent、Kuznyechik-AES、Kuznyechik-Serpent-Camellia、Kuznyechik-Twofish；全部 XTS | 相同 |
| XTS | 512 字节数据单元，单元号为相对宿主起点（系统加密为整盘）的绝对偏移 / 512，与卷扇区大小无关 | 相同 |
| 卷头位置 | 标准卷 0、隐藏卷 64 KiB、两者的内嵌备份（倒数 128 KiB / 64 KiB）、TrueCrypt 5 隐藏卷（倒数 1536 字节）、系统加密（磁盘第 62 扇区） | 1.26 不再读取 TrueCrypt 5 位置 |
| 卷头版本 | 1–5；版本 4 起校验字段 CRC，版本 5 起读取扇区大小（512–4096，512 的倍数） | 相同 |
| 布局 | TrueCrypt 6.0 之前的旧布局（数据紧接 512 字节卷头）；已完成的非系统就地加密；系统分区与整盘系统加密 | 1.26 不读旧布局 |

卷头报告：产品（VeraCrypt / TrueCrypt）、所用卷头、PRF 与迭代次数（Argon2id 另报内存）、加密算法、卷头版本、
所需最低程序版本、扇区大小、系统 / 就地加密标志、卷大小、隐藏卷大小、加密区起止，以及主次密钥相同（VeraCrypt 会对此类卷告警）。

## 头部搜索

每个待读卷头的每种派生都参与搜索：TrueCrypt 的低迭代次数最先完成；VeraCrypt 的 PBKDF2 按输出块拆成独立任务，
在全部核心上并行，先算出单一加密算法所需的前 64 字节，再补齐二、三重级联所需的部分，因此常见的单算法卷不必等待级联所需的派生。
每 1024 次迭代检查一次取消。Argon2id 在 TrueCrypt 派生和首个 VeraCrypt SHA-512 派生都失败后才开始，
单次派生不能中途停止；自动搜索只在 PIM ≤ 31（不超过 13 轮）时包含 Argon2id，更高的 PIM 需在「密钥派生」中明确选择 Argon2id。
任务中心按完成的派生任务数显示进度。

「卷头」默认「自动」：同时尝试标准卷头、隐藏卷头和 TrueCrypt 5 隐藏卷头（后者只用 TrueCrypt 迭代次数），与 VeraCrypt 挂载时一样，
密码错误时的总计算量约为单个卷头的两倍。指定「标准卷」「隐藏卷」「备份卷头」或「密钥派生」可以缩小搜索。

## 读取

解锁后按需读取：密文按请求范围一次读出，按 512 字节单元解密；每个算法层的密钥扩展只在解锁时做一次，
大范围读取在多个线程上解密。读取不经过全局锁，多个线程可同时读取同一卷。
源检材短读、坏区合成字节或越过检材末尾都直接失败，不把零或密文当作明文。

## 与参考实现有意保留的差异

- **TrueCrypt 与 RIPEMD-160**：VeraCrypt 1.26 已移除 TrueCrypt 模式和 RIPEMD-160，这里仍然读取，迭代次数与 VeraCrypt 1.25.9 及 cryptsetup 相同。
  TrueCrypt 的派生开销很小，自动搜索总是包含它们；给出 PIM 时不再尝试 TrueCrypt（TrueCrypt 没有 PIM）。
- **Streebog 系统卷头**：Windows 版 VeraCrypt 以启动迭代次数（200000 / `2048·PIM`）创建系统卷头，跨平台代码则以卷的迭代次数读取，两者都尝试。
- **校验顺序**：先校验两处 CRC 再判断卷头版本与扇区大小；VeraCrypt 先看版本。两处 CRC 都通过的卷头才可能报告「不支持」，随机解密结果不会中止搜索。
- **程序版本**：卷头记录的最低程序版本只用于区分 TrueCrypt 6.0 之前（`< 0x0600`）和 VeraCrypt 1.11 之前（`< 0x010b`）的旧布局，并在结果中报告，不据此拒绝。
- **未完成的加密**：非系统就地加密未完成的卷明确拒绝（与 Windows 版 VeraCrypt 一致）。暂停的系统加密按 VeraCrypt 的方式读取：
  加密区内解密，其余部分按存储原样读取，结果标记「加密未完成」。
- **明文 NTFS 启动扇区**：系统加密时，分区（整盘视图中为加密区）首扇区若以 `EB 52 90 "NTFS "` 开头则按原样读取，
  这是 VeraCrypt 对 Windows 修复后改写启动扇区的处理；结果中报告。

## 桌面与 CLI

在镜像信息的「解锁加密卷」中选择 VeraCrypt 和区域，填写密码（只用密钥文件时可留空），用「添加密钥文件」「添加密钥文件夹」加入密钥文件，
按需选择「卷头」「密钥派生」并填写 PIM。系统加密选「卷头 → 系统加密」并选中被加密的系统分区：卷头从同一磁盘的第 62 扇区读取，
明文接在该分区上；选「整个镜像 / 容器」时生成整盘明文视图的独立检材，加密区以外的字节按原样读取，分区表照常识别。
系统加密的 PIM 上限为 65535，且不提供 Argon2id。

CLI 使用 `unlock --scheme veracrypt`：

```text
lovelycarve-cli -C "<案件目录>" unlock /vc --scheme veracrypt --password "@<密码文件>" --keyfile "<密钥文件>" --keyfile "<密钥文件夹>"
lovelycarve-cli -C "<案件目录>" unlock /disk/part:2 --scheme veracrypt --header system --password "@<密码文件>"
lovelycarve-cli -C "<案件目录>" unlock /vc --scheme veracrypt --header backup --prf sha512 --pim 500 --password "@<密码文件>"
```

`encrypted_volume_unlock` 的 VeraCrypt 选项为 `header`（`auto` / `normal` / `hidden` / `backup` / `system`）、`pim`、`prf`
（`sha512` / `sha256` / `blake2s` / `whirlpool` / `streebog` / `ripemd160` / `argon2id`）和 `keyfiles`（宿主路径），凭据只能是 `password`：

```json
{"imageId":"<已加载镜像 ID>","options":{"scheme":"veracrypt","partitionId":2,"header":"system","credential":{"kind":"password","password":"<密码>"},"keyfiles":["<密钥文件>"]}}
```

结果的 `veracrypt` 字段给出上述卷头信息；`hidden` 表示打开的是隐藏卷。整盘系统加密只能作为独立检材打开（`target: newImage`）。

## 验证与限制

- **VeraCrypt 自检向量**：`src/Common/Tests.c` 中 SHA-256、SHA-512、BLAKE2s、Whirlpool、Streebog 的 PBKDF2 向量，
  以及 `TestSectorBufEncryption` 对全部 15 种算法（含级联）的 CRC-32；另有 IEEE 1619 XTS 向量、TrueCrypt 的 RIPEMD-160 PBKDF2 向量，
  并与 `xts-mode`、`pbkdf2` crate 交叉比对。
- **独立兼容样本**：`scripts/make-veracrypt-compatibility-fixtures.py` 用 Python hashlib / hmac、cryptography（AES、Camellia、Argon2id）
  与 twofish 包自行实现 XTS、卷头、密钥文件和各布局，生成 `src-tauri/vendor/veracrypt/tests/data/compatibility.json`：
  覆盖各 PRF、RIPEMD-160 的 PIM 公式、Argon2id、Twofish / Camellia / AES-Twofish、密钥文件（64 / 128 字节池、只用密钥文件）、
  4096 字节扇区、就地加密、隐藏卷自动识别、内嵌备份卷头、TrueCrypt 7 与 TrueCrypt 5 布局、系统分区与整盘视图、暂停的系统加密、
  明文 NTFS 启动扇区及各种拒绝情形。Serpent、Kuznyechik 与 Streebog 没有可用的 Python 实现，只由 VeraCrypt 自检向量覆盖。
  `just test-veracrypt` 无需外部镜像即可运行这些测试。
- **真实卷**：`tests/cryptsetup_corpus.rs` 读取 cryptsetup 公开测试集 `tests/tcrypt-images.tar.xz`
  （[cryptsetup](https://gitlab.com/cryptsetup/cryptsetup) 仓库，blob `f431393ad321749da327f6362be1a7348d2421f3`，
  SHA-256 `cf65f0b336a623c2d5510e6ad63d364a9ade1dd46ac8407e63a0e95b17823272`，由真实 TrueCrypt / VeraCrypt 生成，不随仓库分发）。
  解压后把 `VC_TCRYPT_IMAGES` 指向 `tcrypt-images` 目录，运行 `just test-veracrypt --release --test cryptsetup_corpus -- --nocapture`：
  与 cryptsetup 的 `tcrypt-compat-test` 相同，每个卷应解出卷序列号 DEAD-BABE 的 FAT，隐藏卷为 CAFE-BABE；
  覆盖 TrueCrypt 5–7（含密钥文件与隐藏卷）、VeraCrypt 各 PRF、Argon2id 默认值与 PIM 8 / 33、PIM 1234、密钥文件（无密码、12 与 72 字符密码）、
  Camellia / Kuznyechik 级联，以及 MBR 系统分区、MBR 整盘与 GPT 系统分区三种系统加密磁盘；TrueCrypt 5.0 之前的 CBC / LRW 卷应以认证失败结束。
  这些系统加密磁盘是截短的副本，卷头记录的加密区比镜像长，读取只要求所选分区在镜像内，与 VeraCrypt 一致。
  `tests/oracle_veracrypt.rs` 另用 `VC_ORACLE`、`VC_HIDDEN_ORACLE`、`VC_SERPENT_ORACLE`、`VC_CASCADE_ORACLE`、`VC_CASCADE3_ORACLE`
  指向单个真实 VeraCrypt 卷比对扇区哈希。未设置环境变量时这些测试跳过，跳过不等于已验证。
- **应用集成**：`encrypted_image_tests` 用 `src-tauri/tests/data/encrypted` 中独立生成的容器验证自动识别隐藏卷、原分区内挂接、快照与导出；
  `just test-cli` 覆盖命令行解锁。

不支持：TrueCrypt 5.0 之前的 LRW / CBC 卷；安全令牌与 EMV 密钥文件；从检材内部直接取密钥文件（先导出到本地）；
外部卷头备份文件与救援盘中的卷头；隐藏操作系统；隐藏卷保护（读取本来只读）；未完成的非系统就地加密。
Argon2id 单次派生不可中断，取消要等当前派生结束。
