# 独立 SquashFS v1/v2 夹具

脚本 `../generate_legacy.py`、本目录的合成镜像和期望正文为 CC0-1.0。
Python stdlib 逐字段编码大小端位域、v1/v2/2.1 inode、目录 / 索引、16/32 位
数据块表、共享片段、UID/GID、硬链接、符号链接和设备 / FIFO / socket。
元数据可压缩或 stored，并覆盖 check marker 和跨 8 KiB 边界 inode。
空文件的 mtime 为 -123，特殊 inode 不记录 mtime；不借用 mkfs 时间。

`payload.bin` 为 9001 字节，另有 32/64 KiB 块的期望正文，覆盖旧式零长度编码。
Python `zlib` 与 `lzma` 生成标准压缩块 / 带 EOS LZMA。
`known-length.block` 与 `v2.0-le-lzma-known.squashfs` 的压缩数据由独立
NanaZip 6.5（7z 命令兼容）生成单文件、非 solid、明文头的 LZMA 7z 包；
取出唯一 packed stream 后，由 Python/liblzma 核对解码字节和无 EOS 状态。
这不调用 backhand、lzma-rust2、lzma-rs 或 mksquashfs。

```text
python vendor/backhand/test_data/generate_legacy.py --sevenzip 7z
just test-vendor backhand --features v1_v2
```

`--sevenzip` 仅在重新生成无 EOS 夹具时需要，应用和测试均不调用外部程序。
不带该参数会重建其他夹具，并保留已存在的无 EOS 夹具。

| 文件 | 字节数 | SHA256 |
| --- | ---: | --- |
| `eos-length.block` | 38 | `3574e75e3720bc1281db52c4ca9d27193e5159b216f00c015f17f3eb6a57dd3d` |
| `known-length.block` | 32 | `52d920f35e5bcdb9ec1c4bfa28c7bf688be72bc6be7d940f4c2c0a7d639be19b` |
| `payload-32k.bin` | 66345 | `5fa929f1383f53fc0bbf309c2665773286ee74b483198d8377a2295a01e1576d` |
| `payload-64k.bin` | 131881 | `03907ad1bd634729dd8cebd2ea48b5db289075a46f639aad4c0bc5f34e01ada0` |
| `payload.bin` | 9001 | `4c0649d8a5ad1c3a69691a75c5c1a5e0e4c6c00fb18219da56a1b0f777003ced` |
| `v1.0-be-stored.squashfs` | 12861 | `456129465cbe35ca826248a470ef2d17151ab676b2fa81157038b8537e52855d` |
| `v1.0-be-zlib.squashfs` | 4714 | `220e356a48cbf156d5a06e21821008b2a009ff58750b9e8a2dff280fc95c679f` |
| `v1.0-le-32k.squashfs` | 33506 | `be5fc60d62a360700a113b2fbf5176b41802e17e856174d1b4362d74622f0f39` |
| `v1.0-le-stored.squashfs` | 12861 | `48974d14a3320a8b8e4a93bf405d30e3501d67f52f62298f35b2d7754967b904` |
| `v1.0-le-zlib.squashfs` | 4711 | `5bb13a858092631a8336426a149229ffe01450883b0b304aef90b961de6f1473` |
| `v2.0-be-stored.squashfs` | 12920 | `efbaa985611a30e00c0d87d59efdc56048d84f10262df902f56fe5d3d71fbe1e` |
| `v2.0-be-zlib.squashfs` | 4759 | `c8809ccc77247cbd33a1ec76300178538d4d768ca01dabafd44e6a82dfcc8ad5` |
| `v2.0-le-64k.squashfs` | 66467 | `18d9c5d80fd665bd84e176a8ec4cecc1067b11b7f47ef4e66507ad21d070e708` |
| `v2.0-le-lzma-known.squashfs` | 4722 | `538700f8c001667662095977061642ca2e963abd8392bae9e87b8e0c8828ae58` |
| `v2.0-le-lzma-props.squashfs` | 4761 | `ccb529843c277fa71b097ecd1b46aad757b15495927f228a8724a360088ae10f` |
| `v2.0-le-stored.squashfs` | 12920 | `48b3e0bce0263a9fe01d6d2448201941f30e7cef94b3a63e7029b22015227a46` |
| `v2.0-le-zlib.squashfs` | 4760 | `5947cb93f0c241233a6e2483fb9cd71c8dbbfe4cd8bd24272405d2ec6f44dfa7` |
| `v2.1-be-stored.squashfs` | 12940 | `dadeadd6fd028bf30418b808a985c98c0759d74cbd5cfc8799a8a96d998b485f` |
| `v2.1-be-zlib.squashfs` | 4774 | `a89ea5d7d993197a8a60b47aeb3e9240150840dc3d521146a24594fc99d79034` |
| `v2.1-le-stored.squashfs` | 12940 | `80da7c08465994b4b14196cf121bc8fc7c17eb95cd1a5f83f72a242a4638b395` |
| `v2.1-le-zlib.squashfs` | 4773 | `c4f12807fbee62d592fb8f7af88c279d3f5630afc07d696434402769d7162272` |
