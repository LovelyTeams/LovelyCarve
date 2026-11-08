// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::{
    fs::File,
    io::{self, Read, Write},
    path::PathBuf,
};

use anyhow::{anyhow, bail};
use clap::Parser;

use cdfs::{DirectoryEntry, ISO9660};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    iso_path: PathBuf,
    file_path: PathBuf,
}

fn main() -> anyhow::Result<()> {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .env()
        .init()?;

    let args = Args::parse();

    let file = File::open(args.iso_path)?;
    let fs = ISO9660::new(file)?;

    let file_path = args
        .file_path
        .to_str()
        .ok_or_else(|| anyhow!("file_path could not be converted to a UTF-8 string"))?;

    match fs.open(file_path).unwrap() {
        Some(DirectoryEntry::File(file)) => {
            let mut stdout = io::stdout();
            let mut text = Vec::new();
            file.read().read_to_end(&mut text)?;
            stdout.write_all(&text)?;
        }
        Some(_) => bail!("{file_path} is not a file."),
        None => bail!("'{file_path}' not found"),
    }

    Ok(())
}
