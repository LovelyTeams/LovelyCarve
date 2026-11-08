//! GA BCP exchange exports. `GAB_ZIP_INDEX.xml` describes files, row counts and
//! column order; the `.bcp` payloads are headerless, tab-delimited text.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use encoding_rs::Encoding;
use quick_xml::{
    events::{BytesStart, Event},
    Reader, XmlVersion,
};
use serde::Serialize;

const PAGE_SIZE: usize = 100;
const MAX_ROW_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize)]
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
        let attribute = attribute.context("BCP 索引属性损坏")?;
        if attribute.key.as_ref() == name {
            return Ok(attribute
                .normalized_value(XmlVersion::Implicit1_0)?
                .into_owned());
        }
    }
    Ok(String::new())
}

fn safe_file(root: &Path, name: &str) -> Result<PathBuf> {
    let relative = Path::new(name);
    if relative.file_name().is_none()
        || relative.components().count() != 1
        || !name.to_ascii_lowercase().ends_with(".bcp")
    {
        bail!("BCP 索引包含不安全的文件名：{name}");
    }
    Ok(root.join(relative))
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
                                        .context("BCP 记录数无效")?
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
                let ended = stack.pop().context("BCP 索引 XML 层级无效")?;
                if ended == "DATA" {
                    match stack.last().map(String::as_str) {
                        Some("WA_COMMON_010014") => {
                            if let (Some(dataset), Some(file)) =
                                (current.as_mut(), current_file.take())
                            {
                                if !file.name.is_empty()
                                    && !dataset
                                        .files
                                        .iter()
                                        .any(|existing| existing.name == file.name)
                                {
                                    dataset.files.push(file);
                                }
                            }
                        }
                        Some("WA_COMMON_010013") => {
                            if let Some(mut dataset) = current.take() {
                                if !dataset.code.is_empty() {
                                    dataset.rows = dataset.files.iter().map(|file| file.rows).sum();
                                    datasets.push(dataset);
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

fn fallback_dataset(path: &Path) -> Result<BcpDataset> {
    let file_name = path
        .file_name()
        .context("BCP 文件名无效")?
        .to_string_lossy()
        .into_owned();
    let code = file_name
        .split('-')
        .find(|part| part.starts_with("WA_"))
        .unwrap_or("BCP")
        .to_owned();
    let mut reader =
        BufReader::new(File::open(path).with_context(|| format!("打开 {} 失败", path.display()))?);
    let mut line = Vec::new();
    let mut rows = 0u64;
    let mut field_count = 0;
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take(MAX_ROW_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if read as u64 > MAX_ROW_BYTES {
            bail!("BCP 记录超过 8 MiB");
        }
        if rows == 0 {
            field_count = line.split(|byte| *byte == b'\t').count();
        }
        rows += 1;
    }
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
        files: vec![BcpFile {
            name: file_name,
            rows,
        }],
        rows,
    })
}

pub fn catalog(path: &Path) -> Result<BcpCatalog> {
    let root = if path.is_dir() {
        path
    } else {
        path.parent().context("BCP 路径无父目录")?
    };
    let index = root.join("GAB_ZIP_INDEX.xml");
    let mut datasets = if index.is_file() {
        let xml =
            fs::read_to_string(&index).with_context(|| format!("读取 {} 失败", index.display()))?;
        parse_index(&xml)?
    } else {
        Vec::new()
    };
    if path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bcp"))
    {
        let file_name = path
            .file_name()
            .context("BCP 文件名无效")?
            .to_string_lossy();
        for dataset in &mut datasets {
            dataset.files.retain(|file| file.name == file_name);
            dataset.rows = dataset.files.iter().map(|file| file.rows).sum();
        }
        datasets.retain(|dataset| !dataset.files.is_empty());
    }
    if datasets.is_empty() {
        let files = if path.is_file() {
            vec![path.to_path_buf()]
        } else {
            fs::read_dir(root)?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("bcp"))
                })
                .collect()
        };
        for file in files {
            let mut found = fallback_dataset(&file)?;
            if let Some(existing) = datasets
                .iter_mut()
                .find(|existing| existing.code == found.code)
            {
                existing.rows += found.rows;
                existing.files.append(&mut found.files);
            } else {
                datasets.push(found);
            }
        }
    }
    if datasets.is_empty() {
        bail!("未找到 BCP 文件或有效的 GAB_ZIP_INDEX.xml");
    }
    for dataset in &datasets {
        for file in &dataset.files {
            safe_file(root, &file.name)?;
        }
    }
    let total_rows = datasets.iter().map(|dataset| dataset.rows).sum();
    let total_files = datasets.iter().map(|dataset| dataset.files.len()).sum();
    Ok(BcpCatalog {
        path: root.to_string_lossy().into_owned(),
        datasets,
        total_rows,
        total_files,
    })
}

pub fn page(path: &Path, code: &str, cursor: Option<BcpCursor>) -> Result<BcpPage> {
    let catalog = catalog(path)?;
    let dataset = catalog
        .datasets
        .iter()
        .find(|dataset| dataset.code == code)
        .context("BCP 数据集不存在")?;
    let encoding =
        Encoding::for_label(dataset.encoding.as_bytes()).context("不支持的 BCP 字符编码")?;
    let mut position = cursor.unwrap_or(BcpCursor {
        file_index: 0,
        byte_offset: 0,
        row_index: 0,
    });
    if position.file_index > dataset.files.len() {
        bail!("BCP 分页位置无效");
    }
    let mut rows = Vec::with_capacity(PAGE_SIZE);
    while position.file_index < dataset.files.len() && rows.len() < PAGE_SIZE {
        let file = &dataset.files[position.file_index];
        let file_path = safe_file(Path::new(&catalog.path), &file.name)?;
        let mut reader = BufReader::new(
            File::open(&file_path).with_context(|| format!("打开 {} 失败", file_path.display()))?,
        );
        if position.byte_offset > reader.get_ref().metadata()?.len() {
            bail!("BCP 分页偏移无效");
        }
        reader.seek(SeekFrom::Start(position.byte_offset))?;
        loop {
            let mut line = Vec::new();
            let read = reader
                .by_ref()
                .take(MAX_ROW_BYTES + 1)
                .read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            if read as u64 > MAX_ROW_BYTES {
                bail!("BCP 记录超过 8 MiB");
            }
            position.byte_offset += read as u64;
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let (decoded, _, _) = encoding.decode(&line);
            let values = decoded.split('\t').map(str::to_owned).collect();
            position.row_index += 1;
            rows.push(BcpRow {
                number: position.row_index,
                file_name: file.name.clone(),
                values,
            });
            if rows.len() == PAGE_SIZE {
                break;
            }
        }
        if position.byte_offset >= reader.get_ref().metadata()?.len() {
            position.file_index += 1;
            position.byte_offset = 0;
        } else {
            break;
        }
    }
    Ok(BcpPage {
        rows,
        next_cursor: (position.file_index < dataset.files.len()).then_some(position),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_columns_and_rows_follow_declared_order() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("one.bcp"), "甲\t1\n乙\t2\n").unwrap();
        fs::write(dir.path().join("GAB_ZIP_INDEX.xml"), r#"<MESSAGE><DATASET name="WA_COMMON_010013"><DATA><ITEM key="A010004" val="WA_TEST_010100"/><ITEM key="I010039" val="UTF-8"/><DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="one.bcp"/><ITEM key="I010034" val="2"/></DATA></DATASET><DATASET name="WA_COMMON_010015"><DATA><ITEM key="A" eng="NAME" chn="名称"/><ITEM key="B" eng="VALUE" chn="值"/></DATA></DATASET></DATA></DATASET></MESSAGE>"#).unwrap();
        let found = catalog(dir.path()).unwrap();
        assert_eq!(found.total_rows, 2);
        assert_eq!(found.datasets[0].fields[0].label, "名称");
        let result = page(dir.path(), "WA_TEST_010100", None).unwrap();
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
        assert!(safe_file(Path::new("/tmp"), "../escape.bcp").is_err());
    }

    #[test]
    fn repeated_index_file_is_counted_once() {
        let xml = r#"<MESSAGE><DATASET name="WA_COMMON_010013"><DATA><ITEM key="A010004" val="WA_TEST_010100"/><DATASET name="WA_COMMON_010014"><DATA><ITEM key="H010020" val="same.bcp"/><ITEM key="I010034" val="4"/></DATA><DATA><ITEM key="H010020" val="same.bcp"/><ITEM key="I010034" val="4"/></DATA></DATASET></DATA></DATASET></MESSAGE>"#;
        let datasets = parse_index(xml).unwrap();
        assert_eq!(datasets[0].files.len(), 1);
        assert_eq!(datasets[0].rows, 4);
    }

    #[test]
    fn reads_real_export_when_sample_is_configured() {
        let Some(sample) = std::env::var_os("BCP_SAMPLE_DIR") else {
            return;
        };
        let root = Path::new(&sample);
        let found = catalog(root).unwrap();
        assert!(found.total_rows > 0);
        assert!(found.total_files > 0);
        let first = &found.datasets[0];
        let first_page = page(root, &first.code, None).unwrap();
        assert!(!first_page.rows.is_empty());
        assert_eq!(first_page.rows[0].values.len(), first.fields.len());
    }
}
