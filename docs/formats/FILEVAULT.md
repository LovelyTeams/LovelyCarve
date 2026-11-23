# FileVault 2 / CoreStorage：filevault-forensic 接入与 libfvde 完整对照

对照日期 2026-10-13。接入 [filevault-forensic 857e04033e09c84324948b2cbfdd9e043b24d62b](https://github.com/SecurityRonin/filevault-forensic/tree/857e04033e09c84324948b2cbfdd9e043b24d62b)，包括 `filevault-core` 与实际调用的 `filevault-forensic::audit_info`。参照 [libfvde 878f3fc8cb30f1f5ac6255e5fdcec9d1af482224](https://github.com/libyal/libfvde/tree/878f3fc8cb30f1f5ac6255e5fdcec9d1af482224) 的 README、公开头文件、metadata / encrypted_metadata / encryption_context / logical_volume 实现与格式文档；**全部 88 个不同公开函数**见后面的表。C 库仅作为参照，运行时不依赖 libfvde DLL。

本功能读取 CoreStorage / FileVault 2，通常得到 HFS+ 明文卷；**APFS 原生加密是不同格式，不能由此解锁**。

## 接入后的工作流

1. GPT Apple CoreStorage GUID 或经 CRC 验证的主/备 CS 头识别为 `filevault`。
2. 加密分区行或“解锁加密卷”对话框选择 FileVault 2，先读取逻辑卷目录。界面显示卷名与大小，多卷必须选择索引，未完成加密的卷不能解锁；转换状态、保护器和最低 PBKDF2 迭代数由 CLI `encryption` 报告。
3. 可以使用密码、包含连字符的个人恢复密码，或已恢复的 16-byte VMK / 32-byte XTS pair。可提供本地 XML / EncryptedRoot.plist.wipekey；wipekey 按物理 UUID + 全零第二密钥、整文件 XTS 单元解码。
4. 单逻辑卷通过已有 EvidenceSession 原分区投影接入目录、预览、范围读、哈希、导出和快照；证据 ID、原盘分区起点和长度保留。多逻辑卷分别打开独立的只读明文会话，各自使用独立 image ID 隔离文件 ID 与索引；源字节仍是原密文，派生视图依赖源会话。
5. `filevault_inspect` 是带 TaskManager 资源预算与取消的只读命令；UI 的异步查询在切区域 / 关闭后取消并防止迟到回填。CLI 与桌面共用命令注册。

`filevault-forensic` 的三类 observations（保护器清单、转换状态、弱 KDF）真实运行，保留 code / severity / note。低于 20,000 是上游观察阈值；它不是密码破解结果或入侵证明。没有可用 password KDF 时的零值表示不可用，不误报为弱 KDF。

## 补齐的差距与明确边界

| 项目 | 直接上游版本 | 本地实现 / libfvde 对照结果 |
| --- | --- | --- |
| 物理头 | CS + method，两处读取未做完整校验 | 固定长度、CRC-32C / seed / version / method / sector / block geometry；主头失败尝试备份并返回来源偏移 |
| metadata copies | 固定第一 slot / primary encrypted copy | 检查多个 pointer、按事务选择、验证 encrypted objects；坏 primary 尝试 secondary；来源 UUID / group / member 匹配 |
| 多逻辑卷 | 第一个 LV / 第一个 extent | 按 LV UUID 枚举、显式选择；按关联 OID 与 LV transaction 解析 family、0305 / 0505，拒绝重复 UUID |
| 逻辑映射 | 无 map 时落到物理 0；仅第一个 segment | 完整多 segment、连续逻辑覆盖、源界限、溢出与 flags；不把其他物理盘的高 16-bit index 截掉 |
| XML | 跨整块 substring 取第一 wrapped blob | 限界 CoreFoundation plist、ID / IDREF、entities、duplicate / nesting / expansion budget；按 KEK UUID 与 active context 匹配 |
| metadata 压缩 | 直接 XML | zlib、0019/0024 continuation；精确 compressed/uncompressed 长度与查环 |
| 凭据 | 第一个 password user | family 内多个 password / recovery user；RFC3394 integrity；16-byte VMK + UUID 派生 tweak 或 32-byte pair；恢复 key 另验证 HFS+ 头及几何 |
| 随机读取 | 固定连续区；短 ciphertext 仍解密零填充尾部 | 逐 logical sector 的 tweak、跨 segment / 未对齐窗口；ciphertext short read 必须错误 |
| 凭据生命周期 | keys Debug 显示数组 | redacted Debug、Zeroize/Drop、AES key schedule 清理；密码/主密钥不进入 IPC 结果或长期 state |
| 加密状态 | 只展示 ConversionStatus | 只对 Complete 解密；Converting / Pending / 未知状态明确失败 |
| 取消与预算 | 无宿主 task | TaskManager；读者检查 source token，PBKDF2 每次最多 2,000,000，整次候选尝试最多 16,000,000；每次派生之间检查取消 |
| 法证观察 | 上游 auditor，未接应用 | 两个 vendor crate 均接入；typed inspect / unlock IPC 与现有 UI / CLI / 导出流程 |

CRC 是损坏校验，**不是对攻击者的密码学认证**。RFC3394 的 unwrap integrity 验证与恢复密钥的明文卷头校验另行执行。

当前仅支持一个物理 CoreStorage 成员、512-byte 加密 sector、8192-byte metadata 对象。多物理成员、部分加密 / reencryption、未加密的 CoreStorage LV、机构恢复私钥自动 unwrap、APFS 原生加密尚未实现；不猜布局或使用零填充掩盖失败。元数据区域上限 256 MiB，plist / 外部上下文上限 16 MiB；复杂的 0505 多入口 base descriptor 明确拒绝。源 header / metadata 选择偏移由 inspect 返回，均相对于指定证据区域。

## 全部公开 API 对照

Rust 的 ownership、Read/Seek、String / UTF-16 与 Result 代替 C 的生命周期、缓冲区大小、FILE* 和错误对象。相同语义可以由一个 Rust 方法覆盖多个 C 函数，不表示 C ABI 兼容。

| libfvde 函数 | 状态 | Rust / 应用对应与边界 |
| --- | --- | --- |
| `libfvde_get_version` | 等价 | Cargo 版本与只读 std::io；无写入路径 |
| `libfvde_get_access_flags_read` | 等价 | Cargo 版本与只读 std::io；无写入路径 |
| `libfvde_get_codepage` | 设计不同 | Rust UTF-8 / 显式 UTF-16 转换；不设置进程级 codepage |
| `libfvde_set_codepage` | 设计不同 | Rust UTF-8 / 显式 UTF-16 转换；不设置进程级 codepage |
| `libfvde_check_volume_signature` | 等价 | storage::check_volume_signature；主/备头 probe，CRC / version 验证 |
| `libfvde_check_volume_signature_wide` | 等价 | storage::check_volume_signature；主/备头 probe，CRC / version 验证 |
| `libfvde_check_volume_signature_file_io_handle` | 等价 | storage::check_volume_signature；主/备头 probe，CRC / version 验证 |
| `libfvde_notify_set_verbose` | 设计不同 | 宿主 tracing / Result；没有全局 C FILE* stream |
| `libfvde_notify_set_stream` | 设计不同 | 宿主 tracing / Result；没有全局 C FILE* stream |
| `libfvde_notify_stream_open` | 设计不同 | 宿主 tracing / Result；没有全局 C FILE* stream |
| `libfvde_notify_stream_close` | 设计不同 | 宿主 tracing / Result；没有全局 C FILE* stream |
| `libfvde_error_free` | 等价 | FileVaultError / anyhow 上下文、Display 与 RAII |
| `libfvde_error_fprint` | 等价 | FileVaultError / anyhow 上下文、Display 与 RAII |
| `libfvde_error_sprint` | 等价 | FileVaultError / anyhow 上下文、Display 与 RAII |
| `libfvde_error_backtrace_fprint` | 等价 | FileVaultError / anyhow 上下文、Display 与 RAII |
| `libfvde_error_backtrace_sprint` | 等价 | FileVaultError / anyhow 上下文、Display 与 RAII |
| `libfvde_volume_initialize` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_volume_free` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_volume_signal_abort` | 等价 | CancellationToken 与 cancellable unlock；一次 PBKDF2 原子完成后响应取消 |
| `libfvde_volume_open` | 等价 | Container::open<R: Read + Seek>；EvidenceReader / std 文件路径 |
| `libfvde_volume_open_wide` | 等价 | Container::open<R: Read + Seek>；EvidenceReader / std 文件路径 |
| `libfvde_volume_open_file_io_handle` | 等价 | Container::open<R: Read + Seek>；EvidenceReader / std 文件路径 |
| `libfvde_volume_open_physical_volume_files` | 未实现多成员 | 只接受一个 bounded evidence reader；多个成员明确拒绝，不能只掩掉 member index |
| `libfvde_volume_open_physical_volume_files_wide` | 未实现多成员 | 只接受一个 bounded evidence reader；多个成员明确拒绝，不能只掩掉 member index |
| `libfvde_volume_open_physical_volume_files_file_io_pool` | 未实现多成员 | 只接受一个 bounded evidence reader；多个成员明确拒绝，不能只掩掉 member index |
| `libfvde_volume_close` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_volume_read_encrypted_root_plist` | 等价 | Container::open external_context；IPC encryptedRootPlistPath；XML / wipekey |
| `libfvde_volume_read_encrypted_root_plist_wide` | 等价 | Container::open external_context；IPC encryptedRootPlistPath；XML / wipekey |
| `libfvde_volume_read_encrypted_root_plist_file_io_handle` | 等价 | Container::open external_context；IPC encryptedRootPlistPath；XML / wipekey |
| `libfvde_volume_get_volume_group` | 等价 | Container header / volume_group_name / volumes / copy provenance |
| `libfvde_volume_unlock` | 等价 | 按 index 的 Container unlock；FileVaultVolume convenience 仅允许唯一 LV |
| `libfvde_volume_read_buffer` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_volume_read_buffer_at_offset` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_volume_seek_offset` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_volume_get_offset` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_volume_is_locked` | 设计不同 | Container 是未解锁库存，FileVaultVolume 是已解锁类型；UI unlocked_partitions 表示状态 |
| `libfvde_volume_get_size` | 等价 | LogicalVolume.info.lv_size / FileVaultVolume::size；源 bounds 与逻辑大小分开 |
| `libfvde_volume_get_encryption_method` | 支持范围内等价 | 当前 method 2 / AES-XTS-128；unsupported methods 显式错误；未加密 LV 仍未实现 |
| `libfvde_volume_get_logical_volume_group_identifier` | 等价 | 同一被选物理头的 logical_volume_group_identifier / inventory volume_group_identifier |
| `libfvde_volume_get_physical_volume_size` | 等价 | Container::header().physical_volume_size；来源区域 bounds 单独保留 |
| `libfvde_volume_get_physical_volume_encryption_method` | 支持范围内等价 | 当前 method 2 / AES-XTS-128；unsupported methods 显式错误；未加密 LV 仍未实现 |
| `libfvde_volume_get_physical_volume_identifier` | 等价 | Container::header().physical_volume_identifier / info.physical_volume_identifier |
| `libfvde_volume_get_logical_volume_size` | 等价 | LogicalVolume.info.lv_size / FileVaultVolume::size；源 bounds 与逻辑大小分开 |
| `libfvde_volume_get_logical_volume_encryption_method` | 支持范围内等价 | 当前 method 2 / AES-XTS-128；unsupported methods 显式错误；未加密 LV 仍未实现 |
| `libfvde_volume_get_logical_volume_identifier` | 等价 | LogicalVolume.info.lv_identifier；family UUID、object OID 与 transaction 独立保留 |
| `libfvde_volume_set_keys` | 等价 | unlock_with_master_key；16-byte VMK / 32-byte XTS pair，卷头验证 |
| `libfvde_volume_set_utf8_password` | 等价 | Container::unlock_with_password[_cancellable]；UTF-16 可转 String；宿主 password 字段 |
| `libfvde_volume_set_utf16_password` | 等价 | Container::unlock_with_password[_cancellable]；UTF-16 可转 String；宿主 password 字段 |
| `libfvde_volume_set_utf8_recovery_password` | 等价 | 同一 UTF-8 password API 遍历 Recovery protector；保留连字符，不修改凭据 |
| `libfvde_volume_set_utf16_recovery_password` | 等价 | 同一 UTF-8 password API 遍历 Recovery protector；保留连字符，不修改凭据 |
| `libfvde_volume_group_free` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_volume_group_get_identifier` | 等价 | 同一被选物理头的 logical_volume_group_identifier / inventory volume_group_identifier |
| `libfvde_volume_group_get_utf8_name_size` | 等价 | Container::volume_group_name / inventory；String / encode_utf16 / length |
| `libfvde_volume_group_get_utf8_name` | 等价 | Container::volume_group_name / inventory；String / encode_utf16 / length |
| `libfvde_volume_group_get_utf16_name_size` | 等价 | Container::volume_group_name / inventory；String / encode_utf16 / length |
| `libfvde_volume_group_get_utf16_name` | 等价 | Container::volume_group_name / inventory；String / encode_utf16 / length |
| `libfvde_volume_group_get_number_of_physical_volumes` | 单成员 | 当前 count 1 / index 0；header 信息；多成员组在校验阶段拒绝 |
| `libfvde_volume_group_get_physical_volume_by_index` | 单成员 | 当前 count 1 / index 0；header 信息；多成员组在校验阶段拒绝 |
| `libfvde_volume_group_get_number_of_logical_volumes` | 等价 | Container::volumes iterator/count；稳定按 UUID 编号的显式 index |
| `libfvde_volume_group_get_logical_volume_by_index` | 等价 | Container::volumes iterator/count；稳定按 UUID 编号的显式 index |
| `libfvde_physical_volume_free` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_physical_volume_get_identifier` | 等价 | Container::header().physical_volume_identifier / info.physical_volume_identifier |
| `libfvde_physical_volume_get_encryption_method` | 支持范围内等价 | 当前 method 2 / AES-XTS-128；unsupported methods 显式错误；未加密 LV 仍未实现 |
| `libfvde_physical_volume_get_size` | 等价 | Container::header().physical_volume_size；来源区域 bounds 单独保留 |
| `libfvde_logical_volume_free` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_logical_volume_unlock` | 等价 | 按 index 的 Container unlock；FileVaultVolume convenience 仅允许唯一 LV |
| `libfvde_logical_volume_read_buffer` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_logical_volume_read_buffer_at_offset` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_logical_volume_seek_offset` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_logical_volume_get_offset` | 等价 | FileVaultVolume / DecryptedVolume Read + Seek / read_at / stream_position；完整 mapping |
| `libfvde_logical_volume_get_identifier` | 等价 | LogicalVolume.info.lv_identifier；family UUID、object OID 与 transaction 独立保留 |
| `libfvde_logical_volume_get_utf8_name_size` | 等价 | LogicalVolume.info.lv_name；String / encode_utf16 / length |
| `libfvde_logical_volume_get_utf8_name` | 等价 | LogicalVolume.info.lv_name；String / encode_utf16 / length |
| `libfvde_logical_volume_get_utf16_name_size` | 等价 | LogicalVolume.info.lv_name；String / encode_utf16 / length |
| `libfvde_logical_volume_get_utf16_name` | 等价 | LogicalVolume.info.lv_name；String / encode_utf16 / length |
| `libfvde_logical_volume_get_size` | 等价 | LogicalVolume.info.lv_size / FileVaultVolume::size；源 bounds 与逻辑大小分开 |
| `libfvde_logical_volume_is_locked` | 设计不同 | Container 是未解锁库存，FileVaultVolume 是已解锁类型；UI unlocked_partitions 表示状态 |
| `libfvde_logical_volume_set_key` | 等价 | unlock_with_master_key；16-byte VMK / 32-byte XTS pair，卷头验证 |
| `libfvde_logical_volume_set_utf8_password` | 等价 | Container::unlock_with_password[_cancellable]；UTF-16 可转 String；宿主 password 字段 |
| `libfvde_logical_volume_set_utf16_password` | 等价 | Container::unlock_with_password[_cancellable]；UTF-16 可转 String；宿主 password 字段 |
| `libfvde_logical_volume_set_utf8_recovery_password` | 等价 | 同一 UTF-8 password API 遍历 Recovery protector；保留连字符，不修改凭据 |
| `libfvde_logical_volume_set_utf16_recovery_password` | 等价 | 同一 UTF-8 password API 遍历 Recovery protector；保留连字符，不修改凭据 |
| `libfvde_encryption_context_plist_initialize` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_encryption_context_plist_free` | 等价 | ownership / Drop，派生 keys zeroize；无手动 C 对象释放 |
| `libfvde_encryption_context_plist_get_data_size` | 设计不同 | 调用方持有输入 Vec/长度；内部明文 context 不作为单独 IPC 导出对象 |
| `libfvde_encryption_context_plist_copy_data` | 设计不同 | 调用方持有输入 Vec/长度；内部明文 context 不作为单独 IPC 导出对象 |
| `libfvde_encryption_context_plist_read_file_io_handle` | 等价 | 有界 external_context / CF parser / 整文件 XTS 解码 |
| `libfvde_encryption_context_plist_decrypt` | 等价 | 有界 external_context / CF parser / 整文件 XTS 解码 |

## 验证记录与再现

- Core 69 tests；auditor 9 tests；入口集成 4 tests；doc tests 2。
- 实际运行公开 dfvfs `fvdetest.qcow2`，MD5 `dd7b1d584f2e07112ec7003d5fcd9864`。CoreStorage 起点 20,480，长度 536,829,952；提取 raw 区域 SHA256 `b964d3ae709712eccb27316ca63a7a210249877bba087a945875a26414aeabd5`。原始 qcow 位于仓库外，未提交。
- 四项 Core oracle（固定 libfvde 参考 sector SHA256、最新事务库存、VMK / XTS pair、实际 HFS 全遍历与 full/stream/seek）及两项 auditor 真镜像 oracle实际运行通过。参考 SHA 来自上游已记录的独立 libfvde 结果；本次没有重新编译 libfvde 对所有 API 逐字段对拍。
- 合成回归覆盖多逻辑卷显式选择、碎片映射、备用 CS 头与 secondary metadata、future object 不覆盖旧 LV transaction、0019/0024 链、zlib、第二 protector / recovery、CF references / entity / duplicate / expansion bomb、截断 ciphertext、零/过大 KDF、取消和 secret Debug。
- 应用层另核对 inspect / unlock / source-partition identity / HFS ranges / revoke。`FVDE_ORACLE_IMAGE` 未设置时这些外部样本测试不执行，不能算真镜像验证。
- 宿主全量 Rust 测试 1024 项通过（12 ignored），前端 777 项通过（1 skipped），IPC / 结构 / 类型检查与生产构建通过。`just test-cli` 全套端到端通过；另以公开样本运行 JSONL inspect → unlock → HFS 浏览 → 卷头导出，SHA-256 与独立 libfvde 参考一致，卸载源后派生会话清空。fmt / clippy / 示例编译均通过；共享 APFS 回归 37 项通过（1 ignored）。
- 尚未完成独立的机构恢复、multi-member、部分加密 corpus，以及 macOS/Linux desktop 与 Dokan 实挂验证。

```powershell
just test-filevault
just clippy-filevault
$env:FVDE_ORACLE_IMAGE = 'C:/path/to/fvdetest.cs.raw'
just test-filevault
$env:CARGO_BUILD_JOBS = '1'
just test --lib filevault_real_oracle
```

CLI 使用现有注册命令：`filevault_inspect` 的 options 可指定 `partitionId` / `offset,length` 和 `encryptedRootPlistPath`；`encrypted_volume_unlock` 指定 `scheme: "filevault"`、`logicalVolumeIndex`、`credential`（`password` 或 `masterKey`）、`target: "sourcePartition"`（单 LV）或 `"newImage"`（多 LV 必须使用独立会话）。结果包含所选 LV UUID / index，原分区挂接不允许不同 LV 复用同一个根索引。不要把真实凭据写进示例仓库或报告；CLI 运行方法见 [CLI 手册](../CLI.md)。

参考：[libfvde 固定源码与格式文档](https://github.com/libyal/libfvde/tree/878f3fc8cb30f1f5ac6255e5fdcec9d1af482224)、[filevault-forensic](https://github.com/SecurityRonin/filevault-forensic/tree/857e04033e09c84324948b2cbfdd9e043b24d62b)、[HFS 对照](HFS.md)、[APFS 边界](APFS.md)。
