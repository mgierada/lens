//! Unified diff parsing.

use std::collections::HashMap;

use serde::Serialize;

/// Line role in a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One hunk line with its position on each side.
#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub text: String,
}

/// A hunk as in `@@ -old_start,old_lines +new_start,new_lines @@ header`.
#[derive(Debug, Clone, Serialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// Lines that were added or removed.
    pub fn changed_lines(&self) -> impl Iterator<Item = &DiffLine> {
        self.lines.iter().filter(|l| l.kind != LineKind::Context)
    }
}

/// Parse a multi-file patch into hunks keyed by file path (new path, or old path for deletions).
pub fn parse_patch(patch: &str) -> HashMap<String, Vec<Hunk>> {
    let mut files: HashMap<String, Vec<Hunk>> = HashMap::new();
    let mut old_path: Option<String> = None;
    let mut current: Option<String> = None;
    let mut hunk: Option<Hunk> = None;
    let mut old_line: u32 = 0;
    let mut new_line: u32 = 0;

    for line in patch.lines() {
        if line.starts_with("diff --git ") {
            flush(&mut files, &current, &mut hunk);
            current = None;
            old_path = None;
        } else if let Some(path) = line.strip_prefix("--- ").filter(|_| hunk.is_none()) {
            old_path = strip_side(path, "a/");
        } else if let Some(path) = line.strip_prefix("+++ ").filter(|_| hunk.is_none()) {
            current = strip_side(path, "b/").or_else(|| old_path.clone());
        } else if let Some(parsed) = line.strip_prefix("@@ ").and_then(parse_hunk_header) {
            flush(&mut files, &current, &mut hunk);
            old_line = parsed.old_start;
            new_line = parsed.new_start;
            hunk = Some(parsed);
        } else if let Some(h) = hunk.as_mut() {
            push_line(h, line, &mut old_line, &mut new_line);
        }
    }
    flush(&mut files, &current, &mut hunk);
    files
}

fn flush(files: &mut HashMap<String, Vec<Hunk>>, path: &Option<String>, hunk: &mut Option<Hunk>) {
    if let (Some(path), Some(h)) = (path, hunk.take()) {
        files.entry(path.clone()).or_default().push(h);
    }
}

fn push_line(hunk: &mut Hunk, line: &str, old_line: &mut u32, new_line: &mut u32) {
    let (kind, text) = match line.split_at_checked(1) {
        Some(("+", text)) => (LineKind::Added, text),
        Some(("-", text)) => (LineKind::Removed, text),
        Some((" ", text)) => (LineKind::Context, text),
        None => (LineKind::Context, ""),
        _ => return, // "\ No newline at end of file" and similar markers.
    };
    let old = (kind != LineKind::Added).then_some(*old_line);
    let new = (kind != LineKind::Removed).then_some(*new_line);
    *old_line += u32::from(old.is_some());
    *new_line += u32::from(new.is_some());
    hunk.lines.push(DiffLine {
        kind,
        old_line: old,
        new_line: new,
        text: text.to_string(),
    });
}

/// Parse `-a,b +c,d @@ header` (the part after the leading `@@ `).
fn parse_hunk_header(rest: &str) -> Option<Hunk> {
    let (ranges, header) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let (old_start, old_lines) = parse_range(old.strip_prefix('-')?)?;
    let (new_start, new_lines) = parse_range(new.strip_prefix('+')?)?;
    Some(Hunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        header: header.trim().to_string(),
        lines: Vec::new(),
    })
}

fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Strip the `a/`/`b/` prefix; `/dev/null` yields `None`.
fn strip_side(path: &str, prefix: &str) -> Option<String> {
    let path = unquote(path.trim_end_matches('\t'));
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(prefix).unwrap_or(&path).to_string())
}

/// Undo git's C-style path quoting (only applied to unusual characters with quotepath=off).
fn unquote(path: &str) -> String {
    let Some(inner) = path.strip_prefix('"').and_then(|p| p.strip_suffix('"')) else {
        return path.to_string();
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(b) = chars.next() {
        if b != b'\\' {
            bytes.push(b);
            continue;
        }
        match chars.next() {
            Some(b't') => bytes.push(b'\t'),
            Some(b'n') => bytes.push(b'\n'),
            Some(d @ b'0'..=b'7') => {
                let mut value = u32::from(d - b'0');
                for _ in 0..2 {
                    if let Some(&next @ b'0'..=b'7') = chars.peek() {
                        value = value * 8 + u32::from(next - b'0');
                        chars.next();
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "\
diff --git a/a.py b/a.py
index 1..2 100644
--- a/a.py
+++ b/a.py
@@ -1,3 +1,3 @@ def f():
 x = 1
-y = 2
+y = 3
 z = 4
@@ -10 +10,2 @@
 tail
+added
diff --git a/gone.py b/gone.py
deleted file mode 100644
--- a/gone.py
+++ /dev/null
@@ -1 +0,0 @@
-bye
";

    #[test]
    fn parses_hunks_and_line_numbers() {
        let files = parse_patch(PATCH);
        let hunks = &files["a.py"];
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].header, "def f():");
        let removed = hunks[0]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Removed)
            .unwrap();
        assert_eq!((removed.old_line, removed.new_line), (Some(2), None));
        let added = hunks[0]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Added)
            .unwrap();
        assert_eq!((added.old_line, added.new_line), (None, Some(2)));
        assert_eq!(hunks[1].new_lines, 2);
        assert_eq!(hunks[1].lines[1].new_line, Some(11));
    }

    #[test]
    fn deleted_files_keyed_by_old_path() {
        let files = parse_patch(PATCH);
        assert_eq!(files["gone.py"][0].lines[0].kind, LineKind::Removed);
    }

    #[test]
    fn unquotes_octal_paths() {
        assert_eq!(unquote("\"a/\\303\\251.py\""), "a/é.py");
        assert_eq!(unquote("plain.py"), "plain.py");
    }
}
