//! Grouping changed symbols into logical changes and ordering them for review.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::lang::SymbolKind;
use crate::model::{Change, ChangedSymbol, Edge, EdgeKind};

/// Commits touching more changes than this are too broad to say "these belong together".
const MAX_COMMIT_GROUP: usize = 200;

/// A cluster before hunks, commits and impact are attached.
pub struct Group {
    /// Most referenced symbol in the group, used to describe it.
    pub main: String,
    pub symbols: Vec<usize>,
}

/// Clusters, largest first: code connected by non-hub calls, with tests, loose code and files
/// attached to the cluster they are most connected to. `commit_groups` lists, per meaningful
/// commit, the changes it touched.
pub fn group(
    symbols: &[ChangedSymbol],
    edges: &[Edge],
    commit_groups: &[Vec<usize>],
) -> Vec<Group> {
    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    let keys = assign_clusters(symbols, edges, commit_groups);
    for (id, key) in keys.into_iter().enumerate() {
        components.entry(key).or_default().push(id);
    }
    let mut groups: Vec<Group> = components
        .into_values()
        .map(|members| {
            let ordered = review_order(&members, symbols, edges);
            Group {
                main: title(&ordered, symbols, edges),
                symbols: ordered,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        let key = |g: &Group| {
            (
                std::cmp::Reverse(g.symbols.len()),
                sort_key(&symbols[g.symbols[0]]),
            )
        };
        key(a).cmp(&key(b))
    });
    groups
}

/// Cluster key per symbol.
///
/// Only calls between non-test symbols merge clusters: tests and shared fixtures touch many
/// unrelated changes, and type annotations / base classes are too weak a link on their own.
/// Everything else (tests, lone code, files) is then attached in tiers of decreasing strength,
/// each run to completion before the next: references and mentions, same file, shared rare
/// tokens, same commit. Attaching never merges two clusters.
fn assign_clusters(
    symbols: &[ChangedSymbol],
    edges: &[Edge],
    commit_groups: &[Vec<usize>],
) -> Vec<usize> {
    let is_test = |id: usize| symbols[id].is_test;
    let usable: Vec<&Edge> = edges.iter().filter(|e| !e.hub && e.from != e.to).collect();
    let mut sets = UnionFind::new(symbols.len());
    let merging = usable.iter().filter(|e| e.kind == EdgeKind::Call);
    for edge in merging.filter(|e| !is_test(e.from) && !is_test(e.to)) {
        sets.union(edge.from, edge.to);
    }
    for (a, b) in new_file_pairs(symbols) {
        sets.union(a, b);
    }
    let roots: Vec<usize> = (0..symbols.len()).map(|id| sets.find(id)).collect();
    let mut sizes: HashMap<usize, usize> = HashMap::new();
    for &root in &roots {
        *sizes.entry(root).or_default() += 1;
    }
    let mut cluster: Vec<Option<usize>> = (0..symbols.len())
        .map(|id| (!is_test(id) && sizes[&roots[id]] > 1).then_some(roots[id]))
        .collect();

    let direct = usable
        .iter()
        .filter(|e| e.kind != EdgeKind::Shared)
        .map(|e| (e.from, e.to));
    let shared = usable
        .iter()
        .filter(|e| e.kind == EdgeKind::Shared)
        .map(|e| (e.from, e.to));
    let tiers: Vec<Vec<(usize, usize)>> = vec![
        direct.chain(module_file_pairs(symbols)).collect(),
        same_file_pairs(symbols),
        shared.collect(),
        commit_pairs(commit_groups),
    ];
    for tier in &tiers {
        attach_loose(&mut cluster, &adjacency(symbols, tier));
    }
    // Whatever is still loose (e.g. config files only linked to each other) groups by any link.
    for &(a, b) in tiers.iter().flatten() {
        if cluster[a].is_none() && cluster[b].is_none() {
            sets.union(a, b);
        }
    }
    (0..symbols.len())
        .map(|id| cluster[id].unwrap_or_else(|| sets.find(id)))
        .collect()
}

/// Undirected adjacency. Code never follows a link into a test: tests must not pull code into
/// their cluster.
fn adjacency(symbols: &[ChangedSymbol], pairs: &[(usize, usize)]) -> HashMap<usize, Vec<usize>> {
    let mut adjacent: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut link = |from: usize, to: usize| {
        if symbols[from].is_test || !symbols[to].is_test {
            adjacent.entry(from).or_default().push(to);
        }
    };
    for &(a, b) in pairs {
        link(a, b);
        link(b, a);
    }
    adjacent
}

fn by_path(symbols: &[ChangedSymbol]) -> HashMap<&str, Vec<usize>> {
    let mut paths: HashMap<&str, Vec<usize>> = HashMap::new();
    for symbol in symbols {
        paths
            .entry(symbol.path.as_str())
            .or_default()
            .push(symbol.id);
    }
    paths
}

/// Module-level changes (mostly import edits) belong with the rest of their file: imports are
/// not references, so this is their direct link.
fn module_file_pairs(symbols: &[ChangedSymbol]) -> Vec<(usize, usize)> {
    let paths = by_path(symbols);
    let modules = symbols.iter().filter(|s| s.kind == SymbolKind::Module);
    modules
        .flat_map(|m| {
            let same_file = paths.get(m.path.as_str()).into_iter().flatten();
            same_file
                .filter(move |&&other| other != m.id)
                .map(move |&other| (m.id, other))
        })
        .collect()
}

/// A newly added file is reviewed as one unit: chain together its non-test changes.
fn new_file_pairs(symbols: &[ChangedSymbol]) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for ids in by_path(symbols).values() {
        let all_added = ids.iter().all(|&id| symbols[id].change == Change::Added);
        if all_added && !symbols[ids[0]].is_test {
            pairs.extend(ids.windows(2).map(|w| (w[0], w[1])));
        }
    }
    pairs
}

/// Every pair of changes in the same file.
fn same_file_pairs(symbols: &[ChangedSymbol]) -> Vec<(usize, usize)> {
    let paths = by_path(symbols);
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for ids in paths.values() {
        for (i, &a) in ids.iter().enumerate() {
            pairs.extend(ids[i + 1..].iter().map(|&b| (a, b)));
        }
    }
    pairs
}

/// Every pair of changes touched by the same (non-generic, not too broad) commit.
fn commit_pairs(commit_groups: &[Vec<usize>]) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for ids in commit_groups.iter().filter(|g| g.len() <= MAX_COMMIT_GROUP) {
        for (i, &a) in ids.iter().enumerate() {
            pairs.extend(ids[i + 1..].iter().map(|&b| (a, b)));
        }
    }
    pairs
}

/// Attach unassigned symbols to the cluster most of their neighbours belong to. Runs in rounds so
/// direct links to clustered code win over indirect ones (e.g. test helpers inherit the cluster
/// of the tests using them).
fn attach_loose(cluster: &mut [Option<usize>], neighbours: &HashMap<usize, Vec<usize>>) {
    loop {
        let assigned: Vec<(usize, usize)> = (0..cluster.len())
            .filter(|&id| cluster[id].is_none())
            .filter_map(|id| Some((id, best_cluster(cluster, neighbours.get(&id)?)?)))
            .collect();
        if assigned.is_empty() {
            return;
        }
        for (id, key) in assigned {
            cluster[id] = Some(key);
        }
    }
}

/// Most common cluster among neighbours; ties go to the lowest key for determinism.
fn best_cluster(cluster: &[Option<usize>], neighbours: &[usize]) -> Option<usize> {
    let mut scores: BTreeMap<usize, usize> = BTreeMap::new();
    for key in neighbours.iter().filter_map(|&n| cluster[n]) {
        *scores.entry(key).or_default() += 1;
    }
    let best = scores
        .into_iter()
        .max_by_key(|&(key, n)| (n, std::cmp::Reverse(key)));
    best.map(|(key, _)| key)
}

/// Callees before callers, tests after everything else.
fn review_order(members: &[usize], symbols: &[ChangedSymbol], edges: &[Edge]) -> Vec<usize> {
    let (tests, code): (Vec<usize>, Vec<usize>) =
        members.iter().partition(|&&id| symbols[id].is_test);
    let mut ordered = topological(&code, symbols, edges);
    ordered.extend(topological(&tests, symbols, edges));
    ordered
}

/// Kahn's algorithm emitting a node once all its callees are emitted. Cycles are broken by
/// position so the result is total and deterministic.
fn topological(members: &[usize], symbols: &[ChangedSymbol], edges: &[Edge]) -> Vec<usize> {
    let inside: BTreeSet<usize> = members.iter().copied().collect();
    let internal = edges
        .iter()
        .filter(|e| inside.contains(&e.from) && inside.contains(&e.to));
    let internal: Vec<&Edge> = internal.filter(|e| e.from != e.to).collect();
    let mut pending: HashMap<usize, usize> = members.iter().map(|&id| (id, 0)).collect();
    let mut callers: HashMap<usize, Vec<usize>> = HashMap::new();
    for edge in &internal {
        *pending.entry(edge.from).or_default() += 1;
        callers.entry(edge.to).or_default().push(edge.from);
    }
    let mut remaining: BTreeSet<(String, u32, usize)> =
        members.iter().map(|&id| keyed(id, symbols)).collect();
    let mut ordered: Vec<usize> = Vec::with_capacity(members.len());
    while !remaining.is_empty() {
        let ready = remaining.iter().find(|(_, _, id)| pending[id] == 0);
        let next = ready
            .or_else(|| remaining.iter().next())
            .cloned()
            .expect("non-empty");
        remaining.remove(&next);
        let id = next.2;
        ordered.push(id);
        for caller in callers.get(&id).into_iter().flatten() {
            if let Some(count) = pending.get_mut(caller) {
                *count = count.saturating_sub(1);
            }
        }
    }
    ordered
}

/// Name of the group's central definition: callables and classes before constants, then most
/// called, then most referenced (within the group).
fn title(ordered: &[usize], symbols: &[ChangedSymbol], edges: &[Edge]) -> String {
    let inside: BTreeSet<usize> = ordered.iter().copied().collect();
    let mut calls: HashMap<usize, usize> = HashMap::new();
    let mut refs: HashMap<usize, usize> = HashMap::new();
    let internal = edges
        .iter()
        .filter(|e| inside.contains(&e.from) && inside.contains(&e.to));
    for edge in internal {
        match edge.kind {
            EdgeKind::Call => *calls.entry(edge.to).or_default() += 1,
            EdgeKind::Reference => *refs.entry(edge.to).or_default() += 1,
            _ => {}
        }
    }
    let definition = |id: usize| {
        !matches!(
            symbols[id].kind,
            SymbolKind::Variable | SymbolKind::Module | SymbolKind::File
        )
    };
    let score = |id: usize| {
        (
            definition(id),
            calls.get(&id).copied().unwrap_or(0),
            refs.get(&id).copied().unwrap_or(0),
            std::cmp::Reverse(id),
        )
    };
    let main = ordered.iter().copied().max_by_key(|&id| score(id));
    let Some(main) = main else {
        return String::new();
    };
    let symbol = &symbols[main];
    match symbol.kind {
        SymbolKind::Module | SymbolKind::File => symbol
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&symbol.path)
            .to_string(),
        _ => symbol.qualname.clone(),
    }
}

fn keyed(id: usize, symbols: &[ChangedSymbol]) -> (String, u32, usize) {
    let (path, line) = sort_key(&symbols[id]);
    (path, line, id)
}

fn sort_key(symbol: &ChangedSymbol) -> (String, u32) {
    let line = symbol.head.or(symbol.base).map_or(0, |r| r.start);
    (symbol.path.clone(), line)
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut current = x;
        while self.parent[current] != root {
            let next = self.parent[current];
            self.parent[current] = root;
            current = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}
