# Android Backup 独立夹具

这些样本只含虚构路径 `apps/com.example/db/main.db` 与 7 字节内容 `payload`，
没有真实检材、个人数据或第三方应用文件。输入由本目录的 Python 标准库脚本生成，
再由独立的 [Android Backup Extractor](https://github.com/nelenkov/android-backup-extractor)
打包；不是由待测 Rust 读取器生成。

使用上游 `latest/abe-38fd634.jar`，下载地址：
[ABE JAR](https://github.com/nelenkov/android-backup-extractor/releases/download/latest/abe-38fd634.jar)。
JAR 的 SHA-256 为 `abcc1d6e8614cc3cd975f5626537492f87460c0658980a3deb29ecb393f83f8a`，
只作为外部生成工具使用，不提交 JAR 或上游代码。夹具属于本仓库的虚构测试数据。

```text
python generate_input.py input.tar
java -jar abe.jar pack input.tar abe-v1.ab
java -jar abe.jar pack-kk input.tar abe-v2.ab
```

两份备份均未加密，使用 zlib 压缩，版本分别为 1 和 2。

| 文件 | SHA-256 |
| --- | --- |
| input.tar（仅生成，不提交） | `35da18aaaab4f4ad5032631781b19444d949a5cf8cc3fe86f8aff6dc1b3a59e5` |
| abe-v1.ab | `bae7adc62e40f8b7b3f1b65d33782db3b0925a26b0aabee781adca3285b48d10` |
| abe-v2.ab | `8a7c54bb12a5349a2c63520684fb4c480ce9f77f0e5c6127e1d23ee9f1c5c489` |
