// 二进制结构解析 IPC：原类型视图里 ELF / Mach-O / PE / Unix ar 文件的结构化报告。
// 后端按文件魔数自动分派（fs_binary_preview → core::binfmt），返回带 `kind` 标签的
// 联合。类型镜像 src-tauri/src/core/binfmt*.rs 的 Serialize 输出。
import { invokeCmd } from './client';
import type { ImageId, NodeId } from './ids';

// ───────────────────────── PE ─────────────────────────

export interface PeDataDirEntry {
  name: string;
  rva: number;
  size: number;
}

export interface PeSection {
  name: string;
  virtual_address: number;
  virtual_size: number;
  raw_offset: number;
  raw_size: number;
  characteristics: number;
  /** 形如 "r-x" */
  perms: string;
  is_code: boolean;
  /** Shannon 熵（0..8），null 表示节区无文件数据 */
  entropy: number | null;
}

export interface PeImport {
  dll: string;
  /** 延迟导入（Delay-load） */
  delayed: boolean;
  functions: string[];
  /** 实际函数总数；大于 functions.length 表示已截断 */
  total: number;
}

export interface PeExportEntry {
  ordinal: number;
  name: string | null;
  rva: number;
  forwarder: string | null;
}

export interface PeExports {
  dll_name: string | null;
  total: number;
  entries: PeExportEntry[];
}

export interface PeDebugEntry {
  kind: string;
  timestamp_utc: string | null;
  pdb_path: string | null;
  guid: string | null;
  age: number | null;
}

export interface PeVersionString {
  key: string;
  value: string;
}

export interface PeVersionInfo {
  file_version: string | null;
  product_version: string | null;
  strings: PeVersionString[];
  translations: string[];
}

export interface PeOverlay {
  offset: number;
  size: number;
}

export interface PeReport {
  format: string;
  machine: number;
  machine_label: string;
  timestamp: number;
  timestamp_utc: string | null;
  characteristics: number;
  characteristic_flags: string[];
  subsystem: number;
  subsystem_label: string;
  dll_characteristics: number;
  dll_characteristic_flags: string[];
  entry_point: number;
  entry_section: string | null;
  /** 十六进制字符串，如 "0x140000000" */
  image_base: string;
  size_of_image: number;
  size_of_headers: number;
  checksum: number;
  linker_version: string;
  os_version: string;
  subsystem_version: string;
  is_dll: boolean;
  is_dotnet: boolean;
  /** Security 目录非空（存在 Authenticode 签名块，未做验签） */
  is_signed: boolean;
  data_directories: PeDataDirEntry[];
  sections: PeSection[];
  imports: PeImport[];
  exports: PeExports | null;
  debug: PeDebugEntry[];
  version_info: PeVersionInfo | null;
  overlay: PeOverlay | null;
  parse_warnings: string[];
  file_size: number;
  truncated: boolean;
}

// ───────────────────────── ELF ─────────────────────────

export interface ElfSegment {
  ptype: string;
  /** 形如 "r-x" */
  perms: string;
  offset: number;
  /** 虚拟地址（十六进制字符串） */
  vaddr: string;
  filesz: number;
  memsz: number;
}

export interface ElfSection {
  name: string;
  sh_type: string;
  /** 形如 "a-x"（alloc / write / exec） */
  flags: string;
  /** 节地址（十六进制字符串） */
  addr: string;
  offset: number;
  size: number;
  /** Shannon 熵（0..8），null 表示 NOBITS 或无文件数据 */
  entropy: number | null;
}

export interface ElfSymbolSummary {
  dynsym_count: number;
  symtab_count: number;
  stripped: boolean;
  exported: string[];
  imported: string[];
  exported_truncated: boolean;
  imported_truncated: boolean;
}

export interface ElfReport {
  format: string;
  endianness: string;
  machine: number;
  machine_label: string;
  os_abi: number;
  os_abi_label: string;
  elf_type: string;
  is_pie: boolean;
  /** 入口地址（十六进制字符串） */
  entry: string;
  interpreter: string | null;
  soname: string | null;
  needed: string[];
  rpaths: string[];
  runpaths: string[];
  /** GNU build-id（十六进制） */
  build_id: string | null;
  security_flags: string[];
  program_headers: ElfSegment[];
  sections: ElfSection[];
  symbol_summary: ElfSymbolSummary;
  file_size: number;
  truncated: boolean;
  parse_warnings: string[];
}

// ──────────────────────── Mach-O ───────────────────────

export interface MachoArch {
  cpu_label: string;
  offset: number;
  size: number;
}

export interface MachoLoadCmd {
  name: string;
  count: number;
}

export interface MachoSection {
  name: string;
  segname: string;
  /** 节地址（十六进制字符串） */
  addr: string;
  size: number;
  offset: number;
  entropy: number | null;
}

export interface MachoSegment {
  name: string;
  /** 虚拟地址（十六进制字符串） */
  vmaddr: string;
  vmsize: number;
  fileoff: number;
  filesize: number;
  /** 初始保护权限，形如 "r-x" */
  initprot: string;
  /** 最大保护权限，形如 "rwx" */
  maxprot: string;
  nsects: number;
  sections: MachoSection[];
}

export interface MachoSymbolSummary {
  total: number;
  defined: number;
  undefined: number;
  stripped: boolean;
  truncated: boolean;
}

export interface MachoReport {
  format: string;
  is_fat: boolean;
  arches: MachoArch[];
  cpu: number;
  cpu_label: string;
  filetype: number;
  filetype_label: string;
  flags: string[];
  /** 入口地址（十六进制字符串） */
  entry: string;
  uuid: string | null;
  min_os: string | null;
  libs: string[];
  load_commands: MachoLoadCmd[];
  segments: MachoSegment[];
  symbol_summary: MachoSymbolSummary;
  /** 存在 LC_CODE_SIGNATURE */
  code_signature: boolean;
  /** 存在 LC_ENCRYPTION_INFO 且 cryptid≠0 */
  encrypted: boolean;
  file_size: number;
  truncated: boolean;
  parse_warnings: string[];
}

// ──────────────────────── Archive ──────────────────────

export interface ArchiveMember {
  name: string;
  size: number;
  /** 成员数据在归档内的文件偏移（十六进制字符串） */
  data_offset: string;
  symbol_count: number;
  member_format: string;
}

export interface ArchiveReport {
  format: string;
  member_count: number;
  total_symbols: number;
  members: ArchiveMember[];
  file_size: number;
  truncated: boolean;
  parse_warnings: string[];
}

// ─────────────────── 标签联合 + 命令 ───────────────────

/** 后端 `#[serde(tag = "kind")]` 内部标签联合：`kind` 与对应报告字段同层。 */
export type NativeReport =
  | ({ kind: 'elf' } & ElfReport)
  | ({ kind: 'macho' } & MachoReport)
  | ({ kind: 'pe' } & PeReport)
  | ({ kind: 'archive' } & ArchiveReport);

/** 解析镜像内文件的二进制结构（自动按魔数分派）。非受支持格式 reject，错误信息可直接展示。 */
export function fsBinaryPreview(args: { imageId: ImageId; nodeId: NodeId }): Promise<NativeReport> {
  return invokeCmd<NativeReport>('fs_binary_preview', args);
}
