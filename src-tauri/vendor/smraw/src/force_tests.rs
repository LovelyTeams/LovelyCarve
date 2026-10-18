use super::*;

#[test]
fn forced_discovery_uses_selected_prefix_and_stops_at_gap() {
    for width in [3, 4, 5] {
        let fixture = Fixture::new();
        let first = fixture.file(&format!("disk.{:0width$}", 1), b"abc");
        let second = fixture.file(&format!("disk.{:0width$}", 2), b"defg");
        fixture.file(&format!("disk.{:0width$}", 4), b"ignored");
        assert!(discover(&first).is_err());
        let image = RawImage::open_forced(&first).unwrap();
        assert_eq!(image.media_size(), 7);
        assert_eq!(image.segments().len(), 2);
        assert!(image.is_forced());
        let middle = RawImage::open_forced(&second).unwrap();
        assert_eq!(middle.media_size(), 4);
        let mut output = [255; 12];
        let result = image.read_at_forced(1, &mut output).unwrap();
        assert_eq!(result.bytes_read, 6);
        assert_eq!(&output[..6], b"bcdefg");
        assert_eq!(&output[6..], &[255; 6]);
        assert!(result.capacity_warning);
        assert!(!result.synthetic);
        assert!(
            !image
                .read_at_forced(7, &mut output)
                .unwrap()
                .capacity_warning
        );
        assert_eq!(output, [0; 12]);
        assert_eq!(fs::read(first).unwrap(), b"abc");
    }
}

#[test]
fn forced_named_first_segment_prefers_001_then_002() {
    for start in [1, 2] {
        let fixture = Fixture::new();
        let first = fixture.file("disk.dd", b"first");
        fixture.file(&format!("disk.{start:03}"), b"next");
        let image = RawImage::open_forced(first).unwrap();
        assert_eq!(image.media_size(), 9);
        assert_eq!(image.segments().len(), 2);
    }
    let fixture = Fixture::new();
    let first = fixture.file("disk.img", b"first");
    fixture.file("disk.001", b"one");
    fixture.file("disk.003", b"ignored");
    assert_eq!(RawImage::open_forced(first).unwrap().media_size(), 8);
    assert!(RawImage::open_forced(&fixture.0).is_err());
}

#[test]
fn forced_reads_preserve_short_prefix_and_strict_stability_checks() {
    let fixture = Fixture::new();
    let path = fixture.file("short.001", b"abcdefgh");
    let second = fixture.file("short.002", b"later");
    let image = RawImage::open_forced(&path).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(3)
        .unwrap();
    let mut output = [99; 20];
    let result = image.read_at_forced(0, &mut output).unwrap();
    assert_eq!(result.bytes_read, 3);
    assert!(result.short_read.is_some());
    assert!(!result.synthetic);
    assert_eq!(&output[..3], b"abc");
    assert!(output[3..].iter().all(|value| *value == 99));
    assert!(image.validate_sources().is_err());
    assert!(image.read_at(0, &mut output).is_err());
    // A missing lazy-open member remains a real error, rather than invented bytes.
    fs::remove_file(second).unwrap();
    assert!(image.read_at_forced(8, &mut output).is_err());
}

#[test]
fn forced_eof_zeroes_only_wholly_outside_requests_and_checks_overflow_abort() {
    let fixture = Fixture::new();
    let image = RawImage::open_forced(fixture.file("single.raw", b"abc")).unwrap();
    assert!(
        !image
            .read_at_forced(u64::MAX, &mut [])
            .unwrap()
            .capacity_warning
    );
    let mut output = [99; 5];
    let result = image.read_at_forced(2, &mut output).unwrap();
    assert_eq!(result.bytes_read, 1);
    assert_eq!(output, [b'c', 99, 99, 99, 99]);
    let result = image.read_at_forced(3, &mut output).unwrap();
    assert_eq!(result.bytes_read, 5);
    assert!(result.synthetic);
    assert_eq!(output, [0; 5]);
    assert!(image.read_at_forced(u64::MAX, &mut output).is_err());
    image.abort_handle().abort();
    assert!(image.read_at_forced(3, &mut output).is_err());
    let empty = RawImage::open_forced(fixture.file("empty.raw", b"")).unwrap();
    assert!(empty.read_at_forced(0, &mut output).unwrap().synthetic);
}

#[test]
fn forced_failure_keeps_prior_segments_and_short_reads_do_not_retry() {
    let fixture = Fixture::new();
    let first = fixture.file("disk.001", b"abcd");
    fixture.file("disk.002", b"efgh");
    let image = RawImage::open_forced(first).unwrap();
    let mut calls = 0;
    let mut output = [99; 8];
    let result = image
        .read_forced_with(0, &mut output, |_, _, bytes| {
            calls += 1;
            if calls == 1 {
                bytes.copy_from_slice(b"abcd");
                Ok(4)
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        })
        .unwrap();
    assert_eq!(result.bytes_read, 4);
    assert!(result.short_read.is_some());
    assert_eq!(calls, 2);
    assert_eq!(&output[..4], b"abcd");
    calls = 0;
    let result = image
        .read_forced_with(0, &mut output, |_, _, bytes| {
            calls += 1;
            bytes[0] = 42;
            Ok(1)
        })
        .unwrap();
    assert_eq!(result.bytes_read, 1);
    assert_eq!(calls, 1);
}

#[test]
#[cfg(windows)]
fn forced_transient_errors_retry_once_and_reopen_cached_handles() {
    for code in [21, 1006, 64] {
        let fixture = Fixture::new();
        let path = fixture.file("disk.raw", b"abcd");
        let image = RawImage::open_forced(&path).unwrap();
        let mut calls = 0;
        let mut output = [99; 4];
        let result = image.read_forced_with(0, &mut output, |_, _, bytes| {
            calls += 1;
            if calls == 1 {
                // Deleting the path makes the retry fail only if it reopens.
                fs::remove_file(&path).unwrap();
                Err(io::Error::from_raw_os_error(code))
            } else {
                bytes.fill(1);
                Ok(bytes.len())
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
        fixture.file("disk.raw", b"abcd");
        calls = 0;
        let result = image
            .read_forced_with(0, &mut output, |_, _, _| {
                calls += 1;
                Err(io::Error::from_raw_os_error(code))
            })
            .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result.bytes_read, 0);
        assert!(result.short_read.is_some());
        calls = 0;
        let result = image
            .read_forced_with(0, &mut output, |_, _, bytes| {
                calls += 1;
                if calls == 1 {
                    Err(io::Error::from_raw_os_error(code))
                } else {
                    bytes.copy_from_slice(b"abcd");
                    Ok(bytes.len())
                }
            })
            .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result.bytes_read, 4);
        assert!(result.short_read.is_none());
        assert_eq!(&output, b"abcd");
    }
}
