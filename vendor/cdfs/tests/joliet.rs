// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::fs::File;

use cdfs::ISO9660;

mod common;
use common::collect_filenames;

const JOLIET_IMAGE: &'static str = "images/joliet.iso";

#[test]
fn joliet_long_filenames_exist() {
    let fs = ISO9660::new(File::open(JOLIET_IMAGE).expect("couldn't open file"))
        .expect("file is not an ISO image");

    let contents = collect_filenames(fs.root());

    assert_eq!(
        contents,
        &[".", "..", "Read Me First, Please.txt", "readme.txt",]
    );
}

#[test]
fn joliet_short_filenames_exist() {
    let fs = ISO9660::new(File::open(JOLIET_IMAGE).expect("couldn't open file"))
        .expect("file is not an ISO image");

    let contents = collect_filenames(
        fs.root_at(0)
            .expect("primary volume descriptor doesn't exist?"),
    );

    assert_eq!(contents, &[".", "..", "README.TXT", "READ_ME_.TXT",]);
}
