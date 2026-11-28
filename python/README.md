# lovelycarve

`lovelycarve` 是 LovelyCarve 的 Python 只读读取库。它复用项目的 Rust 读取器，可以在脚本中打开磁盘镜像和归档、遍历分区与目录、查看文件属性，以及按偏移读取镜像或文件字节。安装 wheel 后可直接使用，无需安装桌面版或 CLI。

## 安装

使用 CPython 3.10 或更新版本。

```sh
python -m pip install lovelycarve
```

| 平台 | wheel 架构 |
| --- | --- |
| Windows | x64、ARM64 |
| macOS 15 或更新版本 | Apple Silicon、Intel |
| Linux（glibc 2.34 或更新版本） | x64、ARM64（`manylinux_2_34`） |

安装包与导入名均为 `lovelycarve`。各平台的 wheel 也随 GitHub Release 提供。[PyPI](https://pypi.org/project/lovelycarve/) · [GitHub Releases](https://github.com/LovelyTeams/LovelyCarve/releases)

## 快速开始

打开一份可读取的 E01 镜像，打印镜像格式、逻辑大小和各卷的顶层目录。把 `disk.E01` 换成你的镜像路径。

```python
from lovelycarve import Image

with Image("disk.E01") as image:
    print(image.info.source_format, image.size)

    for volume in image.children():
        print(volume.id, volume.name)
        for node in image.children(volume):
            print(node.kind, node.name, node.size)
```

下面从 ZIP 归档的顶层选取第一个文件，读取前 64 字节，再分块计算整个文件的 SHA-256。镜像中的文件节点也可以用相同方式读取。

```python
from hashlib import sha256

from lovelycarve import Image

with Image("evidence.zip") as image:
    root = image.children()[0]
    node = next((entry for entry in image.children(root) if entry.is_file), None)

    if node is not None:
        print(node.name, image.read_file(node, offset=0, size=64).hex())

        digest = sha256()
        with image.open(node) as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        print(digest.hexdigest())
```

## API 参考

| 接口 | 返回或用途 |
| --- | --- |
| `Image(path)` | 接受路径字符串或 `Path`，自动识别格式，以只读方式打开 |
| `image.info`、`image.size`、`image.partitions` | 镜像元数据、逻辑字节数和分区信息，关闭镜像后仍可查看 |
| `image.children()` | 列出卷或归档的入口节点 |
| `image.children(parent)` | 列出指定目录的直接子节点 |
| `image.stat(node)` | 返回 `FileStat`，包含大小、inode、mode 和可用的数据段映射 |
| `image.read(offset, size)` | 读取镜像逻辑字节，单次请求最多 64 MiB。对归档调用时读取容器本身 |
| `image.read_file(node, offset=0, size=-1)` | 返回节点的文件字节，默认从偏移处读到文件末尾 |
| `image.open(node, mode="rb")` | 返回可定位的只读二进制流 `ImageFile`，适合分块读取大文件 |

`children`、`stat`、`read_file` 和 `open` 接受 `Node` 或其 `id`。请直接复用目录遍历返回的节点或 ID，ID 只用于对应镜像内的定位。文件时间戳保留读取器给出的字符串与缺失值，时区以读取结果为准。目录列表采用轻量读取，跳过 NTFS 删除记录扫描和额外的未分配空间节点。

`ImageFile` 实现 `io.RawIOBase`，支持 `read`、`readinto`、`seek`、`tell` 和上下文管理。频繁小块读取时可用 `io.BufferedReader` 包装。对缺少原生随机访问的格式，定位后读取可能需要从头流式读取或解压。

关闭文件只释放该文件对象。关闭镜像后，关联的文件对象无法继续发起原生读取，外部缓冲器仍可能持有先前读入的字节。不同文件对象各有自己的游标，共享同一个文件对象时由调用方串行操作游标。原生打开、列目录及读取会释放 GIL，已经进入原生读取的操作可在另一个线程关闭镜像后完成。

## 格式与读取完整性

可读取 RAW / 分段 RAW、EWF、VMDK、VHD / VHDX、QCOW、VDI、DMG、AD1、TAR、ZIP、7z 等容器，并浏览其中支持的 NTFS、FAT / exFAT、ext、APFS 等文件系统。压缩容器可能使用读取器自行清理的临时文件，解包由读取器完成。[格式与读取器指南](https://github.com/LovelyTeams/LovelyCarve/blob/main/docs/formats/README.md)

Python API 专用于读取镜像和归档，文件内容以字节返回。案件管理、索引、搜索、加密卷解锁、阵列组装、外部卷、文件恢复、导出、挂载和插件分析请使用桌面版或 CLI。[桌面版使用指南](https://github.com/LovelyTeams/LovelyCarve/blob/main/docs/USAGE.md) · [CLI 手册](https://github.com/LovelyTeams/LovelyCarve/blob/main/docs/CLI.md)

读取到逻辑末尾时，结果会截短到实际可读范围。格式定义的空洞和未初始化区域按格式返回零。应有范围发生短读，或检材坏区被补零时，读取会报错。

| 异常 | 含义 |
| --- | --- |
| `OSError` | 读取器报告 I/O 或格式错误，也包括检出的坏区补零和意外短读 |
| `FileNotFoundError`、`PermissionError` | 读取器将失败归类为对象不存在或访问被拒绝，两者均为 `OSError` 的子类 |
| `ValueError`、`TypeError` | 参数值或类型无效；文件或镜像关闭后的读取会抛出 `ValueError` |

脚本可按异常类型处理失败。

## 从源码构建

在仓库根目录运行以下命令。源码构建需要 Rust、uv 和目标平台的编译依赖。[开发指南](https://github.com/LovelyTeams/LovelyCarve/blob/main/docs/DEVELOPMENT.md)

```sh
just develop-python
just test-python
just build-python
```

开发环境位于 `python/.venv`，wheel 输出到 `dist-pack/python`，可用 `uv pip install <wheel路径>` 安装到目标环境。配方默认由 uv 使用 Python 3.15，可通过环境变量 `LOVELYCARVE_PYTHON` 或 just 变量 `python_version` 切换解释器。绑定的格式和 lint 检查分别使用 `just fmt-python` 与 `just clippy-python`。

Windows 的 Python 扩展默认静态链接 `libssl_static.lib` 和 `libcrypto_static.lib`，采用与 Python 一致的动态 CRT。x64 默认从 OpenSSL 的 `lib/VC/x64/MD` 取库，构建 ARM64 时将 `OPENSSL_LIB` 指向 `lib/VC/arm64/MD`。可用 `OPENSSL_LIB` 和 `OPENSSL_LIBS` 覆盖目录与库名。

原生扩展通过 PyO3 使用 CPython 3.10 起的稳定 ABI，并以 Rust 后端的 `reader-library` 配置复用读取器。源码构建仍解析共享的 Cargo 依赖，扩展只链接读取所需的实现。
