use crate::filesystem::evidence_session::EvidenceSession;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::evidence::image::PartitionKind;
use crate::search::global_search::GlobalSearchScope;

/// 一段伪随机可打印文本，埋入若干关键词（含跨块、跨段边界与文件末尾的位置）。
fn planted_bytes(len: usize, plants: &[(usize, &[u8])]) -> Vec<u8> {
    let mut state = 0x2545_f491_u32;
    let mut data: Vec<u8> = (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let byte = b' ' + (state % 95) as u8;
            // 偶尔插入换行，覆盖上下文的换行折叠。
            if state % 61 == 0 {
                b'\n'
            } else {
                byte
            }
        })
        .collect();
    for (offset, bytes) in plants {
        data[*offset..*offset + bytes.len()].copy_from_slice(bytes);
    }
    data
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn options(per_file_cap: u32, stop_after_first: bool) -> GlobalSearchOptions {
    GlobalSearchOptions {
        keys: vec!["password".to_string()],
        regexes: Vec::new(),
        codepages: Vec::new(),
        match_case: false,
        search_deleted: true,
        search_empty_space: false,
        calc_hash: false,
        scope: GlobalSearchScope::default(),
        per_file_cap,
        stop_after_first,
        global_cap: u32::MAX,
    }
}

/// 用 `workers` 个线程按 `layout` 扫描 `sources`，返回 worker 交给 drain 线程的
/// 全部产出（无命中、无读错误的单元不产出）。
fn run_scan(
    image: &Arc<EvidenceSession>,
    sources: &[ScanSource],
    options: &GlobalSearchOptions,
    layout: ScanLayout,
    workers: usize,
) -> Vec<SourceOutcome> {
    let matchers = MatcherSet::build_plan(&options.keys, &[], options.match_case, &[])
        .expect("build matchers");
    let mut session = FsSession::new(image);
    let plan = build_work_plan(&mut session, sources, options, layout);
    let cancel = CancellationToken::new();
    let stop = AtomicBool::new(false);
    let pool = ScanPool {
        plan: &plan,
        sources,
        matchers: &matchers,
        options,
        patterns: &options.keys,
        keyword_count: options.keys.len(),
        cancel_flag: &cancel,
        stop_flag: &stop,
        layout,
        next_batch: AtomicUsize::new(0),
        files_done: AtomicU64::new(0),
        files_scanned: AtomicU64::new(0),
    };
    let (tx, rx) = sync_channel(plan.item_count() + 1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let pool = &pool;
            scope.spawn(move || scan_worker(pool, image, &tx));
        }
    });
    drop(tx);
    rx.into_iter().collect()
}

/// 用 `workers` 个线程按 `layout` 扫描一个覆盖整个镜像的 `diskgap:` 源（连续
/// 镜像区间可随机读，大于两段即切段）。返回按偏移排序的记录与命中总数。
fn scan_image(
    image: &Arc<EvidenceSession>,
    options: &GlobalSearchOptions,
    layout: ScanLayout,
    workers: usize,
) -> (Vec<GlobalSearchHit>, u64) {
    let size = image.len();
    let sources = [ScanSource {
        node_id: format!("diskgap:{:#x}:{size:#x}", 0),
        read_id: None,
        name: "gap".to_string(),
        size_bytes: size,
        path: "gap".to_string(),
        volume_key: String::new(),
        source_kind: "diskgap",
    }];
    let mut records = Vec::new();
    let mut total = 0u64;
    for outcome in run_scan(image, &sources, options, layout, workers) {
        assert!(!outcome.read_error);
        total += outcome.hit_count;
        records.extend(outcome.records);
    }
    records.sort_by(|left, right| {
        (left.file_offset, &left.encoding).cmp(&(right.file_offset, &right.encoding))
    });
    (records, total)
}

fn fixture() -> (tempfile::TempDir, Arc<EvidenceSession>, Vec<u8>, Vec<u64>) {
    let len = 300 * 1024;
    let upper = utf16le("PASSWORD");
    // 按文件序排列，互不重叠。
    let plants: Vec<(usize, &[u8])> = vec![
        (0, &b"password"[..]),
        (4090, &b"PassWord"[..]),          // 跨 4 KiB 块边界
        (4096 * 3 - 30, &b"password"[..]), // 后文跨块
        (65536 - 4, &b"password"[..]),     // 跨 64 KiB 段边界
        (65536 + 20, &b"password"[..]),    // 前文落在上一段
        (131_072 + 100, upper.as_slice()), // UTF-16LE
        (len - 70, &b"password"[..]),      // 后文被 EOF 截断
        (len - 8, &b"PASSWORD"[..]),       // 贴文件末尾
    ];
    let offsets = plants.iter().map(|(offset, _)| *offset as u64).collect();
    let data = planted_bytes(len, &plants);
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("disk.raw");
    std::fs::write(&path, &data).expect("write raw fixture");
    let image = Arc::new(EvidenceSession::open(&path).expect("open raw image"));
    (temp, image, data, offsets)
}

const SINGLE_WINDOW: ScanLayout = ScanLayout {
    chunk_bytes: 8 * 1024 * 1024,
    segment_bytes: 1024 * 1024 * 1024,
};

#[test]
fn single_window_scan_reports_every_plant_with_its_raw_context() {
    let (_temp, image, data, offsets) = fixture();
    let (records, total) = scan_image(&image, &options(1000, false), SINGLE_WINDOW, 1);
    assert_eq!(
        records
            .iter()
            .map(|record| record.file_offset)
            .collect::<Vec<_>>(),
        offsets.iter().copied().collect::<Vec<_>>()
    );
    assert_eq!(total, offsets.len() as u64);
    for record in &records {
        assert_eq!(record.match_count, total);
        let (start, end) = context_span(record.file_offset, record.match_len as usize);
        let raw = &data[start as usize..(end as usize).min(data.len())];
        let encoding = SearchEncoding::from_label(&record.encoding).expect("encoding label");
        let decoded = encoding.decode_to_string(raw);
        assert_eq!(
            record.context_full,
            context_full(&decoded, CONTEXT_FULL_CHARS, CONTEXT_FULL_LINES)
        );
        assert_eq!(
            record.context_preview,
            collapse_newlines(&decoded, CONTEXT_PREVIEW_CHARS)
        );
    }
}

#[test]
fn small_chunks_and_segments_match_the_single_window_scan() {
    let (_temp, image, _data, _offsets) = fixture();
    let options = options(1000, false);
    let (reference, reference_total) = scan_image(&image, &options, SINGLE_WINDOW, 1);
    // 块小于 carry 与上下文窗口时，待补后文要跨多个块补齐。
    for chunk_bytes in [4096, 97, 37] {
        let layout = ScanLayout {
            chunk_bytes,
            segment_bytes: 64 * 1024,
        };
        for workers in [1, 4] {
            let (records, total) = scan_image(&image, &options, layout, workers);
            assert_eq!(
                total, reference_total,
                "chunk {chunk_bytes} workers {workers}"
            );
            assert_eq!(records, reference, "chunk {chunk_bytes} workers {workers}");
        }
    }
}

#[test]
fn per_file_cap_keeps_the_earliest_hits_in_both_modes() {
    let (_temp, image, _data, offsets) = fixture();
    let options = options(3, false);
    let segmented = ScanLayout {
        chunk_bytes: 4096,
        segment_bytes: 64 * 1024,
    };
    for (layout, workers) in [(SINGLE_WINDOW, 1), (segmented, 4)] {
        let (records, total) = scan_image(&image, &options, layout, workers);
        assert_eq!(total, offsets.len() as u64);
        assert_eq!(
            records
                .iter()
                .map(|record| record.file_offset)
                .collect::<Vec<_>>(),
            offsets[..3].to_vec()
        );
        assert!(records.iter().all(|record| record.match_count == total));
    }
}

#[test]
fn stop_after_first_keeps_one_hit_with_full_context() {
    let (_temp, image, _data, _offsets) = fixture();
    let (reference, _) = scan_image(&image, &options(1000, false), SINGLE_WINDOW, 1);
    let tiny = ScanLayout {
        chunk_bytes: 37,
        segment_bytes: 64 * 1024,
    };
    let (records, total) = scan_image(&image, &options(1000, true), tiny, 1);
    assert_eq!(total, 1);
    let mut expected = reference[0].clone();
    expected.match_count = 1;
    assert_eq!(records, vec![expected]);
}

#[test]
fn unseekable_sources_are_not_split() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("big.bin"), vec![b'x'; 200 * 1024]).expect("write fixture");
    let image = Arc::new(EvidenceSession::open(temp.path()).expect("open folder evidence"));
    let sources = vec![ScanSource {
        node_id: format!("folder:{}", hex::encode("big.bin")),
        read_id: None,
        name: "big.bin".to_string(),
        size_bytes: 200 * 1024,
        path: "/big.bin".to_string(),
        volume_key: String::new(),
        source_kind: "file",
    }];
    let layout = ScanLayout {
        chunk_bytes: 4096,
        segment_bytes: 64 * 1024,
    };
    let mut session = FsSession::new(&image);
    let plan = build_work_plan(&mut session, &sources, &options(100, false), layout);
    assert_eq!(plan.zero_byte_sources, 0);
    assert_eq!(plan.item_count(), 1);
    assert!(plan.items[0].segment.is_none());
}

#[test]
fn files_without_a_data_stream_scan_as_empty_not_as_read_errors() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/vhd/tests/data/ntfs_fixed.vhd");
    let image = Arc::new(EvidenceSession::open(&path).expect("open NTFS VHD fixture"));
    let partition_id = image
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Ntfs)
        .expect("NTFS partition")
        .id;
    let source = |record: u64| ScanSource {
        node_id: format!("ntfs:part:{partition_id}:{record}"),
        read_id: None,
        name: format!("record {record}"),
        size_bytes: 1,
        path: format!("/record {record}"),
        volume_key: format!("part:{partition_id}"),
        source_kind: "file",
    };
    // MFT record 9 is $Secure: its named $SDS stream and indexes, no unnamed $DATA.
    let secure = source(9);
    let missing = FsSession::new(&image)
        .stream_file(&secure.node_id, 4096, |_| Ok(()))
        .expect_err("$Secure has no unnamed $DATA");
    assert!(missing.downcast_ref::<MissingDataStream>().is_some());
    let options = options(100, false);
    let outcomes = run_scan(&image, &[secure], &options, SINGLE_WINDOW, 1);
    assert!(outcomes.iter().all(|outcome| !outcome.read_error));

    // A record that cannot be read at all is still a read error.
    let outcomes = run_scan(
        &image,
        &[source(u64::from(u32::MAX))],
        &options,
        SINGLE_WINDOW,
        1,
    );
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.read_error)
            .collect::<Vec<_>>(),
        vec![true]
    );
}

#[test]
fn batches_group_neighbours_and_isolate_large_units() {
    let source = |name: &str, size_bytes: u64| ScanSource {
        node_id: name.to_string(),
        read_id: None,
        name: name.to_string(),
        size_bytes,
        path: format!("/{name}"),
        volume_key: String::new(),
        source_kind: "file",
    };
    // 70 个小文件、一个 5 MiB 文件、再 3 个小文件。
    let mut sources: Vec<ScanSource> = (0..70)
        .map(|index| source(&format!("s{index}"), 1024))
        .collect();
    sources.push(source("big", 5 * 1024 * 1024));
    sources.extend((0..3).map(|index| source(&format!("t{index}"), 1024)));
    let work: Vec<WorkItem> = (0..sources.len())
        .map(|source_idx| WorkItem {
            source_idx,
            segment: None,
        })
        .collect();
    assert_eq!(
        batch_ranges(&work, &sources),
        vec![(0, 64), (64, 70), (70, 71), (71, 74)]
    );
}

#[test]
fn collapse_and_context_caps() {
    assert_eq!(collapse_newlines("a\nb\r\nc", 100), "a / b / c");
    let long = "x".repeat(300);
    assert_eq!(collapse_newlines(&long, 50).chars().count(), 50);
    let multi = "l1\nl2\nl3\nl4";
    assert_eq!(context_full(multi, 100, 3).lines().count(), 3);
}
