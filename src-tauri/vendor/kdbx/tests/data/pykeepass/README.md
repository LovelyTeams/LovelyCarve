# pykeepass test databases

Copied unmodified from the `tests/` directory of pykeepass,
<https://github.com/libkeepass/pykeepass>, commit
`aec256659e7b2b497a37891476241b720f60c84e` (GPL-3.0, by Philipp Schmitt,
Evan Widloski and contributors). The databases were written by KeePass 2.x and
KeePassXC (their `Generator`); they hold invented test content. Passwords are
`password` (`test4_blankpass.kdbx`: empty password and `test4.key`); which
keyfile or transformed key opens which file is listed in
`tests/pykeepass_corpus.rs`.

| SHA-256 | Bytes | File |
| --- | --- | --- |
| `65cbe3fcb402de9b245a899b5a8864d858f6a90cd113e47629ca5400247a6cd9` | 2853 | `extra_content.kdbx` |
| `f7fb8b0a11229b2338cf1faa695783b038f8ae84a08aa17f4a1185f56079dc9e` | 187 | `test_invalidversion.key` |
| `bc2a9dab226101f2f19f3b37f572c0a958d8f3d3a2823d358aa9e25a81a3200d` | 4318 | `test3_transformed.kdbx` |
| `5f6fb723d94d597b1e1c27acbe1572c0a0e4607f16748afddff40ef2598b9d7f` | 5838 | `test3.kdbx` |
| `b37d0a7a11eca613e161b29ba6f280af2506040d256e338b9c33c89f142c29ae` | 187 | `test3.key` |
| `400f6b3af6c8573832cef95823b4fcf44986e6e2565fd835eda3f1defa513663` | 3909 | `test4_aes_uncompressed.kdbx` |
| `68f1a0472526b22fba0e6b4d1c64ab5858f59647321307bcd2b015a51a174823` | 1989 | `test4_aes.kdbx` |
| `cfe32681d5ff6f74f601d29ad22b41c9bc310eb8470bc650f83431f2bcd98ebd` | 1959 | `test4_aeskdf.kdbx` |
| `c087fd7660ddf95b4ec9355fe767c98145841abecb4f638631225225342697c9` | 1365 | `test4_argon2id.kdbx` |
| `9d339a8433e6684a2227ca621ff849b0e84c74b44c8e843d13ba5217b34c9125` | 1365 | `test4_blankpass.kdbx` |
| `d34d104856648491f0fbbf8d627673f2de0a0ac96d0000e93858794960ce80a5` | 3893 | `test4_chacha20_uncompressed.kdbx` |
| `e5994bae6f948cba6b38cf8af15c5154d985184e768e58d0b58fb327ee10db34` | 1982 | `test4_chacha20.kdbx` |
| `3cb8994e84f43186bf98cae404061846259a6406d8971a3ec74b9a93c1909898` | 2341 | `test4_hex.kdbx` |
| `0b7f44b7e52fa47c779d80d51089eef1bbc3ad80cae12ecea1ab7ca9c05fc91a` | 64 | `test4_hex.key` |
| `a0b04bd46fd48b963e4c394ac47de48daa34a1c71f115ba1f56e1c8ca6e774fc` | 1849 | `test4_keyx.kdbx` |
| `1ced485f16bcade8c5393425525d13c8d48ac37c6ae443842b17b674bd0d98e2` | 240 | `test4_keyx.keyx` |
| `458796e2c1a3c91d68521fd32e3738809183502f3db747874469aaa3e5376cad` | 4439 | `test4_transformed.kdbx` |
| `edba88821a4bbd5ad9864064e659bf678a742a581dded26a8b40597a3549b0f8` | 3909 | `test4_twofish_uncompressed.kdbx` |
| `02fe89e821bcc110ba9a3a0f942d472a277afc6ff65d961541ee4b7662113bb7` | 1989 | `test4_twofish.kdbx` |
| `b0f96854452ffdf73c448378f91ae136156e95e64c122152621f9dbdeaf4a9a4` | 5575 | `test4.kdbx` |
| `c0832b7d8785b059e1dc6de4a2e8b1ff10cdeb2ae35647b79922109aa0540297` | 128 | `test4.key` |
