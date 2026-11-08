// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::fs::File;

use cdfs::ISO9660;

const JOLIET_IMAGE: &'static str = "images/rockridge.iso";

mod common;
use common::collect_filenames;

#[test]
fn rockridge_default_vd() {
    let fs = ISO9660::new(File::open(JOLIET_IMAGE).expect("couldn't open file"))
        .expect("file is not an ISO image");

    let contents = collect_filenames(fs.root());

    assert_eq!(
        contents,
        &[
            ".",
            "..",
            "1",
            "readme.txt",
            "Read Me First, Please.txt",
            "Really really really really really really really Really really really really really really reallyReally really really really really really really long.txt",
            "rr_moved",
            "this_is_a_symlink",
            "this_is_an_absolute_symlink",
        ]
    );
}

#[test]
fn rockridge_primary_vd() {
    let fs = ISO9660::new(File::open(JOLIET_IMAGE).expect("couldn't open file"))
        .expect("file is not an ISO image");

    let contents = collect_filenames(fs.root_at(0).unwrap());

    assert_eq!(
        contents,
        &[
            ".",
            "..",
            "1",
            "readme.txt",
            "Read Me First, Please.txt",
            "Really really really really really really really Really really really really really really reallyReally really really really really really really long.txt",
            "rr_moved",
            "this_is_a_symlink",
            "this_is_an_absolute_symlink"
        ]
    );
}
