//! End-to-end tests against synthetic git repositories.

use std::path::Path;
use std::process::Command;

use globset::GlobSet;
use lens::analysis::{AnalyzeOptions, analyze};
use lens::model::{CallCheck, Change, ChangedSymbol, Cluster, EdgeKind, Report};
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        status.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn write(dir: &Path, path: &str, content: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

fn remove(dir: &Path, path: &str) {
    std::fs::remove_file(dir.join(path)).unwrap();
}

/// Base: an HTTP helper used by an API client, a CLI and a util module.
/// Feature: retry support in `fetch` + caller, a new test, a moved helper and an unrelated tweak.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    write(root, "pkg/__init__.py", "");
    write(
        root,
        "pkg/http.py",
        "def fetch(url):\n    return get(url)\n\n\ndef get(url):\n    return url\n",
    );
    write(
        root,
        "pkg/api.py",
        "from pkg.http import fetch\n\n\nclass Client:\n    def load(self, url):\n        return fetch(url)\n",
    );
    write(
        root,
        "pkg/cli.py",
        "from pkg import http\n\n\ndef main():\n    return http.fetch(\"x\")\n",
    );
    write(
        root,
        "pkg/util.py",
        "def old_helper():\n    return 1\n\n\ndef unrelated():\n    return 2\n",
    );
    write(
        root,
        "pkg/uses.py",
        "from pkg.util import old_helper\n\n\ndef use():\n    return old_helper()\n",
    );
    write(root, "pkg/other.py", "def lonely():\n    return 0\n");
    write(root, "README.md", "hi\n");
    commit(root, "base");

    git(root, &["checkout", "-q", "-b", "feature"]);
    write(
        root,
        "pkg/http.py",
        "def fetch(url, retries=3):\n    return get(url)\n\n\ndef get(url):\n    return url\n",
    );
    write(
        root,
        "pkg/api.py",
        "from pkg.http import fetch\n\n\nclass Client:\n    def load(self, url):\n        return fetch(url, retries=1)\n",
    );
    commit(root, "Add retry to fetch");
    write(
        root,
        "tests/test_http.py",
        "from pkg.http import fetch\n\n\ndef test_fetch():\n    assert fetch(\"u\")\n",
    );
    commit(root, "Test retry");
    write(root, "pkg/util.py", "def unrelated():\n    return 2\n");
    write(root, "pkg/helpers.py", "def old_helper():\n    return 1\n");
    commit(root, "Move helper");
    write(root, "pkg/other.py", "def lonely():\n    return 1\n");
    write(root, "README.md", "hello\n");
    commit(root, "Tweak lonely");
    dir
}

fn options(use_cache: bool) -> AnalyzeOptions {
    AnalyzeOptions {
        base: "main".into(),
        head: "feature".into(),
        context: 3,
        hub_threshold: 30,
        impact_limit: 20,
        excludes: GlobSet::empty(),
        max_file_size: 1_000_000,
        use_cache,
        blame: true,
    }
}

fn symbol<'a>(report: &'a Report, qualname: &str) -> &'a ChangedSymbol {
    report
        .symbols
        .iter()
        .find(|s| s.qualname == qualname)
        .unwrap_or_else(|| {
            let names: Vec<&str> = report.symbols.iter().map(|s| s.qualname.as_str()).collect();
            panic!("no symbol {qualname} in {names:?}")
        })
}

fn cluster_of<'a>(report: &'a Report, qualname: &str) -> &'a Cluster {
    let id = symbol(report, qualname).id;
    report
        .clusters
        .iter()
        .find(|c| c.symbols.contains(&id))
        .expect("symbol is clustered")
}

fn qualnames(report: &Report, ids: &[usize]) -> Vec<String> {
    ids.iter()
        .map(|&id| report.symbols[id].qualname.clone())
        .collect()
}

#[test]
fn groups_connected_changes_in_review_order() {
    let repo = fixture();
    let report = analyze(repo.path(), &options(false)).unwrap();
    let cluster = cluster_of(&report, "fetch");
    assert_eq!(
        qualnames(&report, &cluster.symbols),
        vec!["fetch", "Client.load", "<module>", "test_fetch"]
    );
    assert_eq!(symbol(&report, "fetch").change, Change::Modified);
    assert_eq!(symbol(&report, "test_fetch").change, Change::Added);
    assert!(symbol(&report, "test_fetch").is_test);
}

#[test]
fn unrelated_changes_stay_separate() {
    let repo = fixture();
    let report = analyze(repo.path(), &options(false)).unwrap();
    let lonely = cluster_of(&report, "lonely");
    // README.md changed in the same commit as `lonely`: the file joins that cluster.
    assert_eq!(
        qualnames(&report, &lonely.symbols),
        vec!["<file>", "lonely"]
    );
    assert_eq!(lonely.title, "Tweak lonely");
    assert!(report.unclustered_files.is_empty());
}

#[test]
fn reports_impact_through_module_imports() {
    let repo = fixture();
    let report = analyze(repo.path(), &options(false)).unwrap();
    let cluster = cluster_of(&report, "fetch");
    let callers: Vec<(&str, Option<&str>)> = cluster
        .impact
        .iter()
        .map(|i| (i.path.as_str(), i.qualname.as_deref()))
        .collect();
    assert_eq!(callers, vec![("pkg/cli.py", Some("main"))]);
}

#[test]
fn detects_moves_and_dangling_references() {
    let repo = fixture();
    let report = analyze(repo.path(), &options(false)).unwrap();
    let moved = symbol(&report, "old_helper");
    assert_eq!(moved.change, Change::Moved);
    assert_eq!(moved.path, "pkg/helpers.py");
    assert_eq!(moved.moved_from.as_ref().unwrap().path, "pkg/util.py");
    let cluster = cluster_of(&report, "old_helper");
    let dangling: Vec<&str> = cluster
        .impact
        .iter()
        .filter(|i| i.dangling)
        .map(|i| i.path.as_str())
        .collect();
    assert_eq!(dangling, vec!["pkg/uses.py"]);
}

#[test]
fn attaches_commits_to_clusters() {
    let repo = fixture();
    let report = analyze(repo.path(), &options(false)).unwrap();
    let summaries = |cluster: &Cluster| -> Vec<String> {
        let commits = cluster.commits.iter();
        let found = commits.filter_map(|sha| report.commits.iter().find(|c| &c.sha == sha));
        found.map(|c| c.summary.clone()).collect()
    };
    assert_eq!(
        summaries(cluster_of(&report, "fetch")),
        vec!["Add retry to fetch", "Test retry"]
    );
    assert_eq!(
        summaries(cluster_of(&report, "lonely")),
        vec!["Tweak lonely"]
    );
}

#[test]
fn hub_symbols_do_not_merge_clusters() {
    let repo = fixture();
    let mut opts = options(false);
    opts.hub_threshold = 0;
    let report = analyze(repo.path(), &opts).unwrap();
    assert!(symbol(&report, "fetch").hub);
    let cluster = cluster_of(&report, "test_fetch");
    assert_eq!(
        qualnames(&report, &cluster.symbols),
        vec!["<module>", "test_fetch"]
    );
}

#[test]
fn cached_run_matches_cold_run() {
    let repo = fixture();
    let cold = analyze(repo.path(), &options(true)).unwrap();
    assert!(repo.path().join(".git/lens/index.db").exists());
    let warm = analyze(repo.path(), &options(true)).unwrap();
    let json = |r: &Report| serde_json::to_string(r).unwrap();
    assert_eq!(json(&cold), json(&warm));
}

#[test]
fn deleted_symbols_are_reported() {
    let repo = fixture();
    let root = repo.path();
    remove(root, "pkg/other.py");
    commit(root, "Drop other");
    let report = analyze(root, &options(false)).unwrap();
    let lonely = symbol(&report, "lonely");
    assert_eq!(lonely.change, Change::Deleted);
    assert!(lonely.head.is_none());
}

#[test]
fn tests_attach_to_code_instead_of_merging_clusters() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "tests/test_both.py",
        "from pkg.http import fetch\nfrom pkg.other import lonely\n\n\n\
         def _helper():\n    return fetch(\"u\")\n\n\n\
         def test_both():\n    assert _helper() and lonely() and fetch(\"v\")\n",
    );
    commit(root, "Cross-cutting test");
    let report = analyze(root, &options(false)).unwrap();
    let fetch_cluster = cluster_of(&report, "fetch");
    assert_ne!(fetch_cluster.id, cluster_of(&report, "lonely").id);
    // `test_both` calls fetch twice (directly and via the helper) and lonely once.
    assert_eq!(cluster_of(&report, "test_both").id, fetch_cluster.id);
    assert_eq!(cluster_of(&report, "_helper").id, fetch_cluster.id);
}

#[test]
fn test_helpers_follow_the_tests_using_them() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "tests/test_other.py",
        "from pkg.other import lonely\n\n\n\
         def _expected():\n    return 1\n\n\n\
         def test_lonely():\n    assert lonely() == _expected()\n",
    );
    commit(root, "Test lonely");
    let report = analyze(root, &options(false)).unwrap();
    let lonely = cluster_of(&report, "lonely");
    assert_eq!(
        qualnames(&report, &lonely.symbols),
        vec!["<file>", "lonely", "<module>", "_expected", "test_lonely"]
    );
}

#[test]
fn type_references_do_not_merge_clusters() {
    let repo = fixture();
    let root = repo.path();
    write(root, "pkg/types.py", "class Retry:\n    attempts = 3\n");
    write(
        root,
        "pkg/http.py",
        "from pkg.types import Retry\n\n\ndef fetch(url, retries=3, policy: Retry = None):\n    \
         return get(url)\n\n\ndef get(url):\n    return url\n",
    );
    write(
        root,
        "pkg/other.py",
        "from pkg.types import Retry\n\n\ndef lonely(policy: Retry):\n    return 1\n\n\n\
         def use_lonely():\n    return lonely(None)\n",
    );
    commit(root, "Retry policy");
    let report = analyze(root, &options(false)).unwrap();
    let lonely = cluster_of(&report, "lonely");
    assert_ne!(cluster_of(&report, "fetch").id, lonely.id);
    assert_eq!(cluster_of(&report, "use_lonely").id, lonely.id);
    let lonely_id = symbol(&report, "lonely").id;
    let retry_id = symbol(&report, "Retry").id;
    let edge = report
        .edges
        .iter()
        .find(|e| e.from == lonely_id && e.to == retry_id);
    assert_eq!(edge.expect("type edge kept").kind, EdgeKind::Reference);
}

#[test]
fn module_level_changes_join_their_file() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "pkg/other.py",
        "import os\n\n\ndef lonely():\n    return 2\n\n\ndef use_lonely():\n    return lonely()\n",
    );
    commit(root, "Import os");
    let report = analyze(root, &options(false)).unwrap();
    let module = report
        .symbols
        .iter()
        .find(|s| s.qualname == "<module>" && s.path == "pkg/other.py")
        .expect("module change");
    let lonely = cluster_of(&report, "lonely");
    assert!(lonely.symbols.contains(&module.id));
}

#[test]
fn summarizes_signature_changes() {
    let repo = fixture();
    let report = analyze(repo.path(), &options(false)).unwrap();
    let fetch = symbol(&report, "fetch");
    let summary = fetch.summary.as_ref().expect("summary");
    let texts: Vec<&str> = summary.signature.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["+retries=3"]);
    assert_eq!(fetch.signature.as_deref(), Some("(url, retries=3)"));
    assert_eq!(
        symbol(&report, "test_fetch")
            .summary
            .as_ref()
            .unwrap()
            .calls_added,
        vec!["fetch"]
    );
}

#[test]
fn summarizes_reference_and_flow_changes() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "pkg/other.py",
        "def lonely():\n    if check():\n        raise ValueError('bad')\n    return len([])\n\n\n\
         def check():\n    return True\n",
    );
    commit(root, "Validate lonely");
    let report = analyze(root, &options(false)).unwrap();
    let summary = symbol(&report, "lonely").summary.clone().expect("summary");
    assert_eq!(summary.calls_added, vec!["check"]);
    assert_eq!(summary.flow_added, vec!["raise ValueError"]);
    assert!(summary.signature.is_empty());
}

#[test]
fn checks_calls_against_changed_signatures() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "pkg/http.py",
        "def fetch(url, retries=3):\n    return get(url)\n\n\ndef get(url, mode):\n    return url\n",
    );
    commit(root, "Require mode");
    let report = analyze(root, &options(false)).unwrap();
    let get = symbol(&report, "get");
    assert_eq!(get.broken_calls, 1);
    let fetch = symbol(&report, "fetch").id;
    let edge = report
        .edges
        .iter()
        .find(|e| e.from == fetch && e.to == get.id)
        .unwrap();
    assert_eq!(
        edge.call_check,
        Some(CallCheck::Broken {
            reason: "line 2: missing required argument 'mode'".into()
        })
    );
    let cluster = cluster_of(&report, "fetch");
    let cli = cluster
        .impact
        .iter()
        .find(|i| i.path == "pkg/cli.py")
        .unwrap();
    assert_eq!(cli.call_check, Some(CallCheck::Ok));
}

#[test]
fn constructor_changes_reach_class_callers() {
    let repo = fixture();
    let root = repo.path();
    git(root, &["checkout", "-q", "main"]);
    write(
        root,
        "pkg/models.py",
        "class Repo:\n    def __init__(self, path):\n        self.path = path\n",
    );
    write(
        root,
        "pkg/open.py",
        "from pkg.models import Repo\n\n\ndef open_repo():\n    return Repo(\"p\")\n",
    );
    commit(root, "Add repo");
    git(root, &["checkout", "-q", "feature"]);
    git(root, &["merge", "-q", "--no-edit", "main"]);
    write(
        root,
        "pkg/models.py",
        "class Repo:\n    def __init__(self, path, mode):\n        self.path = path\n",
    );
    commit(root, "Repo mode");
    let report = analyze(root, &options(false)).unwrap();
    let init = symbol(&report, "Repo.__init__");
    assert_eq!(init.broken_calls, 1);
    let impact = &cluster_of(&report, "Repo.__init__").impact;
    let open = impact
        .iter()
        .find(|i| i.path == "pkg/open.py")
        .expect("class caller found");
    assert_eq!(open.qualname.as_deref(), Some("open_repo"));
    assert!(matches!(open.call_check, Some(CallCheck::Broken { .. })));
}

#[test]
fn parameters_shadow_module_level_names() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "tests/test_fixture.py",
        "def fetch():\n    return 1\n\n\ndef test_uses_injected(fetch):\n    assert fetch(\"a\", \"b\")\n",
    );
    commit(root, "Fixture-style test");
    let report = analyze(root, &options(false)).unwrap();
    let test = symbol(&report, "test_uses_injected").id;
    assert!(
        report.edges.iter().all(|e| e.from != test),
        "{:?}",
        report.edges
    );
}

#[test]
fn config_files_are_linked_by_mentions_and_titled_by_commits() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "configs/data/highfive_batch-27-assets.yaml",
        "path: data\n",
    );
    write(
        root,
        "configs/cycle_27_locations.yml",
        "batches:\n  - name: \"batch-27-assets\"\n",
    );
    commit(root, "feat: add batch 27 training");
    write(root, "NOTES.md", "batch 27 notes\n");
    commit(root, "docs");
    let report = analyze(root, &options(false)).unwrap();
    let file = |path: &str| {
        report
            .symbols
            .iter()
            .find(|s| s.path == path)
            .expect(path)
            .id
    };
    let (cycle, data) = (
        file("configs/cycle_27_locations.yml"),
        file("configs/data/highfive_batch-27-assets.yaml"),
    );
    let mention = report
        .edges
        .iter()
        .find(|e| e.from == cycle && e.to == data)
        .expect("mention");
    assert_eq!(mention.kind, EdgeKind::Mention);
    assert_eq!(mention.via.as_deref(), Some("batch-27-assets"));
    let cluster = report
        .clusters
        .iter()
        .find(|c| c.symbols.contains(&cycle))
        .unwrap();
    assert!(cluster.symbols.contains(&data));
    assert_eq!(cluster.title, "feat: add batch 27 training");
    assert_eq!(
        cluster.subtitle,
        "configs/ · cycle_27_locations.yml, highfive_batch-27-assets.yaml"
    );
}

#[test]
fn graph_page_embeds_the_report_safely() {
    let repo = fixture();
    let root = repo.path();
    write(
        root,
        "pkg/other.py",
        "def lonely():\n    return \"</script><!--\"\n",
    );
    commit(root, "Tricky string");
    let report = analyze(root, &options(false)).unwrap();
    let page = lens::html::render_graph(&report, Some(1)).unwrap();
    assert!(page.contains("const FOCUS = 1;"));
    assert!(!page.contains("/*LENS_REPORT*/"));
    assert_eq!(
        page.matches("</script>").count(),
        lens::html::GRAPH_TEMPLATE.matches("</script>").count()
    );
}
