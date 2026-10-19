//! Read only Jet4 reference usage bitmaps; reject other page types before body reads.
use lovelycarve_lib::core::{
    extensions::plugin::HostView, filesystem::evidence_session::EvidenceSession,
};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        arguments.len() >= 4,
        "usage: probe_access_bitmap <image> <root-id> <path> <page>..."
    );
    let image = Arc::new(EvidenceSession::open(&arguments[0])?);
    // The evidence view a plugin sees: every volume under its volume key.
    let view = HostView::evidence("probe".to_owned(), "probe".to_owned(), image);
    let path = format!("{}/{}", arguments[1], arguments[2]);
    let source = view
        .stat(&path)?
        .ok_or_else(|| anyhow::anyhow!("source not found"))?;
    let mut results = Vec::new();
    for number in &arguments[3..] {
        let page = number.parse::<u32>()?;
        let offset = page as u64 * 4096;
        anyhow::ensure!(offset + 4096 <= source.size, "bitmap page outside source");
        let header = view.read(&path, offset, 8)?;
        anyhow::ensure!(
            header.len() == 8 && header[0] == 5,
            "reference does not point to a usage bitmap; body not read"
        );
        let bytes = view.read(&path, offset, 4096)?;
        anyhow::ensure!(
            bytes.len() == 4096 && bytes[..8] == header,
            "short read or inconsistent bitmap header"
        );
        results.push(serde_json::json!({"page":page,"bytes":bytes}));
    }
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
