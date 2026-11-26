//! Language-agnostic normalisation of extractor output into cacheable per-blob facts.
//!
//! Resolves symbol nesting (qualnames, methods), enclosing symbols of references and content
//! hashes used for change and move detection.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::lang::{CallArgs, Import, RawFacts, RawSymbol, Signature, SymbolKind};

/// A definition with resolved nesting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    /// Dotted path inside the file, e.g. `Repo.save`. Duplicates get a `#n` suffix.
    pub qualname: String,
    pub kind: SymbolKind,
    pub parent: Option<u32>,
    pub start_line: u32,
    pub end_line: u32,
    /// Hash of the symbol text excluding nested definitions; changes only when this symbol's
    /// own code changes.
    pub own_hash: u64,
    /// Indentation-insensitive hash of the full text; equal across moves.
    pub move_hash: u64,
    pub signature: Option<Signature>,
    /// Control-flow markers in the symbol's own body (not nested definitions).
    #[serde(default)]
    pub markers: Vec<String>,
}

/// A reference with its innermost enclosing symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ref {
    pub name: String,
    pub line: u32,
    pub is_call: bool,
    pub enclosing: Option<u32>,
    pub call: Option<CallArgs>,
}

/// Normalised, path-independent facts about one blob.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Facts {
    pub symbols: Vec<Symbol>,
    pub refs: Vec<Ref>,
    pub imports: Vec<Import>,
    pub has_errors: bool,
}

impl Facts {
    /// Innermost symbol whose line range contains `line`.
    pub fn innermost_at_line(&self, line: u32) -> Option<u32> {
        let containing = self.symbols.iter().enumerate();
        let containing = containing.filter(|(_, s)| s.start_line <= line && line <= s.end_line);
        containing
            .min_by_key(|(_, s)| s.end_line - s.start_line)
            .map(|(i, _)| i as u32)
    }

    /// Symbol index by qualname.
    pub fn qualname_index(&self) -> HashMap<&str, u32> {
        let entries = self.symbols.iter().enumerate();
        entries
            .map(|(i, s)| (s.qualname.as_str(), i as u32))
            .collect()
    }

    /// Whether `name` is a parameter of `enclosing` or of any definition around it.
    pub fn is_parameter(&self, enclosing: Option<u32>, name: &str) -> bool {
        let mut current = enclosing;
        while let Some(sym) = current {
            let symbol = &self.symbols[sym as usize];
            let mut params = symbol.signature.iter().flat_map(|s| s.params.iter());
            if params.any(|p| p.name == name) {
                return true;
            }
            current = symbol.parent;
        }
        false
    }
}

/// Build [`Facts`] from raw extractor output for `source`.
pub fn normalize(raw: RawFacts, source: &[u8]) -> Facts {
    let mut raw_symbols: Vec<RawSymbol> = raw.symbols;
    raw_symbols.sort_by_key(|s| (s.start_byte, std::cmp::Reverse(s.end_byte)));
    raw_symbols.dedup_by_key(|s| (s.start_byte, s.end_byte));
    let parents: Vec<Option<usize>> = compute_parents(&raw_symbols);
    let mut symbols: Vec<Symbol> = build_symbols(&raw_symbols, &parents, source);
    for marker in raw.markers {
        if let Some(i) = enclosing_symbol(&raw_symbols, &parents, marker.byte) {
            symbols[i].markers.push(marker.text);
        }
    }
    let refs: Vec<Ref> = raw
        .refs
        .into_iter()
        .map(|r| Ref {
            enclosing: enclosing_symbol(&raw_symbols, &parents, r.byte).map(|i| i as u32),
            name: r.name,
            line: r.line,
            is_call: r.is_call,
            call: r.call,
        })
        .collect();
    Facts {
        symbols,
        refs,
        imports: raw.imports,
        has_errors: raw.has_errors,
    }
}

/// Parent of each symbol, given symbols sorted by start (outer first on ties).
fn compute_parents(symbols: &[RawSymbol]) -> Vec<Option<usize>> {
    let mut parents: Vec<Option<usize>> = Vec::with_capacity(symbols.len());
    let mut stack: Vec<usize> = Vec::new();
    for (i, symbol) in symbols.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&top| symbols[top].end_byte < symbol.end_byte)
        {
            stack.pop();
        }
        parents.push(stack.last().copied());
        stack.push(i);
    }
    parents
}

fn build_symbols(raw: &[RawSymbol], parents: &[Option<usize>], source: &[u8]) -> Vec<Symbol> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); raw.len()];
    for (i, parent) in parents.iter().enumerate() {
        if let Some(p) = parent {
            children[*p].push(i);
        }
    }
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut symbols: Vec<Symbol> = Vec::with_capacity(raw.len());
    for (i, symbol) in raw.iter().enumerate() {
        let parent: Option<&Symbol> = parents[i].map(|p| &symbols[p]);
        let base = parent.map_or(symbol.name.clone(), |p| {
            format!("{}.{}", p.qualname, symbol.name)
        });
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        let qualname = if *count == 1 {
            base
        } else {
            format!("{base}#{count}")
        };
        let is_method = symbol.kind == SymbolKind::Function
            && parent.is_some_and(|p| p.kind == SymbolKind::Class);
        let child_ranges = children[i]
            .iter()
            .map(|&c| (raw[c].start_byte, raw[c].end_byte));
        let text = &source[symbol.start_byte..symbol.end_byte.min(source.len())];
        symbols.push(Symbol {
            name: symbol.name.clone(),
            qualname,
            kind: if is_method {
                SymbolKind::Method
            } else {
                symbol.kind
            },
            parent: parents[i].map(|p| p as u32),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            own_hash: own_hash(source, symbol.start_byte, symbol.end_byte, child_ranges),
            move_hash: move_hash(text),
            signature: symbol.signature.clone(),
            markers: Vec::new(),
        });
    }
    symbols
}

/// Innermost symbol containing `byte`. Relies on start-sorted symbols: the last symbol starting
/// at or before `byte` either contains it or has an ancestor that does.
fn enclosing_symbol(
    symbols: &[RawSymbol],
    parents: &[Option<usize>],
    byte: usize,
) -> Option<usize> {
    let after = symbols.partition_point(|s| s.start_byte <= byte);
    let mut candidate: Option<usize> = after.checked_sub(1);
    while let Some(i) = candidate {
        if byte < symbols[i].end_byte {
            return Some(i);
        }
        candidate = parents[i];
    }
    None
}

fn own_hash(
    source: &[u8],
    start: usize,
    end: usize,
    children: impl Iterator<Item = (usize, usize)>,
) -> u64 {
    let mut hasher = Fnv::new();
    let mut cursor = start;
    for (child_start, child_end) in children {
        hasher.write(&source[cursor..child_start]);
        cursor = child_end;
    }
    hasher.write(&source[cursor..end.min(source.len())]);
    hasher.finish()
}

fn move_hash(text: &[u8]) -> u64 {
    let mut hasher = Fnv::new();
    let lines = text.split(|&b| b == b'\n').map(|line| line.trim_ascii());
    for line in lines.filter(|line| !line.is_empty()) {
        hasher.write(line);
        hasher.write(b"\n");
    }
    hasher.finish()
}

/// FNV-1a: stable across builds, so hashes can be cached.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;
    use crate::lang::python::Python;

    fn facts(source: &str) -> Facts {
        let raw = Python::new().extract(source.as_bytes()).unwrap();
        normalize(raw, source.as_bytes())
    }

    fn symbol<'a>(facts: &'a Facts, qualname: &str) -> &'a Symbol {
        facts
            .symbols
            .iter()
            .find(|s| s.qualname == qualname)
            .expect(qualname)
    }

    #[test]
    fn builds_qualnames_and_methods() {
        let f = facts("class K:\n    def m(self):\n        def inner():\n            pass\n");
        assert_eq!(symbol(&f, "K.m").kind, SymbolKind::Method);
        assert_eq!(symbol(&f, "K.m.inner").kind, SymbolKind::Function);
    }

    #[test]
    fn duplicate_qualnames_get_suffix() {
        let f = facts("def f():\n    pass\n\ndef f():\n    return 1\n");
        assert!(f.symbols.iter().any(|s| s.qualname == "f#2"));
    }

    #[test]
    fn own_hash_ignores_nested_changes() {
        let before = facts("class K:\n    x = 1\n    def m(self):\n        return 1\n");
        let after = facts("class K:\n    x = 1\n    def m(self):\n        return 2\n");
        assert_eq!(symbol(&before, "K").own_hash, symbol(&after, "K").own_hash);
        assert_ne!(
            symbol(&before, "K.m").own_hash,
            symbol(&after, "K.m").own_hash
        );
    }

    #[test]
    fn move_hash_ignores_indentation() {
        let nested = facts("class K:\n    def m(self):\n        return 1\n");
        let top = facts("def m(self):\n    return 1\n");
        assert_eq!(
            symbol(&nested, "K.m").move_hash,
            symbol(&top, "m").move_hash
        );
    }

    #[test]
    fn refs_get_innermost_enclosing_symbol() {
        let f = facts("def a():\n    b()\n\nc()\n");
        let enclosing: Vec<(&str, Option<&str>)> = f
            .refs
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    r.enclosing.map(|i| f.symbols[i as usize].qualname.as_str()),
                )
            })
            .collect();
        assert_eq!(enclosing, vec![("b", Some("a")), ("c", None)]);
    }
}
