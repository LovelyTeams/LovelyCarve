//! The command set. Reading inside evidence uses the verbs people already
//! know (`ls`, `cat`, `stat`, `find`, `grep`…); everything else is
//! `<noun> <verb>`. Every command is a typed clap/serde value: the same value
//! is parsed from argv, sent to a case backend and run there.
pub mod analysis;
pub mod browse;
pub mod carve;
pub mod case;
pub mod crypto;
pub mod evidence;
pub mod find;
pub mod inspect;
pub mod plugin;
pub mod raid;
pub mod read;
pub mod search;
pub mod system;
pub mod tasks;
pub mod timeline;

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    exec::{Exec, RequestOptions},
    types::{parse_host_path, HostPath, Secret},
    CliError, Output,
};

pub use case::CaseCommand;
pub use system::{BatchArgs, CompletionArgs, RpcArgs, SchemaArgs};

/// What the command line holds.
#[derive(Parser, Debug)]
#[command(
    name = "lovelycarve-cli",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    subcommand_required = false
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: Globals,
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Options every command takes.
#[derive(Args, Debug, Clone, Default)]
pub struct Globals {
    /// 案件目录；默认取 LOVELYCARVE_CASE，或当前目录及其上级中的案件
    #[arg(short = 'C', long = "case", global = true, value_parser = parse_host_path, value_name = "目录")]
    pub case: Option<HostPath>,
    /// 先打开这份检材（可重复）；别名取文件名
    #[arg(long = "image", global = true, value_parser = parse_host_path, value_name = "检材路径")]
    pub images: Vec<HostPath>,
    /// 文件时间的显示时区：auto、evidence、case、UTC、IANA 名称或 ±HH:MM
    #[arg(long, global = true, allow_hyphen_values = true, value_parser = parse_zone, value_name = "时区")]
    pub tz: Option<String>,
    /// 输出 JSON（文件内容为十六进制）；--json=字段1,字段2 只保留列表条目的这些字段
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "", value_name = "字段")]
    pub json: Option<String>,
    /// 不输出进度等非必要信息
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,
    /// 生命周期与进度事件以 JSONL 写到标准错误
    #[arg(long, global = true)]
    pub events: bool,
    /// 只显示解析后的请求，不执行
    #[arg(long, global = true)]
    pub dry_run: bool,
    /// 确认删除或丢弃数据的操作
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    /// 显示帮助
    #[arg(short = 'h', long, global = true)]
    pub help: bool,
    /// 显示版本
    #[arg(short = 'V', long, global = true)]
    pub version: bool,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Command {
    /// 案件：状态、创建、后台进程、锁定与删除
    Case(case::CaseArgs),
    /// 检材：打开、列出、查看、哈希、导出与移除
    Evidence(evidence::EvidenceArgs),
    /// 检材中的分区与卷：文件系统、加密与索引状态
    Volumes(browse::VolumesArgs),
    /// 列出目录
    Ls(browse::LsArgs),
    /// 递归列出目录树
    Tree(browse::TreeArgs),
    /// 文件或目录的属性
    Stat(browse::StatArgs),
    /// 输出文件内容（原始字节）
    Cat(read::CatArgs),
    /// 读取文本：识别编码并转为 UTF-8，分段读取
    Text(read::TextArgs),
    /// 提取可打印字符串
    Strings(read::StringsArgs),
    /// 计算文件哈希
    Hash(read::HashArgs),
    /// 识别文件内容类型（不看扩展名）
    Identify(read::IdentifyArgs),
    /// 列出命名数据流、资源分叉与扩展属性
    Streams(read::StreamsArgs),
    /// 导出文件或目录到本机
    Export(read::ExportArgs),
    /// 导出 mactime body 文件（文件系统时间线）
    Bodyfile(read::BodyfileArgs),
    /// 按文件名查找（卷索引）
    Find(find::FindArgs),
    /// 在文件内容中搜索关键词或正则
    Grep(find::GrepArgs),
    /// 文件系统日志：NTFS USN 与 ext4 jbd2
    Journal(find::JournalArgs),
    /// 解析文件结构：SQLite、Realm、ZIP、CSV、可执行文件、GGUF、Vim 交换文件、KeePass 数据库
    Inspect(inspect::InspectArgs),
    /// SQLite 数据库：表、记录、只读查询与删除记录恢复
    Sqlite(inspect::SqliteArgs),
    /// 识别图片中的文字
    Ocr(inspect::ImageArgs),
    /// 识别图片中的二维码
    Qr(inspect::QrArgs),
    /// 加密卷的类型、状态与元数据
    Encryption(crypto::EncryptionArgs),
    /// 解锁 BitLocker、LUKS、VeraCrypt、FileVault 或加密的 iOS 备份
    // Boxed: its credential and VeraCrypt options outweigh every other command.
    Unlock(Box<crypto::UnlockArgs>),
    /// RAID 与 LVM 组装
    Raid(raid::RaidArgs),
    /// NTFS 卷影副本
    Vss(raid::VssArgs),
    /// 全局搜索的历史与命中
    Search(search::SearchArgs),
    /// 时间线：构建、查询与导出
    Timeline(timeline::TimelineArgs),
    /// 文件恢复：文件系统记录恢复与签名雕刻
    Carve(carve::CarveArgs),
    /// 插件包与插件市场
    Plugin(plugin::PluginArgs),
    /// 运行插件分析器
    Analyze(analysis::AnalyzeArgs),
    /// 分析结果：结果树、记录、运行审计与 BCP 导入
    Results(analysis::ResultsArgs),
    /// 后台任务
    Tasks(tasks::TasksArgs),
    /// 卷索引
    Index(tasks::IndexArgs),
    /// 只读挂载（Windows）；不带参数时列出挂载
    Mount(tasks::MountArgs),
    /// 卸载挂载
    Umount(tasks::UmountArgs),
    /// 文件时间的显示时区
    Tz(system::TzArgs),
    /// 版本、构建功能与组件
    System,
    /// 本机物理磁盘
    Disks,
    /// 命令的参数与输出结构（JSON）
    Schema(SchemaArgs),
    /// 生成 shell 补全脚本
    Completion(CompletionArgs),
    /// 交互式 shell：在一个进程中连续执行命令
    Shell,
    /// 从标准输入逐行执行 JSONL 请求
    Batch(BatchArgs),
    /// 按名称调用任意后端命令（与桌面端 IPC 相同的参数）
    Rpc(RpcArgs),
    /// 补全候选（由补全脚本调用）
    #[command(name = "__complete", hide = true)]
    Complete(system::CompleteArgs),
}

/// How a command's result is shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    Json,
    Bytes,
}

/// What the runtime needs to know about a command before running it.
#[derive(Clone, Copy, Debug)]
pub struct Meta {
    pub output: OutputKind,
    /// A read-only query that may overlap others (batch `--jobs`, backend
    /// connections).
    pub concurrent: bool,
    /// Destroys or discards data: asks first, or needs `-y`.
    pub confirm: Option<&'static str>,
    /// Result fields that, when set, make the result incomplete.
    pub incomplete: &'static [&'static str],
    /// A lookup whose `null` result means `notFound`.
    pub missing: Option<&'static str>,
    /// What it creates lives only as long as the process (an unlocked
    /// volume, a carving scan, a mount): pointless in a one-shot command.
    pub lasting: bool,
}

impl Meta {
    const QUERY: Self = Self {
        output: OutputKind::Json,
        concurrent: true,
        confirm: None,
        incomplete: &[],
        missing: None,
        lasting: false,
    };

    const LASTING: Self = Self {
        lasting: true,
        ..Self::ACTION
    };

    const ACTION: Self = Self {
        concurrent: false,
        ..Self::QUERY
    };

    const fn destructive(question: &'static str) -> Self {
        Self {
            confirm: Some(question),
            ..Self::ACTION
        }
    }

    const fn bytes(mut self) -> Self {
        self.output = OutputKind::Bytes;
        self
    }

    const fn incomplete(mut self, fields: &'static [&'static str]) -> Self {
        self.incomplete = fields;
        self
    }

    const fn missing(mut self, message: &'static str) -> Self {
        self.missing = Some(message);
        self
    }
}

const READ_FAULTS: &[&str] = &["readFaultsDetected", "syntheticBytes"];

/// Every command's metadata, by the path `Command::path` gives.
pub fn meta_of(path: &str) -> Meta {
    match path {
        "cat" | "carve cat" => Meta::QUERY.bytes(),
        "hash" => Meta::QUERY.incomplete(&[
            "items.readFaultsDetected",
            "items.syntheticBytes",
            "items.truncated",
        ]),
        "grep" => Meta::QUERY.incomplete(&["summary.readErrors", "summary.truncated"]),
        "find" => Meta::QUERY.incomplete(&["unindexed"]),
        "sqlite query" => Meta::QUERY.incomplete(&["shortenedCells"]),
        "evidence hash" => Meta::QUERY.incomplete(READ_FAULTS),
        "evidence export" => Meta::ACTION.incomplete(READ_FAULTS),
        "export" => Meta::ACTION.incomplete(&[
            "items.readFaultsDetected",
            "items.syntheticBytes",
            "items.truncated",
            "items.cancelled",
        ]),
        "carve export" => Meta::ACTION.incomplete(&[
            "failed",
            "syntheticArtifacts",
            "reportError",
            "files.failed",
            "files.readFaults.readFaultsDetected",
        ]),
        "results show" => Meta::QUERY.missing("找不到分析记录"),
        "results run" => Meta::QUERY.missing("找不到分析运行"),
        "tasks show" => Meta::QUERY.missing("找不到任务"),
        "case rm" => {
            Meta::destructive("这会删除该案件目录中由 LovelyCarve 创建的台账、检材数据与暂存区。")
        }
        "evidence rm" => Meta::destructive("这会从案件中移除该检材及其卷索引、搜索与分析结果。"),
        "search rm" => Meta::destructive("这会删除这次搜索及其全部命中。"),
        "carve rm" => Meta::destructive("这会删除这次扫描及其结果记录。"),
        "plugin rm" => Meta::destructive("这会卸载该插件包。"),
        "plugin install" => Meta::destructive(
            "这会安装该插件包，已安装的同 ID 版本会被替换；它的代码会在插件进程中运行。",
        ),
        "plugin market install" => Meta::destructive(
            "这会下载并安装该插件包，已安装的同 ID 版本会被替换；它的代码会在插件进程中运行。",
        ),
        "tasks rm" => Meta::destructive("这会删除该插件任务及其发布的分析结果。"),
        "index rm" => Meta::destructive("这会删除该卷的索引。"),
        "umount" => Meta::destructive("这会卸载该挂载点，正在读取它的程序会失去访问。"),
        "carve start" | "mount" | "unlock" | "vss open" | "raid assemble" => Meta::LASTING,
        "case status"
        | "case ls"
        | "case link"
        | "evidence ls"
        | "evidence show"
        | "evidence bad-regions"
        | "volumes"
        | "ls"
        | "tree"
        | "stat"
        | "text"
        | "strings"
        | "identify"
        | "streams"
        | "journal"
        | "inspect"
        | "sqlite tables"
        | "sqlite rows"
        | "sqlite deleted"
        | "encryption"
        | "vss ls"
        | "search ls"
        | "search hits"
        | "timeline status"
        | "timeline ls"
        | "timeline stats"
        | "carve ls"
        | "carve formats"
        | "carve status"
        | "carve results"
        | "plugin ls"
        | "plugin inspect"
        | "plugin readme"
        | "plugin market sources"
        | "plugin market ls"
        | "analyze ls"
        | "results ls"
        | "results rows"
        | "tasks ls"
        | "index ls"
        | "tz"
        | "system"
        | "disks"
        | "schema"
        | "completion"
        | "__complete" => Meta::QUERY,
        _ => Meta::ACTION,
    }
}

impl Command {
    /// The command as typed: `case open`, `ls`; a group given alone names
    /// its default command (`case` → `case status`).
    pub fn path(&self) -> &'static str {
        match self {
            Self::Case(args) => case::path(args),
            Self::Evidence(args) => evidence::path(args),
            Self::Volumes(_) => "volumes",
            Self::Ls(_) => "ls",
            Self::Tree(_) => "tree",
            Self::Stat(_) => "stat",
            Self::Cat(_) => "cat",
            Self::Text(_) => "text",
            Self::Strings(_) => "strings",
            Self::Hash(_) => "hash",
            Self::Identify(_) => "identify",
            Self::Streams(_) => "streams",
            Self::Export(_) => "export",
            Self::Bodyfile(_) => "bodyfile",
            Self::Find(_) => "find",
            Self::Grep(_) => "grep",
            Self::Journal(_) => "journal",
            Self::Inspect(_) => "inspect",
            Self::Sqlite(args) => inspect::sqlite_path(args),
            Self::Ocr(_) => "ocr",
            Self::Qr(_) => "qr",
            Self::Encryption(_) => "encryption",
            Self::Unlock(_) => "unlock",
            Self::Raid(args) => raid::raid_path(args),
            Self::Vss(args) => raid::vss_path(args),
            Self::Search(args) => search::path(args),
            Self::Timeline(args) => timeline::path(args),
            Self::Carve(args) => carve::path(args),
            Self::Plugin(args) => plugin::path(args),
            Self::Analyze(args) => analysis::analyze_path(args),
            Self::Results(args) => analysis::results_path(args),
            Self::Tasks(args) => tasks::tasks_path(args),
            Self::Index(args) => tasks::index_path(args),
            Self::Mount(_) => "mount",
            Self::Umount(_) => "umount",
            Self::Tz(args) => system::tz_path(args),
            Self::System => "system",
            Self::Disks => "disks",
            Self::Schema(_) => "schema",
            Self::Completion(_) => "completion",
            Self::Shell => "shell",
            Self::Batch(_) => "batch",
            Self::Rpc(_) => "rpc",
            Self::Complete(_) => "__complete",
        }
    }

    pub fn meta(&self) -> Meta {
        meta_of(self.path())
    }

    /// The secret options, to read before the command leaves the process
    /// where they were typed.
    pub fn secrets_mut(&mut self) -> Vec<(&'static str, &mut Secret)> {
        match self {
            Self::Evidence(args) => evidence::secrets_mut(args),
            Self::Unlock(args) => crypto::secrets_mut(args),
            Self::Sqlite(args) => inspect::sqlite_secrets_mut(args),
            Self::Inspect(args) => inspect::inspect_secrets_mut(args),
            _ => Vec::new(),
        }
    }
}

/// Run a command where the evidence is held.
pub async fn run(
    exec: &Exec,
    command: Command,
    options: &RequestOptions,
) -> Result<Output, CliError> {
    match command {
        Command::Case(args) => case::run(exec, args).await,
        Command::Evidence(args) => evidence::run(exec, args).await,
        Command::Volumes(args) => browse::volumes(exec, args).await,
        Command::Ls(args) => browse::ls(exec, args, options).await,
        Command::Tree(args) => browse::tree(exec, args, options).await,
        Command::Stat(args) => browse::stat(exec, args, options).await,
        Command::Cat(args) => read::cat(exec, args).await,
        Command::Text(args) => read::text(exec, args).await,
        Command::Strings(args) => read::strings(exec, args).await,
        Command::Hash(args) => read::hash(exec, args).await,
        Command::Identify(args) => read::identify(exec, args).await,
        Command::Streams(args) => read::streams(exec, args).await,
        Command::Export(args) => read::export(exec, args).await,
        Command::Bodyfile(args) => read::bodyfile(exec, args).await,
        Command::Find(args) => find::find(exec, args).await,
        Command::Grep(args) => find::grep(exec, args).await,
        Command::Journal(args) => find::journal(exec, args).await,
        Command::Inspect(args) => inspect::inspect(exec, args).await,
        Command::Sqlite(args) => inspect::sqlite(exec, args).await,
        Command::Ocr(args) => inspect::ocr(exec, args).await,
        Command::Qr(args) => inspect::qr(exec, args).await,
        Command::Encryption(args) => crypto::encryption(exec, args).await,
        Command::Unlock(args) => crypto::unlock(exec, *args).await,
        Command::Raid(args) => raid::raid(exec, args).await,
        Command::Vss(args) => raid::vss(exec, args).await,
        Command::Search(args) => search::run(exec, args).await,
        Command::Timeline(args) => timeline::run(exec, args).await,
        Command::Carve(args) => carve::run(exec, args).await,
        Command::Plugin(args) => plugin::run(exec, args).await,
        Command::Analyze(args) => analysis::analyze(exec, args).await,
        Command::Results(args) => analysis::results(exec, args).await,
        Command::Tasks(args) => tasks::tasks(exec, args).await,
        Command::Index(args) => tasks::index(exec, args).await,
        Command::Mount(args) => tasks::mount(exec, args).await,
        Command::Umount(args) => tasks::umount(exec, args).await,
        Command::Tz(args) => system::tz(exec, args, options).await,
        Command::Disks => system::disks().await,
        Command::Rpc(args) => system::rpc(exec, args).await,
        Command::System
        | Command::Schema(_)
        | Command::Completion(_)
        | Command::Complete(_)
        | Command::Shell
        | Command::Batch(_) => Err(CliError::new(
            "usage",
            "这个命令不能在 shell、batch 或案件后台进程中运行",
        )),
    }
}

/// The text form of a result; `None` prints the generic layout.
pub fn render(command: &Command, value: &Value) -> Option<String> {
    match command {
        Command::Case(args) => case::render(args, value),
        Command::Evidence(args) => evidence::render(args, value),
        Command::Volumes(_) => Some(browse::render_volumes(value)),
        Command::Ls(args) => Some(browse::render_ls(args, value)),
        Command::Tree(_) => Some(browse::render_tree(value)),
        Command::Stat(_) => Some(browse::render_stat(value)),
        Command::Text(_) => value["text"].as_str().map(str::to_owned),
        Command::Strings(_) => Some(read::render_strings(value)),
        Command::Hash(_) => Some(read::render_hash(value)),
        Command::Streams(_) if value["items"].as_array().is_some_and(Vec::is_empty) => {
            Some("（没有命名数据流、资源分叉或扩展属性）\n".to_owned())
        }
        Command::Streams(_) => Some(super::render::table(
            value["items"].as_array().map_or(&[][..], Vec::as_slice),
            &[("名称", "name"), ("大小", "size")],
        )),
        Command::Export(_) => Some(read::render_export(value)),
        Command::Sqlite(args) => inspect::render_sqlite(args, value),
        Command::Find(_) => Some(find::render_find(value)),
        Command::Grep(_) => Some(find::render_grep(value)),
        Command::Search(args) => search::render(args, value),
        Command::Tasks(args) => tasks::render_tasks(args, value),
        Command::Index(args) => tasks::render_index(args, value),
        Command::Carve(args) => carve::render(args, value),
        Command::Plugin(args) => plugin::render(args, value),
        Command::Analyze(_) | Command::Results(_) => analysis::render(command, value),
        _ => None,
    }
}

/// A `--tz` value, checked where it is typed: a bad zone fails before any
/// evidence is opened.
fn parse_zone(text: &str) -> Result<String, String> {
    super::timezone::validate(text)
        .map(|()| text.to_owned())
        .map_err(|error| error.message)
}

/// Sizes for `--min-size` and friends: plain bytes or with K, M, G, T.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (number, unit) = match text.find(|character: char| !character.is_ascii_digit()) {
        Some(index) => text.split_at(index),
        None => (text, ""),
    };
    let base: u64 = number
        .parse()
        .map_err(|_| format!("无效大小：{text}（例如 4096、64K、10M、2G）"))?;
    let multiplier: u64 = match unit
        .trim()
        .to_ascii_uppercase()
        .trim_end_matches("IB")
        .trim_end_matches('B')
    {
        "" => 1,
        "K" => 1 << 10,
        "M" => 1 << 20,
        "G" => 1 << 30,
        "T" => 1 << 40,
        _ => return Err(format!("无效大小单位：{text}（可用 K、M、G、T）")),
    };
    base.checked_mul(multiplier)
        .ok_or_else(|| format!("大小超出范围：{text}"))
}

/// A task ID for backend work, so it can be found and cancelled.
pub fn task_id(path: &str) -> String {
    format!("cli-{}-{}", path.replace(' ', "-"), uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn sizes_take_binary_units() {
        assert_eq!(parse_size("4096"), Ok(4096));
        assert_eq!(parse_size("64K"), Ok(65536));
        assert_eq!(parse_size("10MiB"), Ok(10 << 20));
        assert_eq!(parse_size("2g"), Ok(2 << 30));
        assert!(parse_size("1X").is_err());
        assert!(parse_size("").is_err());
    }
}
