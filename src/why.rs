//! Commit context ("why") for changes: range commits, per-file commits and per-hunk blame.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use rayon::prelude::*;

use crate::diff::{Hunk, LineKind};
use crate::git::{Commit, Git};

/// Commit context for a review range.
pub struct Why {
    /// Oldest first.
    pub commits: Vec<Commit>,
    pub by_file: HashMap<String, Vec<String>>,
    /// Parallel to the hunk list given to [`collect`].
    pub by_hunk: Vec<Vec<String>>,
}

impl Why {
    /// Sort and dedupe SHAs by commit order (oldest first), dropping SHAs outside the range.
    pub fn ordered(&self, shas: impl IntoIterator<Item = String>) -> Vec<String> {
        let position: HashMap<&str, usize> = self
            .commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.sha.as_str(), i))
            .collect();
        let mut known: Vec<(usize, String)> = shas
            .into_iter()
            .filter_map(|sha| Some((*position.get(sha.as_str())?, sha)))
            .collect();
        known.sort();
        known.dedup();
        known.into_iter().map(|(_, sha)| sha).collect()
    }
}

/// Gather commit context. Blame failures for a file degrade to "no hunk commits".
pub fn collect(
    git: &Git,
    base: &str,
    head: &str,
    hunks: &[(&str, &Hunk)],
    blame: bool,
) -> Result<Why> {
    let mut commits = git.commits(base, head)?;
    commits.reverse();
    let by_file = git.commits_by_file(base, head)?;
    let mut why = Why {
        commits,
        by_file,
        by_hunk: vec![Vec::new(); hunks.len()],
    };
    if !blame || why.commits.is_empty() {
        return Ok(why);
    }
    let blamed = blame_files(git, base, head, hunks);
    let in_range: HashSet<&str> = why.commits.iter().map(|c| c.sha.as_str()).collect();
    let by_hunk: Vec<Vec<String>> = hunks
        .iter()
        .map(|(path, hunk)| {
            let Some(lines) = blamed.get(*path) else {
                return Vec::new();
            };
            let added = hunk.lines.iter().filter(|l| l.kind == LineKind::Added);
            let shas = added.filter_map(|l| lines.get(&l.new_line?));
            shas.filter(|sha| in_range.contains(sha.as_str()))
                .cloned()
                .collect()
        })
        .collect();
    why.by_hunk = by_hunk.into_iter().map(|shas| why.ordered(shas)).collect();
    Ok(why)
}

fn blame_files(
    git: &Git,
    base: &str,
    head: &str,
    hunks: &[(&str, &Hunk)],
) -> HashMap<String, HashMap<u32, String>> {
    let mut ranges: BTreeMap<&str, Vec<(u32, u32)>> = BTreeMap::new();
    for (path, hunk) in hunks {
        let added = hunk.lines.iter().filter(|l| l.kind == LineKind::Added);
        let entry = ranges.entry(path).or_default();
        entry.extend(added.filter_map(|l| l.new_line).map(|n| (n, n)));
    }
    ranges
        .into_par_iter()
        .filter(|(_, lines)| !lines.is_empty())
        .filter_map(|(path, lines)| {
            let blamed = git.blame(base, head, path, &merge_ranges(lines)).ok()?;
            Some((path.to_string(), blamed))
        })
        .collect()
}

/// Merge overlapping or adjacent inclusive ranges.
fn merge_ranges(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_adjacent_ranges() {
        let merged = merge_ranges(vec![(5, 5), (1, 1), (2, 3), (7, 7), (6, 6)]);
        assert_eq!(merged, vec![(1, 3), (5, 7)]);
    }
}
