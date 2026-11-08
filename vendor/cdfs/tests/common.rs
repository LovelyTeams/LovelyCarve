// SPDX-License-Identifier: (MIT OR Apache-2.0)

use cdfs::{ISO9660Reader, ISODirectory};

pub fn collect_filenames<T: ISO9660Reader>(directory: &ISODirectory<T>) -> Vec<String> {
    directory
        .contents()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .into_iter()
        .map(|item| item.identifier().to_string())
        .collect::<Vec<_>>()
}
