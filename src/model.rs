//! Report schema emitted as JSON. This is the contract with frontends (e.g. the nvim plugin).
//!
//! All cross references are indices into the top-level arrays (`hunks`, `symbols`).

use serde::Serialize;

use crate::diff::Hunk;
use crate::git::{Commit, FileStatus};
pub use crate::lang::CallCheck;
use crate::lang::{CallArgs, SymbolKind};

/// Bump on breaking changes to the JSON layout.
pub const SCHEMA_VERSION: u32 = 1;

/// Full analysis result.
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub repo: String,
    pub base: Revision,
    pub head: Revision,
    pub merge_base: String,
    pub stats: Stats,
    /// Commits in `merge_base..head`, oldest first.
    pub commits: Vec<Commit>,
    pub files: Vec<FileReport>,
    pub hunks: Vec<HunkReport>,
    pub symbols: Vec<ChangedSymbol>,
    pub edges: Vec<Edge>,
    pub clusters: Vec<Cluster>,
    /// Changed files with no symbol-level analysis (unsupported language, excluded, etc.).
    pub unclustered_files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Revision {
    pub rev: String,
    pub sha: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub files: usize,
    pub hunks: usize,
    pub symbols: usize,
    pub clusters: usize,
    pub additions: usize,
    pub deletions: usize,
    pub indexed_files: usize,
}

#[derive(Debug, Serialize)]
pub struct FileReport {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub language: Option<&'static str>,
    pub hunks: Vec<usize>,
    /// Commits in range touching this file, oldest first.
    pub commits: Vec<String>,
    pub parse_errors: bool,
}

#[derive(Debug, Serialize)]
pub struct HunkReport {
    pub id: usize,
    pub path: String,
    #[serde(flatten)]
    pub hunk: Hunk,
    /// Changed symbols this hunk touches.
    pub symbols: Vec<usize>,
    /// Commits that introduced the added lines (from blame), oldest first.
    pub commits: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Added,
    Modified,
    Deleted,
    Moved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    High,
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub path: String,
    pub qualname: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

/// A changed definition, or `<module>` for top-level code outside definitions.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedSymbol {
    pub id: usize,
    pub path: String,
    pub qualname: String,
    pub kind: SymbolKind,
    pub change: Change,
    pub moved_from: Option<Location>,
    pub base: Option<LineRange>,
    pub head: Option<LineRange>,
    pub is_test: bool,
    /// Distinct callers/referrers of this symbol across the head revision.
    pub ref_count: usize,
    /// Referenced so widely that its edges do not merge clusters.
    pub hub: bool,
    /// Head signature, e.g. `(url, retries=3) -> str`.
    pub signature: Option<String>,
    /// What changed inside the symbol (signature, references, control flow).
    pub summary: Option<ChangeSummary>,
    /// Calls (changed or unchanged callers) that no longer match the new signature.
    pub broken_calls: usize,
}

/// One described change of a signature.
#[derive(Debug, Clone, Serialize)]
pub struct SignatureChange {
    /// `param_added`, `param_removed`, `param_changed`, `params_reordered`, `returns`,
    /// `decorator_added`, `decorator_removed`, `async`, `base_added`, `base_removed`.
    pub kind: &'static str,
    pub text: String,
}

/// Structural description of a symbol change, derived from both versions' syntax trees.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ChangeSummary {
    pub signature: Vec<SignatureChange>,
    pub calls_added: Vec<String>,
    pub calls_removed: Vec<String>,
    /// Non-call references (types, values passed around).
    pub uses_added: Vec<String>,
    pub uses_removed: Vec<String>,
    /// Control-flow markers such as `raise ValueError`.
    pub flow_added: Vec<String>,
    pub flow_removed: Vec<String>,
}

impl ChangeSummary {
    /// Whether nothing was detected.
    pub fn is_empty(&self) -> bool {
        [
            &self.calls_added,
            &self.calls_removed,
            &self.uses_added,
            &self.uses_removed,
            &self.flow_added,
            &self.flow_removed,
        ]
        .iter()
        .all(|v| v.is_empty())
            && self.signature.is_empty()
    }
}

/// A call site, kept for signature checks (not serialised).
#[derive(Debug, Clone)]
pub struct CallSite {
    pub line: u32,
    pub args: CallArgs,
    /// Callee as written, e.g. `self.repo.save`.
    pub callee: String,
}

/// How two changes are linked. Only calls are strong enough to merge clusters; the others
/// attach loose changes to clusters. Ordered weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// Both changes contain the same rare token (e.g. `cycle_59`), see `via`.
    Shared,
    /// `from`'s changed lines mention `to`'s file name (or a distinctive part of it).
    Mention,
    /// Type annotation, base class, decorator, value passed as argument.
    Reference,
    /// Called or instantiated.
    Call,
}

/// `from` references `to`. Both are changed symbols.
#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    /// The mentioned name or shared token, for text links.
    pub via: Option<String>,
    pub confidence: Confidence,
    pub hub: bool,
    /// Worst result of checking `from`'s calls against `to`'s new signature.
    pub call_check: Option<CallCheck>,
    #[serde(skip)]
    pub calls: Vec<CallSite>,
}

/// Unchanged code that references a changed symbol.
#[derive(Debug, Clone, Serialize)]
pub struct Impact {
    pub path: String,
    /// Enclosing definition of the reference; `None` for module-level code.
    pub qualname: Option<String>,
    pub line: u32,
    pub target: usize,
    pub confidence: Confidence,
    /// The target no longer exists under the referenced name (deleted or moved).
    pub dangling: bool,
    /// Worst result of checking these calls against the target's new signature.
    pub call_check: Option<CallCheck>,
    #[serde(skip)]
    pub calls: Vec<CallSite>,
}

/// A logical change: connected changed symbols plus their hunks, commits and impact.
#[derive(Debug, Serialize)]
pub struct Cluster {
    pub id: usize,
    pub title: String,
    /// Where the change lives, e.g. `fiftyone/ · build.py, main.py +2`.
    pub subtitle: String,
    /// Review order: dependencies before dependents, tests last.
    pub symbols: Vec<usize>,
    pub hunks: Vec<usize>,
    pub commits: Vec<String>,
    pub impact: Vec<Impact>,
    /// Impact entries before truncation.
    pub impact_total: usize,
}
