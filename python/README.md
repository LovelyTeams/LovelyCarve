# lovelycarve

使用 PyO3 将现有 Rust 镜像、文件系统及归档读取器提供给 Python。包名与导入名均为 `lovelycarve`。只提供只读读取能力，不解析文件内容，不提供案件、任务、索引、搜索、雕刻、导出、挂载或插件 API，也不初始化桌面或 CLI 运行时。

## 安装与验证

在仓库根目录操作，需要 Rust、uv 及后端原有的编译依赖；Windows 的 OpenSSL 配置沿用 [开发指南](../docs/DEVELOPMENT.md)。默认使用 uv 管理的 Python 3.15，可用 `LOVELYCARVE_PYTHON` 或 just 同名变量 `python_version` 指定其他解释器。

```powershell
just develop-python
just test-python
just build-python
```

开发环境位于 `python/.venv`。wheel 输出到 `dist-pack/python`，可用 `uv pip install <wheel路径>` 安装到目标环境。PyO3 使用 Python 3.10 起的稳定 ABI；实际平台兼容性以对应平台构建和测试为准。`just fmt-python` 与 `just clippy-python` 检查绑定的 Rust 代码。

Windows Python 配方默认链接 OpenSSL `lib/VC/x64/MD` 下的 `libssl_static.lib` / `libcrypto_static.lib`，与 Python / Rust 的动态 CRT 匹配，并避免 wheel 依赖宿主 PATH 中的 OpenSSL DLL；仍使用开发指南中的 OpenSSL 路径变量，可通过 `OPENSSL_LIB` 覆盖目录、`OPENSSL_LIBS` 覆盖库名。

## 使用

```python
from lovelycarve import Image

with Image("disk.E01") as image:
    print(image.info.source_format, image.size)
    print(image.partitions)
    print(image.read(0, 512))
    for root in image.children():
        print(root.id, root.name)
        for entry in image.children(root):
            print(entry.name, entry.kind, entry.size, entry.timestamps)
            if entry.is_file:
                with image.open(entry) as file:
                    print(file.read(32))
                    file.seek(10)
                    buffer = bytearray(16)
                    file.readinto(buffer)
                break
```

`Image.children()` 返回卷或归档的入口节点；将返回的 `Node` 或其 `id` 传给 `children`、`stat`、`open` 和 `read_file`。节点 ID 是当前镜像内部的定位值，不是宿主路径，不应自行拼接。目录列表使用轻量读取策略，不执行 NTFS 删除记录扫描，也不额外显示未分配空间节点。

`Image.read(offset, size)` 读取镜像逻辑字节，单次最多 64 MiB，越过逻辑 EOF 的部分截短；归档返回容器本身的字节，文件内容通过节点读取。`Image.read_file(node, offset=0, size=-1)` 返回文件字节，默认读取到 EOF；读取大文件请使用 `Image.open(node)` 分块读取。

`ImageFile` 实现只读 `io.RawIOBase`：`read`、`readinto`、`seek`、`tell` 和上下文管理；需要缓冲时可自行包装 `io.BufferedReader`。关闭文件不关闭镜像，关闭镜像后原文件对象不能继续读取。已有外部缓冲器可能仍持有之前读入的字节。共享文件对象的游标操作需要调用方串行化；不同文件对象拥有独立游标。

`Image.stat(node)` 返回文件大小、inode、mode 与可用的数据段映射。时间戳保留读取器返回的字符串和缺失值，不额外推断时区。原生打开、列目录及读取操作释放 GIL；已开始的原生读取可以在另一个线程关闭镜像后完成。

## 格式与边界

复用 `DiskImage` / `EvidenceSession` 的自动识别和原生文件系统分派，例如 RAW / 分段 RAW、EWF、VMDK、VHD / VHDX、QCOW、VDI、DMG、AD1、TAR、ZIP、7Z，以及其支持的 NTFS、FAT / exFAT、ext、APFS 等文件系统。各格式的范围、损坏恢复与限制见 [格式文档](../docs/formats/README.md)。绑定本身不新增解密、阵列组装、外部卷或文件内容解析接口，不能通过它解锁加密卷。

所有证据由现有读取器只读打开；压缩容器仍可能使用读取器自行清理的临时解压文件。不依赖外部解包命令。底层尚未提供随机访问的格式在 `seek` 后可能需要从头流式读取或解压。

读取失败抛出 `OSError`；检测到坏区补零或应有字节范围发生短读时同样失败，不能把缺失源字节当作真实内容返回。格式定义的空洞和未初始化区域仍按格式返回零。参数错误使用 `ValueError` / `TypeError`；关闭后的 I/O 使用 `ValueError`。

Python 扩展通过 Rust 后端的 `reader-library` 库配置复用读取器；该配置不编译应用、命令、案件、索引、分析、预览、搜索和插件运行模块。当前 Cargo 依赖声明仍与后端共用，源码构建会处理其依赖图，但扩展只链接读取路径需要的实现。此配置用于 `--lib` 构建；默认 CLI、桌面及后端测试配置保持完整功能。Python 公共 API 仅限上述读取功能。
