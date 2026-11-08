# 独立 JFFS2 夹具

`generate.py` 使用 Python stdlib 逐字段构造 Linux v6.12 `jffs2.h` 的新格式节点，
没有调用 jffs2-rs、本地 Rust 读取器或 mkfs。脚本、镜像与期望正文均为 CC0-1.0。

大小端各覆盖 stored、RTIME、zlib 和独立 LZO literal 编码；包含 ZERO 空洞、
乱序版本、重叠片段、最终截断、硬链接、GC 副本、NOR ACCURATE 废弃节点、
删除标记、中文子目录、符号链接、空文件和秒级 Unix 时间戳。
`payload.bin` 是三个硬链接共同的 9001 字节期望内容。

```text
python vendor/jffs2/test_data/generate.py
just test-vendor jffs2
```

| 文件 | 字节数 | SHA256 |
| --- | ---: | --- |
| `synthetic-be-lzo.jffs2` | 10528 | `e16ee25a1ae42834e10e3980a02433b4ba61e250afb91e4c0c59d8c13e0d5350` |
| `synthetic-be-rtime.jffs2` | 2604 | `520ca22a64d49e83b417769de718c4a798aff9b57633f8f1eb622a2f93923696` |
| `synthetic-be-stored.jffs2` | 10480 | `0953c0bcf3fe2a4f2195f48bd253b4711d26cf61921df8a63278cd99499f7445` |
| `synthetic-be-zlib.jffs2` | 2160 | `4d2b5c23e75a46b38778ef240c88201f899ac408ad678e17c3043bf533a1fa9b` |
| `synthetic-le-lzo.jffs2` | 10528 | `4794f3dbccd5773a8fb61cee569af4eb8acc8d4ed4fe962ace9ecc0d5dfb307b` |
| `synthetic-le-rtime.jffs2` | 2604 | `293c077f4c8f64b66ce81001386bbc89f85c7f9bfe354fcb2ef25774995725ca` |
| `synthetic-le-stored.jffs2` | 10480 | `8c012cbf21dd331834743229200c04eaf634f82dbedd5f189b7a3e94a74d1169` |
| `synthetic-le-zlib.jffs2` | 2160 | `7df1e1655c806956469ec14a84ed047fb2d4a3d7e618b24b43d4deaba4fa81f7` |
| `payload.bin` | 9001 | `5c5a2cf16b758203667ae8270fcd4c9075ab17917dc3cf9b6f08fec1185faa4f` |
