//! `docs search`, `docs list` and `docs get`: the user guides and format
//! guides embedded at build time, searched offline the way cf searches its
//! documentation site. Pages split into sections at their `##` and `###`
//! headings; a search ranks sections and a page is read as Markdown.
use serde::Serialize;

use super::{
    search::{self, Document},
    CliError,
};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/cli_docs.rs"));
}

/// Longest description in a search result, in characters.
const DESCRIPTION_CHARS: usize = 120;

/// One page section: the heading (the page title for the text before the
/// first heading) and its Markdown, heading line included.
struct Section<'a> {
    heading: &'a str,
    markdown: &'a str,
}

struct Page<'a> {
    path: &'a str,
    title: &'a str,
    markdown: &'a str,
    sections: Vec<Section<'a>>,
}

/// The heading text of a `#`–`###` line, with its level.
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    let text = line[level..].strip_prefix(' ')?;
    (1..=3).contains(&level).then_some((level, text.trim()))
}

fn page<'a>(path: &'a str, markdown: &'a str) -> Page<'a> {
    let mut title = path;
    let mut starts: Vec<(usize, &str)> = vec![(0, "")];
    let mut fenced = false;
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            match heading(trimmed) {
                Some((1, text)) if title == path => title = text,
                Some((2 | 3, text)) => starts.push((offset, text)),
                _ => {}
            }
        }
        offset += line.len();
    }
    let mut sections = Vec::new();
    for (index, (start, text)) in starts.iter().enumerate() {
        let end = starts
            .get(index + 1)
            .map_or(markdown.len(), |(next, _)| *next);
        let body = &markdown[*start..end];
        if body.trim().is_empty() {
            continue;
        }
        sections.push(Section {
            heading: if text.is_empty() { title } else { text },
            markdown: body,
        });
    }
    Page {
        path,
        title,
        markdown,
        sections,
    }
}

fn pages() -> Vec<Page<'static>> {
    embedded::PAGES
        .iter()
        .map(|(path, markdown)| page(path, markdown))
        .collect()
}

/// One line of Markdown as text: link targets, code marks, emphasis and
/// table pipes dropped; `<placeholders>` stay.
fn strip_inline(line: &str, text: &mut String) {
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            // `[text](target)` keeps the text.
            ']' if characters.peek() == Some(&'(') => {
                for inner in characters.by_ref() {
                    if inner == ')' {
                        break;
                    }
                }
            }
            '[' | '`' | '*' | '|' => {}
            _ => text.push(character),
        }
    }
}

/// Plain text of Markdown for the index: headings and HTML dropped, code kept.
fn plain(markdown: &str) -> String {
    let mut text = String::with_capacity(markdown.len());
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || heading(trimmed).is_some() || trimmed.starts_with('<') {
            continue;
        }
        strip_inline(trimmed, &mut text);
        text.push('\n');
    }
    text
}

fn wide(character: char) -> bool {
    unicode_width::UnicodeWidthChar::width(character).unwrap_or(0) > 1
}

/// The section's first paragraph of prose, outside code, tables and HTML,
/// on one line and shortened to a description.
fn describe(section: &Section<'_>) -> String {
    let mut paragraph: Vec<&str> = Vec::new();
    let mut fenced = false;
    for line in section.markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
        } else if !fenced
            && !trimmed.is_empty()
            && heading(trimmed).is_none()
            && !trimmed.starts_with(['|', '<'])
        {
            paragraph.push(trimmed.trim_start_matches("- ").trim_start_matches("> "));
            continue;
        }
        if !paragraph.is_empty() {
            break;
        }
    }
    let mut joined = String::new();
    for line in paragraph {
        let joins_tightly =
            joined.chars().last().is_none_or(wide) || line.chars().next().is_some_and(wide);
        if !joins_tightly {
            joined.push(' ');
        }
        joined.push_str(line);
    }
    let mut text = String::new();
    strip_inline(&joined, &mut text);
    if text.chars().count() <= DESCRIPTION_CHARS {
        return text;
    }
    let mut short: String = text.chars().take(DESCRIPTION_CHARS - 1).collect();
    short.push('…');
    short
}

#[derive(Serialize)]
pub struct Hit {
    title: String,
    section: String,
    description: String,
    page: String,
}

/// Up to five sections for `query`, best first.
pub fn search(query: &str) -> Vec<Hit> {
    let pages = pages();
    let sections: Vec<(&Page<'_>, &Section<'_>)> = pages
        .iter()
        .flat_map(|page| page.sections.iter().map(move |section| (page, section)))
        .collect();
    let documents: Vec<Document> = sections
        .iter()
        .map(|(page, section)| {
            [
                search::tokenize(section.heading),
                search::tokenize(&format!("{} {}", page.title, page.path)),
                search::tokenize(&plain(section.markdown)),
                Vec::new(),
            ]
        })
        .collect();
    search::rank(&documents, query, search::RESULTS)
        .into_iter()
        .map(|index| {
            let (page, section) = sections[index];
            Hit {
                title: page.title.to_owned(),
                section: section.heading.to_owned(),
                description: describe(section),
                page: page.path.to_owned(),
            }
        })
        .collect()
}

#[derive(Serialize)]
pub struct Listing {
    page: &'static str,
    title: &'static str,
}

pub fn list() -> Vec<Listing> {
    pages()
        .into_iter()
        .map(|page| Listing {
            page: page.path,
            title: page.title,
        })
        .collect()
}

/// A page in a JSONL session, where raw output would arrive hex encoded.
#[derive(Serialize)]
pub struct Reading {
    pub page: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub markdown: String,
}

fn normalize(page: &str) -> String {
    let page = page.trim().replace('\\', "/");
    let page = page.strip_prefix("docs/").unwrap_or(&page).to_lowercase();
    if page.ends_with(".md") {
        page
    } else {
        format!("{page}.md")
    }
}

/// A page, or one of its sections, by path and heading.
pub fn get(page: &str, section: Option<&str>) -> Result<Reading, CliError> {
    let pages = pages();
    let wanted = normalize(page);
    let found = pages
        .iter()
        .find(|candidate| candidate.path.to_lowercase() == wanted)
        .ok_or_else(|| {
            CliError::new(
                "notFound",
                format!("没有文档页面 {page}；用 docs list 查看全部页面"),
            )
        })?;
    let Some(section) = section.map(str::trim).filter(|section| !section.is_empty()) else {
        return Ok(Reading {
            page: found.path.to_owned(),
            title: found.title.to_owned(),
            section: None,
            markdown: found.markdown.to_owned(),
        });
    };
    let lowered = section.to_lowercase();
    let matched = found
        .sections
        .iter()
        .find(|candidate| candidate.heading == section)
        .or_else(|| {
            found
                .sections
                .iter()
                .find(|candidate| candidate.heading.to_lowercase() == lowered)
        })
        .or_else(|| {
            let mut containing = found
                .sections
                .iter()
                .filter(|candidate| candidate.heading.to_lowercase().contains(&lowered));
            match (containing.next(), containing.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        })
        .ok_or_else(|| {
            let headings: Vec<&str> = found
                .sections
                .iter()
                .map(|section| section.heading)
                .collect();
            CliError::new(
                "notFound",
                format!(
                    "{} 中没有章节 {section}；可用的章节：{}",
                    found.path,
                    headings.join("、")
                ),
            )
        })?;
    Ok(Reading {
        page: found.path.to_owned(),
        title: found.title.to_owned(),
        section: Some(matched.heading.to_owned()),
        markdown: matched.markdown.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_split_into_sections_outside_code_fences() {
        let markdown = "# 标题\n\n导言。\n\n## 第一节\n\n```text\n## 不是标题\n```\n\n正文 [链接](a.md) `代码`，\n第二行 continues\nhere。\n\n### 小节\n\n| 表 | 格 |\n\n> 细节 `<路径>`\n";
        let page = page("x.md", markdown);
        assert_eq!(page.title, "标题");
        let headings: Vec<&str> = page
            .sections
            .iter()
            .map(|section| section.heading)
            .collect();
        assert_eq!(headings, ["标题", "第一节", "小节"]);
        assert!(page.sections[1].markdown.contains("## 不是标题"));
        assert_eq!(
            describe(&page.sections[1]),
            "正文 链接 代码，第二行 continues here。"
        );
        assert_eq!(describe(&page.sections[0]), "导言。");
        assert_eq!(describe(&page.sections[2]), "细节 <路径>");
    }

    #[test]
    fn every_embedded_guide_is_searchable_and_readable() {
        let listing = list();
        assert!(listing.iter().any(|page| page.page == "CLI.md"));
        assert!(listing.iter().any(|page| page.page == "formats/NTFS.md"));
        let hits = search("BitLocker 恢复密码");
        assert!(!hits.is_empty() && hits.len() <= 5);
        assert!(hits
            .iter()
            .any(|hit| hit.page.contains("BITLOCKER") || hit.page == "CLI.md"));
        let page = get("formats/ntfs", None).unwrap();
        assert_eq!(page.page, "formats/NTFS.md");
        assert!(page.markdown.starts_with("# NTFS"));
        let section = get("docs\\CLI.md", Some("JSONL 会话")).unwrap();
        assert_eq!(section.section.as_deref(), Some("JSONL 会话"));
        assert!(section.markdown.starts_with("## JSONL 会话"));
        assert!(get("nothing.md", None).is_err());
        assert!(get("CLI.md", Some("不存在的章节")).is_err());
    }
}
