// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::{fs::File, path::PathBuf};

use anyhow::{anyhow, bail};
use clap::Parser;
use time::format_description::{self, FormatItem};

use cdfs::{DirectoryEntry, ExtraAttributes, ISO9660Reader, ISODirectory, ISO9660};

const INDENT: &str = "  ";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    iso_path: PathBuf,
    dir_path: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let time_format =
        format_description::parse("[month repr:short] [day] [year] [hour repr:24]:[minute]")?;

    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .env()
        .init()?;

    let args = Args::parse();

    let file = File::open(args.iso_path)?;
    let fs = ISO9660::new(file)?;

    match args.dir_path {
        Some(dir_path) => {
            let dir_path = dir_path
                .to_str()
                .ok_or_else(|| anyhow!("{dir_path:?} could not be converted to a UTF-8 string"))?;

            match fs.open(dir_path)? {
                Some(DirectoryEntry::Directory(dir)) => {
                    print_tree(&dir, 0, &time_format);
                }
                Some(DirectoryEntry::File(_)) | Some(DirectoryEntry::Symlink(_)) => {
                    bail!("'{dir_path}' is not a directory");
                }
                None => {
                    bail!("'{dir_path}' does not exist");
                }
            }
        }

        None => print_tree(fs.root(), 0, &time_format),
    }

    Ok(())
}

fn print_tree<T: ISO9660Reader>(dir: &ISODirectory<T>, level: usize, time_format: &[FormatItem]) {
    for entry in dir.contents() {
        match entry.unwrap() {
            DirectoryEntry::Directory(dir) => {
                if dir.identifier == "." || dir.identifier == ".." {
                    continue;
                }

                if let Some(mode) = dir.mode() {
                    print!("{mode} ");
                }
                print!(" {} ", dir.modify_time().format(&time_format).unwrap());

                println!("{}{}/", INDENT.repeat(level), dir.identifier);
                print_tree(&dir, level + 1, time_format);
            }
            DirectoryEntry::File(file) => {
                if let Some(mode) = file.mode() {
                    print!("{mode} ");
                }
                print!(" {} ", file.modify_time().format(&time_format).unwrap());

                println!("{}{}", INDENT.repeat(level), file.identifier);
            }
            DirectoryEntry::Symlink(link) => {
                if let Some(mode) = link.mode() {
                    print!("{mode} ");
                }

                print!(" {} ", dir.modify_time().format(&time_format).unwrap());

                let target = match link.target() {
                    Some(target) if target == "\u{0}" => "..",
                    Some(target) if target == "\u{1}" => ".",
                    Some(target) if target.is_empty() => "‽‽‽",
                    Some(target) => target,
                    None => "‽‽‽",
                };

                println!("{}{} → {target}", INDENT.repeat(level), link.identifier);
            }
        }
    }
}
