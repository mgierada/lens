//! Python support: tree-sitter extraction and import resolution.

use anyhow::{Context, Result, anyhow};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, Query, QueryCursor};

mod shape;

use super::{
    CallArgs, CallCheck, CallTarget, Import, Language, RawFacts, RawRef, RawSymbol, SymbolKind,
    nested_scope_candidates,
};

const SYMBOLS_QUERY: &str = include_str!("symbols.scm");
const REFS_QUERY: &str = include_str!("refs.scm");
/// Directories commonly used as import roots; stripped to produce module name aliases.
const SOURCE_ROOTS: [&str; 3] = ["src.", "lib.", "python."];
const RECEIVERS: [&str; 2] = ["self.", "cls."];

/// Python language support.
pub struct Python {
    language: tree_sitter::Language,
    symbols: Query,
    refs: Query,
}

impl Python {
    /// Compile the grammar queries. Panics only if bundled queries are invalid.
    pub fn new() -> Self {
        let language: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        let symbols = Query::new(&language, SYMBOLS_QUERY).expect("valid symbols query");
        let refs = Query::new(&language, REFS_QUERY).expect("valid refs query");
        Self {
            language,
            symbols,
            refs,
        }
    }

    fn extract_symbols(&self, root: Node, source: &[u8]) -> Vec<RawSymbol> {
        let names = self.symbols.capture_names();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.symbols, root, source);
        let mut symbols: Vec<RawSymbol> = Vec::new();
        while let Some(m) = matches.next() {
            let mut name: Option<String> = None;
            let mut definition: Option<(Node, SymbolKind)> = None;
            for capture in m.captures() {
                match names[capture.index as usize] {
                    "name" => name = node_text(capture.node, source),
                    "definition.function" => {
                        definition = Some((capture.node, SymbolKind::Function))
                    }
                    "definition.class" => definition = Some((capture.node, SymbolKind::Class)),
                    "definition.variable" => {
                        definition = Some((capture.node, SymbolKind::Variable))
                    }
                    _ => {}
                }
            }
            if let (Some(name), Some((node, kind))) = (name, definition) {
                let outer = with_decorators(node);
                let mut symbol = raw_symbol(name, kind, outer);
                symbol.signature = shape::signature(node, outer, source);
                symbols.push(symbol);
            }
        }
        symbols
    }

    fn extract_refs(&self, root: Node, source: &[u8]) -> Vec<RawRef> {
        let names = self.refs.capture_names();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.refs, root, source);
        let mut refs: Vec<RawRef> = Vec::new();
        while let Some(m) = matches.next() {
            for capture in m.captures() {
                let is_call = names[capture.index as usize] == "reference.call";
                if !is_call && !is_read(capture.node) {
                    continue;
                }
                let Some(name) = dotted_text(capture.node, source) else {
                    continue;
                };
                refs.push(RawRef {
                    name,
                    byte: capture.node.start_byte(),
                    line: capture.node.start_position().row as u32 + 1,
                    is_call,
                    call: if is_call {
                        shape::call_args(capture.node, source)
                    } else {
                        None
                    },
                });
            }
        }
        // A node can match several patterns; keep one ref per position, preferring calls.
        refs.sort_by_key(|r| (r.byte, !r.is_call));
        refs.dedup_by_key(|r| r.byte);
        refs
    }
}

impl Default for Python {
    fn default() -> Self {
        Self::new()
    }
}

impl Language for Python {
    fn id(&self) -> &'static str {
        "python"
    }

    fn version(&self) -> u32 {
        3
    }

    fn matches(&self, path: &str) -> bool {
        path.ends_with(".py") || path.ends_with(".pyi")
    }

    fn extract(&self, source: &[u8]) -> Result<RawFacts> {
        let mut parser = Parser::new();
        parser
            .set_language(&self.language)
            .context("loading python grammar")?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| anyhow!("python parse failed"))?;
        let root = tree.root_node();
        Ok(RawFacts {
            symbols: self.extract_symbols(root, source),
            refs: self.extract_refs(root, source),
            imports: extract_imports(root, source),
            markers: shape::markers(root, source),
            has_errors: root.has_error(),
        })
    }

    fn module_names(&self, path: &str) -> Vec<String> {
        let Some(full) = path_module(path) else {
            return Vec::new();
        };
        let aliases = SOURCE_ROOTS
            .iter()
            .filter_map(|root| full.strip_prefix(root));
        let aliases: Vec<String> = aliases.map(str::to_string).collect();
        std::iter::once(full).chain(aliases).collect()
    }

    fn resolve_import(&self, path: &str, import: &Import) -> Option<String> {
        if import.level == 0 {
            return Some(join_dotted(&[
                &import.module,
                import.name.as_deref().unwrap_or(""),
            ]));
        }
        let module = path_module(path)?;
        let mut package: Vec<&str> = module.split('.').filter(|p| !p.is_empty()).collect();
        if !is_package_init(path) {
            package.pop()?;
        }
        for _ in 1..import.level {
            package.pop()?;
        }
        let base = package.join(".");
        Some(join_dotted(&[
            &base,
            &import.module,
            import.name.as_deref().unwrap_or(""),
        ]))
    }

    fn scope_candidates(&self, name: &str, enclosing: Option<&str>) -> Vec<String> {
        let Some(member) = RECEIVERS.iter().find_map(|r| name.strip_prefix(r)) else {
            return nested_scope_candidates(name, enclosing);
        };
        // `self.x` inside `Class.method` refers to `Class.x`.
        let class = enclosing
            .and_then(|e| e.rsplit_once('.'))
            .map(|(class, _)| class);
        class
            .map(|c| vec![format!("{c}.{member}")])
            .unwrap_or_default()
    }

    fn is_test_path(&self, path: &str) -> bool {
        let file = path.rsplit('/').next().unwrap_or(path);
        file.starts_with("test_")
            || file.ends_with("_test.py")
            || file == "conftest.py"
            || path.starts_with("tests/")
            || path.contains("/tests/")
    }

    fn check_call(&self, target: &CallTarget, call: &CallArgs) -> CallCheck {
        shape::check_call(target, call)
    }

    fn constructor_name(&self) -> Option<&'static str> {
        Some("__init__")
    }

    fn is_builtin(&self, name: &str) -> bool {
        shape::is_builtin(name)
    }

    fn is_receiver(&self, name: &str) -> bool {
        matches!(name, "self" | "cls")
    }
}

fn raw_symbol(name: String, kind: SymbolKind, node: Node) -> RawSymbol {
    RawSymbol {
        name,
        kind,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_line: node.start_position().row as u32 + 1,
        end_line: node.end_position().row as u32 + 1,
        signature: None,
    }
}

/// Whether a name node is read (a reference), as opposed to being defined, bound, imported or
/// being an inner part of an attribute chain (only the outermost chain counts).
fn is_read(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    let is_field = |field: &str| {
        let child = parent.child_by_field_name(field);
        child.is_some_and(|c| c.id() == node.id())
    };
    match parent.kind() {
        "function_definition"
        | "class_definition"
        | "keyword_argument"
        | "default_parameter"
        | "typed_default_parameter"
        | "named_expression" => !is_field("name"),
        "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => {
            !is_field("left")
        }
        "as_pattern" => !is_field("alias"),
        "attribute"
        | "parameters"
        | "lambda_parameters"
        | "typed_parameter"
        | "list_splat_pattern"
        | "dictionary_splat_pattern"
        | "import_statement"
        | "import_from_statement"
        | "aliased_import"
        | "dotted_name"
        | "global_statement"
        | "nonlocal_statement"
        | "pattern_list"
        | "tuple_pattern"
        | "list_pattern" => false,
        _ => true,
    }
}

/// Decorated definitions span their decorators too.
fn with_decorators(node: Node) -> Node {
    match node.parent() {
        Some(parent) if parent.kind() == "decorated_definition" => parent,
        _ => node,
    }
}

fn node_text(node: Node, source: &[u8]) -> Option<String> {
    node.utf8_text(source).ok().map(str::to_string)
}

/// Text of an identifier or attribute chain, `None` for anything that is not a plain dotted name
/// (e.g. `foo().bar`).
fn dotted_text(node: Node, source: &[u8]) -> Option<String> {
    let text: String = node.utf8_text(source).ok()?.split_whitespace().collect();
    let valid = text
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    (valid && !text.is_empty()).then_some(text)
}

fn extract_imports(root: Node, source: &[u8]) -> Vec<Import> {
    let mut imports: Vec<Import> = Vec::new();
    let mut stack: Vec<Node> = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" => imports.extend(plain_imports(node, source)),
            "import_from_statement" => imports.extend(from_imports(node, source)),
            _ => {
                let mut cursor = node.walk();
                stack.extend(node.children(&mut cursor));
            }
        }
    }
    imports
}

/// `import a.b` binds `a`; `import a.b as x` binds `x` to `a.b`.
fn plain_imports(node: Node, source: &[u8]) -> Vec<Import> {
    let mut cursor = node.walk();
    let names: Vec<Node> = node.children_by_field_name("name", &mut cursor).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let (module, alias) = name_and_alias(name, source)?;
            let local = alias.unwrap_or_else(|| module.split('.').next().unwrap_or("").to_string());
            let module = if name.kind() == "aliased_import" {
                module
            } else {
                local.clone()
            };
            Some(Import {
                local: Some(local),
                module,
                name: None,
                level: 0,
            })
        })
        .collect()
}

/// `from m import a, b as c` and `from . import *`.
fn from_imports(node: Node, source: &[u8]) -> Vec<Import> {
    let Some(module_node) = node.child_by_field_name("module_name") else {
        return Vec::new();
    };
    let (module, level) = from_module(module_node, source);
    let mut cursor = node.walk();
    let has_wildcard = node
        .children(&mut cursor)
        .any(|c| c.kind() == "wildcard_import");
    if has_wildcard {
        return vec![Import {
            local: None,
            module,
            name: None,
            level,
        }];
    }
    let mut cursor = node.walk();
    let names: Vec<Node> = node.children_by_field_name("name", &mut cursor).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let (member, alias) = name_and_alias(name, source)?;
            let local = alias.unwrap_or_else(|| member.clone());
            Some(Import {
                local: Some(local),
                module: module.clone(),
                name: Some(member),
                level,
            })
        })
        .collect()
}

fn from_module(node: Node, source: &[u8]) -> (String, u32) {
    if node.kind() != "relative_import" {
        return (node_text(node, source).unwrap_or_default(), 0);
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    let level = children
        .iter()
        .find(|c| c.kind() == "import_prefix")
        .and_then(|prefix| node_text(*prefix, source))
        .map_or(0, |dots| dots.matches('.').count() as u32);
    let module = children
        .iter()
        .find(|c| c.kind() == "dotted_name")
        .and_then(|name| node_text(*name, source))
        .unwrap_or_default();
    (module, level)
}

fn name_and_alias(node: Node, source: &[u8]) -> Option<(String, Option<String>)> {
    if node.kind() != "aliased_import" {
        return Some((node_text(node, source)?, None));
    }
    let name = node_text(node.child_by_field_name("name")?, source)?;
    let alias = node
        .child_by_field_name("alias")
        .and_then(|a| node_text(a, source));
    Some((name, alias))
}

/// Dotted module path derived from the file path, e.g. `pkg/mod.py` -> `pkg.mod`.
fn path_module(path: &str) -> Option<String> {
    let stem = path
        .strip_suffix(".py")
        .or_else(|| path.strip_suffix(".pyi"))?;
    let stem = if stem == "__init__" {
        ""
    } else {
        stem.strip_suffix("/__init__").unwrap_or(stem)
    };
    Some(stem.replace('/', "."))
}

fn is_package_init(path: &str) -> bool {
    path == "__init__.py" || path.ends_with("/__init__.py")
}

fn join_dotted(parts: &[&str]) -> String {
    let parts: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    parts.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(local: Option<&str>, module: &str, name: Option<&str>, level: u32) -> Import {
        Import {
            local: local.map(str::to_string),
            module: module.to_string(),
            name: name.map(str::to_string),
            level,
        }
    }

    #[test]
    fn extracts_symbols_with_decorator_ranges() {
        let source =
            b"X = 1\n\n@dec\ndef f():\n    pass\n\nclass K:\n    def m(self):\n        pass\n";
        let facts = Python::new().extract(source).unwrap();
        let summary: Vec<(&str, SymbolKind, u32, u32)> = facts
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind, s.start_line, s.end_line))
            .collect();
        assert!(summary.contains(&("X", SymbolKind::Variable, 1, 1)));
        assert!(summary.contains(&("f", SymbolKind::Function, 3, 5)));
        assert!(summary.contains(&("K", SymbolKind::Class, 7, 9)));
        assert!(summary.contains(&("m", SymbolKind::Function, 8, 9)));
    }

    #[test]
    fn extracts_refs_and_marks_calls() {
        let source = b"def f(x: Model):\n    self.client.fetch(handler)\n    foo().bar()\n";
        let facts = Python::new().extract(source).unwrap();
        let refs: Vec<(&str, bool)> = facts
            .refs
            .iter()
            .map(|r| (r.name.as_str(), r.is_call))
            .collect();
        assert!(refs.contains(&("Model", false)));
        assert!(refs.contains(&("self.client.fetch", true)));
        assert!(refs.contains(&("handler", false)));
        assert!(refs.contains(&("foo", true)));
        assert!(!refs.iter().any(|(name, _)| name.contains('(')));
    }

    #[test]
    fn extracts_imports() {
        let source = b"import a.b\nimport c.d as e\nfrom .m import f, g as h\nfrom ..p import *\n";
        let facts = Python::new().extract(source).unwrap();
        let expected = vec![
            import(Some("a"), "a", None, 0),
            import(Some("e"), "c.d", None, 0),
            import(Some("f"), "m", Some("f"), 1),
            import(Some("h"), "m", Some("g"), 1),
            import(None, "p", None, 2),
        ];
        for item in expected {
            assert!(
                facts.imports.contains(&item),
                "missing {item:?} in {:?}",
                facts.imports
            );
        }
    }

    #[test]
    fn module_names_strip_source_roots() {
        let python = Python::new();
        assert_eq!(
            python.module_names("src/pkg/__init__.py"),
            vec!["src.pkg", "pkg"]
        );
        assert_eq!(python.module_names("pkg/mod.py"), vec!["pkg.mod"]);
        assert!(python.module_names("README.md").is_empty());
    }

    #[test]
    fn resolves_relative_imports() {
        let python = Python::new();
        let sibling = import(Some("f"), "m", Some("f"), 1);
        assert_eq!(
            python.resolve_import("pkg/sub/x.py", &sibling).unwrap(),
            "pkg.sub.m.f"
        );
        let parent = import(Some("f"), "", Some("f"), 2);
        assert_eq!(
            python.resolve_import("pkg/sub/x.py", &parent).unwrap(),
            "pkg.f"
        );
        assert_eq!(
            python.resolve_import("pkg/__init__.py", &sibling).unwrap(),
            "pkg.m.f"
        );
        let too_deep = import(Some("f"), "", Some("f"), 3);
        assert!(python.resolve_import("pkg/x.py", &too_deep).is_none());
    }

    #[test]
    fn self_calls_resolve_to_enclosing_class() {
        let python = Python::new();
        assert_eq!(
            python.scope_candidates("self.save", Some("Repo.update")),
            vec!["Repo.save"]
        );
        assert_eq!(
            python.scope_candidates("helper", Some("f")),
            vec!["f.helper", "helper"]
        );
    }
}
