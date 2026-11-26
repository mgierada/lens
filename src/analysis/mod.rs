//! End-to-end analysis of a branch comparison.
//!
//! Pipeline: index head → diff → symbol changes → reference resolution → clusters → commit context.

mod changes;
mod cluster;
mod links;
mod naming;
mod resolve;
mod summary;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use globset::GlobSet;

use crate::diff::{Hunk, LineKind, parse_patch};
use crate::facts::Facts;
use crate::git::{FileStatus, Git, RawChange};
use crate::index::{BlobKey, Index, IndexOptions, IndexedFile};
use crate::lang::{Language, Registry, SymbolKind};
use crate::model::{
    Change, ChangedSymbol, Cluster, Edge, FileReport, HunkReport, LineRange, Location, Report,
    Revision, SCHEMA_VERSION, Stats,
};
use crate::why::{self, Why};

use changes::{ChangeSet, Loc, SymbolChange};
use resolve::Links;

/// Pseudo-qualname for top-level code outside any definition.
pub const MODULE_QUALNAME: &str = "<module>";
/// Pseudo-qualname for a whole file without language support.
pub const FILE_QUALNAME: &str = "<file>";

/// Inputs for [`analyze`].
pub struct AnalyzeOptions {
    pub base: String,
    pub head: String,
    /// Context lines per hunk.
    pub context: u32,
    /// Symbols with more distinct referrers than this do not merge clusters.
    pub hub_threshold: usize,
    /// Impact entries kept per changed symbol.
    pub impact_limit: usize,
    pub excludes: GlobSet,
    pub max_file_size: u64,
    pub use_cache: bool,
    pub blame: bool,
}

/// A file touched by the diff, with facts for both sides when its language is supported.
pub struct ChangedFile<'a> {
    pub change: RawChange,
    pub lang: Option<&'a dyn Language>,
    pub base: Option<Arc<Facts>>,
    pub head: Option<Arc<Facts>>,
    pub hunks: Vec<Hunk>,
}

/// Compare `base` and `head` (merge-base semantics, like `base...head`).
pub fn analyze(repo: &Path, options: &AnalyzeOptions) -> Result<Report> {
    let git = Git::open(repo)?;
    let registry = Registry::new();
    let base_sha = git.resolve(&options.base)?;
    let head_sha = git.resolve(&options.head)?;
    let merge_base = git.merge_base(&base_sha, &head_sha)?;

    let index_options = IndexOptions {
        excludes: options.excludes.clone(),
        max_file_size: options.max_file_size,
        use_cache: options.use_cache,
    };
    let mut index = Index::open(&git, &registry, index_options.use_cache)?;
    let head_files = index.snapshot(&head_sha, &index_options)?;
    let diff = Diff {
        git: &git,
        merge_base: &merge_base,
        head: &head_sha,
    };
    let files = changed_files(&diff, &registry, &mut index, &head_files, options)?;

    let set = changes::detect(&files);
    let mut links = resolve::link(&registry, &files, &head_files, &set, options.hub_threshold);
    let broken_calls = summary::check_calls(&files, &set, &mut links);
    let hunk_list: Vec<(usize, &Hunk)> = files
        .iter()
        .enumerate()
        .flat_map(|(i, f)| f.hunks.iter().map(move |h| (i, h)))
        .collect();
    let blame_input: Vec<(&str, &Hunk)> = hunk_list
        .iter()
        .map(|(i, h)| (files[*i].change.path(), *h))
        .collect();
    let why = why::collect(&git, &merge_base, &head_sha, &blame_input, options.blame)?;

    let parts = ReportParts {
        files: &files,
        set: &set,
        links: &links,
        broken_calls: &broken_calls,
        why: &why,
        hunks: &hunk_list,
    };
    let mut report = parts.build(options.impact_limit);
    report.repo = git.root().display().to_string();
    report.base = Revision {
        rev: options.base.clone(),
        sha: base_sha,
    };
    report.head = Revision {
        rev: options.head.clone(),
        sha: head_sha,
    };
    report.merge_base = merge_base;
    report.stats.indexed_files = head_files.len();
    Ok(report)
}

struct Diff<'a> {
    git: &'a Git,
    merge_base: &'a str,
    head: &'a str,
}

fn changed_files<'a>(
    diff: &Diff,
    registry: &'a Registry,
    index: &mut Index,
    head_files: &[IndexedFile],
    options: &AnalyzeOptions,
) -> Result<Vec<ChangedFile<'a>>> {
    let raw = diff.git.diff_raw(diff.merge_base, diff.head)?;
    let mut patches = parse_patch(&diff.git.diff_patch(
        diff.merge_base,
        diff.head,
        options.context,
    )?);
    let head_by_path: HashMap<&str, &IndexedFile> =
        head_files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut files: Vec<ChangedFile> = Vec::with_capacity(raw.len());
    for change in raw {
        let hunks = patches.remove(change.path()).unwrap_or_default();
        let lang = registry
            .for_path(change.path())
            .filter(|_| !options.excludes.is_match(change.path()));
        let head = change.new_path.as_deref().and_then(|p| head_by_path.get(p));
        let head: Option<Arc<Facts>> = head.map(|f| f.facts.clone());
        // A head file missing from the snapshot was skipped (e.g. too large): treat as opaque.
        let lang = lang.filter(|_| change.new_path.is_none() || head.is_some());
        let base = match (lang, &change.old_blob) {
            (Some(lang), Some(blob)) => index.facts_for(&BlobKey {
                blob: blob.clone(),
                lang: lang.id(),
            })?,
            _ => None,
        };
        files.push(ChangedFile {
            change,
            lang,
            base,
            head,
            hunks,
        });
    }
    files.sort_by(|a, b| a.change.path().cmp(b.change.path()));
    Ok(files)
}

struct Located {
    path: String,
    qualname: String,
    kind: SymbolKind,
    range: LineRange,
}

struct ReportParts<'a> {
    files: &'a [ChangedFile<'a>],
    set: &'a ChangeSet,
    links: &'a Links,
    broken_calls: &'a [usize],
    why: &'a Why,
    hunks: &'a [(usize, &'a Hunk)],
}

impl ReportParts<'_> {
    fn build(&self, impact_limit: usize) -> Report {
        let symbols: Vec<ChangedSymbol> = self
            .set
            .changes
            .iter()
            .enumerate()
            .map(|(id, c)| self.symbol(id, c))
            .collect();
        let hunks: Vec<HunkReport> = self.hunk_reports();
        let mut edges: Vec<Edge> = self.links.edges.clone();
        edges.extend(self.text_links(&symbols));
        let commit_groups = self.commit_groups(&hunks);
        let clusters = self.clusters(&symbols, &hunks, &edges, &commit_groups, impact_limit);
        let files = self.file_reports();
        let represented: HashSet<&str> = symbols.iter().map(|s| s.path.as_str()).collect();
        let unclustered_files = files
            .iter()
            .filter(|f| !represented.contains(f.path.as_str()))
            .filter(|f| f.hunks.iter().all(|&h| hunks[h].symbols.is_empty()))
            .map(|f| f.path.clone())
            .collect();
        let changed_lines = hunks.iter().flat_map(|h| h.hunk.changed_lines());
        let (additions, deletions) = changed_lines.fold((0, 0), |(a, d), l| match l.kind {
            LineKind::Added => (a + 1, d),
            _ => (a, d + 1),
        });
        Report {
            schema_version: SCHEMA_VERSION,
            repo: String::new(),
            base: Revision {
                rev: String::new(),
                sha: String::new(),
            },
            head: Revision {
                rev: String::new(),
                sha: String::new(),
            },
            merge_base: String::new(),
            stats: Stats {
                files: files.len(),
                hunks: hunks.len(),
                symbols: symbols.len(),
                clusters: clusters.len(),
                additions,
                deletions,
                indexed_files: 0,
            },
            commits: self.why.commits.clone(),
            files,
            hunks,
            symbols,
            edges,
            clusters,
            unclustered_files,
        }
    }

    fn symbol(&self, id: usize, change: &SymbolChange) -> ChangedSymbol {
        let (signature, summary) = summary::summarize(self.files, change);
        let head = change.head.and_then(|loc| self.located(loc, true));
        let base = change.base.and_then(|loc| self.located(loc, false));
        let file = change
            .head
            .or(change.base)
            .map(|l| l.file)
            .or(change.module_of);
        let lang = file.and_then(|f| self.files[f].lang);
        let primary = head.as_ref().or(base.as_ref());
        let (path, qualname, kind) = match (primary, file) {
            (Some(p), _) => (p.path.clone(), p.qualname.clone(), p.kind),
            (None, Some(f)) => {
                let path = self.files[f].change.path().to_string();
                match self.files[f].lang {
                    Some(_) => (path, MODULE_QUALNAME.to_string(), SymbolKind::Module),
                    None => (path, FILE_QUALNAME.to_string(), SymbolKind::File),
                }
            }
            (None, None) => (
                String::new(),
                MODULE_QUALNAME.to_string(),
                SymbolKind::Module,
            ),
        };
        let moved_from = base
            .as_ref()
            .filter(|_| change.change == Change::Moved)
            .map(|b| Location {
                path: b.path.clone(),
                qualname: b.qualname.clone(),
            });
        ChangedSymbol {
            id,
            is_test: lang.is_some_and(|l| l.is_test_path(&path)),
            path,
            qualname,
            kind,
            change: change.change,
            moved_from,
            base: base.map(|b| b.range),
            head: head.map(|h| h.range),
            ref_count: self.links.ref_counts[id],
            hub: self.links.hub[id],
            signature,
            summary,
            broken_calls: self.broken_calls[id],
        }
    }

    /// Where a changed symbol lives on one side of the diff.
    fn located(&self, loc: Loc, head: bool) -> Option<Located> {
        let file = &self.files[loc.file];
        let (facts, path) = if head {
            (file.head.as_deref()?, file.change.new_path.clone()?)
        } else {
            (file.base.as_deref()?, file.change.old_path.clone()?)
        };
        let symbol = facts.symbols.get(loc.sym as usize)?;
        Some(Located {
            path,
            qualname: symbol.qualname.clone(),
            kind: symbol.kind,
            range: LineRange {
                start: symbol.start_line,
                end: symbol.end_line,
            },
        })
    }

    fn hunk_reports(&self) -> Vec<HunkReport> {
        self.hunks
            .iter()
            .enumerate()
            .map(|(id, (file, hunk))| HunkReport {
                id,
                path: self.files[*file].change.path().to_string(),
                hunk: (*hunk).clone(),
                symbols: self.hunk_symbols(*file, hunk),
                commits: self.why.by_hunk[id].clone(),
            })
            .collect()
    }

    fn hunk_symbols(&self, file: usize, hunk: &Hunk) -> Vec<usize> {
        let ids = hunk.changed_lines().filter_map(|line| match line.kind {
            LineKind::Added => self.set.at_head_line(self.files, file, line.new_line?),
            _ => self.set.at_base_line(self.files, file, line.old_line?),
        });
        ids.collect::<BTreeSet<usize>>().into_iter().collect()
    }

    fn file_reports(&self) -> Vec<FileReport> {
        let mut next_hunk = 0;
        self.files
            .iter()
            .map(|file| {
                let hunks: Vec<usize> = (next_hunk..next_hunk + file.hunks.len()).collect();
                next_hunk += file.hunks.len();
                let commits = self
                    .why
                    .by_file
                    .get(file.change.path())
                    .cloned()
                    .unwrap_or_default();
                let errors = [&file.base, &file.head]
                    .iter()
                    .any(|f| f.as_ref().is_some_and(|f| f.has_errors));
                FileReport {
                    path: file.change.path().to_string(),
                    old_path: file
                        .change
                        .old_path
                        .clone()
                        .filter(|p| p != file.change.path()),
                    status: file.change.status,
                    language: file.lang.map(|l| l.id()),
                    hunks,
                    commits: self.why.ordered(commits),
                    parse_errors: errors,
                }
            })
            .collect()
    }

    /// Mention and shared-token links, from every changed line to the change owning it.
    fn text_links(&self, symbols: &[ChangedSymbol]) -> Vec<Edge> {
        let mut lines: Vec<links::OwnedLine> = Vec::new();
        for (file, hunk) in self.hunks {
            for line in hunk.changed_lines() {
                let owner = match line.kind {
                    LineKind::Added => {
                        self.set
                            .at_head_line(self.files, *file, line.new_line.unwrap_or(0))
                    }
                    _ => self
                        .set
                        .at_base_line(self.files, *file, line.old_line.unwrap_or(0)),
                };
                if let Some(owner) = owner {
                    lines.push(links::OwnedLine {
                        owner,
                        text: &line.text,
                    });
                }
            }
        }
        let files: Vec<links::FileNode> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::File)
            .map(|s| links::FileNode {
                id: s.id,
                path: &s.path,
            })
            .collect();
        links::text_links(&lines, &files)
    }

    /// Per meaningful commit, the changes whose hunks it introduced.
    fn commit_groups(&self, hunks: &[HunkReport]) -> Vec<Vec<usize>> {
        let creators = self.file_creators();
        let generic: HashSet<&str> = self
            .why
            .commits
            .iter()
            .filter(|c| naming::is_generic_subject(&c.summary))
            .map(|c| c.sha.as_str())
            .collect();
        let mut groups: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
        for hunk in hunks {
            for sha in hunk
                .commits
                .iter()
                .filter(|s| !generic.contains(s.as_str()))
            {
                groups
                    .entry(sha)
                    .or_default()
                    .extend(hunk.symbols.iter().copied());
            }
        }
        // New files belong to the commit that created them, even without hunks (empty files).
        for (id, change) in self.set.changes.iter().enumerate() {
            let file = change
                .head
                .or(change.base)
                .map(|l| l.file)
                .or(change.module_of);
            let creator = file.and_then(|f| creators.get(self.files[f].change.path()));
            if let Some(sha) = creator.filter(|s| !generic.contains(s.as_str())) {
                groups.entry(sha.as_str()).or_default().insert(id);
            }
        }
        groups
            .into_values()
            .map(|ids| ids.into_iter().collect())
            .collect()
    }

    fn clusters(
        &self,
        symbols: &[ChangedSymbol],
        hunks: &[HunkReport],
        edges: &[Edge],
        commit_groups: &[Vec<usize>],
        impact_limit: usize,
    ) -> Vec<Cluster> {
        let mut hunks_by_symbol: HashMap<usize, Vec<usize>> = HashMap::new();
        for hunk in hunks {
            for &symbol in &hunk.symbols {
                hunks_by_symbol.entry(symbol).or_default().push(hunk.id);
            }
        }
        let subjects: Vec<(&str, &str)> = self
            .why
            .commits
            .iter()
            .map(|c| (c.sha.as_str(), c.summary.as_str()))
            .collect();
        let creators = self.file_creators();
        let mut clusters: Vec<Cluster> = cluster::group(symbols, edges, commit_groups)
            .into_iter()
            .enumerate()
            .map(|(id, group)| {
                let cluster_hunks: BTreeSet<usize> = group
                    .symbols
                    .iter()
                    .flat_map(|s| hunks_by_symbol.get(s).into_iter().flatten().copied())
                    .collect();
                let (title, subtitle) =
                    describe_cluster(&group, symbols, hunks, &cluster_hunks, &subjects, &creators);
                let commits = cluster_hunks
                    .iter()
                    .flat_map(|&h| hunks[h].commits.iter().cloned());
                let impact_all = group.symbols.iter().map(|&s| &self.links.impact[s]);
                let impact_total = impact_all.clone().map(Vec::len).sum();
                let impact = impact_all
                    .flat_map(|i| i.iter().take(impact_limit).cloned())
                    .collect();
                Cluster {
                    id,
                    title,
                    subtitle,
                    commits: self.why.ordered(commits),
                    hunks: cluster_hunks.into_iter().collect(),
                    symbols: group.symbols,
                    impact,
                    impact_total,
                }
            })
            .collect();
        disambiguate_titles(&mut clusters, symbols);
        clusters
    }

    /// For files added in the range, the commit that created them (their oldest commit).
    fn file_creators(&self) -> HashMap<&str, String> {
        let added = self
            .files
            .iter()
            .filter(|f| f.change.status == FileStatus::Added);
        added
            .filter_map(|f| {
                let path = f.change.path();
                let commits = self.why.by_file.get(path)?.clone();
                let oldest = self.why.ordered(commits).into_iter().next()?;
                Some((path, oldest))
            })
            .collect()
    }
}

/// Title from the commit contributing most changed lines, subtitle from files and main symbol.
fn describe_cluster(
    group: &cluster::Group,
    symbols: &[ChangedSymbol],
    hunks: &[HunkReport],
    cluster_hunks: &BTreeSet<usize>,
    subjects: &[(&str, &str)],
    creators: &HashMap<&str, String>,
) -> (String, String) {
    // Files each commit contributed to this cluster. Counting files, not lines, keeps bulky
    // generated data from deciding the title; a new file counts for its creating commit.
    let mut touched: BTreeSet<(&str, &str)> = BTreeSet::new();
    for &h in cluster_hunks {
        let hunk = &hunks[h];
        match creators.get(hunk.path.as_str()) {
            Some(creator) => {
                touched.insert((creator.as_str(), hunk.path.as_str()));
            }
            None => touched.extend(
                hunk.commits
                    .iter()
                    .map(|c| (c.as_str(), hunk.path.as_str())),
            ),
        }
    }
    let mut weights: HashMap<String, usize> = HashMap::new();
    for (sha, _) in touched {
        *weights.entry(sha.to_string()).or_default() += 1;
    }
    let mut paths: BTreeMap<String, usize> = BTreeMap::new();
    for &id in &group.symbols {
        *paths.entry(symbols[id].path.clone()).or_default() += 1;
    }
    naming::describe(&weights, subjects, &paths, &group.main)
}

/// Clusters sharing a title (one big commit split into parts) get their main file appended.
fn disambiguate_titles(clusters: &mut [Cluster], symbols: &[ChangedSymbol]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for cluster in clusters.iter() {
        *counts.entry(cluster.title.clone()).or_default() += 1;
    }
    for cluster in clusters.iter_mut().filter(|c| counts[&c.title] > 1) {
        // Name the cluster after its main code file; tests only when there is nothing else.
        let has_code = cluster.symbols.iter().any(|&id| !symbols[id].is_test);
        let mut files: BTreeMap<&str, usize> = BTreeMap::new();
        for &id in cluster
            .symbols
            .iter()
            .filter(|&&id| !has_code || !symbols[id].is_test)
        {
            *files.entry(symbols[id].path.as_str()).or_default() += 1;
        }
        let main = files.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)));
        if let Some((path, _)) = main {
            let parts: Vec<&str> = path.rsplit('/').take(2).collect();
            let name = parts.into_iter().rev().collect::<Vec<&str>>().join("/");
            cluster.title = format!("{} · {name}", cluster.title);
        }
    }
}
