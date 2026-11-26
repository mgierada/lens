//! Human-readable cluster titles: what the change is (commit subject) and where it lives (files).

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Basenames listed in a files label before eliding.
const LABEL_FILES: usize = 3;
/// Directory components kept in a files label.
const LABEL_DIR_DEPTH: usize = 2;
/// Words that carry no meaning on their own in a commit subject.
const GENERIC_WORDS: &[&str] = &[
    "fix",
    "fixes",
    "fixed",
    "docs",
    "doc",
    "update",
    "updates",
    "updated",
    "wip",
    "lint",
    "linter",
    "test",
    "tests",
    "unit",
    "typo",
    "typos",
    "refactor",
    "cleanup",
    "review",
    "format",
    "formatting",
    "chore",
    "minor",
    "tweak",
    "tweaks",
    "changes",
    "misc",
    "pr",
    "comments",
];

/// Whether a commit subject says nothing about *what* changed (`fix`, `docs`, `fix: linter`).
pub fn is_generic_subject(subject: &str) -> bool {
    let text = strip_conventional_prefix(subject).to_ascii_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.len() <= 1 || words.iter().all(|w| GENERIC_WORDS.contains(w))
}

/// `feat(scope)!: add x` -> `add x`.
fn strip_conventional_prefix(subject: &str) -> &str {
    let Some((prefix, rest)) = subject.split_once(':') else {
        return subject;
    };
    let is_type = !prefix.is_empty()
        && prefix.len() <= 20
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "()!-_ ".contains(c));
    if is_type { rest.trim() } else { subject }
}

/// Title and subtitle for a cluster.
///
/// The title is the meaningful commit that touched most of the cluster's files (`commit_files`),
/// earliest first on ties. `commits` are `(sha, subject)` oldest first; `paths` are the cluster's
/// files with their number of changes; `main` is its central definition.
pub fn describe(
    commit_files: &HashMap<String, usize>,
    commits: &[(&str, &str)],
    paths: &BTreeMap<String, usize>,
    main: &str,
) -> (String, String) {
    let files = files_label(paths);
    let best = commits
        .iter()
        .enumerate()
        .filter(|(_, (_, subject))| !is_generic_subject(subject))
        .filter_map(|(position, (sha, subject))| {
            let files = *commit_files.get(*sha)?;
            Some((files, std::cmp::Reverse(position), *subject))
        })
        .max();
    match best {
        Some((_, _, subject)) => (subject.to_string(), join_label(&files, main)),
        None => (files.clone(), main.to_string()),
    }
}

fn join_label(files: &str, main: &str) -> String {
    if main.is_empty() || files.contains(main) {
        files.to_string()
    } else {
        format!("{files} · {main}")
    }
}

/// `fiftyone/ · build.py, main.py +2`: common directory (last components) and the files with
/// the most changes.
pub fn files_label(paths: &BTreeMap<String, usize>) -> String {
    let dirs: Vec<Vec<&str>> = paths
        .keys()
        .map(|p| {
            let mut parts: Vec<&str> = p.split('/').collect();
            parts.pop();
            parts
        })
        .collect();
    let common: Vec<&str> = dirs.first().map_or_else(Vec::new, |first| {
        let shared =
            (0..first.len()).take_while(|&i| dirs.iter().all(|d| d.get(i) == first.get(i)));
        shared.map(|i| first[i]).collect()
    });
    let dir = common[common.len().saturating_sub(LABEL_DIR_DEPTH)..].join("/");
    let mut ranked: Vec<(&String, &usize)> = paths.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let names: BTreeSet<&str> = ranked
        .iter()
        .take(LABEL_FILES)
        .map(|(p, _)| p.rsplit('/').next().unwrap_or(p))
        .collect();
    let names: Vec<&str> = names.into_iter().collect();
    let more = paths.len().saturating_sub(LABEL_FILES);
    let more = if more > 0 {
        format!(" +{more}")
    } else {
        String::new()
    };
    let files = format!("{}{more}", names.join(", "));
    if dir.is_empty() {
        files
    } else {
        format!("{dir}/ · {files}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_subjects() {
        assert!(is_generic_subject("fix"));
        assert!(is_generic_subject("docs"));
        assert!(is_generic_subject("fix: linter"));
        assert!(is_generic_subject("fix: unit test"));
        assert!(is_generic_subject("update"));
        assert!(!is_generic_subject("feat: add fiftyone"));
        assert!(!is_generic_subject("feat: use 4 GPUs"));
        assert!(!is_generic_subject("docs: update deployed models"));
    }

    #[test]
    fn titles_prefer_the_meaningful_commit_touching_most_files() {
        // Oldest first. `a` is generic; `b` and `d` tie on files, `b` is older.
        let commits = [
            ("a", "fix"),
            ("b", "feat: add fiftyone"),
            ("c", "add logo audit"),
            ("d", "feat: tweak fiftyone build"),
        ];
        let files: HashMap<String, usize> = [
            ("a".to_string(), 5),
            ("b".to_string(), 2),
            ("c".to_string(), 1),
            ("d".to_string(), 2),
        ]
        .into();
        let paths: BTreeMap<String, usize> = [
            ("pkg/fiftyone/build.py".to_string(), 3),
            ("pkg/fiftyone/main.py".to_string(), 1),
        ]
        .into();
        let (title, subtitle) = describe(&files, &commits, &paths, "Backbone");
        assert_eq!(title, "feat: add fiftyone");
        assert_eq!(subtitle, "pkg/fiftyone/ · build.py, main.py · Backbone");
    }

    #[test]
    fn titles_fall_back_to_files() {
        let paths: BTreeMap<String, usize> = [
            ("a/b/c/one.yml".to_string(), 1),
            ("a/b/c/two.yml".to_string(), 1),
            ("a/b/c/three.yml".to_string(), 1),
            ("a/b/c/four.yml".to_string(), 1),
        ]
        .into();
        let (title, subtitle) = describe(&HashMap::new(), &[], &paths, "one.yml");
        assert_eq!(title, "b/c/ · four.yml, one.yml, three.yml +1");
        assert_eq!(subtitle, "one.yml");
    }
}
