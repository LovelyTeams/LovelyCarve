# 小米 `.bak` 备份

ZIP 与本机目录检材中的 `.bak` / `.ab` 自动尝试按小米 / Android Backup 解码。
成功后原路径显示为同名只读目录，内部保留 `apps/<包名>/…` 的 TAR 原始层级，
可用既有文件树、预览、哈希与导出流程读取。普通数据库备份不因扩展名被强行解释。

## 格式与参考

实现位于 `src-tauri/src/core/evidence/readers/mi_bak.rs`，ZIP 投影位于
`zip_image/bak.rs`。按完整 `ANDROID BACKUP\n` 魔数定位，避免把小米前缀中
偶然出现的 `AN` 两个字符当作起点；Android Backup 版本支持 1–5。
支持未压缩 TAR、zlib 和 GZIP payload；加密方式仅支持 `none`。

格式交叉参考：

- [NNNNzs/mi-bak-decryption](https://github.com/NNNNzs/mi-bak-decryption)，
  `lib/decrypt.js` 去掉前缀，`lib/extract.js` 解析 Android Backup 头再提取 TAR。
- [pgw00k/MIUIBakupExtract](https://github.com/pgw00k/MIUIBakupExtract)，
  `miui_backup_extractor.py` 记录 MIUI 文本头、Android Backup 头和 TAR 数据布局。
- [android-backup-extractor 的 MIUI 支持讨论](https://github.com/nelenkov/android-backup-extractor/pull/79)，
  描述小米在标准 Android 备份前追加的文本头。

以上仅用于格式对照，不引入或执行这些工具，也不复制其实现。

## 只读与边界

- 原始 ZIP / `.bak` 不改写。TAR 只物化一次，后续按索引惰性读取；临时文件随读取器释放。
  ZIP 成员直接流式解码，不额外落地完整 `.bak`。临时磁盘占用约为已展开 TAR 的总大小。
- 自动展开只接入 ZIP 和本机目录，不递归展开 TAR / 7z / 磁盘文件系统内的备份。
  也可直接选取 `.bak` / `.ab` 文件作为逻辑检材，见 [Android 备份](ANDROID_BACKUP.md)。
- 未识别、损坏、加密备份保留为文件并记录日志。当前无备份密码输入 / AES 解密流程，
  `.split` 拼接 APK 也不在本次能力内。
- ZIP 成员通过解压读取器完整消费，核对成员逻辑长度并校验 CRC；成功后才发布展开目录。
  GZIP 拼接成员与尾部校验见 [GZIP](GZIP.md)。TAR 使用既有路径校验、链接解析和条目上限。
  ZIP 总条目上限为 2,000,000，无法完整加入某个备份时保留该备份的文件节点。
- 展开节点不暴露原 ZIP 物理数据偏移，避免把临时 TAR 偏移当作证据源偏移。
  TAR 内链接仍在该备份自己的命名空间内解析。

## 验证

```powershell
just test zip_image --lib
just test mi_bak --lib
just test folder_image --lib
$env:LOVELYCARVE_MI_ZIP_TEST = '<本地小米备份 ZIP 路径>'
just test indexes_real_xiaomi_zip_when_env_path_is_set --lib -- --nocapture
```

合成用例覆盖 ZIP STORED / DEFLATE 与未压缩 / zlib payload 的组合、嵌套路径、
完整及限长读取、流式读取、链接隔离、原始字节保持、无效 / 加密备份回退与坏 CRC。
可选真实样本测试检查所有 `.bak` 的展开、条目大小及抽样读取；未设置环境变量时跳过，
真实检材不提交到仓库。
