//! Python callable shapes: signatures, call-site arguments, control-flow markers and argument
//! binding checks.

use tree_sitter::Node;

use crate::lang::{
    CallArgs, CallCheck, CallTarget, Param, ParamKind, RawMarker, Signature, SymbolKind,
};

/// Longest snippet kept for annotations, defaults, decorators and markers.
const MAX_TEXT: usize = 60;

const BUILTINS: &[&str] = &[
    "abs",
    "all",
    "any",
    "bool",
    "bytes",
    "callable",
    "dict",
    "dir",
    "divmod",
    "enumerate",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "hasattr",
    "hash",
    "id",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "list",
    "map",
    "max",
    "min",
    "next",
    "object",
    "open",
    "print",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
    "Exception",
    "ValueError",
    "TypeError",
    "KeyError",
    "IndexError",
    "RuntimeError",
    "AttributeError",
    "NotImplementedError",
    "AssertionError",
    "StopIteration",
];

/// Whether `name` is a Python builtin (ignored in change summaries).
pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// Compact single-line text of a node, truncated for display.
pub fn snippet(node: Node, source: &[u8]) -> String {
    let text = node.utf8_text(source).unwrap_or_default();
    let compact: String = text.split_whitespace().collect::<Vec<&str>>().join(" ");
    if compact.chars().count() <= MAX_TEXT {
        return compact;
    }
    let cut: String = compact.chars().take(MAX_TEXT - 1).collect();
    format!("{cut}…")
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// Signature of a `function_definition` or `class_definition` node. `outer` is the node
/// including decorators (a `decorated_definition`), when present.
pub fn signature(definition: Node, outer: Node, source: &[u8]) -> Option<Signature> {
    let decorators: Vec<String> = if outer.kind() == "decorated_definition" {
        let decorators = named_children(outer)
            .into_iter()
            .filter(|c| c.kind() == "decorator");
        let texts = decorators.map(|d| snippet(d, source));
        texts
            .map(|t| t.trim_start_matches('@').to_string())
            .collect()
    } else {
        Vec::new()
    };
    match definition.kind() {
        "function_definition" => Some(function_signature(definition, decorators, source)),
        "class_definition" => Some(Signature {
            decorators,
            bases: definition
                .child_by_field_name("superclasses")
                .map(|args| {
                    named_children(args)
                        .into_iter()
                        .map(|b| snippet(b, source))
                        .collect()
                })
                .unwrap_or_default(),
            ..Signature::default()
        }),
        _ => None,
    }
}

fn function_signature(definition: Node, decorators: Vec<String>, source: &[u8]) -> Signature {
    let is_async = definition.child(0).is_some_and(|c| c.kind() == "async");
    let returns = definition
        .child_by_field_name("return_type")
        .map(|r| snippet(r, source));
    let params = definition
        .child_by_field_name("parameters")
        .map(|p| parameters(p, source))
        .unwrap_or_default();
    Signature {
        params,
        returns,
        decorators,
        is_async,
        bases: Vec::new(),
    }
}

fn parameters(node: Node, source: &[u8]) -> Vec<Param> {
    let mut params: Vec<Param> = Vec::new();
    let mut keyword_only = false;
    for child in named_children(node) {
        match child.kind() {
            "positional_separator" => {
                for param in params
                    .iter_mut()
                    .filter(|p| p.kind == ParamKind::Positional)
                {
                    param.kind = ParamKind::PositionalOnly;
                }
            }
            "keyword_separator" => keyword_only = true,
            "comment" => {}
            _ => {
                let Some(param) = parameter(child, source, keyword_only) else {
                    continue;
                };
                keyword_only |= param.kind == ParamKind::VarPositional;
                params.push(param);
            }
        }
    }
    params
}

fn parameter(node: Node, source: &[u8], keyword_only: bool) -> Option<Param> {
    let plain = if keyword_only {
        ParamKind::KeywordOnly
    } else {
        ParamKind::Positional
    };
    let text = |n: Node| snippet(n, source);
    let field = |name: &str| node.child_by_field_name(name);
    let (name, kind) = match node.kind() {
        "identifier" => (text(node), plain),
        "list_splat_pattern" => (splat_name(node, source)?, ParamKind::VarPositional),
        "dictionary_splat_pattern" => (splat_name(node, source)?, ParamKind::VarKeyword),
        "default_parameter" | "typed_default_parameter" => (text(field("name")?), plain),
        "typed_parameter" => {
            let inner = *named_children(node).first()?;
            match inner.kind() {
                "list_splat_pattern" => (splat_name(inner, source)?, ParamKind::VarPositional),
                "dictionary_splat_pattern" => (splat_name(inner, source)?, ParamKind::VarKeyword),
                _ => (text(inner), plain),
            }
        }
        _ => return None,
    };
    Some(Param {
        name,
        kind,
        annotation: field("type").map(text),
        default: field("value").map(text),
    })
}

fn splat_name(node: Node, source: &[u8]) -> Option<String> {
    named_children(node).first().map(|n| snippet(*n, source))
}

/// Arguments of the `call` node whose `function` is `callee`.
pub fn call_args(callee: Node, source: &[u8]) -> Option<CallArgs> {
    let call = callee.parent().filter(|p| p.kind() == "call")?;
    let arguments = call.child_by_field_name("arguments")?;
    if arguments.kind() == "generator_expression" {
        return Some(CallArgs {
            positional: 1,
            ..CallArgs::default()
        });
    }
    let mut args = CallArgs::default();
    for child in named_children(arguments) {
        match child.kind() {
            "keyword_argument" => {
                let name = child
                    .child_by_field_name("name")
                    .map(|n| snippet(n, source));
                args.keywords.extend(name);
            }
            "list_splat" => args.star = true,
            "dictionary_splat" => args.double_star = true,
            "comment" => {}
            _ => args.positional += 1,
        }
    }
    Some(args)
}

/// `raise X`, `except X` and `yield` markers in the tree.
pub fn markers(root: Node, source: &[u8]) -> Vec<RawMarker> {
    let mut markers: Vec<RawMarker> = Vec::new();
    let mut stack: Vec<Node> = vec![root];
    while let Some(node) = stack.pop() {
        let text = match node.kind() {
            "raise_statement" => Some(raised(node, source)),
            "except_clause" => Some(caught(node, source)),
            "yield" => Some("yield".to_string()),
            _ => None,
        };
        if let Some(text) = text {
            markers.push(RawMarker {
                text,
                byte: node.start_byte(),
            });
        }
        stack.extend(named_children(node));
    }
    markers.sort_by_key(|m| m.byte);
    markers
}

fn raised(node: Node, source: &[u8]) -> String {
    let Some(value) = named_children(node).into_iter().next() else {
        return "raise".to_string();
    };
    let exception = match value.kind() {
        "call" => value.child_by_field_name("function").unwrap_or(value),
        _ => value,
    };
    format!("raise {}", snippet(exception, source))
}

fn caught(node: Node, source: &[u8]) -> String {
    let value = node.child_by_field_name("value");
    value.map_or("except".to_string(), |v| {
        format!("except {}", snippet(v, source))
    })
}

/// Bind a call's arguments to the target's parameters following Python's rules.
pub fn check_call(target: &CallTarget, call: &CallArgs) -> CallCheck {
    if let Some(decorator) = signature_changing_decorator(target.signature) {
        return CallCheck::Unknown {
            reason: format!("@{decorator} may change the signature"),
        };
    }
    if call.star || call.double_star {
        return CallCheck::Unknown {
            reason: "call forwards *args/**kwargs".to_string(),
        };
    }
    let params: &[Param] = if receives_implicit_first(target) {
        target.signature.params.get(1..).unwrap_or_default()
    } else {
        &target.signature.params
    };
    match bind(params, call) {
        Ok(()) => CallCheck::Ok,
        Err(reason) => CallCheck::Broken { reason },
    }
}

/// Decorators known to keep the decorated function's parameters.
const SIGNATURE_PRESERVING: &[&str] = &[
    "staticmethod",
    "classmethod",
    "property",
    "setter",
    "getter",
    "deleter",
    "cached_property",
    "cache",
    "lru_cache",
    "abstractmethod",
    "override",
    "overload",
    "final",
    "contextmanager",
    "asynccontextmanager",
];

/// First decorator that is not known to preserve the signature.
fn signature_changing_decorator(signature: &Signature) -> Option<&str> {
    signature
        .decorators
        .iter()
        .map(String::as_str)
        .find(|decorator| {
            let name = decorator.split('(').next().unwrap_or(decorator);
            let last = name.rsplit('.').next().unwrap_or(name);
            !SIGNATURE_PRESERVING.contains(&last)
        })
}

/// Methods called through an instance (or classmethods) get `self`/`cls` implicitly.
fn receives_implicit_first(target: &CallTarget) -> bool {
    if target.kind != SymbolKind::Method {
        return false;
    }
    let decorated = |name: &str| target.signature.decorators.iter().any(|d| d == name);
    if decorated("staticmethod") {
        return false;
    }
    if decorated("classmethod") {
        return true;
    }
    // `Class.method(obj, ...)` passes the instance explicitly.
    let class = target.qualname.rsplit('.').nth(1);
    let receiver = target
        .callee
        .rsplit_once('.')
        .map(|(r, _)| r.rsplit('.').next().unwrap_or(r));
    !(class.is_some() && receiver == class)
}

fn bind(params: &[Param], call: &CallArgs) -> Result<(), String> {
    let positional: Vec<&Param> = params
        .iter()
        .filter(|p| matches!(p.kind, ParamKind::PositionalOnly | ParamKind::Positional))
        .collect();
    let has = |kind: ParamKind| params.iter().any(|p| p.kind == kind);
    let passed = call.positional as usize;
    if passed > positional.len() && !has(ParamKind::VarPositional) {
        return Err(format!(
            "takes {} positional argument(s), call passes {passed}",
            positional.len()
        ));
    }
    let mut bound: Vec<&str> = positional
        .iter()
        .take(passed)
        .map(|p| p.name.as_str())
        .collect();
    for keyword in &call.keywords {
        let param = params.iter().find(|p| &p.name == keyword);
        match param.map(|p| p.kind) {
            Some(ParamKind::Positional | ParamKind::KeywordOnly) => {
                if bound.contains(&keyword.as_str()) {
                    return Err(format!("multiple values for '{keyword}'"));
                }
                bound.push(keyword);
            }
            Some(ParamKind::PositionalOnly) if !has(ParamKind::VarKeyword) => {
                return Err(format!("'{keyword}' is positional-only"));
            }
            _ if has(ParamKind::VarKeyword) => {}
            _ => return Err(format!("unexpected keyword argument '{keyword}'")),
        }
    }
    let required = params.iter().filter(|p| {
        p.default.is_none() && !matches!(p.kind, ParamKind::VarPositional | ParamKind::VarKeyword)
    });
    for param in required {
        if !bound.contains(&param.name.as_str()) {
            return Err(format!("missing required argument '{}'", param.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::Language;
    use crate::lang::python::Python;

    fn facts(source: &str) -> crate::lang::RawFacts {
        Python::new().extract(source.as_bytes()).unwrap()
    }

    fn signature_of(source: &str, name: &str) -> Signature {
        let facts = facts(source);
        let symbol = facts.symbols.into_iter().find(|s| s.name == name).unwrap();
        symbol.signature.unwrap()
    }

    fn param(
        name: &str,
        kind: ParamKind,
        annotation: Option<&str>,
        default: Option<&str>,
    ) -> Param {
        Param {
            name: name.to_string(),
            kind,
            annotation: annotation.map(str::to_string),
            default: default.map(str::to_string),
        }
    }

    #[test]
    fn extracts_function_signatures() {
        let source = "@cache\nasync def f(a, /, b: int, c=1, *args, d: str = 'x', **kw) -> bool:\n    pass\n";
        let signature = signature_of(source, "f");
        assert_eq!(
            signature.params,
            vec![
                param("a", ParamKind::PositionalOnly, None, None),
                param("b", ParamKind::Positional, Some("int"), None),
                param("c", ParamKind::Positional, None, Some("1")),
                param("args", ParamKind::VarPositional, None, None),
                param("d", ParamKind::KeywordOnly, Some("str"), Some("'x'")),
                param("kw", ParamKind::VarKeyword, None, None),
            ]
        );
        assert_eq!(signature.returns.as_deref(), Some("bool"));
        assert_eq!(signature.decorators, vec!["cache"]);
        assert!(signature.is_async);
    }

    #[test]
    fn keyword_separator_makes_following_params_keyword_only() {
        let signature = signature_of("def f(a, *, b):\n    pass\n", "f");
        assert_eq!(signature.params[1].kind, ParamKind::KeywordOnly);
    }

    #[test]
    fn extracts_class_bases() {
        let signature = signature_of("class K(Base, metaclass=Meta):\n    pass\n", "K");
        assert_eq!(signature.bases, vec!["Base", "metaclass=Meta"]);
    }

    #[test]
    fn extracts_call_args_and_markers() {
        let source = "def f():\n    g(1, x, key=2, *rest)\n    try:\n        pass\n    except KeyError:\n        raise ValueError('no')\n";
        let facts = facts(source);
        let call = facts
            .refs
            .iter()
            .find(|r| r.name == "g")
            .unwrap()
            .call
            .clone()
            .unwrap();
        assert_eq!(call.positional, 2);
        assert_eq!(call.keywords, vec!["key"]);
        assert!(call.star);
        let markers: Vec<&str> = facts.markers.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(markers, vec!["except KeyError", "raise ValueError"]);
    }

    fn check(params: Vec<Param>, kind: SymbolKind, callee: &str, call: CallArgs) -> CallCheck {
        let signature = Signature {
            params,
            ..Signature::default()
        };
        let target = CallTarget {
            signature: &signature,
            kind,
            qualname: "Repo.save",
            callee,
        };
        check_call(&target, &call)
    }

    fn args(positional: u32, keywords: &[&str]) -> CallArgs {
        CallArgs {
            positional,
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            ..CallArgs::default()
        }
    }

    #[test]
    fn binds_arguments_like_python() {
        let params = || {
            vec![
                param("self", ParamKind::Positional, None, None),
                param("item", ParamKind::Positional, None, None),
                param("force", ParamKind::KeywordOnly, None, Some("False")),
            ]
        };
        let method = SymbolKind::Method;
        assert_eq!(
            check(params(), method, "repo.save", args(1, &[])),
            CallCheck::Ok
        );
        assert_eq!(
            check(params(), method, "repo.save", args(1, &["force"])),
            CallCheck::Ok
        );
        assert_eq!(
            check(params(), method, "Repo.save", args(2, &[])),
            CallCheck::Ok
        );
        let missing = check(params(), method, "repo.save", args(0, &[]));
        assert_eq!(
            missing,
            CallCheck::Broken {
                reason: "missing required argument 'item'".into()
            }
        );
        let extra = check(params(), method, "repo.save", args(2, &[]));
        assert!(matches!(extra, CallCheck::Broken { .. }));
        let unknown = check(params(), method, "repo.save", args(1, &["nope"]));
        assert_eq!(
            unknown,
            CallCheck::Broken {
                reason: "unexpected keyword argument 'nope'".into()
            }
        );
        let twice = check(params(), SymbolKind::Function, "save", args(2, &["item"]));
        assert_eq!(
            twice,
            CallCheck::Broken {
                reason: "multiple values for 'item'".into()
            }
        );
    }

    #[test]
    fn unknown_decorators_make_calls_unknown() {
        let signature = Signature {
            params: vec![param("a", ParamKind::Positional, None, None)],
            decorators: vec!["html(\"page.html\")".into()],
            ..Signature::default()
        };
        let target = CallTarget {
            signature: &signature,
            kind: SymbolKind::Function,
            qualname: "f",
            callee: "f",
        };
        let result = check_call(&target, &args(0, &["html_request"]));
        assert!(matches!(result, CallCheck::Unknown { .. }));
        let cached = Signature {
            decorators: vec!["functools.lru_cache(maxsize=1)".into()],
            ..signature.clone()
        };
        let target = CallTarget {
            signature: &cached,
            ..target
        };
        assert!(matches!(
            check_call(&target, &args(0, &[])),
            CallCheck::Broken { .. }
        ));
    }

    #[test]
    fn splat_calls_are_unknown() {
        let call = CallArgs {
            star: true,
            ..CallArgs::default()
        };
        let result = check(Vec::new(), SymbolKind::Function, "f", call);
        assert!(matches!(result, CallCheck::Unknown { .. }));
    }
}
