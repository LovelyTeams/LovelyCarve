/** 文件节点图标的形状：文件名 → 图标名。
 *
 *  图标名即 `src/assets/file-icons/` 下 SVG 的文件名（不含扩展名）。图标只管形状，
 *  配色仍由 iconCategoryOf 的类别决定（FileNodeIcon 的 cat-* 类）。
 *
 *  判定顺序：
 *  1. 可执行、压缩包、表格、邮件、数据库这五个取证类别直接取类别图标；
 *  2. 按名称表查：先看整个文件名，再依次去掉最左一段看剩下的多段后缀
 *     （`docker-compose.yml` → `yml`），最后看第一个点之前的主名（`Dockerfile.prod`）；
 *  3. 名称表没有认出的按类别兜底，其他文件用通用文件图标。
 *  查找不区分大小写：检材里 `SETUP.EXE`、`IMG_0001.JPG` 很常见。
 *
 *  名称表与图标键表取自 Zed 默认图标主题（GPL-3.0-or-later），出处与许可见
 *  THIRD_PARTY_NOTICES.md；EXTRA_SUFFIXES_BY_ICON_KEY 是本仓库补充的后缀。 */

import { categoryOfName, type IconCategory } from './fileCategory';

/** `src/assets/file-icons/` 下的 SVG 文件名（不含扩展名）。 */
export type FileIconName = string;

export type Associations = ReadonlyArray<readonly [iconKey: string, names: readonly string[]]>;

/** 整个文件名或第一个点之前的主名。 */
export const STEMS_BY_ICON_KEY: Associations = [
  ['docker', ['Containerfile', 'Dockerfile', '.dockerignore']],
  ['ruby', ['Podfile']],
  ['heroku', ['Procfile']],
];

/** 后缀（可含点的多段后缀，也可以是整个文件名）。 */
export const SUFFIXES_BY_ICON_KEY: Associations = [
  ['astro', ['astro']],
  ['audio', ['aac', 'flac', 'm4a', 'mka', 'mp3', 'ogg', 'opus', 'wav', 'wma', 'wv']],
  ['backup', ['bak']],
  ['ballerina', ['bal']],
  ['bicep', ['bicep']],
  ['bun', ['lockb']],
  ['c', ['c', 'h']],
  ['cairo', ['cairo']],
  ['code', ['handlebars', 'metadata', 'rkt', 'scm']],
  ['coffeescript', ['coffee']],
  ['cpp', ['c++', 'h++', 'cc', 'cpp', 'cppm', 'cxx', 'hh', 'hpp', 'hxx', 'inl', 'ixx']],
  ['crystal', ['cr', 'ecr']],
  ['csharp', ['cs']],
  ['csproj', ['csproj']],
  ['css', ['css', 'pcss', 'postcss']],
  ['cue', ['cue']],
  ['dart', ['dart']],
  ['diff', ['diff']],
  ['docker', ['docker-compose.yml', 'docker-compose.yaml', 'compose.yml', 'compose.yaml']],
  ['document', ['doc', 'docx', 'mdx', 'odp', 'ods', 'odt', 'pdf', 'ppt', 'pptx', 'rtf', 'txt', 'xls', 'xlsx']],
  ['editorconfig', ['editorconfig']],
  ['elixir', ['eex', 'ex', 'exs', 'heex', 'leex', 'neex']],
  ['elm', ['elm']],
  ['erlang', ['Emakefile', 'app.src', 'erl', 'escript', 'hrl', 'rebar.config', 'xrl', 'yrl']],
  ['eslint', [
    'eslint.config.cjs', 'eslint.config.cts', 'eslint.config.js', 'eslint.config.mjs',
    'eslint.config.mts', 'eslint.config.ts', 'eslintrc', 'eslintrc.js', 'eslintrc.json',
  ]],
  ['font', ['otf', 'ttf', 'woff', 'woff2']],
  ['fsharp', ['fs']],
  ['fsproj', ['fsproj']],
  ['gitlab', ['gitlab-ci.yml', 'gitlab-ci.yaml']],
  ['gleam', ['gleam']],
  ['go', ['go', 'mod', 'work']],
  ['graphql', ['gql', 'graphql', 'graphqls']],
  ['haskell', ['hs']],
  ['hcl', ['hcl']],
  ['helm', [
    'helmfile.yaml', 'helmfile.yml', 'Chart.yaml', 'Chart.yml', 'Chart.lock', 'values.yaml',
    'values.yml', 'requirements.yaml', 'requirements.yml', 'tpl',
  ]],
  ['html', ['htm', 'html']],
  ['image', [
    'avif', 'bmp', 'gif', 'heic', 'heif', 'ico', 'j2k', 'jfif', 'jp2', 'jpeg', 'jpg',
    'jxl', 'png', 'psd', 'qoi', 'svg', 'tiff', 'webp',
  ]],
  ['ipynb', ['ipynb']],
  ['java', ['java']],
  ['javascript', ['cjs', 'js', 'mjs']],
  ['json', ['json', 'jsonc']],
  ['julia', ['jl']],
  ['kdl', ['kdl']],
  ['kotlin', ['kt']],
  ['lock', ['lock']],
  ['log', ['log']],
  ['lua', ['lua']],
  ['luau', ['luau']],
  ['markdown', ['markdown', 'md']],
  ['metal', ['metal']],
  ['nim', ['nim', 'nims', 'nimble']],
  ['nix', ['nix']],
  ['ocaml', ['ml', 'mli', 'mlx']],
  ['odin', ['odin']],
  ['php', ['php']],
  ['prettier', [
    'prettier.config.cjs', 'prettier.config.js', 'prettier.config.mjs', 'prettierignore',
    'prettierrc', 'prettierrc.cjs', 'prettierrc.js', 'prettierrc.json', 'prettierrc.json5',
    'prettierrc.mjs', 'prettierrc.toml', 'prettierrc.yaml', 'prettierrc.yml',
  ]],
  ['prisma', ['prisma']],
  ['puppet', ['pp']],
  ['python', ['py']],
  ['r', ['r', 'R']],
  ['react', ['cjsx', 'ctsx', 'jsx', 'mjsx', 'mtsx', 'tsx']],
  ['roc', ['roc']],
  ['ruby', ['rb']],
  ['rust', ['rs']],
  ['sass', ['sass', 'scss']],
  ['scala', ['scala', 'sc']],
  ['settings', ['conf', 'ini']],
  ['solidity', ['sol']],
  ['storage', [
    'accdb', 'csv', 'dat', 'db', 'dbf', 'dll', 'fmp', 'fp7', 'frm', 'gdb', 'ib', 'ldf',
    'mdb', 'mdf', 'myd', 'myi', 'pdb', 'psv', 'RData', 'rdata', 'sav', 'sdf', 'sql',
    'sqlite', 'ssv', 'tsv',
  ]],
  ['stylelint', [
    'stylelint.config.cjs', 'stylelint.config.js', 'stylelint.config.mjs', 'stylelintignore',
    'stylelintrc', 'stylelintrc.cjs', 'stylelintrc.js', 'stylelintrc.json', 'stylelintrc.mjs',
    'stylelintrc.yaml', 'stylelintrc.yml',
  ]],
  ['surrealql', ['surql']],
  ['svelte', ['svelte']],
  ['swift', ['swift']],
  ['tcl', ['tcl']],
  ['template', ['hbs', 'plist', 'xml']],
  ['terminal', [
    'bash', 'bash_aliases', 'bash_login', 'bash_logout', 'bash_profile', 'bashrc', 'brushrc',
    'fish', 'nu', 'profile', 'ps1', 'sh', 'zlogin', 'zlogout', 'zprofile', 'zsh', 'zsh_aliases',
    'zsh_histfile', 'zsh_history', 'zshenv', 'zshrc',
  ]],
  ['terraform', ['tf', 'tfvars']],
  ['toml', ['toml']],
  ['typescript', ['cts', 'mts', 'ts']],
  ['v', ['v', 'vsh', 'vv']],
  ['vcs', [
    'COMMIT_EDITMSG', 'EDIT_DESCRIPTION', 'MERGE_MSG', 'NOTES_EDITMSG', 'TAG_EDITMSG',
    'gitattributes', 'gitignore', 'gitkeep', 'gitmodules',
  ]],
  ['vbproj', ['vbproj']],
  ['video', ['avi', 'm4v', 'mkv', 'mov', 'mp4', 'webm', 'wmv']],
  ['vs_sln', ['sln']],
  ['vs_suo', ['suo']],
  ['vue', ['vue']],
  ['vyper', ['vy', 'vyi']],
  ['wgsl', ['wgsl']],
  ['yaml', ['yaml', 'yml']],
  ['zig', ['zig']],
];

/** 检材里常见、上表没有的后缀，接到已有的图标键上（不覆盖上表）。 */
export const EXTRA_SUFFIXES_BY_ICON_KEY: Associations = [
  ['document', ['text']],
  ['json', ['json5']],
  ['lock', ['kdbx', 'kdb', 'keyx']],
  ['kotlin', ['kts']],
  ['python', ['pyi']],
  ['settings', ['cfg', 'env', 'properties']],
  ['terminal', ['bat', 'cmd', 'psm1']],
];

/** 图标键 → 图标。没有条目的键（backup）不出图标，查找继续往下走。 */
export const ICON_BY_KEY: Readonly<Record<string, FileIconName>> = {
  astro: 'astro',
  audio: 'audio',
  ballerina: 'ballerina',
  bicep: 'file',
  bun: 'bun',
  c: 'c',
  cairo: 'cairo',
  code: 'code',
  coffeescript: 'coffeescript',
  cpp: 'cpp',
  crystal: 'file',
  csharp: 'file',
  csproj: 'file',
  css: 'css',
  cue: 'file',
  dart: 'dart',
  diff: 'diff',
  docker: 'docker',
  document: 'book',
  editorconfig: 'editorconfig',
  elixir: 'elixir',
  elm: 'elm',
  erlang: 'erlang',
  eslint: 'eslint',
  font: 'font',
  fsharp: 'fsharp',
  fsproj: 'file',
  gitlab: 'gitlab',
  gleam: 'gleam',
  go: 'go',
  graphql: 'graphql',
  haskell: 'haskell',
  hcl: 'hcl',
  helm: 'helm',
  heroku: 'heroku',
  html: 'html',
  image: 'image',
  ipynb: 'jupyter',
  java: 'java',
  javascript: 'javascript',
  json: 'code',
  julia: 'julia',
  kdl: 'kdl',
  kotlin: 'kotlin',
  lock: 'lock',
  log: 'info',
  lua: 'lua',
  luau: 'luau',
  markdown: 'book',
  metal: 'metal',
  nim: 'nim',
  nix: 'nix',
  ocaml: 'ocaml',
  odin: 'odin',
  php: 'php',
  prettier: 'prettier',
  prisma: 'prisma',
  puppet: 'puppet',
  python: 'python',
  r: 'r',
  react: 'react',
  roc: 'roc',
  ruby: 'ruby',
  rust: 'rust',
  sass: 'sass',
  scala: 'scala',
  settings: 'settings',
  solidity: 'file',
  storage: 'database',
  stylelint: 'javascript',
  surrealql: 'surrealql',
  svelte: 'html',
  swift: 'swift',
  tcl: 'tcl',
  template: 'html',
  terminal: 'terminal',
  terraform: 'terraform',
  toml: 'toml',
  typescript: 'typescript',
  v: 'v',
  vbproj: 'file',
  vcs: 'git',
  video: 'video',
  vs_sln: 'file',
  vs_suo: 'file',
  vue: 'vue',
  vyper: 'vyper',
  wgsl: 'wgsl',
  yaml: 'yaml',
  zig: 'zig',
};

/** 类别图标：前五个取证类别先于名称表，其余在名称表没有认出时兜底。 */
const CATEGORY_ICON: Readonly<Record<IconCategory, FileIconName>> = {
  executable: 'binary',
  archive: 'archive',
  spreadsheet: 'table',
  email: 'envelope',
  database: 'database',
  image: 'image',
  video: 'video',
  audio: 'audio',
  pdf: 'book',
  document: 'book',
  code: 'code',
  other: 'file',
};

const CATEGORY_FIRST: ReadonlySet<IconCategory> = new Set(['executable', 'archive', 'spreadsheet', 'email', 'database']);

/** 不按文件名判定的节点：目录、符号链接与取证视图里的特殊节点。 */
export const NODE_ICON = {
  folder: 'folder',
  folderOpen: 'folder_open',
  symlink: 'link',
  partition: 'hard_drive',
  unallocated: 'unallocated',
  container: 'docker',
  snapshot: 'history_rerun',
  dataStream: 'layers',
} as const satisfies Record<string, FileIconName>;

function keysByName(...tables: Associations[]): Map<string, string> {
  const keys = new Map<string, string>();
  for (const table of tables) {
    for (const [key, names] of table) {
      for (const name of names) {
        const lower = name.toLowerCase();
        if (!keys.has(lower)) keys.set(lower, key);
      }
    }
  }
  return keys;
}

const STEM_KEYS = keysByName(STEMS_BY_ICON_KEY);
const SUFFIX_KEYS = keysByName(SUFFIXES_BY_ICON_KEY, EXTRA_SUFFIXES_BY_ICON_KEY);

function iconOfPart(part: string): FileIconName | null {
  const key = STEM_KEYS.get(part) ?? SUFFIX_KEYS.get(part);
  return key === undefined ? null : ICON_BY_KEY[key] ?? null;
}

/** 名称表查找；入参须已转小写。没有认出时返回 null。 */
function iconOfLowerName(lowerName: string): FileIconName | null {
  const whole = iconOfPart(lowerName);
  if (whole) return whole;
  let rest = lowerName;
  let firstStem: string | null = null;
  for (let dot = rest.indexOf('.'); dot >= 0; dot = rest.indexOf('.')) {
    firstStem ??= rest.slice(0, dot);
    rest = rest.slice(dot + 1);
    const icon = iconOfPart(rest);
    if (icon) return icon;
  }
  return firstStem ? iconOfPart(firstStem) : null;
}

/** 文件（非目录）的图标。category 缺省时按文件名现算，与 categoryOfName 同口径。 */
export function fileIconOf(name: string, category: IconCategory = categoryOfName(name)): FileIconName {
  if (CATEGORY_FIRST.has(category)) return CATEGORY_ICON[category];
  return iconOfLowerName(name.toLowerCase()) ?? CATEGORY_ICON[category];
}

/** 上面各表可能给出的全部图标名；随包的 SVG 与它一一对应（测试保证）。 */
export const FILE_ICON_NAMES: ReadonlySet<FileIconName> = new Set([
  ...Object.values(ICON_BY_KEY),
  ...Object.values(CATEGORY_ICON),
  ...Object.values(NODE_ICON),
]);
