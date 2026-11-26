//! Language support abstraction.
//!
//! The analysis core only talks to [`Language`]. Adding a language means implementing the trait
//! (usually tree-sitter queries plus import resolution) and registering it in [`Registry::new`].

pub mod python;

use serde::{Deserialize, Serialize};

/// Kind of a definition. `Method` and `Module` are assigned by the core, not by extractors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Variable,
    Module,
    /// A whole file the analysis has no language support for (configs, docs, data).
    File,
}

/// How an argument can be bound to a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    /// Positional only (before `/` in Python).
    PositionalOnly,
    /// Positional or keyword.
    Positional,
    /// Keyword only (after `*` or `*args` in Python).
    KeywordOnly,
    /// Collects extra positional arguments (`*args`).
    VarPositional,
    /// Collects extra keyword arguments (`**kwargs`).
    VarKeyword,
}

/// One declared parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub kind: ParamKind,
    pub annotation: Option<String>,
    pub default: Option<String>,
}

/// Shape of a callable or class, used to describe and check signature changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub params: Vec<Param>,
    pub returns: Option<String>,
    pub decorators: Vec<String>,
    pub is_async: bool,
    /// Base classes (classes only).
    pub bases: Vec<String>,
}

/// Arguments passed at a call site.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallArgs {
    pub positional: u32,
    pub keywords: Vec<String>,
    /// `*iterable` forwarded.
    pub star: bool,
    /// `**mapping` forwarded.
    pub double_star: bool,
}

/// A notable control-flow construct, e.g. `raise ValueError`.
#[derive(Debug, Clone)]
pub struct RawMarker {
    pub text: String,
    pub byte: usize,
}

/// The callee of a checked call.
pub struct CallTarget<'a> {
    pub signature: &'a Signature,
    pub kind: SymbolKind,
    pub qualname: &'a str,
    /// The callee as written at the call site, e.g. `self.repo.save` or `Repo`.
    pub callee: &'a str,
}

/// Whether a call still matches the callee's (new) signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum CallCheck {
    Ok,
    Broken { reason: String },
    Unknown { reason: String },
}

impl CallCheck {
    /// Higher is more important to a reviewer.
    pub fn severity(&self) -> u8 {
        match self {
            CallCheck::Ok => 0,
            CallCheck::Unknown { .. } => 1,
            CallCheck::Broken { .. } => 2,
        }
    }
}

/// A definition as reported by a language extractor, before nesting is resolved.
#[derive(Debug, Clone)]
pub struct RawSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-based, inclusive.
    pub start_line: u32,
    /// 1-based, inclusive.
    pub end_line: u32,
    /// Callable or class shape, when the language can describe it.
    pub signature: Option<Signature>,
}

/// A usage of a (possibly dotted) name, e.g. `fetch` or `self.client.fetch`.
#[derive(Debug, Clone)]
pub struct RawRef {
    pub name: String,
    pub byte: usize,
    /// 1-based.
    pub line: u32,
    pub is_call: bool,
    /// Arguments of the call, for calls.
    pub call: Option<CallArgs>,
}

/// An import binding. Stored path-independent so facts can be cached per blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// Name bound in the importing file. `None` for wildcard imports.
    pub local: Option<String>,
    /// Module part as written (without leading dots for relative imports).
    pub module: String,
    /// Imported member for `from x import name` forms.
    pub name: Option<String>,
    /// Relative import level (number of leading dots); 0 for absolute imports.
    pub level: u32,
}

/// Everything an extractor reports about one source file.
#[derive(Debug, Clone, Default)]
pub struct RawFacts {
    pub symbols: Vec<RawSymbol>,
    pub refs: Vec<RawRef>,
    pub imports: Vec<Import>,
    /// Notable control flow (e.g. `raise ValueError`), attributed to the enclosing symbol.
    pub markers: Vec<RawMarker>,
    pub has_errors: bool,
}

/// Language-specific knowledge needed by the analysis core.
pub trait Language: Send + Sync {
    /// Stable identifier, used as cache key and in reports.
    fn id(&self) -> &'static str;

    /// Bump whenever extraction output changes, to invalidate cached facts.
    fn version(&self) -> u32;

    /// Whether this language handles the given repo-relative path.
    fn matches(&self, path: &str) -> bool;

    /// Extract definitions, references and imports. Must not depend on the file path.
    fn extract(&self, source: &[u8]) -> anyhow::Result<RawFacts>;

    /// Fully qualified module names for a path. The first entry is canonical; the rest are
    /// aliases (e.g. with a `src/` root stripped).
    fn module_names(&self, path: &str) -> Vec<String>;

    /// Absolute dotted target an import binds to, as seen from `path`.
    fn resolve_import(&self, path: &str, import: &Import) -> Option<String>;

    /// Qualnames in the same file that `name` may refer to from inside `enclosing`,
    /// most specific first.
    fn scope_candidates(&self, name: &str, enclosing: Option<&str>) -> Vec<String> {
        nested_scope_candidates(name, enclosing)
    }

    /// Whether the path holds tests. Tests are ordered last within a cluster.
    fn is_test_path(&self, _path: &str) -> bool {
        false
    }

    /// Check a call against a callee signature. Default: no binding rules known.
    fn check_call(&self, _target: &CallTarget, _call: &CallArgs) -> CallCheck {
        CallCheck::Unknown {
            reason: "call checking not supported for this language".to_string(),
        }
    }

    /// Name of the method that calling a class runs (e.g. `__init__`), if any.
    fn constructor_name(&self) -> Option<&'static str> {
        None
    }

    /// Whether a referenced name is a builtin, omitted from change summaries.
    fn is_builtin(&self, _name: &str) -> bool {
        false
    }

    /// Whether `name` is an implicit receiver parameter (`self`, `this`), whose attributes are
    /// meaningful references rather than local values.
    fn is_receiver(&self, _name: &str) -> bool {
        false
    }
}

/// Default lexical scoping: try `outer.inner.name`, `outer.name`, then `name`.
pub fn nested_scope_candidates(name: &str, enclosing: Option<&str>) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let mut scope: Option<&str> = enclosing;
    while let Some(current) = scope {
        candidates.push(format!("{current}.{name}"));
        scope = current.rsplit_once('.').map(|(parent, _)| parent);
    }
    candidates.push(name.to_string());
    candidates
}

/// Set of supported languages.
pub struct Registry {
    languages: Vec<Box<dyn Language>>,
}

impl Registry {
    /// Registry with every built-in language.
    pub fn new() -> Self {
        Self {
            languages: vec![Box::new(python::Python::new())],
        }
    }

    /// Language handling `path`, if any.
    pub fn for_path(&self, path: &str) -> Option<&dyn Language> {
        self.languages
            .iter()
            .find(|lang| lang.matches(path))
            .map(|lang| lang.as_ref())
    }

    /// Language with the given id.
    pub fn by_id(&self, id: &str) -> Option<&dyn Language> {
        self.languages
            .iter()
            .find(|lang| lang.id() == id)
            .map(|lang| lang.as_ref())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
