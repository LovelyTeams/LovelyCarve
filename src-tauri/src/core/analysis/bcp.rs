//! GA BCP exchange exports. `GAB_ZIP_INDEX.xml` describes files, row counts and
//! column order; the `.bcp` payloads are headerless, tab-delimited text.

use std::{
    io::{BufRead, Read},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result};
use encoding_rs::Encoding;
use quick_xml::{
    events::{BytesStart, Event},
    Reader, XmlVersion,
};
use serde::Serialize;

use crate::core::common::error::Failure;
use crate::core::evidence::readers::zip_image::ZipImage;

pub(crate) mod import;
mod source;
use source::BcpSource;

const PAGE_SIZE: usize = 100;
const MAX_ROW_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpField {
    pub key: String,
    pub name: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpFile {
    pub name: String,
    pub rows: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpDataset {
    pub code: String,
    pub encoding: String,
    pub fields: Vec<BcpField>,
    pub files: Vec<BcpFile>,
    pub rows: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpCatalog {
    pub path: String,
    pub datasets: Vec<BcpDataset>,
    pub total_rows: u64,
    pub total_files: usize,
}

/// The original record source and its parsed index, retained by the evidence session.
#[derive(Debug)]
pub(crate) struct BcpPackage {
    source: BcpSource,
    catalog: BcpCatalog,
}

#[derive(Debug, Clone, Copy, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpCursor {
    pub file_index: usize,
    pub byte_offset: u64,
    pub row_index: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpRow {
    pub number: u64,
    pub file_name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpPage {
    pub rows: Vec<BcpRow>,
    pub next_cursor: Option<BcpCursor>,
}

fn attribute(element: &BytesStart<'_>, name: &str) -> Result<String> {
    for attribute in element.attributes() {
        let attribute = attribute.context(Failure::corrupt("BCP 索引属性损坏"))?;
        if attribute.key.as_ref() == name {
            return Ok(attribute
                .normalized_value(XmlVersion::Implicit1_0)?
                .into_owned());
        }
    }
    Ok(String::new())
}

fn parse_index(xml: &str) -> Result<Vec<BcpDataset>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<BcpDataset> = None;
    let mut current_file: Option<BcpFile> = None;
    let mut datasets = Vec::new();
    loop {
        match reader.read_event().context("读取 BCP 索引失败")? {
            Event::Start(element) => {
                let name = if element.name().as_ref() == "DATASET" {
                    attribute(&element, "name")?
                } else {
                    element.name().as_ref().to_owned()
                };
                if name == "DATA"
                    && stack
                        .last()
                        .is_some_and(|parent| parent == "WA_COMMON_010013")
                {
                    current = Some(BcpDataset {
                        encoding: "UTF-8".into(),
                        ..BcpDataset::default()
                    });
                } else if name == "DATA"
                    && stack
                        .last()
                        .is_some_and(|parent| parent == "WA_COMMON_010014")
                {
                    current_file = Some(BcpFile::default());
                }
                stack.push(name);
            }
            Event::Empty(element) if element.name().as_ref() == "ITEM" => {
                let key = attribute(&element, "key")?;
                let parent = stack
                    .iter()
                    .rev()
                    .find(|name| name.starts_with("WA_COMMON_"))
                    .map(String::as_str);
                match parent {
                    Some("WA_COMMON_010013") => {
                        if let Some(dataset) = current.as_mut() {
                            match key.as_str() {
                                "A010004" => dataset.code = attribute(&element, "val")?,
                                "I010039" => dataset.encoding = attribute(&element, "val")?,
                                _ => {}
                            }
                        }
                    }
                    Some("WA_COMMON_010014") => {
                        if let Some(file) = current_file.as_mut() {
                            match key.as_str() {
                                "H010020" => file.name = attribute(&element, "val")?,
                                "I010034" => {
                                    file.rows = attribute(&element, "val")?
                                        .parse()
                                        .context(Failure::corrupt("BCP 记录数无效"))?
                                }
                                _ => {}
                            }
                        }
                    }
                    Some("WA_COMMON_010015") => {
                        if let Some(dataset) = current.as_mut() {
                            dataset.fields.push(BcpField {
                                key,
                                name: attribute(&element, "eng")?,
                                label: attribute(&element, "chn")?,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                let ended = stack
                    .pop()
                    .context(Failure::corrupt("BCP 索引 XML 层级无效"))?;
                if ended == "DATA" {
                    match stack.last().map(String::as_str) {
                        Some("WA_COMMON_010014") => {
                            if let (Some(dataset), Some(file)) =
                                (current.as_mut(), current_file.take())
                            {
                                if !file.name.is_empty() {
                                    dataset.files.push(file);
                                }
                            }
                        }
                        Some("WA_COMMON_010013") => {
                            if let Some(dataset) = current.take() {
                                if !dataset.code.is_empty() {
                                    merge_dataset(&mut datasets, dataset)?;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(datasets)
}

fn dataset_encoding(label: &str) -> Result<&'static Encoding> {
    let label = match label.trim() {
        "" | "UTF-8无BOM" => "UTF-8",
        other => other,
    };
    Encoding::for_label(label.as_bytes())
        .filter(|encoding| encoding.is_ascii_compatible())
        .with_context(|| Failure::unsupported(format!("不支持的 BCP 字符编码：{label}")))
}

fn merge_dataset(datasets: &mut Vec<BcpDataset>, mut incoming: BcpDataset) -> Result<()> {
    let files = std::mem::take(&mut incoming.files);
    let dataset = match datasets
        .iter_mut()
        .find(|existing| existing.code == incoming.code)
    {
        Some(existing) => {
            anyhow::ensure!(
                existing.fields == incoming.fields
                    && (existing.encoding == incoming.encoding
                        || dataset_encoding(&existing.encoding)?
                            == dataset_encoding(&incoming.encoding)?),
                "BCP 数据集 {} 的分片字段或编码不一致",
                existing.code
            );
            existing
        }
        None => {
            datasets.push(incoming);
            datasets
                .last_mut()
                .context(Failure::not_found("BCP 数据集不存在"))?
        }
    };
    for file in files {
        if let Some(existing) = dataset
            .files
            .iter()
            .find(|existing| existing.name == file.name)
        {
            anyhow::ensure!(
                existing.rows == file.rows,
                "BCP 分片 {} 的索引记录数不一致",
                file.name
            );
        } else {
            dataset.rows = dataset
                .rows
                .checked_add(file.rows)
                .context(Failure::corrupt("BCP 记录总数溢出"))?;
            dataset.files.push(file);
        }
    }
    Ok(())
}

fn read_line(reader: &mut dyn BufRead, line: &mut Vec<u8>) -> Result<usize> {
    line.clear();
    let read = reader.take(MAX_ROW_BYTES + 1).read_until(b'\n', line)?;
    anyhow::ensure!(read as u64 <= MAX_ROW_BYTES, "BCP 记录超过 8 MiB");
    Ok(read)
}

fn fallback_dataset(
    source: &BcpSource,
    name: String,
    token: &tokio_util::sync::CancellationToken,
) -> Result<BcpDataset> {
    let code = name
        .split('-')
        .find(|part| part.starts_with("WA_"))
        .unwrap_or("BCP")
        .to_owned();
    let (rows, field_count) = source.with_rows(&name, 0, |reader, _| {
        let mut line = Vec::new();
        let mut rows = 0u64;
        let mut field_count = 0;
        while read_line(reader, &mut line)? != 0 {
            anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
            if rows == 0 {
                field_count = line.split(|byte| *byte == b'\t').count();
            }
            rows += 1;
        }
        Ok((rows, field_count))
    })?;
    Ok(BcpDataset {
        code,
        encoding: "UTF-8".into(),
        fields: (1..=field_count)
            .map(|index| BcpField {
                key: String::new(),
                name: format!("FIELD_{index}"),
                label: format!("字段 {index}"),
            })
            .collect(),
        files: vec![BcpFile { name, rows }],
        rows,
    })
}

impl BcpPackage {
    pub(crate) fn open(path: &Path, zip: Option<Arc<ZipImage>>) -> Result<Self> {
        Self::open_cancellable(path, zip, &tokio_util::sync::CancellationToken::new())
    }

    pub(crate) fn open_cancellable(
        path: &Path,
        zip: Option<Arc<ZipImage>>,
        token: &tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
        let (source, loose_files) = BcpSource::open(path, zip, token)?;
        let mut datasets = Vec::<BcpDataset>::new();
        if let Some(xml) = source.index()? {
            datasets = parse_index(&xml)?;
        }
        if datasets.is_empty() {
            for file in loose_files {
                let mut incoming = fallback_dataset(&source, file, token)?;
                if let Some(existing) = datasets
                    .iter_mut()
                    .find(|dataset| dataset.code == incoming.code)
                {
                    existing.rows += incoming.rows;
                    existing.files.append(&mut incoming.files);
                } else {
                    datasets.push(incoming);
                }
            }
        }
        anyhow::ensure!(
            !datasets.is_empty(),
            Failure::not_found("未找到 BCP 文件或有效的 GAB_ZIP_INDEX.xml")
        );
        for file in datasets.iter().flat_map(|dataset| &dataset.files) {
            source::validate_name(&file.name)?;
        }
        let total_rows = datasets.iter().try_fold(0u64, |sum, dataset| {
            sum.checked_add(dataset.rows)
                .context(Failure::corrupt("BCP 记录总数溢出"))
        })?;
        let total_files = datasets.iter().map(|dataset| dataset.files.len()).sum();
        let catalog = BcpCatalog {
            path: source.path().to_string_lossy().into_owned(),
            datasets,
            total_rows,
            total_files,
        };
        Ok(Self { source, catalog })
    }

    pub(crate) fn catalog(&self) -> &BcpCatalog {
        &self.catalog
    }

    pub(crate) fn member_size(&self, name: &str) -> Result<u64> {
        self.source.file_size(name)
    }

    /// Visit each shard once, recording offsets in the original logical member.
    pub(crate) fn visit_rows(
        &self,
        dataset: &BcpDataset,
        token: &tokio_util::sync::CancellationToken,
        mut visit: impl FnMut(&BcpFile, u64, u64, Vec<String>) -> Result<()>,
    ) -> Result<()> {
        let encoding = dataset_encoding(&dataset.encoding)?;
        let mut line = Vec::new();
        for file in &dataset.files {
            self.source.with_rows(&file.name, 0, |reader, size| {
                let mut offset = 0;
                let mut count = 0;
                loop {
                    anyhow::ensure!(!token.is_cancelled(), Failure::cancelled("BCP 导入已取消"));
                    let length = read_line(reader, &mut line)? as u64;
                    if length == 0 {
                        break;
                    }
                    let values = decode_values(encoding, &line, &file.name, offset)?;
                    visit(file, offset, length, values)?;
                    offset += length;
                    count += 1;
                }
                anyhow::ensure!(offset == size, "BCP 分片读取不完整：{}", file.name);
                anyhow::ensure!(
                    count == file.rows,
                    "BCP 分片 {} 的实际记录数 {count} 与索引记录数 {} 不符",
                    file.name,
                    file.rows
                );
                Ok(())
            })?;
        }
        Ok(())
    }

    pub(crate) fn page(
        &self,
        code: &str,
        cursor: Option<BcpCursor>,
        start_row: Option<u64>,
    ) -> Result<BcpPage> {
        let dataset = self
            .catalog
            .datasets
            .iter()
            .find(|dataset| dataset.code == code)
            .context(Failure::not_found("BCP 数据集不存在"))?;
        let encoding = dataset_encoding(&dataset.encoding)?;
        anyhow::ensure!(
            cursor.is_none() || start_row.is_none(),
            "BCP 游标与起始记录不能同时使用"
        );
        let mut position = cursor.unwrap_or(BcpCursor {
            file_index: 0,
            byte_offset: 0,
            row_index: 0,
        });
        let mut skip_rows = 0;
        if let Some(start_row) = start_row {
            anyhow::ensure!(
                (1..=dataset.rows).contains(&start_row),
                "BCP 起始记录超出范围"
            );
            position.row_index = start_row - 1;
            skip_rows = position.row_index;
            while skip_rows >= dataset.files[position.file_index].rows {
                skip_rows -= dataset.files[position.file_index].rows;
                position.file_index += 1;
            }
        }
        anyhow::ensure!(
            position.file_index <= dataset.files.len(),
            Failure::invalid_input("BCP 分页位置无效")
        );
        let mut rows = Vec::with_capacity(PAGE_SIZE);
        let mut line = Vec::new();
        let mut page_bytes = 0;
        while position.file_index < dataset.files.len() && rows.len() < PAGE_SIZE {
            let file = &dataset.files[position.file_index];
            let end = self
                .source
                .with_rows(&file.name, position.byte_offset, |reader, size| {
                    for _ in 0..skip_rows {
                        let read = read_line(reader, &mut line)?;
                        anyhow::ensure!(read != 0, "BCP 分片实际记录数少于索引：{}", file.name);
                        position.byte_offset += read as u64;
                    }
                    if skip_rows != 0 {
                        anyhow::ensure!(
                            position.byte_offset < size,
                            "BCP 分片实际记录数少于索引：{}",
                            file.name
                        );
                    }
                    skip_rows = 0;
                    while rows.len() < PAGE_SIZE && page_bytes < MAX_ROW_BYTES {
                        let read = read_line(reader, &mut line)?;
                        if read == 0 {
                            break;
                        }
                        let values =
                            decode_values(encoding, &line, &file.name, position.byte_offset)?;
                        position.byte_offset += read as u64;
                        page_bytes += read as u64;
                        position.row_index = position
                            .row_index
                            .checked_add(1)
                            .context(Failure::corrupt("BCP 记录序号溢出"))?;
                        rows.push(BcpRow {
                            number: position.row_index,
                            file_name: file.name.clone(),
                            values,
                        });
                    }
                    anyhow::ensure!(
                        position.byte_offset <= size,
                        "BCP 分片长度已变化：{}",
                        file.name
                    );
                    if position.byte_offset == size {
                        anyhow::ensure!(
                            reader.fill_buf()?.is_empty(),
                            "BCP 分片长度已变化：{}",
                            file.name
                        );
                        Ok(true)
                    } else {
                        anyhow::ensure!(
                            !reader.fill_buf()?.is_empty(),
                            "BCP 分片读取不完整：{}",
                            file.name
                        );
                        Ok(false)
                    }
                })?;
            if end {
                position.file_index += 1;
                position.byte_offset = 0;
            }
            if page_bytes >= MAX_ROW_BYTES {
                break;
            }
        }
        Ok(BcpPage {
            rows,
            next_cursor: (position.file_index < dataset.files.len()).then_some(position),
        })
    }
}

fn decode_values(
    encoding: &'static Encoding,
    line: &[u8],
    file: &str,
    offset: u64,
) -> Result<Vec<String>> {
    let payload = line.trim_suffix(b"\n");
    let payload = payload.trim_suffix(b"\r");
    let payload = if offset == 0 && encoding == encoding_rs::UTF_8 {
        payload.trim_prefix(b"\xef\xbb\xbf")
    } else {
        payload
    };
    let decoded = encoding
        .decode_without_bom_handling_and_without_replacement(payload)
        .with_context(|| {
            Failure::corrupt(format!("BCP 记录字符编码无效：{file}，偏移 {offset}"))
        })?;
    Ok(decoded.split('\t').map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, File},
        io::Write,
    };

    #[test]
    fn index_columns_and_rows_follow_declared_order() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("one.bcp"), "甲\t1\n乙\t2\n").unwrap();
        fs::write(dir.path().join("GAB_ZIP_INDEX.xml"), r#"<MESSAGE><DATASET name="WA_COMMON_010013"><DATA><ITEM key="A010004" val="WA_TEST_010100"/><ITEM key="I010039" val="UTF-8"/><DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="one.bcp"/><ITEM key="I010034" val="2"/></DATA></DATASET><DATASET name="WA_COMMON_010015"><DATA><ITEM key="A" eng="NAME" chn="名称"/><ITEM key="B" eng="VALUE" chn="值"/></DATA></DATASET></DATA></DATASET></MESSAGE>"#).unwrap();
        let package = BcpPackage::open(dir.path(), None).unwrap();
        let found = package.catalog();
        assert_eq!(found.total_rows, 2);
        assert_eq!(found.datasets[0].fields[0].label, "名称");
        let result = package.page("WA_TEST_010100", None, None).unwrap();
        assert_eq!(
            result
                .rows
                .iter()
                .map(|row| &row.values)
                .collect::<Vec<_>>(),
            vec![
                &vec!["甲".to_string(), "1".to_string()],
                &vec!["乙".to_string(), "2".to_string()]
            ]
        );
        assert!(result.next_cursor.is_none());
    }

    #[test]
    fn rejects_index_path_escape() {
        for name in [
            "../escape.bcp",
            "..\\escape.bcp",
            "C:escape.bcp",
            "escape.bcp:stream.bcp",
        ] {
            assert!(source::validate_name(name).is_err());
        }
    }

    #[test]
    fn repeated_index_file_is_counted_once() {
        let xml = r#"<MESSAGE><DATASET name="WA_COMMON_010013"><DATA><ITEM key="A010004" val="WA_TEST_010100"/><DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="same.bcp"/><ITEM key="I010034" val="4"/></DATA><DATA><ITEM key="H010020" val="same.bcp"/><ITEM key="I010034" val="4"/></DATA></DATASET></DATA></DATASET></MESSAGE>"#;
        let datasets = parse_index(xml).unwrap();
        assert_eq!(datasets[0].files.len(), 1);
        assert_eq!(datasets[0].rows, 4);
    }

    #[test]
    fn zip_shards_share_order_across_cursor_and_record_jump() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.zip");
        let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let table = |name: &str, rows: u64, encoding: &str| {
            format!(
                r#"<DATA><ITEM key="A010004" val="TEST"/><ITEM key="I010039" val="{encoding}"/><DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="{name}"/><ITEM key="I010034" val="{rows}"/></DATA></DATASET><DATASET name="WA_COMMON_010015"><DATA><ITEM key="A" eng="MESSAGE" chn="内容"/><ITEM key="B" eng="SENDER" chn="发送者"/></DATA></DATASET></DATA>"#
            )
        };
        let first = table("part-2.bcp", 101, "UTF-8无BOM");
        let second = table("part-1.bcp", 2, "UTF-8");
        let index = format!(
            r#"<MESSAGE><DATASET name="WA_COMMON_010013">{first}{second}{second}</DATASET></MESSAGE>"#
        );
        archive.start_file("GAB_ZIP_INDEX.xml", options).unwrap();
        archive.write_all(index.as_bytes()).unwrap();
        archive.start_file("part-2.bcp", options).unwrap();
        for number in 1..=101 {
            write!(archive, "消息 {number}\t\r\n").unwrap();
        }
        archive.start_file("part-1.bcp", options).unwrap();
        archive.write_all("结尾\t甲\n最后\t乙".as_bytes()).unwrap();
        archive.finish().unwrap();
        let package = BcpPackage::open(dir.path(), None).unwrap();
        let catalog = package.catalog();
        assert_eq!(
            (
                catalog.datasets.len(),
                catalog.total_files,
                catalog.total_rows
            ),
            (1, 2, 103)
        );
        let first = package.page("TEST", None, None).unwrap();
        assert_eq!(first.rows.len(), 100);
        let next = package.page("TEST", first.next_cursor, None).unwrap();
        let jumped = package.page("TEST", None, Some(101)).unwrap();
        assert_eq!(
            serde_json::to_value(&next).unwrap(),
            serde_json::to_value(&jumped).unwrap()
        );
        assert_eq!(
            (jumped.rows[1].number, jumped.rows[1].file_name.as_str()),
            (102, "part-1.bcp")
        );
        assert_eq!(jumped.rows[0].values, ["消息 101", ""]);
        assert!(jumped.next_cursor.is_none());
        assert!(package.page("TEST", first.next_cursor, Some(101)).is_err());
        assert!(package.page("TEST", None, Some(104)).is_err());
        let direct =
            BcpPackage::open(&path, Some(Arc::new(ZipImage::open(&path).unwrap()))).unwrap();
        assert_eq!(direct.catalog().total_rows, 103);
    }

    #[test]
    fn reads_real_export_when_sample_is_configured() {
        let Some(sample) = std::env::var_os("BCP_SAMPLE_DIR") else {
            return;
        };
        let root = Path::new(&sample);
        let package = BcpPackage::open(root, None).unwrap();
        let found = package.catalog();
        assert!(found.total_rows > 0);
        assert!(found.total_files > 0);
        let first = &found.datasets[0];
        let first_page = package.page(&first.code, None, None).unwrap();
        assert!(!first_page.rows.is_empty());
        assert_eq!(first_page.rows[0].values.len(), first.fields.len());
    }
}
