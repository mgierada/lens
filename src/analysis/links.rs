//! Language-agnostic text links between changes.
//!
//! Configs, data files and docs are tied together by names, not imports: a training config lists
//! `batch-27-assets`, which is the new `highfive_batch-27-assets.yaml`. Two kinds of links:
//! - mention: a changed line mentions a changed file's name (or a distinctive part of it),
//! - shared: two changes contain the same rare token (e.g. `cycle_59`, an MLflow run id).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use aho_corasick::AhoCorasick;

use crate::model::{Confidence, Edge, EdgeKind};

/// A changed line and the change that owns it.
pub struct OwnedLine<'a> {
    pub owner: usize,
    pub text: &'a str,
}

/// A whole-file change that other changes can mention.
pub struct FileNode<'a> {
    pub id: usize,
    pub path: &'a str,
}

/// Shortest file-name fragment accepted as a mention key.
const MIN_KEY_LEN: usize = 6;
/// Shortest token considered for shared-token links.
const MIN_TOKEN_LEN: usize = 5;
/// Tokens in more changes than this are too common to mean "same thing".
const MAX_TOKEN_SPREAD: usize = 6;
/// Longest run of name parts combined into one token (`mod_y26_cycle`).
const MAX_TOKEN_PARTS: usize = 3;
const STOP_TOKENS: &[&str] = &[
    "utf_8", "sha256", "sha1", "base64", "float16", "float32", "float64", "int8", "int16", "int32",
    "int64", "uint8", "python3", "x86_64", "md5", "http2",
];

/// Mention and shared-token edges.
pub fn text_links(lines: &[OwnedLine], files: &[FileNode]) -> Vec<Edge> {
    let mut edges = mentions(lines, files);
    edges.extend(shared_tokens(lines, files));
    edges
}

fn edge(from: usize, to: usize, kind: EdgeKind, via: String) -> Edge {
    Edge {
        from,
        to,
        kind,
        via: Some(via),
        confidence: Confidence::Low,
        hub: false,
        call_check: None,
        calls: Vec::new(),
    }
}

/// Names a file is referred to by: its file name, stem, and distinctive stem suffixes
/// (`highfive_batch-27-assets` → `batch-27-assets`).
pub fn mention_keys(path: &str) -> Vec<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let parts: Vec<&str> = stem.split('_').collect();
    let suffixes = (1..parts.len()).map(|i| parts[i..].join("_"));
    let distinctive = |key: &String| {
        key.len() >= MIN_KEY_LEN
            && key
                .chars()
                .any(|c| c.is_ascii_digit() || c == '-' || c == '_')
    };
    let mut keys: Vec<String> = std::iter::once(stem.to_string())
        .chain(suffixes)
        .filter(distinctive)
        .collect();
    if name.len() >= MIN_KEY_LEN {
        keys.insert(0, name.to_string());
    }
    keys.dedup();
    keys
}

fn mentions(lines: &[OwnedLine], files: &[FileNode]) -> Vec<Edge> {
    let mut keys: Vec<String> = Vec::new();
    let mut key_owner: Vec<usize> = Vec::new();
    for file in files {
        for key in mention_keys(file.path) {
            keys.push(key);
            key_owner.push(file.id);
        }
    }
    if keys.is_empty() {
        return Vec::new();
    }
    let Ok(matcher) = AhoCorasick::new(&keys) else {
        return Vec::new();
    };
    let mut found: BTreeMap<(usize, usize), String> = BTreeMap::new();
    for line in lines {
        for m in matcher.find_overlapping_iter(line.text) {
            let target = key_owner[m.pattern().as_usize()];
            if target != line.owner {
                let key = keys[m.pattern().as_usize()].clone();
                let entry = found
                    .entry((line.owner, target))
                    .or_insert_with(|| key.clone());
                if key.len() > entry.len() {
                    *entry = key;
                }
            }
        }
    }
    let found = found.into_iter();
    found
        .map(|((from, to), via)| edge(from, to, EdgeKind::Mention, via))
        .collect()
}

/// Distinctive tokens of a text: runs of 1–3 name parts containing both letters and digits,
/// normalised to lowercase with `_` separators (`Cycle-59` → `cycle_59`).
pub fn rare_tokens(text: &str) -> BTreeSet<String> {
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    let words = text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    for word in words.filter(|w| w.len() >= MIN_TOKEN_LEN) {
        let parts: Vec<String> = word
            .split(['_', '-'])
            .filter(|p| !p.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        for size in 1..=MAX_TOKEN_PARTS.min(parts.len()) {
            for window in parts.windows(size) {
                let token = window.join("_");
                let has_digit = token.chars().any(|c| c.is_ascii_digit());
                let has_alpha = token.chars().any(|c| c.is_ascii_alphabetic());
                let accepted = token.len() >= MIN_TOKEN_LEN
                    && has_digit
                    && has_alpha
                    && !STOP_TOKENS.contains(&token.as_str());
                if accepted {
                    tokens.insert(token);
                }
            }
        }
    }
    tokens
}

fn shared_tokens(lines: &[OwnedLine], files: &[FileNode]) -> Vec<Edge> {
    let mut owners: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for line in lines {
        for token in rare_tokens(line.text) {
            owners.entry(token).or_default().insert(line.owner);
        }
    }
    for file in files {
        for token in rare_tokens(file.path) {
            owners.entry(token).or_default().insert(file.id);
        }
    }
    let mut linked: HashSet<(usize, usize)> = HashSet::new();
    let mut edges: Vec<Edge> = Vec::new();
    for (token, ids) in owners {
        if ids.len() < 2 || ids.len() > MAX_TOKEN_SPREAD {
            continue;
        }
        let ids: Vec<usize> = ids.into_iter().collect();
        for (i, &a) in ids.iter().enumerate() {
            for &b in &ids[i + 1..] {
                if linked.insert((a, b)) {
                    edges.push(edge(a, b, EdgeKind::Shared, token.clone()));
                }
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_keys_include_distinctive_suffixes() {
        let keys = mention_keys("configs/highfive_batch-27-assets.yaml");
        assert_eq!(
            keys,
            vec![
                "highfive_batch-27-assets.yaml",
                "highfive_batch-27-assets",
                "batch-27-assets"
            ]
        );
        assert_eq!(mention_keys("README.md"), vec!["README.md"]);
    }

    #[test]
    fn finds_mentions_of_changed_files() {
        let lines = [
            OwnedLine {
                owner: 0,
                text: "  - name: \"batch-27-assets\"",
            },
            OwnedLine {
                owner: 1,
                text: "unrelated",
            },
        ];
        let files = [FileNode {
            id: 2,
            path: "data/highfive_batch-27-assets.yaml",
        }];
        let edges = text_links(&lines, &files);
        let mention = edges.iter().find(|e| e.kind == EdgeKind::Mention).unwrap();
        assert_eq!((mention.from, mention.to), (0, 2));
        assert_eq!(mention.via.as_deref(), Some("batch-27-assets"));
    }

    #[test]
    fn rare_tokens_need_letters_and_digits() {
        let tokens = rare_tokens("mod_y26_cycle_59_logos_opt1.csv utf-8 hello");
        assert!(tokens.contains("cycle_59"));
        assert!(tokens.contains("logos_opt1"));
        assert!(!tokens.contains("hello"));
        assert!(!tokens.contains("utf_8"));
    }

    #[test]
    fn shared_tokens_link_changes_once() {
        let lines = [
            OwnedLine {
                owner: 0,
                text: "csv = 'mod_y26_cycle_59_logos_opt1_tmp1.csv'",
            },
            OwnedLine {
                owner: 1,
                text: "cycle: cycle_59",
            },
        ];
        let edges = text_links(&lines, &[]);
        let shared: Vec<&Edge> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Shared)
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!((shared[0].from, shared[0].to), (0, 1));
    }
}
