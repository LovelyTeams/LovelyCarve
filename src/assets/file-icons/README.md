# 文件节点图标

`FileNodeIcon`（文件列表、目录树、预览标签页与详情面板）使用的单色 SVG 图标。图标名即文件名（不含扩展名），
文件名 → 图标的判定在 `src/utils/fileIcons.ts`；配色由文件类别决定，与图标无关。

文件里的 fill / stroke 颜色只表示「这里要画」：渲染时内联为 `<svg>`，绘制颜色统一换成 `currentColor`
（`src/utils/svgIcon.ts`），`<mask>` / `<clipPath>` 内的颜色保持原样。

## `zed/`

来自 [zed-industries/zed](https://github.com/zed-industries/zed) 提交
[`2f808b3c2de049ecfdbad0fdb26763cebfcaa3b4`](https://github.com/zed-industries/zed/tree/2f808b3c2de049ecfdbad0fdb26763cebfcaa3b4)，
文件内容未改动，许可为 GPL-3.0-or-later（全文见本目录 `LICENSE-GPL` 与仓库根目录 `THIRD_PARTY_NOTICES.md`）：

- `assets/icons/file_icons/`：默认图标主题引用的文件类型图标、`folder` / `folder_open` 与 `archive`；
- `assets/icons/`：`binary`、`envelope`、`table`、`link`、`history_rerun`。该目录的 `LICENSES` 声明其中部分图形源自
  Lucide（ISC，见 `THIRD_PARTY_NOTICES.md`）。

`src/utils/fileIcons.ts` 中的文件名 / 后缀表与图标键表取自同一提交的
`crates/theme/src/icon_theme.rs`（GPL-3.0-or-later），查找顺序按 `crates/file_icons/src/file_icons.rs`，
另加不区分大小写与取证类别的补充。

## 本目录

`hard_drive`（分区 / 逻辑卷）、`layers`（NTFS 命名数据流）、`unallocated`（未分配空间与分区间隙）按同一
16px 网格与 1.2 线宽绘制：前两者按 Lucide 的 `hard-drive` / `layers` 图形（ISC）缩放，`unallocated` 是
`zed/` 同源的虚线方框（Zed `text_snippet` 去掉文字行）。

更新上游图标时整目录替换 `zed/`，同步 `fileIcons.ts` 的表与上面的提交号；`src/components/display/fileIconSvg.test.ts`
检查随包图标与查找结果一一对应、换色后不残留占位颜色。
