//! `cli search`: find commands from a plain-language task description.
//! BM25+ over the command, its summary, description and context (group
//! descriptions, arguments, options, keywords), with prefix and fuzzy matching
//! and per-field boosts. Han text is indexed as character bigrams so Chinese
//! queries match too.
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use super::catalog::{Catalog, Command};

const RESULTS: usize = 5;
const K: f64 = 1.2;
const B: f64 = 0.7;
const D: f64 = 0.5;
const PREFIX_WEIGHT: f64 = 0.375;
const FUZZY_WEIGHT: f64 = 0.45;
const FUZZY: f64 = 0.2;
/// English words that carry no meaning in a task description.
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "in", "of", "to", "for", "on", "at", "by", "from", "into", "with", "and",
    "or", "is", "are", "be", "it", "its", "my", "me", "i", "this", "that", "how", "do", "can",
];
const FIELDS: [(Field, f64); 4] = [
    (Field::Command, 8.0),
    (Field::Summary, 5.0),
    (Field::Description, 2.0),
    (Field::Context, 0.5),
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Field {
    Command,
    Summary,
    Description,
    Context,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Match {
    pub command: String,
    pub summary: String,
}

fn is_han(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF)
}

/// Words split at whitespace and punctuation, lowercased; Han runs become
/// overlapping character bigrams (a lone character stays a unigram).
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut han: Vec<char> = Vec::new();
    let flush_han = |han: &mut Vec<char>, tokens: &mut Vec<String>| {
        match han.len() {
            0 => {}
            1 => tokens.push(han[0].to_string()),
            _ => tokens.extend(han.windows(2).map(|pair| pair.iter().collect::<String>())),
        }
        han.clear();
    };
    for character in text.chars() {
        if is_han(character) {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            han.push(character);
        } else if character.is_alphanumeric() {
            flush_han(&mut han, &mut tokens);
            word.extend(character.to_lowercase());
        } else {
            flush_han(&mut han, &mut tokens);
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
        }
    }
    flush_han(&mut han, &mut tokens);
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}

fn edit_distance(left: &str, right: &str, limit: usize) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > limit {
        return None;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.iter().enumerate() {
        let mut current = vec![row + 1; right.len() + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        previous = current;
    }
    let distance = previous[right.len()];
    (distance <= limit).then_some(distance)
}

fn documents(catalog: &Catalog) -> Vec<(&Command, BTreeMap<Field, Vec<String>>)> {
    catalog
        .commands
        .iter()
        .map(|command| {
            let mut context = Vec::new();
            for length in 1..command.full_path.len() {
                if let Some(description) =
                    catalog.description(&command.full_path[..length].join(" "))
                {
                    context.push(description.to_owned());
                }
            }
            for param in command.params.iter().filter(|param| param.visible()) {
                context.push(format!("{} {}", param.option, param.description));
            }
            // The descriptions are Chinese: the English keywords and the
            // backend operation name describe the command for English queries.
            let mut description = vec![command.description.clone()];
            description.extend(command.keywords.iter().cloned());
            description.extend(command.ipc_command.iter().cloned());
            let fields = BTreeMap::from([
                (Field::Command, tokenize(&command.command)),
                (Field::Summary, tokenize(&command.summary)),
                (Field::Description, tokenize(&description.join(" "))),
                (Field::Context, tokenize(&context.join(" "))),
            ]);
            (command, fields)
        })
        .collect()
}

/// Up to five commands for `query`, best first.
pub fn search(catalog: &Catalog, query: &str) -> Vec<Match> {
    let documents = documents(catalog);
    let total = documents.len() as f64;
    let mut frequency: HashMap<(Field, &str), usize> = HashMap::new();
    let mut lengths: HashMap<Field, f64> = HashMap::new();
    for (_, fields) in &documents {
        for (field, tokens) in fields {
            *lengths.entry(*field).or_default() += tokens.len() as f64;
            let mut seen: Vec<&str> = tokens.iter().map(String::as_str).collect();
            seen.sort_unstable();
            seen.dedup();
            for token in seen {
                *frequency.entry((*field, token)).or_default() += 1;
            }
        }
    }
    let averages: HashMap<Field, f64> = lengths
        .into_iter()
        .map(|(field, length)| (field, (length / total).max(1.0)))
        .collect();
    let terms: Vec<String> = tokenize(query)
        .into_iter()
        .filter(|term| !STOP_WORDS.contains(&term.as_str()))
        .collect();
    let mut scores: Vec<(f64, usize)> = Vec::new();
    for (index, (_, fields)) in documents.iter().enumerate() {
        let mut score = 0.0;
        for term in &terms {
            // Short words would prefix-match half the vocabulary.
            let loose = term.chars().count() >= 3;
            let limit = if loose {
                (term.chars().count() as f64 * FUZZY).round() as usize
            } else {
                0
            };
            for (field, boost) in FIELDS {
                let tokens = &fields[&field];
                let average = averages.get(&field).copied().unwrap_or(1.0);
                let length = tokens.len() as f64;
                let mut best: HashMap<&str, f64> = HashMap::new();
                for token in tokens {
                    let weight = if token == term {
                        1.0
                    } else if loose && token.starts_with(term.as_str()) {
                        let extra = (token.chars().count() - term.chars().count()) as f64;
                        let length = term.chars().count() as f64;
                        PREFIX_WEIGHT * length / (length + 0.3 * extra)
                    } else if let Some(distance) = (limit > 0)
                        .then(|| edit_distance(term, token, limit))
                        .flatten()
                    {
                        let length = term.chars().count() as f64;
                        FUZZY_WEIGHT * length / (length + distance as f64)
                    } else {
                        continue;
                    };
                    let entry = best.entry(token.as_str()).or_default();
                    *entry = entry.max(weight);
                }
                for (token, weight) in best {
                    let term_frequency = tokens
                        .iter()
                        .filter(|candidate| *candidate == token)
                        .count() as f64;
                    let documents_with =
                        frequency.get(&(field, token)).copied().unwrap_or(1) as f64;
                    let idf = (1.0 + (total - documents_with + 0.5) / (documents_with + 0.5)).ln();
                    let bm25 = idf
                        * (D + term_frequency * (K + 1.0)
                            / (term_frequency + K * (1.0 - B + B * length / average)));
                    score += boost * weight * bm25;
                }
            }
        }
        if score > 0.0 {
            scores.push((score, index));
        }
    }
    scores.sort_by(|left, right| right.0.total_cmp(&left.0).then(left.1.cmp(&right.1)));
    scores
        .into_iter()
        .take(RESULTS)
        .map(|(_, index)| {
            let command = documents[index].0;
            Match {
                command: command.command.clone(),
                summary: command.summary.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    fn top(query: &str) -> String {
        search(catalog().unwrap(), query)
            .into_iter()
            .next()
            .map(|found| found.command)
            .unwrap_or_default()
    }

    #[test]
    fn tokenizer_splits_words_and_han_bigrams() {
        assert_eq!(
            tokenize("Open usn-journal 打开检材"),
            ["open", "usn", "journal", "打开", "开检", "检材"]
        );
        assert_eq!(tokenize("库"), ["库"]);
    }

    #[test]
    fn plain_language_finds_the_expected_command() {
        for (query, expected) in [
            ("open a disk image", "images open"),
            ("打开检材", "images open"),
            (
                "sqlite deleted records",
                "files sqlite deleted-records list",
            ),
            ("计算文件哈希", "files hash"),
            ("image_load", "images open"),
            ("list files in a directory", "files list"),
            ("export a file", "files export"),
            ("unlock bitlocker", "bitlocker unlock"),
            ("search file contents for a keyword", "files search"),
        ] {
            assert_eq!(top(query), format!("lovelycarve-cli {expected}"), "{query}");
        }
        let recovered: Vec<String> = search(catalog().unwrap(), "recover deleted files")
            .into_iter()
            .map(|found| found.command)
            .collect();
        assert!(recovered.contains(&"lovelycarve-cli carve scans create".to_owned()));
        assert!(search(catalog().unwrap(), "zzzz-no-such-thing").is_empty());
        assert!(search(catalog().unwrap(), "list").len() <= 5);
    }
}
