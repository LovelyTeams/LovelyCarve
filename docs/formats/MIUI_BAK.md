# 小米 `.bak` 备份

ZIP 与本机目录检材中的 `.bak` / `.ab` 自动尝试按小米 / Android Backup 解码。
成功后原路径显示为隐藏 `.bak` / `.ab` 后缀的只读目录，内部保留 `apps/<包名>/…` 的 TAR 原始层级，
可用既有文件树、预览、哈希与导出流程读取。普通数据库备份不因扩展名被强行解释。
目录的节点 ID 仍编码原始备份路径；普通 `.bak` 文件与真实同名后缀目录不改名。

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

- 原始 ZIP / `.bak` 不改写。每个备份的 TAR 在一次加载中最多物化一次，后续按索引惰性读取；
  临时文件是关闭即删除的匿名文件，随读取器释放，程序退出或异常结束时由系统删除。
  ZIP 成员直接流式解码，不额外落地完整 `.bak`。临时磁盘占用约为已物化 TAR 的总大小。
- ZIP 检材首次加载（或没有可用清单）时解码全部备份。桌面先打开证据标签页，在后台解包，
  完成后自动加载内容；解包期间可切换其他标签。标签页与任务中心显示当前备份和累计输入进度，
  可取消加载。关闭标签或切换案件会取消在途任务，未完成的临时 TAR 自动释放。
  后端同时发送 `image-load-progress` 和 `task-progress`，CLI 调用仍等待打开结果返回。
- 在案件中加载时，完整解码成功后把每个候选成员的结果（展开出的条目，或保留为文件）写入
  检材缓存 `evidence/<uid>/cache/zip-backups.zst`，并绑定 ZIP 长度与各候选成员的路径、
  目录序号、本地头偏移、CRC-32 和大小。再次打开同一案件时直接按清单恢复备份目录，
  不解码任何备份；首次读取某个备份内的文件时才解码该备份，解出的条目须与清单逐条一致，
  否则拒绝读取。按需解码不报告进度、不能取消。清单缺失、损坏或与 ZIP 不符时回退完整解码并
  重写清单；不在案件中加载（含 CLI 未指定 `--case`）时每次完整解码。本机目录检材中的备份
  仍在每次加载时解码。
- 索引指纹包含备份目录投影版本及实际展开的备份集合。打开旧案件时先下线不兼容的
  快照和索引，以实时目录浏览衔接后台重建；不会复用尚未展开时的文件类型或旧显示名。
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
完整及限长读取、流式读取、链接隔离、原始字节保持、无效 / 加密备份回退与坏 CRC，
以及清单恢复后的按需解码、ZIP 变化后重新解码、清单与备份内容不符时拒绝读取和证据指纹不变。
可选真实样本测试检查所有 `.bak` 的展开、条目大小及抽样读取，再按清单重新打开并逐个按需解码
核对；未设置环境变量时跳过，真实检材不提交到仓库。
