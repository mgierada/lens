//! Thin wrapper over the `git` CLI. Reads objects directly, never touches the working tree.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

/// Kind of change for one file between two trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
}

/// One entry of `git diff --raw`.
#[derive(Debug, Clone)]
pub struct RawChange {
    pub status: FileStatus,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
}

impl RawChange {
    /// Path identifying the file in reports: the new path, or the old one for deletions.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

/// A blob in a tree listing.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: String,
    pub blob: String,
    pub size: u64,
}

/// A commit in the reviewed range.
#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub sha: String,
    pub summary: String,
    pub body: String,
}

/// Handle to a repository.
pub struct Git {
    root: PathBuf,
}

impl Git {
    /// Open the repository containing `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let probe = Self {
            root: path.to_path_buf(),
        };
        let top = probe
            .run(&["rev-parse", "--show-toplevel"])
            .context("not a git repository")?;
        Ok(Self {
            root: PathBuf::from(top.trim()),
        })
    }

    /// Repository top-level directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Shared `.git` directory (correct for worktrees too).
    pub fn common_dir(&self) -> Result<PathBuf> {
        let dir = PathBuf::from(self.run(&["rev-parse", "--git-common-dir"])?.trim());
        Ok(if dir.is_absolute() {
            dir
        } else {
            self.root.join(dir)
        })
    }

    /// Resolve a revision to a commit SHA.
    pub fn resolve(&self, rev: &str) -> Result<String> {
        let spec = format!("{rev}^{{commit}}");
        let out = self.run(&["rev-parse", "--verify", "--quiet", &spec]);
        Ok(out
            .with_context(|| format!("unknown revision '{rev}'"))?
            .trim()
            .to_string())
    }

    /// Best common ancestor of two commits.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        let out = self.run(&["merge-base", a, b]).context("no merge base")?;
        Ok(out.trim().to_string())
    }

    /// Recursive blob listing of a tree.
    pub fn ls_tree(&self, rev: &str) -> Result<Vec<TreeEntry>> {
        let out = self.run(&["ls-tree", "-r", "-l", "-z", "--full-tree", rev])?;
        Ok(out.split('\0').filter_map(parse_tree_entry).collect())
    }

    /// File-level changes between two commits, with rename detection.
    pub fn diff_raw(&self, base: &str, head: &str) -> Result<Vec<RawChange>> {
        let out = self.run(&["diff", "--raw", "-z", "-M", "--no-abbrev", base, head])?;
        parse_raw_diff(&out)
    }

    /// Unified patch between two commits.
    pub fn diff_patch(&self, base: &str, head: &str, context: u32) -> Result<String> {
        let unified = format!("-U{context}");
        let args = [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-M",
            &unified,
            base,
            head,
        ];
        self.run(&args)
    }

    /// Commits reachable from `head` but not `base`, newest first.
    pub fn commits(&self, base: &str, head: &str) -> Result<Vec<Commit>> {
        let range = format!("{base}..{head}");
        let out = self.run(&["log", "--format=%H%x1f%s%x1f%b%x1e", &range])?;
        Ok(out.split('\x1e').filter_map(parse_commit).collect())
    }

    /// Commits in `base..head` touching each path (new-side paths).
    pub fn commits_by_file(&self, base: &str, head: &str) -> Result<HashMap<String, Vec<String>>> {
        let range = format!("{base}..{head}");
        let out = self.run(&["log", "--format=%x1e%H", "--name-only", "-z", &range])?;
        let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
        for record in out.split('\x1e').filter(|r| !r.trim().is_empty()) {
            let mut lines = record
                .split(['\n', '\0'])
                .map(str::trim)
                .filter(|l| !l.is_empty());
            let Some(sha) = lines.next() else { continue };
            for path in lines {
                by_file
                    .entry(path.to_string())
                    .or_default()
                    .push(sha.to_string());
            }
        }
        Ok(by_file)
    }

    /// Commit that last touched each line in `ranges` (1-based, inclusive) of `path` at `head`,
    /// limited to `base..head`.
    pub fn blame(
        &self,
        base: &str,
        head: &str,
        path: &str,
        ranges: &[(u32, u32)],
    ) -> Result<HashMap<u32, String>> {
        let range_args: Vec<String> = ranges.iter().map(|(s, e)| format!("-L{s},{e}")).collect();
        let rev = format!("{base}..{head}");
        let mut args: Vec<&str> = vec!["blame", "--porcelain"];
        args.extend(range_args.iter().map(String::as_str));
        args.extend([rev.as_str(), "--", path]);
        Ok(parse_blame(&self.run(&args)?))
    }

    /// Read blob contents in one `git cat-file --batch` process.
    pub fn read_blobs(&self, shas: &[String]) -> Result<HashMap<String, Vec<u8>>> {
        let mut child = self
            .command(&["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("spawning git cat-file")?;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let input: String = shas.iter().map(|sha| format!("{sha}\n")).collect();
        let writer = thread::spawn(move || stdin.write_all(input.as_bytes()));
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let blobs = read_batch(BufReader::new(stdout), shas.len())?;
        writer
            .join()
            .map_err(|_| anyhow!("cat-file writer panicked"))??;
        child.wait()?;
        Ok(blobs)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .args(["-c", "core.quotepath=off"])
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.command(args).output().context("running git")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {} failed: {}", args.join(" "), stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn read_batch(mut reader: impl BufRead, count: usize) -> Result<HashMap<String, Vec<u8>>> {
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::with_capacity(count);
    let mut header = String::new();
    for _ in 0..count {
        header.clear();
        reader.read_line(&mut header)?;
        let parts: Vec<&str> = header.split_whitespace().collect();
        let [sha, _kind, size] = parts.as_slice() else {
            continue;
        };
        let size: usize = size.parse()?;
        let mut content = vec![0u8; size + 1];
        reader.read_exact(&mut content)?;
        content.truncate(size);
        blobs.insert(sha.to_string(), content);
    }
    Ok(blobs)
}

fn parse_tree_entry(record: &str) -> Option<TreeEntry> {
    let (meta, path) = record.split_once('\t')?;
    let parts: Vec<&str> = meta.split_whitespace().collect();
    let [_mode, kind, blob, size] = parts.as_slice() else {
        return None;
    };
    if *kind != "blob" {
        return None;
    }
    Some(TreeEntry {
        path: path.to_string(),
        blob: blob.to_string(),
        size: size.parse().ok()?,
    })
}

fn parse_raw_diff(out: &str) -> Result<Vec<RawChange>> {
    let mut tokens = out.split('\0').filter(|t| !t.is_empty());
    let mut changes: Vec<RawChange> = Vec::new();
    while let Some(meta) = tokens.next() {
        let parts: Vec<&str> = meta.trim_start_matches(':').split_whitespace().collect();
        let [_, _, old_blob, new_blob, status] = parts.as_slice() else {
            bail!("unexpected diff --raw record: {meta}");
        };
        let status = parse_status(status)?;
        let first = tokens.next().map(str::to_string);
        let paired = matches!(status, FileStatus::Renamed | FileStatus::Copied);
        let second = if paired {
            tokens.next().map(str::to_string)
        } else {
            first.clone()
        };
        let (old_path, new_path) = match status {
            FileStatus::Added => (None, first),
            FileStatus::Deleted => (first, None),
            _ => (first, second),
        };
        changes.push(RawChange {
            status,
            old_path,
            new_path,
            old_blob: non_null_sha(old_blob),
            new_blob: non_null_sha(new_blob),
        });
    }
    Ok(changes)
}

fn parse_status(status: &str) -> Result<FileStatus> {
    Ok(match status.chars().next() {
        Some('A') => FileStatus::Added,
        Some('M') => FileStatus::Modified,
        Some('D') => FileStatus::Deleted,
        Some('R') => FileStatus::Renamed,
        Some('C') => FileStatus::Copied,
        Some('T') => FileStatus::TypeChanged,
        _ => bail!("unsupported diff status '{status}'"),
    })
}

fn non_null_sha(sha: &str) -> Option<String> {
    (!sha.bytes().all(|b| b == b'0')).then(|| sha.to_string())
}

fn parse_commit(record: &str) -> Option<Commit> {
    let mut fields = record.trim_start_matches('\n').splitn(3, '\x1f');
    let sha = fields.next()?.trim();
    if sha.is_empty() {
        return None;
    }
    Some(Commit {
        sha: sha.to_string(),
        summary: fields.next().unwrap_or_default().trim().to_string(),
        body: fields.next().unwrap_or_default().trim().to_string(),
    })
}

/// Map final line number -> commit SHA from `git blame --porcelain` output.
fn parse_blame(out: &str) -> HashMap<u32, String> {
    let mut lines: HashMap<u32, String> = HashMap::new();
    for line in out.lines().filter(|l| !l.starts_with('\t')) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let is_header = matches!(parts.len(), 3 | 4) && is_sha(parts[0]);
        let Some(final_line) = is_header.then(|| parts[2].parse::<u32>().ok()).flatten() else {
            continue;
        };
        lines.insert(final_line, parts[0].to_string());
    }
    lines
}

fn is_sha(token: &str) -> bool {
    matches!(token.len(), 40 | 64) && token.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raw_diff_with_rename() {
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let zero = "0".repeat(40);
        let out = format!(
            ":100644 100644 {sha_a} {sha_b} R090\0old.py\0new.py\0\
             :000000 100644 {zero} {sha_b} A\0added.py\0"
        );
        let changes = parse_raw_diff(&out).unwrap();
        assert_eq!(changes[0].status, FileStatus::Renamed);
        assert_eq!(changes[0].old_path.as_deref(), Some("old.py"));
        assert_eq!(changes[0].new_path.as_deref(), Some("new.py"));
        assert_eq!(changes[1].status, FileStatus::Added);
        assert!(changes[1].old_blob.is_none());
        assert_eq!(changes[1].path(), "added.py");
    }

    #[test]
    fn parses_blame_headers_only() {
        let sha = "c".repeat(40);
        let out = format!("{sha} 1 5 2\nauthor x\nsummary s\n\tcode\n{sha} 2 6\n\tcode\n");
        let lines = parse_blame(&out);
        assert_eq!(lines.get(&5), Some(&sha));
        assert_eq!(lines.get(&6), Some(&sha));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn parses_tree_entries() {
        let entry = parse_tree_entry(&format!("100644 blob {} 42\tpkg/a b.py", "d".repeat(40)));
        let entry = entry.unwrap();
        assert_eq!(entry.path, "pkg/a b.py");
        assert_eq!(entry.size, 42);
        assert!(parse_tree_entry(&format!("160000 commit {} -\tsub", "e".repeat(40))).is_none());
    }
}
