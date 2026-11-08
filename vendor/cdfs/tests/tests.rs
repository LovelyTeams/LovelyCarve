// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::{fs::File, io::Read};

use cdfs::{DirectoryEntry, ISO9660};

mod common;
use common::collect_filenames;

const TEST_IMAGE: &'static str = "images/test.iso";

#[test]
fn test_dir() {
    let fs = ISO9660::new(File::open(TEST_IMAGE).unwrap()).unwrap();

    let filenames = collect_filenames(fs.root());

    assert_eq!(filenames, &[".", "..", "A", "GPL_3_0.TXT"]);
}

#[test]
fn test_large_file() {
    let fs = ISO9660::new(File::open(TEST_IMAGE).unwrap()).unwrap();

    let file = match fs.open("gpl_3_0.txt").unwrap().unwrap() {
        DirectoryEntry::File(file) => file,
        _ => panic!("Not a file"),
    };

    let mut text = String::new();
    file.read().read_to_string(&mut text).unwrap();
    let hash = md5::compute(text);
    assert_eq!(format!("{:x}", hash), "1ebbd3e34237af26da5dc08a4e440464");
}

#[test]
fn test_extra_slashes() {
    let fs = ISO9660::new(File::open(TEST_IMAGE).unwrap()).unwrap();

    assert!(fs.open("///a/b/c/1").unwrap().is_some());
    assert!(fs.open("a/b/c/1///").unwrap().is_some());
    assert!(fs.open("a/b//c/1").unwrap().is_some());
    assert!(fs.open("/a/b//c////1/").unwrap().is_some());
}

#[test]
fn test_large_dir() {
    let fs = ISO9660::new(File::open(TEST_IMAGE).unwrap()).unwrap();

    let dir = match fs.open("a/b/c").unwrap().unwrap() {
        DirectoryEntry::Directory(dir) => dir,
        _ => panic!("Not a directory"),
    };

    // 200 files, plus '.' and '..'
    assert_eq!(dir.contents().map(Result::unwrap).count(), 202);
    assert_eq!(dir.block_count(), 4);
}
