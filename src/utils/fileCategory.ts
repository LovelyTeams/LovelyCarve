/** 扩展名 → 文件类别。
 *
 *  两个消费方共用同一套扩展名表：
 *  - DiskFileDetail 的预览分类（markdown/image/pdf/audio/video/code）决定
 *    「原类型视图」怎么渲染；
 *  - DiskFileList 的图标分类（iconCategoryOf）决定列表里名称前的图标与配色，
 *    额外覆盖取证关心但不可预览的类别（压缩包/可执行/数据库/邮件等）。
 *  分类只看扩展名，不嗅探内容——列表里几十万行不可能逐个读文件头。 */

export const MD_EXT = new Set(['md', 'markdown', 'mdown', 'mkd']);
export const IMAGE_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'ico', 'avif', 'svg']);
export const PDF_EXT = new Set(['pdf']);
export const AUDIO_EXT = new Set(['mp3', 'wav', 'ogg', 'flac', 'm4a', 'aac']);
export const VIDEO_EXT = new Set(['mp4', 'webm', 'mov', 'mkv']);
export const CODE_EXT = new Set([
  'txt', 'text', 'log', 'conf', 'cfg', 'ini', 'env', 'properties', 'csv', 'tsv',
  'json', 'json5', 'jsonc', 'xml', 'yaml', 'yml', 'toml', 'plist',
  'js', 'mjs', 'cjs', 'jsx', 'ts', 'mts', 'cts', 'tsx', 'vue', 'svelte',
  'py', 'pyi', 'rb', 'php', 'pl', 'lua', 'r',
  'rs', 'go', 'c', 'h', 'cc', 'cpp', 'cxx', 'hpp', 'hxx', 'cs', 'java', 'kt', 'kts',
  'swift', 'm', 'mm', 'scala', 'clj', 'ex', 'exs', 'erl', 'hs', 'dart',
  'sh', 'bash', 'zsh', 'fish', 'ps1', 'psm1', 'bat', 'cmd',
  'sql', 'graphql', 'gql', 'proto',
  'html', 'htm', 'css', 'scss', 'sass', 'less', 'styl',
  'dockerfile', 'makefile', 'cmake', 'gradle', 'gitignore', 'gitattributes', 'editorconfig',
]);

/** JSON / YAML / XML——预览侧走 jsoncrack 式节点图可视化（图/树）。CODE_EXT 的
 *  子集，但分类时先于 CODE_EXT 判定，从「按文本渲染」升级为「按结构渲染」。
 *  toml/plist 不在此列，仍按代码文本渲染；csv/tsv 见 CSV_EXT 的表格视图。 */
export const STRUCTURED_EXT = new Set(['json', 'json5', 'jsonc', 'yaml', 'yml', 'xml']);

/** CSV / TSV——预览侧走表格视图（CsvViewer）。CODE_EXT 的子集，但分类时先于
 *  CODE_EXT 判定，从「按文本渲染」升级为「按表格渲染」。 */
export const CSV_EXT = new Set(['csv', 'tsv']);

export const ARCHIVE_EXT = new Set([
  'zip', 'rar', '7z', 'gz', 'bz2', 'xz', 'zst', 'lz4', 'tar', 'tgz', 'tbz2', 'txz',
  'cab', 'iso', 'wim', 'arj', 'jar', 'war', 'apk', 'ipa',
]);
/** Windows PE（MZ/PE 头）——预览侧走二进制结构解析视图。EXECUTABLE_EXT 的
 *  子集：elf/so/dylib 等非 PE 可执行格式不在此列（但同样进二进制视图）。 */
export const PE_EXT = new Set(['exe', 'dll', 'sys', 'scr', 'ocx', 'cpl', 'drv', 'efi', 'mui']);
/** Unix ar 归档（.a 静态库 / MSVC 导入库 / .deb 控制档）——预览侧走二进制结构
 *  视图（成员列表）。Magika 无 ar 标签，归类靠扩展名 + `!<arch>\n` 字节嗅探。 */
export const AR_EXT = new Set(['a', 'ar', 'lib']);
/** GGUF（llama.cpp / ggml 大模型权重容器）——预览侧走 GGUF 结构解析视图
 *  （头部 / 元数据 / 张量布局 / 量化 / 参数量）。 */
export const GGUF_EXT = new Set(['gguf']);
/** ZIP 压缩包——预览侧走内部结构目录树 + 成员内容预览（ZipViewer，经
 *  fs_zip_preview 解析中央目录、fs_zip_member_read 解压单成员）。仅纯 `.zip`：
 *  jar/apk/war/ipa 等 ZIP 容器仍由 Magika 识别 / PK 魔数路径进入该视图，但不放进
 *  这里——它们可能使用未编译支持的压缩方法。docx/xlsx/pptx 虽也是 ZIP，但分类时
 *  先命中 RICH_EXT（Flyfish 渲染视图）。 */
export const ZIP_EXT = new Set(['zip']);
export const EXECUTABLE_EXT = new Set([
  'exe', 'dll', 'sys', 'com', 'scr', 'msi', 'drv', 'ocx', 'cpl', 'efi', 'mui',
  'elf', 'so', 'dylib', 'bin', 'o', 'ko',
]);
export const DOCUMENT_EXT = new Set([
  'doc', 'docx', 'odt', 'rtf', 'ppt', 'pptx', 'odp', 'pages', 'key', 'wps',
]);
export const SPREADSHEET_EXT = new Set(['xls', 'xlsx', 'xlsm', 'xlsb', 'ods', 'et']);
export const DATABASE_EXT = new Set([
  'db', 'db3', 'sqlite', 'sqlite3', 'mdb', 'accdb', 'edb', 'frm', 'ibd', 'realm',
]);
/** SQLite 库——预览侧走 SQLite 表浏览视图。DATABASE_EXT 的子集：
 *  mdb/edb/frm 等非 SQLite 库不在此列。`.db` 有歧义（也可能是其他格式），
 *  预览组件会按文件头魔数兜底报错而不是渲染乱码。 */
export const SQLITE_EXT = new Set(['db', 'db3', 'sqlite', 'sqlite3']);
/** Realm 移动端对象库——预览侧走 Realm 表浏览视图（vendored realm-db-reader，
 *  内存读取，不落临时文件）。 */
export const REALM_EXT = new Set(['realm']);
export const EMAIL_EXT = new Set(['eml', 'msg', 'pst', 'ost', 'mbox']);

/** Flyfish File Viewer（@file-viewer/vue3-full）「rich」预览——预览侧交给浏览器
 *  原生渲染库（RichPreviewViewer），覆盖文档/办公/邮件/非 zip 压缩包/CAD/3D/图表
 *  等本仓库无原生解析器的格式。集合按库 registry（DEFAULT_RENDERER_DEFINITIONS）
 *  逐项核对，并剔除：本仓库保留原生视图的格式（zip 系→ZipViewer、csv/tsv→表格、
 *  md/代码/文本/结构化→文本类、图片/音视频→媒体视图、PE/ELF/ar→二进制视图、
 *  sqlite/realm/gguf→各自结构视图）与库不支持的格式（pst/ost、wim、
 *  arj、ipa、lz4、vsdx、wps/wpt/et/dps、mmd/puml/plantuml 等文本图表源码）。 */
export const RICH_EXT = new Set([
  // Word / 办公文档（含旧版二进制与 ODF）
  'doc', 'docx', 'docm', 'dot', 'dotx', 'dotm', 'rtf', 'odt', 'odp', 'pages',
  // Excel（csv/tsv 仍走 CsvViewer；.et WPS 私有格式库不支持）
  'xls', 'xlsx', 'xlsm', 'xlsb', 'xlt', 'xltx', 'xltm', 'ods', 'fods', 'numbers',
  // PowerPoint
  'ppt', 'pptx', 'pptm', 'pot', 'potx', 'potm', 'pps', 'ppsx', 'ppsm',
  // 版式文档
  'pdf', 'ofd', 'epub',
  // 邮件（pst/ost 库不支持，排除）
  'eml', 'msg', 'mbox',
  // 非 zip 压缩包（zip/jar/war/apk/ear 仍走 ZipViewer；wim/arj/ipa/lz4/ar 排除）
  '7z', 'rar', 'tar', 'gz', 'gzip', 'tgz', 'bz2', 'bzip2', 'tbz', 'tbz2',
  'xz', 'txz', 'lzma', 'zst', 'tzst', 'cab', 'cpio', 'iso', 'xar', 'lha',
  'lzh', 'cbz', 'cbr', 'zipx',
  // CAD（vsd/vsdx 库不支持，排除）
  'dwg', 'dxf', 'dwf', 'dwfx', 'xps',
  // 3D / 模型
  'stl', 'obj', 'gltf', 'glb', 'fbx', 'dae', '3ds', '3mf', 'amf', 'ply',
  'pcd', 'vrml', 'wrl', 'vtk', 'vtp', 'xyz', 'usd', 'usda', 'usdc', 'usdz',
  'kmz', 'step', 'stp', 'iges', 'igs', 'ifc', '3dm', 'brep',
  // 图表 / 脑图（文本型图表源码 mmd/mermaid/puml 仍走文本视图）
  'drawio', 'dio', 'excalidraw', 'xmind',
  // Typst
  'typ', 'typst',
  // 数据 / 地理 / EDA 等（geojson/kml/gpx 是文本，保留走结构化/代码视图）
  'shp', 'psd', 'ai', 'eps', 'webarchive', 'parquet', 'avro', 'gds', 'oas',
  'oasis', 'olb', 'dra', 'dsn',
]);

export type IconCategory =
  | 'image'
  | 'video'
  | 'audio'
  | 'pdf'
  | 'document'
  | 'spreadsheet'
  | 'code'
  | 'archive'
  | 'executable'
  | 'database'
  | 'email'
  | 'other';

/** 文件分类条用的类别元数据：顺序即条上展示顺序，label 为中文短名。
 *  'other' 排在最后兜底。与 iconCategoryOf 同一套 IconCategory，列表图标 /
 *  配色（FileNodeIcon 的 cat-* 类）与分类筛选共用，不再各列一份。 */
export const CATEGORY_META: { key: IconCategory; label: string }[] = [
  { key: 'image', label: '图片' },
  { key: 'video', label: '视频' },
  { key: 'audio', label: '音频' },
  { key: 'pdf', label: 'PDF' },
  { key: 'document', label: '文档' },
  { key: 'spreadsheet', label: '表格' },
  { key: 'code', label: '代码/文本' },
  { key: 'archive', label: '压缩包' },
  { key: 'executable', label: '可执行' },
  { key: 'database', label: '数据库' },
  { key: 'email', label: '邮件' },
  { key: 'other', label: '其他' },
];

/** 文件名 → 类别。扩展名取最后一段、转小写后交给 iconCategoryOf；无扩展名
 *  归 'other'。与 FileNodeIcon 的 category 计算同口径（目录不在此函数职责内，
 *  调用方自行排除）。 */
export function categoryOfName(name: string): IconCategory {
  const idx = name.lastIndexOf('.');
  const ext = idx <= 0 || idx === name.length - 1 ? '' : name.slice(idx + 1);
  return iconCategoryOf(ext.toLowerCase());
}

/** 列表图标分类。csv/tsv 在预览侧走表格视图，图标按表格更直观，
 *  所以电子表格判定先于 CODE_EXT。入参须已转小写。 */
export function iconCategoryOf(extLower: string): IconCategory {
  if (!extLower) return 'other';
  if (IMAGE_EXT.has(extLower)) return 'image';
  if (VIDEO_EXT.has(extLower)) return 'video';
  if (AUDIO_EXT.has(extLower)) return 'audio';
  if (PDF_EXT.has(extLower)) return 'pdf';
  if (MD_EXT.has(extLower) || DOCUMENT_EXT.has(extLower)) return 'document';
  if (SPREADSHEET_EXT.has(extLower) || CSV_EXT.has(extLower)) return 'spreadsheet';
  if (ARCHIVE_EXT.has(extLower) || AR_EXT.has(extLower)) return 'archive';
  if (EXECUTABLE_EXT.has(extLower)) return 'executable';
  if (DATABASE_EXT.has(extLower)) return 'database';
  if (EMAIL_EXT.has(extLower)) return 'email';
  if (CODE_EXT.has(extLower)) return 'code';
  return 'other';
}
