//! Symbol-level change detection between base and head facts of changed files.

use std::collections::{HashMap, HashSet};

use crate::diff::{DiffLine, LineKind};
use crate::facts::Facts;
use crate::git::FileStatus;
use crate::lang::SymbolKind;
use crate::model::Change;

use super::ChangedFile;

/// A symbol in a changed file: `file` indexes the changed-file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Loc {
    pub file: usize,
    pub sym: u32,
}

/// One changed symbol. Module-level changes have neither `head` nor `base`.
#[derive(Debug, Clone)]
pub struct SymbolChange {
    pub change: Change,
    pub head: Option<Loc>,
    pub base: Option<Loc>,
    pub module_of: Option<usize>,
}

/// Detected changes plus lookup tables from symbol locations to change ids.
///
/// Added/deleted/moved definitions are collapsed to their outermost symbol; their nested
/// definitions map to the same change id.
pub struct ChangeSet {
    pub changes: Vec<SymbolChange>,
    pub head_locs: HashMap<Loc, usize>,
    pub base_locs: HashMap<Loc, usize>,
    pub modules: HashMap<usize, usize>,
    /// Unchanged classes whose constructor changed: calling the class reaches that change.
    pub constructors: HashMap<Loc, usize>,
}

impl ChangeSet {
    /// Change id owning a head-side line, falling back to the file's module-level change.
    pub fn at_head_line(&self, files: &[ChangedFile], file: usize, line: u32) -> Option<usize> {
        let Some(facts) = files[file].head.as_deref() else {
            return self.modules.get(&file).copied();
        };
        self.at_line(facts, &self.head_locs, file, line)
    }

    /// Change id owning a base-side line, falling back to the file's module-level change.
    pub fn at_base_line(&self, files: &[ChangedFile], file: usize, line: u32) -> Option<usize> {
        let Some(facts) = files[file].base.as_deref() else {
            return self.modules.get(&file).copied();
        };
        self.at_line(facts, &self.base_locs, file, line)
    }

    fn at_line(
        &self,
        facts: &Facts,
        locs: &HashMap<Loc, usize>,
        file: usize,
        line: u32,
    ) -> Option<usize> {
        let Some(innermost) = facts.innermost_at_line(line) else {
            return self.modules.get(&file).copied();
        };
        let mut current: Option<u32> = Some(innermost);
        while let Some(sym) = current {
            if let Some(id) = locs.get(&Loc { file, sym }) {
                return Some(*id);
            }
            current = facts.symbols[sym as usize].parent;
        }
        None
    }
}

/// Detect changed symbols across all analysed files.
pub fn detect(files: &[ChangedFile]) -> ChangeSet {
    let changes: Vec<SymbolChange> = files
        .iter()
        .enumerate()
        .flat_map(|(i, f)| detect_file(i, f))
        .collect();
    let changes = pair_moves(changes, files);
    index_changes(changes, files)
}

fn detect_file(file: usize, changed: &ChangedFile) -> Vec<SymbolChange> {
    if changed.lang.is_none() {
        // No language support: the whole file is one change node.
        return vec![SymbolChange {
            change: module_change_kind(changed),
            head: None,
            base: None,
            module_of: Some(file),
        }];
    }
    let empty = Facts::default();
    let base: &Facts = changed.base.as_deref().unwrap_or(&empty);
    let head: &Facts = changed.head.as_deref().unwrap_or(&empty);
    let base_names = base.qualname_index();
    let head_names = head.qualname_index();

    let mut changes: Vec<SymbolChange> = Vec::new();
    let added = outermost_missing(head, &base_names);
    for (sym, symbol) in head.symbols.iter().enumerate() {
        let sym = sym as u32;
        let loc = Some(Loc { file, sym });
        match base_names.get(symbol.qualname.as_str()) {
            Some(&b) if base.symbols[b as usize].own_hash != symbol.own_hash => {
                let base_loc = Some(Loc { file, sym: b });
                changes.push(symbol_change(Change::Modified, loc, base_loc));
            }
            None if added.contains(&sym) => changes.push(symbol_change(Change::Added, loc, None)),
            _ => {}
        }
    }
    for sym in outermost_missing(base, &head_names) {
        changes.push(symbol_change(
            Change::Deleted,
            None,
            Some(Loc { file, sym }),
        ));
    }
    // A file added, deleted or renamed without any definition change (e.g. an empty
    // `__init__.py`) is still a change worth a node.
    let whole_file = matches!(
        changed.change.status,
        FileStatus::Added | FileStatus::Deleted | FileStatus::Renamed
    );
    if has_module_change(changed, base, head) || (changes.is_empty() && whole_file) {
        changes.push(SymbolChange {
            change: module_change_kind(changed),
            head: None,
            base: None,
            module_of: Some(file),
        });
    }
    changes
}

fn symbol_change(change: Change, head: Option<Loc>, base: Option<Loc>) -> SymbolChange {
    SymbolChange {
        change,
        head,
        base,
        module_of: None,
    }
}

/// Symbols whose qualname is absent on the other side, excluding those nested in another
/// such symbol.
fn outermost_missing(facts: &Facts, other: &HashMap<&str, u32>) -> HashSet<u32> {
    let mut missing: HashSet<u32> = HashSet::new();
    let mut outermost: HashSet<u32> = HashSet::new();
    for (i, symbol) in facts.symbols.iter().enumerate() {
        if other.contains_key(symbol.qualname.as_str()) {
            continue;
        }
        let i = i as u32;
        if !symbol.parent.is_some_and(|p| missing.contains(&p)) {
            outermost.insert(i);
        }
        missing.insert(i);
    }
    outermost
}

/// Whether any non-blank changed line falls outside every definition.
fn has_module_change(changed: &ChangedFile, base: &Facts, head: &Facts) -> bool {
    let lines = changed.hunks.iter().flat_map(|h| h.changed_lines());
    lines
        .filter(|l| !l.text.trim().is_empty())
        .any(|line| outside_symbols(line, base, head))
}

fn outside_symbols(line: &DiffLine, base: &Facts, head: &Facts) -> bool {
    match (line.kind, line.old_line, line.new_line) {
        (LineKind::Added, _, Some(n)) => head.innermost_at_line(n).is_none(),
        (LineKind::Removed, Some(o), _) => base.innermost_at_line(o).is_none(),
        _ => false,
    }
}

fn module_change_kind(changed: &ChangedFile) -> Change {
    match changed.change.status {
        FileStatus::Added => Change::Added,
        FileStatus::Deleted => Change::Deleted,
        _ => Change::Modified,
    }
}

/// Merge added/deleted pairs with identical (indentation-insensitive) bodies into moves.
fn pair_moves(changes: Vec<SymbolChange>, files: &[ChangedFile]) -> Vec<SymbolChange> {
    let mut deleted_by_hash: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, change) in changes.iter().enumerate() {
        if let (Change::Deleted, Some(loc)) = (change.change, change.base)
            && let Some(hash) = movable_hash(files[loc.file].base.as_deref(), loc.sym)
        {
            deleted_by_hash.entry(hash).or_default().push(i);
        }
    }
    let mut consumed: HashSet<usize> = HashSet::new();
    let mut merged: Vec<SymbolChange> = changes.clone();
    for change in merged.iter_mut().filter(|c| c.change == Change::Added) {
        let Some(loc) = change.head else { continue };
        let Some(hash) = movable_hash(files[loc.file].head.as_deref(), loc.sym) else {
            continue;
        };
        let candidates = deleted_by_hash.get_mut(&hash);
        let Some(source) = candidates.and_then(|c| (!c.is_empty()).then(|| c.remove(0))) else {
            continue;
        };
        consumed.insert(source);
        change.change = Change::Moved;
        change.base = changes[source].base;
    }
    let kept = merged
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !consumed.contains(i));
    kept.map(|(_, c)| c).collect()
}

fn movable_hash(facts: Option<&Facts>, sym: u32) -> Option<u64> {
    let symbol = facts?.symbols.get(sym as usize)?;
    (symbol.kind != SymbolKind::Variable).then_some(symbol.move_hash)
}

fn index_changes(changes: Vec<SymbolChange>, files: &[ChangedFile]) -> ChangeSet {
    let mut head_locs: HashMap<Loc, usize> = HashMap::new();
    let mut base_locs: HashMap<Loc, usize> = HashMap::new();
    let mut modules: HashMap<usize, usize> = HashMap::new();
    for (id, change) in changes.iter().enumerate() {
        let collapsed = change.change != Change::Modified;
        if let Some(loc) = change.head {
            for sym in members(files[loc.file].head.as_deref(), loc.sym, collapsed) {
                head_locs
                    .entry(Loc {
                        file: loc.file,
                        sym,
                    })
                    .or_insert(id);
            }
        }
        if let Some(loc) = change.base {
            for sym in members(files[loc.file].base.as_deref(), loc.sym, collapsed) {
                base_locs
                    .entry(Loc {
                        file: loc.file,
                        sym,
                    })
                    .or_insert(id);
            }
        }
        if let Some(file) = change.module_of {
            modules.insert(file, id);
        }
    }
    let constructors = changed_constructors(&changes, files, &head_locs);
    ChangeSet {
        changes,
        head_locs,
        base_locs,
        modules,
        constructors,
    }
}

/// Map each unchanged class to its changed constructor (e.g. `__init__`), so `Class(...)` calls
/// count as calls of the constructor.
fn changed_constructors(
    changes: &[SymbolChange],
    files: &[ChangedFile],
    head_locs: &HashMap<Loc, usize>,
) -> HashMap<Loc, usize> {
    let mut constructors: HashMap<Loc, usize> = HashMap::new();
    for (id, change) in changes.iter().enumerate() {
        let Some(loc) = change.head else { continue };
        let file = &files[loc.file];
        let (Some(lang), Some(facts)) = (file.lang, file.head.as_deref()) else {
            continue;
        };
        let symbol = &facts.symbols[loc.sym as usize];
        let Some(parent) = symbol
            .parent
            .filter(|_| lang.constructor_name() == Some(symbol.name.as_str()))
        else {
            continue;
        };
        let class = Loc {
            file: loc.file,
            sym: parent,
        };
        let is_class = facts.symbols[parent as usize].kind == SymbolKind::Class;
        if is_class && !head_locs.contains_key(&class) {
            constructors.insert(class, id);
        }
    }
    constructors
}

/// The symbol itself plus, when collapsed, all nested definitions.
fn members(facts: Option<&Facts>, sym: u32, collapsed: bool) -> Vec<u32> {
    let Some(facts) = facts.filter(|_| collapsed) else {
        return vec![sym];
    };
    let is_descendant = |mut current: Option<u32>| {
        while let Some(c) = current {
            if c == sym {
                return true;
            }
            current = facts.symbols[c as usize].parent;
        }
        false
    };
    let following = (sym + 1..facts.symbols.len() as u32).take_while(|&j| is_descendant(Some(j)));
    std::iter::once(sym).chain(following).collect()
}
