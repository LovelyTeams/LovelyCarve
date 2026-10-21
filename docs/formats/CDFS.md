# ISO 9660 与原始光盘 BIN

LovelyCarve 使用 vendored `cdfs` 只读库浏览 ISO 9660 / Joliet / Rock Ridge。
普通 ISO 的 2048 字节逻辑数据可直接读取；`MODE1/2352` 原始光盘 BIN
由 `core::evidence::readers::optical_disc` 提供文件系统专用的逻辑投影。
布局参考 [libodraw 光盘 RAW 格式说明](https://github.com/libyal/libodraw/blob/main/documentation/Optical%20disc%20RAW%20format.asciidoc)。

## 打开与读取

将 `.bin` 或 `.iso` 作为镜像文件添加，无需先转换。识别依据为内容而非扩展名：
原始扇区的同步头、Mode 1 标记、BCD 地址与 ISO 9660 主卷描述符共同确认布局。
卷描述符的大小端几何、逻辑块大小及根目录范围均须有效。

原始 BIN 每个物理扇区为 2352 字节，文件系统仅使用偏移 16 之后的 2048 字节。
镜像信息报告 2352 字节物理扇区，分区几何覆盖完整源文件；文件树仍为 `CDFS`，
复用现有目录浏览、Joliet 名称、预览、哈希、索引和文件导出。
普通 ISO 保留既有识别和读取方式。

源镜像的大小、十六进制视图、整镜像哈希及 RAW 源导出仍对应原始 BIN，包含扇区头
和校验区；文件读取跳过这些字节，不生成临时整盘 ISO，也不替换检材身份。
每次读取校验所访问扇区的同步头、Mode 1 标记及连续 MSF 地址。坏头、非法 BCD、
来源短读、文件 extent 越过卷尾及截断扇区明确失败，不能把旧缓冲区内容返回为文件字节。
文件读取以主卷描述符声明的逻辑卷长度为界，不把轨道尾部填充作为文件内容。

普通文件支持直接逻辑范围读取，按文件长度裁剪，跨物理扇区时与完整读取、流式读取一致。
逻辑文件位置不伪装为连续的原始字节范围；现有源读故障审计沿用原始物理偏移。

## 限制

- 原始光盘路径当前支持一个从文件头开始、具有连续 MSF 地址的 `MODE1/2352` 数据轨。
- 不解析 CUE、CCD、TOC、多轨音频、混合模式、Mode 2 / XA、2448 字节含子通道扇区，
  也不以裸 UDF 作为 ISO 9660 打开。已匹配 RAW 同步头但模式不支持时明确报错。
- 不解码、校验或修复 CD EDC / ECC；不会根据这些字段进行纠错或将错误数据默认为可靠。
- ISO 9660 特性边界仍由当前 `cdfs` 实现决定；该适配不增加 ISO 多 extent、交错文件
  或其他未实现布局的支持。

## 验证

```text
just test raw_cd_mode1 --lib
just check
just clippy
just check-examples
just test-cli
```

合成测试逐字段构造 ISO 9660 + Joliet 与原始扇区，核对中文名称、跨扇区范围读、
全量与流式内容、文件导出及原始检材字节。损坏测试覆盖截断、几何超界、坏扇区头、
非法 BCD、虚报长度导致的短读以及越界文件 extent。

可选外部回归设置 `LOVELYCARVE_RAW_CD_IMAGE=<镜像路径>` 后运行
`just test raw_cd_mode1_real_image --lib -- --ignored`，要求卷根下有 `data` 目录及非空文件。
真实检材、目录清单和哈希不提交到仓库。
