use exfat::timestamp::Timestamp;
use exfat::DiskPartition;
use exfat::{Item, Root};
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;

const ENTRY_SIZE: usize = 32;
const ROOT_OFFSET: usize = 0x9000;
const DIR1_OFFSET: usize = 0xa000;

fn check_timestamp(
    ts: &Timestamp,
    day: u8,
    month: u8,
    year: u16,
    hour: u8,
    minute: u8,
    second: u8,
    utc_offset: i8,
) {
    assert_eq!(day, ts.date().day);
    assert_eq!(month, ts.date().month);
    assert_eq!(year, ts.date().year);
    assert_eq!(hour, ts.time().hour);
    assert_eq!(minute, ts.time().minute);
    assert_eq!(second, ts.time().second);
    assert_eq!(utc_offset, ts.utc_offset());
}

#[derive(Clone)]
struct MemoryImage(Vec<u8>);

impl DiskPartition for MemoryImage {
    type Err = io::Error;

    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, Self::Err> {
        let offset = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let Some(available) = self.0.get(offset..) else {
            return Ok(0);
        };
        let amount = available.len().min(buf.len());
        buf[..amount].copy_from_slice(&available[..amount]);
        Ok(amount)
    }
}

fn test_image() -> Vec<u8> {
    include_bytes!("exfat.img").to_vec()
}

fn mark_entry_set_deleted(image: &mut [u8], primary_offset: usize) {
    image[primary_offset] &= 0x7f;
    image[primary_offset + ENTRY_SIZE] &= 0x7f;
    image[primary_offset + ENTRY_SIZE * 2] &= 0x7f;
}

fn item_names(items: Vec<Item<MemoryImage>>) -> Vec<String> {
    items
        .into_iter()
        .map(|item| match item {
            Item::Directory(directory) => directory.name().to_string(),
            Item::File(file) => file.name().to_string(),
        })
        .collect()
}

#[test]
fn read_image() {
    // Open the image.
    let image: PathBuf = ["tests", "exfat.img"].iter().collect();
    let image = File::open(image).expect("cannot open exfat.img");

    // Open root directory.
    let root = Root::open(image).expect("cannot open the root directory");

    // Check image properties.
    assert_eq!(Some("Test image"), root.volume_label());

    // Check items in the root of image.
    let items = Vec::from_iter(root.into_iter());

    assert_eq!(2, items.len());

    for i in items {
        match i {
            Item::Directory(d) => {
                // Check directory properties.
                assert_eq!("dir1", d.name());

                // Check timestamps
                check_timestamp(d.timestamps().created(), 6, 3, 2023, 13, 2, 32, 0);
                check_timestamp(d.timestamps().modified(), 6, 3, 2023, 13, 3, 18, 0);
                check_timestamp(d.timestamps().accessed(), 6, 3, 2023, 13, 2, 32, 0);

                // Check items.
                let mut items = d.open().expect("cannot open dir1");

                assert_eq!(1, items.len());

                match items.remove(0) {
                    Item::Directory(_) => panic!("unexpected item in dir1"),
                    Item::File(mut f) => {
                        // Check file properties.
                        assert_eq!("file2", f.name());
                        assert_eq!(13, f.len());

                        // Check file content.
                        let mut c = String::new();

                        f.read_to_string(&mut c).expect("cannot read file2");

                        assert_eq!("Test file 2.\n", c);

                        // Check timestamps
                        check_timestamp(f.timestamps().created(), 6, 3, 2023, 13, 3, 18, 0);
                        check_timestamp(f.timestamps().modified(), 6, 3, 2023, 13, 3, 18, 0);
                        check_timestamp(f.timestamps().accessed(), 6, 3, 2023, 13, 3, 18, 0);
                    }
                };
            }
            Item::File(mut f) => {
                // Check file properties.
                assert_eq!("file1", f.name());
                assert_eq!(13, f.len());

                // Check file content.
                let mut c = String::new();

                f.read_to_string(&mut c).expect("cannot read file1");

                assert_eq!("Test file 1.\n", c);

                // Check timestamps
                check_timestamp(f.timestamps().created(), 6, 3, 2023, 13, 3, 6, 0);
                check_timestamp(f.timestamps().modified(), 6, 3, 2023, 13, 3, 6, 0);
                check_timestamp(f.timestamps().accessed(), 6, 3, 2023, 13, 3, 6, 0);
            }
        }
    }
}

#[test]
fn root_skips_deleted_entries_and_continues_to_later_files() {
    let mut image = test_image();
    mark_entry_set_deleted(&mut image, ROOT_OFFSET + ENTRY_SIZE * 3);

    let root = Root::open(MemoryImage(image)).expect("cannot open the root directory");

    assert_eq!(item_names(root.into_iter().collect()), vec!["file1"]);
}

#[test]
fn subdirectory_skips_deleted_entries_and_continues_to_later_files() {
    let mut image = test_image();
    image.copy_within(
        DIR1_OFFSET..(DIR1_OFFSET + ENTRY_SIZE * 3),
        DIR1_OFFSET + ENTRY_SIZE * 3,
    );
    mark_entry_set_deleted(&mut image, DIR1_OFFSET);

    let root = Root::open(MemoryImage(image)).expect("cannot open the root directory");
    let dir1 = root
        .into_iter()
        .find_map(|item| match item {
            Item::Directory(directory) if directory.name() == "dir1" => Some(directory),
            _ => None,
        })
        .expect("cannot find dir1");
    let items = dir1.open().expect("cannot open dir1");

    assert_eq!(item_names(items), vec!["file2"]);
}
