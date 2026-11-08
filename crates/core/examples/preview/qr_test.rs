//! 临时手测：对一张图片跑 `core::preview::qrcode::scan`，打印识别结果。
//! 用法：`just qr-test <图片路径> [期望数量]`（或在 VS dev shell 里 `cargo run --example qr_test
//! -- <图片路径> [期望数量]`，二维码识别已是默认 feature `qrcode-wechat`）。

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: qr_test <image> [expected]");
    let expected: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let bytes = std::fs::read(&path).expect("read image");
    println!(
        "file: {} ({} bytes), expected={}",
        path,
        bytes.len(),
        expected
    );

    let started = std::time::Instant::now();
    let out = lovelycarve_core::preview::qrcode::scan(
        &bytes,
        expected,
        |attempts, total, found| {
            if attempts == 1 || attempts % 24 == 0 || attempts >= total {
                eprintln!("  progress {attempts}/{total} (found {found})");
            }
        },
        || false,
    )
    .expect("scan failed");

    println!("--- result (in {:?}) ---", started.elapsed());
    println!("detector : {}", out.detector);
    println!("image     : {}x{}", out.width, out.height);
    println!("attempts  : {}", out.attempts);
    println!("cancelled : {}", out.cancelled);
    println!("codes     : {}", out.codes.len());
    for (i, c) in out.codes.iter().enumerate() {
        println!("  #{} text      : {:?}", i + 1, c.text);
        println!("     transform : {}", c.transform);
        println!("     points    : {:?}", c.points);
    }
}
