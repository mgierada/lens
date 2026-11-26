//! What changed inside each symbol (signature, references, control flow) and whether calls still
//! match changed signatures.

use std::collections::{HashMap, HashSet};

use crate::facts::{Facts, Symbol};
use crate::lang::{CallCheck, CallTarget, Language, Param, ParamKind, Signature, SymbolKind};
use crate::model::{CallSite, Change, ChangeSummary, Confidence, SignatureChange};

use super::ChangedFile;
use super::changes::{ChangeSet, Loc, SymbolChange};
use super::resolve::Links;

/// One side of a changed symbol: its file facts and, for definitions, the symbol index.
#[derive(Clone, Copy)]
struct Side<'a> {
    facts: &'a Facts,
    lang: &'a dyn Language,
    /// `None` for module-level code.
    sym: Option<u32>,
}

impl<'a> Side<'a> {
    fn symbol(&self) -> Option<&'a Symbol> {
        self.sym.map(|s| &self.facts.symbols[s as usize])
    }

    /// The symbol's own references (excluding nested definitions), split into calls and uses.
    /// Builtins and names reached through the symbol's own parameters (local values) are left
    /// out; receiver attributes such as `self.repo` are kept. A name that is called is only
    /// listed under calls.
    fn references(&self) -> (Vec<String>, Vec<String>) {
        // Names that mean something outside the function body: imports and definitions of this
        // file. Anything else (local variables, loop targets) is noise in a summary.
        let known: HashSet<&str> = self
            .facts
            .imports
            .iter()
            .filter_map(|i| i.local.as_deref())
            .chain(self.facts.symbols.iter().map(|s| s.name.as_str()))
            .collect();
        let meaningful = |r: &&crate::facts::Ref| {
            let head = r.name.split('.').next().unwrap_or(&r.name);
            if self.lang.is_receiver(head) {
                return r.name.contains('.');
            }
            let local = self.facts.is_parameter(self.sym, head);
            !local && known.contains(head) && !self.lang.is_builtin(&r.name)
        };
        let own = self.facts.refs.iter().filter(|r| r.enclosing == self.sym);
        let (calls, uses): (Vec<_>, Vec<_>) = own.filter(meaningful).partition(|r| r.is_call);
        let calls = unique(calls.into_iter().map(|r| r.name.clone()));
        let called: HashSet<&String> = calls.iter().collect();
        let uses = uses
            .into_iter()
            .map(|r| r.name.clone())
            .filter(|n| !called.contains(n));
        let uses = unique(uses);
        (calls, uses)
    }

    fn markers(&self) -> Vec<String> {
        self.symbol().map(|s| s.markers.clone()).unwrap_or_default()
    }

    /// Signature used for calls: a class is called through its constructor.
    fn callable(&self) -> Option<(&'a Signature, SymbolKind, &'a str)> {
        let symbol = self.symbol()?;
        if symbol.kind != SymbolKind::Class {
            return Some((symbol.signature.as_ref()?, symbol.kind, &symbol.qualname));
        }
        let name = self.lang.constructor_name()?;
        let parent = self.sym;
        let constructor = self
            .facts
            .symbols
            .iter()
            .find(|s| s.parent == parent && s.name == name)?;
        Some((
            constructor.signature.as_ref()?,
            constructor.kind,
            &constructor.qualname,
        ))
    }
}

fn side<'a>(files: &'a [ChangedFile], loc: Option<Loc>, head: bool) -> Option<Side<'a>> {
    let loc = loc?;
    let file = &files[loc.file];
    let facts = if head {
        file.head.as_deref()?
    } else {
        file.base.as_deref()?
    };
    Some(Side {
        facts,
        lang: file.lang?,
        sym: Some(loc.sym),
    })
}

fn module_side<'a>(files: &'a [ChangedFile], file: usize, head: bool) -> Option<Side<'a>> {
    let changed = &files[file];
    let facts = if head {
        changed.head.as_deref()?
    } else {
        changed.base.as_deref()?
    };
    Some(Side {
        facts,
        lang: changed.lang?,
        sym: None,
    })
}

fn sides<'a>(
    files: &'a [ChangedFile],
    change: &SymbolChange,
) -> (Option<Side<'a>>, Option<Side<'a>>) {
    match change.module_of {
        Some(file) => (
            module_side(files, file, false),
            module_side(files, file, true),
        ),
        None => (
            side(files, change.base, false),
            side(files, change.head, true),
        ),
    }
}

/// Head signature text and change summary for one change.
pub fn summarize(
    files: &[ChangedFile],
    change: &SymbolChange,
) -> (Option<String>, Option<ChangeSummary>) {
    let (base, head) = sides(files, change);
    let signature = head
        .and_then(|h| h.symbol())
        .and_then(|s| s.signature.as_ref())
        .map(render_signature);
    let summary = match (change.change, base, head) {
        (Change::Deleted, _, _) => None,
        (_, Some(base), Some(head)) => Some(diff(base, head)),
        (_, None, Some(head)) => Some(added(head)),
        _ => None,
    };
    (signature, summary.filter(|s| !s.is_empty()))
}

fn added(head: Side) -> ChangeSummary {
    let (calls, uses) = head.references();
    ChangeSummary {
        calls_added: calls,
        uses_added: uses,
        flow_added: head.markers(),
        ..ChangeSummary::default()
    }
}

fn diff(base: Side, head: Side) -> ChangeSummary {
    let (base_calls, base_uses) = base.references();
    let (head_calls, head_uses) = head.references();
    let (flow_added, flow_removed) = multiset_diff(&base.markers(), &head.markers());
    let signature = match (
        base.symbol().and_then(|s| s.signature.as_ref()),
        head.symbol(),
    ) {
        (Some(before), Some(symbol)) => symbol
            .signature
            .as_ref()
            .map(|after| signature_changes(before, after))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    ChangeSummary {
        signature,
        calls_added: missing_from(&head_calls, &base_calls),
        calls_removed: missing_from(&base_calls, &head_calls),
        uses_added: missing_from(&head_uses, &base_uses),
        uses_removed: missing_from(&base_uses, &head_uses),
        flow_added,
        flow_removed,
    }
}

/// Describe how a signature changed.
pub fn signature_changes(before: &Signature, after: &Signature) -> Vec<SignatureChange> {
    let mut changes: Vec<SignatureChange> = Vec::new();
    let mut push = |kind: &'static str, text: String| changes.push(SignatureChange { kind, text });
    let old: HashMap<&str, &Param> = before.params.iter().map(|p| (p.name.as_str(), p)).collect();
    let new: HashMap<&str, &Param> = after.params.iter().map(|p| (p.name.as_str(), p)).collect();
    for param in &after.params {
        match old.get(param.name.as_str()) {
            None => push("param_added", format!("+{}", describe_param(param))),
            Some(previous) if *previous != param => push(
                "param_changed",
                format!("{} → {}", describe_param(previous), describe_param(param)),
            ),
            _ => {}
        }
    }
    for param in before
        .params
        .iter()
        .filter(|p| !new.contains_key(p.name.as_str()))
    {
        push("param_removed", format!("-{}", describe_param(param)));
    }
    let common = |params: &[Param], other: &HashMap<&str, &Param>| -> Vec<String> {
        let shared = params
            .iter()
            .filter(|p| other.contains_key(p.name.as_str()));
        shared.map(|p| p.name.clone()).collect()
    };
    let (old_order, new_order) = (common(&before.params, &new), common(&after.params, &old));
    if old_order != new_order {
        let text = format!(
            "order ({}) → ({})",
            old_order.join(", "),
            new_order.join(", ")
        );
        push("params_reordered", text);
    }
    if before.returns != after.returns {
        let show = |r: &Option<String>| r.clone().unwrap_or_else(|| "∅".to_string());
        push(
            "returns",
            format!(
                "returns {} → {}",
                show(&before.returns),
                show(&after.returns)
            ),
        );
    }
    let (added, removed) = set_diff(&before.decorators, &after.decorators);
    added
        .into_iter()
        .for_each(|d| push("decorator_added", format!("+@{d}")));
    removed
        .into_iter()
        .for_each(|d| push("decorator_removed", format!("-@{d}")));
    if before.is_async != after.is_async {
        let text = if after.is_async {
            "now async"
        } else {
            "no longer async"
        };
        push("async", text.to_string());
    }
    let (added, removed) = set_diff(&before.bases, &after.bases);
    added
        .into_iter()
        .for_each(|b| push("base_added", format!("+base {b}")));
    removed
        .into_iter()
        .for_each(|b| push("base_removed", format!("-base {b}")));
    changes
}

/// Render a signature, e.g. `(a, /, b: int = 1, *, c) -> str`.
pub fn render_signature(signature: &Signature) -> String {
    if signature.params.is_empty() && !signature.bases.is_empty() {
        return format!("({})", signature.bases.join(", "));
    }
    let mut parts: Vec<String> = Vec::new();
    let mut previous: Option<ParamKind> = None;
    let has_var_positional = signature
        .params
        .iter()
        .any(|p| p.kind == ParamKind::VarPositional);
    for param in &signature.params {
        if previous == Some(ParamKind::PositionalOnly) && param.kind != ParamKind::PositionalOnly {
            parts.push("/".to_string());
        }
        let first_keyword_only = param.kind == ParamKind::KeywordOnly
            && previous != Some(ParamKind::KeywordOnly)
            && !has_var_positional;
        if first_keyword_only {
            parts.push("*".to_string());
        }
        parts.push(render_param(param));
        previous = Some(param.kind);
    }
    if previous == Some(ParamKind::PositionalOnly) {
        parts.push("/".to_string());
    }
    let prefix = if signature.is_async { "async " } else { "" };
    let returns = signature
        .returns
        .as_ref()
        .map(|r| format!(" -> {r}"))
        .unwrap_or_default();
    format!("{prefix}({}){returns}", parts.join(", "))
}

fn render_param(param: &Param) -> String {
    let star = match param.kind {
        ParamKind::VarPositional => "*",
        ParamKind::VarKeyword => "**",
        _ => "",
    };
    let mut text = format!("{star}{}", param.name);
    if let Some(annotation) = &param.annotation {
        text.push_str(&format!(": {annotation}"));
    }
    if let Some(default) = &param.default {
        let eq = if param.annotation.is_some() {
            " = "
        } else {
            "="
        };
        text.push_str(&format!("{eq}{default}"));
    }
    text
}

/// Parameter text plus its binding kind when that is not the plain positional-or-keyword kind.
fn describe_param(param: &Param) -> String {
    let kind = match param.kind {
        ParamKind::PositionalOnly => " (positional-only)",
        ParamKind::KeywordOnly => " (keyword-only)",
        _ => "",
    };
    format!("{}{kind}", render_param(param))
}

/// Check every call to a symbol whose callable signature changed, on edges and impact.
/// Returns the number of broken calls per change id.
pub fn check_calls(files: &[ChangedFile], set: &ChangeSet, links: &mut Links) -> Vec<usize> {
    let mut broken: Vec<usize> = vec![0; set.changes.len()];
    for (id, change) in set.changes.iter().enumerate() {
        if !matches!(change.change, Change::Modified | Change::Moved) {
            continue;
        }
        let (Some(base), Some(head)) = (
            side(files, change.base, false),
            side(files, change.head, true),
        ) else {
            continue;
        };
        let (Some(before), Some(after)) = (base.callable(), head.callable()) else {
            continue;
        };
        if before.0 == after.0 {
            continue;
        }
        let check = |calls: &[CallSite]| worst_check(head.lang, after, calls);
        for edge in links.edges.iter_mut().filter(|e| e.to == id) {
            if set.changes[edge.from].change == Change::Deleted {
                continue;
            }
            edge.call_check = check(&edge.calls).map(|c| trusted(c, edge.confidence));
            broken[id] += usize::from(matches!(edge.call_check, Some(CallCheck::Broken { .. })));
        }
        for impact in links.impact[id].iter_mut() {
            impact.call_check = check(&impact.calls).map(|c| trusted(c, impact.confidence));
            broken[id] += usize::from(matches!(impact.call_check, Some(CallCheck::Broken { .. })));
        }
        links.impact[id].sort_by(|a, b| {
            let severity =
                |i: &crate::model::Impact| i.call_check.as_ref().map_or(0, |c| c.severity());
            severity(b)
                .cmp(&severity(a))
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
    }
    broken
}

/// A name-only match may be a different function: never report it as broken.
fn trusted(check: CallCheck, confidence: Confidence) -> CallCheck {
    match (check, confidence) {
        (CallCheck::Broken { reason }, Confidence::Low) => CallCheck::Unknown {
            reason: format!("matched by name only; {reason}"),
        },
        (check, _) => check,
    }
}

/// Worst outcome over a referrer's call sites; `None` when it never calls the target.
fn worst_check(
    lang: &dyn Language,
    callable: (&Signature, SymbolKind, &str),
    calls: &[CallSite],
) -> Option<CallCheck> {
    let (signature, kind, qualname) = callable;
    calls
        .iter()
        .map(|site| {
            let target = CallTarget {
                signature,
                kind,
                qualname,
                callee: &site.callee,
            };
            match lang.check_call(&target, &site.args) {
                CallCheck::Broken { reason } => CallCheck::Broken {
                    reason: format!("line {}: {reason}", site.line),
                },
                other => other,
            }
        })
        .max_by_key(CallCheck::severity)
}

fn unique(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    items.filter(|item| seen.insert(item.clone())).collect()
}

fn missing_from(items: &[String], other: &[String]) -> Vec<String> {
    let other: HashSet<&String> = other.iter().collect();
    items
        .iter()
        .filter(|i| !other.contains(i))
        .cloned()
        .collect()
}

fn set_diff(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
    (missing_from(after, before), missing_from(before, after))
}

/// Occurrence-aware difference: `raise X` twice before and once after is one removal.
fn multiset_diff(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>) {
    let count = |items: &[String]| {
        let mut counts: HashMap<String, i64> = HashMap::new();
        for item in items {
            *counts.entry(item.clone()).or_default() += 1;
        }
        counts
    };
    let (old, new) = (count(before), count(after));
    let delta = |items: &[String], mine: &HashMap<String, i64>, theirs: &HashMap<String, i64>| {
        let mut out: Vec<String> = Vec::new();
        for item in unique(items.iter().cloned()) {
            let extra = mine[&item] - theirs.get(&item).copied().unwrap_or(0);
            out.extend(std::iter::repeat_n(item, extra.max(0) as usize));
        }
        out
    };
    (delta(after, &new, &old), delta(before, &old, &new))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(name: &str, kind: ParamKind, default: Option<&str>) -> Param {
        Param {
            name: name.into(),
            kind,
            annotation: None,
            default: default.map(Into::into),
        }
    }

    fn texts(changes: &[SignatureChange]) -> Vec<String> {
        changes.iter().map(|c| c.text.clone()).collect()
    }

    #[test]
    fn describes_parameter_changes() {
        let before = Signature {
            params: vec![
                param("url", ParamKind::Positional, None),
                param("timeout", ParamKind::Positional, Some("10")),
                param("verbose", ParamKind::Positional, None),
            ],
            returns: Some("str".into()),
            ..Signature::default()
        };
        let after = Signature {
            params: vec![
                param("url", ParamKind::Positional, None),
                param("timeout", ParamKind::KeywordOnly, Some("30")),
                param("retries", ParamKind::KeywordOnly, Some("3")),
            ],
            returns: Some("bytes".into()),
            decorators: vec!["cache".into()],
            ..Signature::default()
        };
        assert_eq!(
            texts(&signature_changes(&before, &after)),
            vec![
                "timeout=10 → timeout=30 (keyword-only)",
                "+retries=3 (keyword-only)",
                "-verbose",
                "returns str → bytes",
                "+@cache",
            ]
        );
    }

    #[test]
    fn detects_reordering() {
        let a = param("a", ParamKind::Positional, None);
        let b = param("b", ParamKind::Positional, None);
        let before = Signature {
            params: vec![a.clone(), b.clone()],
            ..Signature::default()
        };
        let after = Signature {
            params: vec![b, a],
            ..Signature::default()
        };
        assert_eq!(
            texts(&signature_changes(&before, &after)),
            vec!["order (a, b) → (b, a)"]
        );
    }

    #[test]
    fn renders_signatures_with_separators() {
        let signature = Signature {
            params: vec![
                param("a", ParamKind::PositionalOnly, None),
                param("b", ParamKind::Positional, Some("1")),
                param("c", ParamKind::KeywordOnly, None),
            ],
            returns: Some("str".into()),
            is_async: true,
            ..Signature::default()
        };
        assert_eq!(
            render_signature(&signature),
            "async (a, /, b=1, *, c) -> str"
        );
    }

    #[test]
    fn multiset_diff_counts_occurrences() {
        let before = vec![
            "raise X".to_string(),
            "raise X".to_string(),
            "yield".to_string(),
        ];
        let after = vec!["raise X".to_string(), "raise Y".to_string()];
        let (added, removed) = multiset_diff(&before, &after);
        assert_eq!(added, vec!["raise Y"]);
        assert_eq!(removed, vec!["raise X", "yield"]);
    }
}
