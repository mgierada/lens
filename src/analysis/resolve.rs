//! Reference resolution: links changed symbols to each other (edges) and finds unchanged code that
//! depends on them (impact).
//!
//! Only references that can possibly hit a changed symbol are resolved: a cheap prefilter on the
//! last name component keeps whole-repo resolution fast.

use std::collections::{BTreeMap, HashMap, HashSet};

use rayon::prelude::*;

use crate::facts::{Facts, Ref};
use crate::index::IndexedFile;
use crate::lang::{Language, Registry};
use crate::model::{CallSite, Change, Confidence, Edge, EdgeKind, Impact};

use super::ChangedFile;
use super::changes::{ChangeSet, Loc};

/// Resolved relationships for all changes.
pub struct Links {
    pub edges: Vec<Edge>,
    /// Impact per change id, sorted by path and line.
    pub impact: Vec<Vec<Impact>>,
    /// Distinct referrers per change id.
    pub ref_counts: Vec<usize>,
    pub hub: Vec<bool>,
}

/// A name some change can be reached by.
#[derive(Debug, Clone, Copy)]
struct Target {
    id: usize,
    /// Name only exists on the base side (deleted, or old location of a moved symbol).
    dangling: bool,
}

/// Name lookup tables for one side of the diff.
#[derive(Default)]
struct NameTable {
    qualified: HashMap<String, Target>,
    short: HashMap<String, Vec<Target>>,
}

impl NameTable {
    fn insert(&mut self, lang: &dyn Language, path: &str, facts: &Facts, sym: u32, target: Target) {
        let symbol = &facts.symbols[sym as usize];
        for module in lang.module_names(path) {
            let name = format!("{module}.{}", symbol.qualname);
            self.qualified.entry(name).or_insert(target);
        }
        let short = self.short.entry(symbol.name.clone()).or_default();
        if !short.iter().any(|t| t.id == target.id) {
            short.push(target);
        }
    }
}

/// Who holds a reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Caller {
    Changed(usize),
    Unchanged {
        path: String,
        qualname: Option<String>,
    },
}

struct Hit {
    caller: Caller,
    line: u32,
    target: Target,
    kind: EdgeKind,
    confidence: Confidence,
    call: Option<CallSite>,
}

/// File being resolved, with the mapping from its symbols to change ids.
struct Scope<'a> {
    path: &'a str,
    facts: &'a Facts,
    lang: &'a dyn Language,
    changed_file: Option<usize>,
    locs: &'a HashMap<Loc, usize>,
    /// Classes whose constructor changed (head side only).
    constructors: Option<&'a HashMap<Loc, usize>>,
}

impl Scope<'_> {
    /// Change reached by referencing `sym`: its own change, or its changed constructor.
    fn target_of(&self, sym: u32) -> Option<usize> {
        let constructor = || {
            let loc = Loc {
                file: self.changed_file?,
                sym,
            };
            self.constructors?.get(&loc).copied()
        };
        self.change_of(sym).or_else(constructor)
    }

    fn change_of(&self, sym: u32) -> Option<usize> {
        self.locs
            .get(&Loc {
                file: self.changed_file?,
                sym,
            })
            .copied()
    }
}

/// Resolve references in the head snapshot (edges + impact) and in deleted base code (edges).
pub fn link(
    registry: &Registry,
    files: &[ChangedFile],
    head: &[IndexedFile],
    set: &ChangeSet,
    hub_threshold: usize,
) -> Links {
    let (head_names, base_names) = name_tables(files, set);
    let counts = short_name_counts(head, &head_names);
    let changed_by_path: HashMap<&str, usize> = files
        .iter()
        .enumerate()
        .filter_map(|(i, f)| Some((f.change.new_path.as_deref()?, i)))
        .collect();

    let mut hits: Vec<Hit> = head
        .par_iter()
        .filter_map(|file| {
            let scope = Scope {
                path: &file.path,
                facts: &file.facts,
                lang: registry.by_id(file.lang)?,
                changed_file: changed_by_path.get(file.path.as_str()).copied(),
                locs: &set.head_locs,
                constructors: Some(&set.constructors),
            };
            Some(resolve_file(&scope, &head_names, &counts, set))
        })
        .flatten()
        .collect();
    hits.extend(deleted_code_hits(files, set, &base_names, &counts));
    aggregate(hits, set.changes.len(), hub_threshold)
}

fn name_tables(files: &[ChangedFile], set: &ChangeSet) -> (NameTable, NameTable) {
    let mut head = NameTable::default();
    let mut base = NameTable::default();
    for (loc, &id) in &set.head_locs {
        let file = &files[loc.file];
        let (Some(lang), Some(path), Some(facts)) = (
            file.lang,
            file.change.new_path.as_deref(),
            file.head.as_deref(),
        ) else {
            continue;
        };
        head.insert(
            lang,
            path,
            facts,
            loc.sym,
            Target {
                id,
                dangling: false,
            },
        );
    }
    for (loc, &id) in &set.base_locs {
        let file = &files[loc.file];
        let (Some(lang), Some(path), Some(facts)) = (
            file.lang,
            file.change.old_path.as_deref(),
            file.base.as_deref(),
        ) else {
            continue;
        };
        base.insert(
            lang,
            path,
            facts,
            loc.sym,
            Target {
                id,
                dangling: false,
            },
        );
        // Head code still using a base-only name points at something that no longer exists.
        if matches!(set.changes[id].change, Change::Deleted | Change::Moved) {
            head.insert(lang, path, facts, loc.sym, Target { id, dangling: true });
        }
    }
    for (loc, &id) in &set.constructors {
        let file = &files[loc.file];
        let (Some(lang), Some(path), Some(facts)) = (
            file.lang,
            file.change.new_path.as_deref(),
            file.head.as_deref(),
        ) else {
            continue;
        };
        head.insert(
            lang,
            path,
            facts,
            loc.sym,
            Target {
                id,
                dangling: false,
            },
        );
    }
    (head, base)
}

/// How many head definitions share each candidate short name (for fallback uniqueness).
fn short_name_counts(head: &[IndexedFile], names: &NameTable) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let symbols = head.iter().flat_map(|f| f.facts.symbols.iter());
    for symbol in symbols.filter(|s| names.short.contains_key(&s.name)) {
        *counts.entry(symbol.name.clone()).or_default() += 1;
    }
    counts
}

fn resolve_file(
    scope: &Scope,
    names: &NameTable,
    counts: &HashMap<String, usize>,
    set: &ChangeSet,
) -> Vec<Hit> {
    let candidates = scope
        .facts
        .refs
        .iter()
        .filter(|r| names.short.contains_key(last_part(&r.name)));
    let candidates: Vec<&Ref> = candidates.collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let qualnames = scope.facts.qualname_index();
    candidates
        .into_iter()
        .filter_map(|r| {
            let (target, confidence) = resolve_ref(r, scope, &qualnames, names, counts)?;
            let caller = caller_of(r, scope, set);
            let kind = if r.is_call {
                EdgeKind::Call
            } else {
                EdgeKind::Reference
            };
            let call = r.call.clone().map(|args| CallSite {
                line: r.line,
                args,
                callee: r.name.clone(),
            });
            Some(Hit {
                caller,
                line: r.line,
                target,
                kind,
                confidence,
                call,
            })
        })
        .collect()
}

/// Resolution order: same-file scope, explicit imports, wildcard imports, unique-name fallback.
fn resolve_ref(
    r: &Ref,
    scope: &Scope,
    qualnames: &HashMap<&str, u32>,
    names: &NameTable,
    counts: &HashMap<String, usize>,
) -> Option<(Target, Confidence)> {
    let enclosing = r
        .enclosing
        .map(|e| scope.facts.symbols[e as usize].qualname.as_str());
    let (head, rest) = r
        .name
        .split_once('.')
        .map_or((r.name.as_str(), None), |(h, t)| (h, Some(t)));
    // A parameter (e.g. an injected pytest fixture) shadows module-level and imported names.
    let shadowed = scope.facts.is_parameter(r.enclosing, head);
    if shadowed && rest.is_none() {
        return None;
    }
    let local = scope.lang.scope_candidates(&r.name, enclosing);
    if let Some(&sym) = local.iter().find_map(|c| qualnames.get(c.as_str())) {
        let id = scope.target_of(sym)?;
        return Some((
            Target {
                id,
                dangling: false,
            },
            Confidence::High,
        ));
    }

    if shadowed {
        return fallback(r, names, counts);
    }
    let bound = scope
        .facts
        .imports
        .iter()
        .find(|i| i.local.as_deref() == Some(head));
    if let Some(import) = bound {
        let module = scope.lang.resolve_import(scope.path, import)?;
        let full = rest.map_or(module.clone(), |rest| format!("{module}.{rest}"));
        return names.qualified.get(&full).map(|t| (*t, Confidence::High));
    }

    let wildcards = scope.facts.imports.iter().filter(|i| i.local.is_none());
    let via_wildcard = wildcards
        .filter_map(|i| scope.lang.resolve_import(scope.path, i))
        .find_map(|module| names.qualified.get(&format!("{module}.{}", r.name)));
    if let Some(target) = via_wildcard {
        return Some((*target, Confidence::High));
    }

    fallback(r, names, counts)
}

/// Last resort for calls: a unique definition with the same name.
fn fallback(
    r: &Ref,
    names: &NameTable,
    counts: &HashMap<String, usize>,
) -> Option<(Target, Confidence)> {
    r.is_call
        .then(|| unique_by_name(last_part(&r.name), names, counts))
        .flatten()
}

/// Fall back to a bare-name match when exactly one definition in the repo has that name.
fn unique_by_name(
    name: &str,
    names: &NameTable,
    counts: &HashMap<String, usize>,
) -> Option<(Target, Confidence)> {
    let [target] = names.short.get(name)?.as_slice() else {
        return None;
    };
    let unique = counts.get(name).copied().unwrap_or(0) <= 1;
    unique.then_some((*target, Confidence::Low))
}

fn caller_of(r: &Ref, scope: &Scope, set: &ChangeSet) -> Caller {
    let changed = match r.enclosing {
        Some(sym) => scope.change_of(sym),
        None => scope
            .changed_file
            .and_then(|f| set.modules.get(&f).copied()),
    };
    match changed {
        Some(id) => Caller::Changed(id),
        None => Caller::Unchanged {
            path: scope.path.to_string(),
            qualname: r
                .enclosing
                .map(|e| scope.facts.symbols[e as usize].qualname.clone()),
        },
    }
}

/// Edges out of deleted code, which only exists on the base side.
fn deleted_code_hits(
    files: &[ChangedFile],
    set: &ChangeSet,
    names: &NameTable,
    counts: &HashMap<String, usize>,
) -> Vec<Hit> {
    let deleted: HashSet<usize> = set
        .base_locs
        .values()
        .copied()
        .filter(|&id| set.changes[id].change == Change::Deleted)
        .collect();
    if deleted.is_empty() {
        return Vec::new();
    }
    files
        .iter()
        .enumerate()
        .filter_map(|(i, file)| {
            let facts = file.base.as_deref()?;
            let scope = Scope {
                path: file.change.old_path.as_deref()?,
                facts,
                lang: file.lang?,
                changed_file: Some(i),
                locs: &set.base_locs,
                constructors: None,
            };
            Some(resolve_file(&scope, names, counts, set))
        })
        .flatten()
        .filter(|hit| matches!(hit.caller, Caller::Changed(id) if deleted.contains(&id)))
        .collect()
}

fn aggregate(hits: Vec<Hit>, count: usize, hub_threshold: usize) -> Links {
    let mut referrers: Vec<HashSet<&Caller>> = vec![HashSet::new(); count];
    for hit in &hits {
        referrers[hit.target.id].insert(&hit.caller);
    }
    let ref_counts: Vec<usize> = referrers.iter().map(HashSet::len).collect();
    let hub: Vec<bool> = ref_counts.iter().map(|&n| n > hub_threshold).collect();

    let mut edges: BTreeMap<(usize, usize), (EdgeKind, Confidence, Vec<CallSite>)> =
        BTreeMap::new();
    let mut impact: Vec<BTreeMap<(String, Option<String>), Impact>> = vec![BTreeMap::new(); count];
    for hit in &hits {
        let to = hit.target.id;
        match &hit.caller {
            Caller::Changed(from) if *from != to => {
                let (kind, confidence, calls) =
                    edges
                        .entry((*from, to))
                        .or_insert((hit.kind, hit.confidence, Vec::new()));
                *kind = (*kind).max(hit.kind);
                *confidence = (*confidence).max(hit.confidence);
                calls.extend(hit.call.clone());
            }
            Caller::Changed(_) => {}
            Caller::Unchanged { path, qualname } => {
                let key = (path.clone(), qualname.clone());
                let entry = impact[to].entry(key).or_insert_with(|| Impact {
                    path: path.clone(),
                    qualname: qualname.clone(),
                    line: hit.line,
                    target: to,
                    confidence: hit.confidence,
                    dangling: hit.target.dangling,
                    call_check: None,
                    calls: Vec::new(),
                });
                entry.line = entry.line.min(hit.line);
                entry.calls.extend(hit.call.clone());
                entry.confidence = entry.confidence.max(hit.confidence);
            }
        }
    }
    let edges = edges
        .into_iter()
        .map(|((from, to), (kind, confidence, calls))| Edge {
            from,
            to,
            kind,
            via: None,
            confidence,
            hub: hub[to],
            call_check: None,
            calls,
        })
        .collect();
    let impact = impact
        .into_iter()
        .map(|m| m.into_values().collect())
        .collect();
    Links {
        edges,
        impact,
        ref_counts,
        hub,
    }
}

fn last_part(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}
