# lens

Structure-aware branch comparison for code review.

`lens` compares two branches and groups the diff into **logical changes** (clusters) instead of a
flat list of files. Changed symbols (functions, classes, methods, module-level variables) are
linked through the references between them, so a cluster shows a definition together with the
changed code that uses it and the tests that exercise it. For every cluster it also reports:

- **impact**: unchanged code that references the changed symbols,
- **dangling references**: code still using a deleted or moved symbol under its old name,
- **why**: the commits that introduced each hunk (via `git blame`).

Currently supported language: **Python**. The analysis core is language-agnostic; see
[Adding a language](#adding-a-language).

## Build

Requires Rust (stable) and `git` on `PATH`.

```sh
cargo build --release
# binary: target/release/lens
```

Optionally put it on your `PATH`:

```sh
cargo install --path .
```

## Usage

`lens` analyses the repository given by `--repo` (default: the **current directory**). Either run
it from inside the repository you want to review, or point at it explicitly:

```sh
# from inside the repo under review
cd ~/code/my-service
lens analyze main..my-feature

# from anywhere
lens analyze main..my-feature --repo ~/code/my-service
```

> If you see `unknown revision '<branch>'`, the branch does not exist in the repository `lens` is
> looking at. Check `--repo`, and for branches that only exist on a remote use `origin/<branch>`
> (or `git fetch` first).

### Revisions

All forms use merge-base semantics (like `git diff base...head`): only changes made on the head
side since it diverged from base are shown.

```sh
lens analyze main..feature        # range form
lens analyze main...feature       # same thing
lens analyze main feature         # two arguments
lens analyze main                 # head defaults to HEAD
lens analyze HEAD~5..HEAD         # any revision git understands
lens analyze main..origin/feature # remote branches
```

### Output formats

```sh
lens analyze main..feature                     # human-readable text with diffs
lens analyze main..feature --no-diff           # text, clusters and symbols only
lens analyze main..feature --format json       # compact JSON (for tools / editor plugins)
lens analyze main..feature --format json --pretty
lens analyze main..feature --format html > graph.html   # interactive graph, open in a browser
```

Example text output:

```
lens main..feature (merge-base 4261a26b)
16 files, +485 -69, 36 hunks, 45 symbols, 23 clusters, 1380 files indexed

== [1] fetch (+2) ==
  commit 1a2b3c4d Add retry to fetch
  commit 5e6f7a8b Test retry
  ~ pkg/http.py:1 fetch [Function]
  ~ pkg/api.py:5 Client.load [Method] -> fetch
  + tests/test_http.py:4 test_fetch [Function] -> fetch
  impact: 1 unchanged referrer(s)
    pkg/cli.py:5 main -> fetch
```

Symbol markers: `+` added, `-` deleted, `~` modified, `>` moved. `-> x` lists changed symbols this
one references; a trailing `?` marks a low-confidence (name-only) match and `✗` a call that no
longer matches the new signature. Indented lines summarise the change: `signature:` changes,
`calls:`/`uses:` references added (`+`) or removed (`-`), `flow:` raise/except/yield changes.

Within a cluster, symbols are ordered for review: dependencies before the code that uses them,
tests last. Clusters are ordered largest first.

### Options

| Option | Default | Description |
|---|---|---|
| `--repo PATH` | `.` | Repository to analyse. |
| `--format text\|json` | `text` | Output format. |
| `--pretty` | off | Pretty-print JSON. |
| `-U, --context N` | `3` | Context lines per hunk. |
| `--hub-threshold N` | `30` | Symbols with more distinct referrers than this are "hubs": their edges are reported but do not merge clusters (stops e.g. a logger from gluing everything together). |
| `--impact-limit N` | `20` | Impact entries kept per changed symbol (the total is still reported). |
| `--exclude GLOB` | — | Skip matching paths; repeatable. `node_modules`, `vendor`, `site-packages`, `.venv`, `venv`, `__pycache__` are always excluded. |
| `--max-file-size BYTES` | `1000000` | Skip larger files when indexing. |
| `--no-cache` | off | Don't read or write the index cache. |
| `--no-blame` | off | Skip per-hunk blame; noticeably faster on very large diffs. |
| `--no-diff` | off | Text output without hunk contents. |

## Neovim plugin

Requires Neovim 0.10+. No plugin dependencies; [diffview.nvim](https://github.com/sindrets/diffview.nvim)
is used for side-by-side diffs when installed.

### Install (lazy.nvim)

```lua
{
  "lens", -- or your git URL
  dir = "~/00_code/lens", -- local checkout
  build = "cargo build --release",
  cmd = { "Lens", "LensRefresh", "LensClose" },
  opts = {},
}
```

The plugin uses, in order: `opts.cmd`, the checkout's `target/release/lens`, then `lens` on `PATH`.

### Commands

| Command | Description |
|---|---|
| `:Lens [range]` | Open a review tab for the repository of the current working directory. `range` takes the same forms as the CLI (`main..feature`, `main`, …) and completes branch names. Without it, `opts.base` or the detected default branch (`origin/HEAD`, `main`, `master`) is compared with `HEAD`. |
| `:LensRefresh` | Re-run the analysis (viewed marks are kept). |
| `:LensClose` | Close the review tab. |

### Layout

```
┌ panel ───────────────┬ stack ─────────────────────────────────────┐
│ ▾ [1] fetch (+2)     │ [1/12] fetch (+2)                          │
│     ~ fetch  http.py │   1a2b3c4d Add retry to fetch              │
│     ~ Client.load    │                                            │
│     + test_fetch     │ ~ fetch  function modified  pkg/http.py:1  │
│ ▸ [2] lonely         │     ← used by Client.load, test_fetch      │
│                      │ @@ -1,2 +1,2 @@                            │
│ Not analysed (1)     │ -def fetch(url):                           │
│     README.md        │ +def fetch(url, retries=3):                │
│                      ├ info ──────────────────────────────────────┤
│                      │ unchanged referrers (1): pkg/cli.py:5 main │
└──────────────────────┴────────────────────────────────────────────┘
```

- **panel** – clusters (largest first) and their symbols in review order; files lens could not
  analyse at the bottom.
- **stack** – every hunk of the current cluster, grouped under the symbol it belongs to, with
  relations (`→ uses`, `← used by`, impact) and commits (end-of-line SHAs on hunk headers).
- **info** – details of the symbol under the cursor: relations, unchanged referrers (jump with
  `<CR>`), commits.

Files open at the reviewed head revision: the real file when that revision is checked out (so LSP
works), otherwise a read-only `lens://<sha>/<path>` buffer.

### Keys (all lens windows)

| Key | Action |
|---|---|
| `<CR>` | Panel: open/fold cluster, jump to symbol, diff an unanalysed file. Stack: expand symbol header / diff at hunk line. Info: jump to referrer or symbol. |
| `]c` / `[c` | Next / previous cluster |
| `]h` / `[h` | Next / previous hunk |
| `]s` / `[s` | Next / previous symbol |
| `e` | Toggle full symbol body (head version, removals interleaved) instead of hunks |
| `gd` | Side-by-side diff of the file (Diffview or built-in `diffthis`) |
| `gf` | Open the file at head in the previous tab |
| `K` | Floating symbol details |
| `x` | Toggle viewed (symbol, or whole cluster on a cluster line); kept for the session |
| `i` | Toggle info window |
| `<Tab>` | Cycle focus panel → stack → info |
| `R` | Refresh |
| `gG` | Open the interactive graph of all clusters in the browser, focused on the current one |
| `q` | Close |

### Configuration

Defaults (see `lua/lens/config.lua`):

```lua
require("lens").setup({
  cmd = nil,          -- path to the lens binary
  base = nil,         -- default base revision
  args = {},          -- extra CLI args, e.g. { "--no-blame", "--hub-threshold", "50" }
  panel = { width = 42 },
  info = { height = 12, enabled = true },
  diff_tool = "auto", -- "auto" | "diffview" | "builtin"
  icons = { added = "+", modified = "~", deleted = "-", moved = ">",
            expanded = "▾", collapsed = "▸", viewed = "✓" },
  keymaps = { select = "<CR>", next_cluster = "]c", prev_cluster = "[c",
              next_hunk = "]h", prev_hunk = "[h", next_symbol = "]s", prev_symbol = "[s",
              open_diff = "gd", open_file = "gf", hover = "K", expand = "e",
              toggle_info = "i", toggle_viewed = "x", focus = "<Tab>",
              refresh = "R", close = "q", graph = "gG" }, -- set a key to "" to disable it
})
```

Highlight groups (`LensAdd`, `LensDelete`, `LensHunkHeader`, `LensSymbol`, `LensImpact`, …) link
to standard groups and can be overridden; see `lua/lens/highlights.lua`.

## Graph view

`--format html` (or `gG` in Neovim) produces a self-contained page with an interactive graph
(cytoscape.js, loaded from cdnjs with Subresource Integrity, so the first view needs network).

- **Files** view (default): each cluster is a box holding its changed files; **Symbols** view
  shows every changed definition instead.
- Arrows: solid = code references, dashed blue = name mentions (configs, docs), dotted = shared
  rare token, red = a call that no longer matches the new signature. Nodes are coloured by change
  (added / modified / deleted / moved); tests are faded.
- Click a cluster, file or symbol for details: what changed (signature, calls, flow), links,
  unchanged referrers with call checks, commits. The cluster list on the left zooms to a cluster.
- Toggles for tests, mentions and shared tokens; search highlights matching nodes.

## How it works

1. **Index** – every supported file at the head revision is parsed with tree-sitter (in
   parallel). Files are read directly from git objects; the working tree is never touched.
2. **Detect changes** – symbols on both sides are matched by qualified name and compared by a
   hash of their own code (nested definitions excluded), so changing a method does not mark its
   class as changed. Identical bodies that disappear in one place and appear in another are
   reported as **moved**. Changed top-level code outside definitions becomes a `<module>` entry.
3. **Resolve references** across the whole repository: same-file scope (incl. `self.x`), imports
   (absolute, relative, aliased, `src/`/`lib/` layouts), wildcard imports, and finally a
   unique-name fallback for calls (low confidence).
4. **Link files by name** – files without language support (YAML/CSV configs, docs, data, lock
   files) are whole-file nodes. A changed line that mentions a changed file's name, or a
   distinctive part of it (`batch-27-assets` → `highfive_batch-27-assets.yaml`), creates a
   *mention* link; changes containing the same rare token (`cycle_59`, an MLflow run id) get a
   weaker *shared* link.
5. **Cluster** – only *calls* between non-test code merge clusters, and a newly added file is
   one unit. Everything else (tests, constants, import-only `<module>` changes, files) is then
   *attached*, never merged, in tiers of decreasing strength: references and mentions, same file,
   shared tokens, same commit (generic commits such as `fix`/`docs` are ignored). Hub symbols don't
   merge clusters. Unchanged referrers become the cluster's impact.
6. **Name** – a cluster's title is the meaningful commit subject that touched most of its files
   (a new file counts for the commit that created it; ties go to the earliest), its subtitle the
   directory, main files and central definition.
7. **Summarise** – for every changed symbol, compare the base and head syntax trees: signature
   (parameters added/removed/changed, reordering, return annotation, decorators, `async`, class
   bases), calls and other references added/removed (builtins and the symbol's own parameters are
   ignored), and control flow (`raise X`, `except X`, `yield`).
8. **Check calls** – when a symbol's signature changed (a class counts through its `__init__`),
   every call to it — from changed and unchanged code — is bound against the new signature with
   Python's rules: `ok`, `broken` (with the reason), or `unknown` (forwarded `*args`/`**kwargs`, a
   decorator that may rewrite the signature, or a callee matched by name only). Broken calls are
   listed first and never truncated. Parameters shadow module-level names, so injected values such
   as pytest fixtures are not mistaken for the functions of the same name.
9. **Why** – commits in the range, per file and per hunk (blame on added lines).

### Cache

Parsed facts are cached per blob in `.git/lens/index.db` (inside the repository's git dir, so
it's never committed and disappears with the repo). Unchanged files are parsed once and reused
across branches and runs; the first run on a large repo is the slow one. Delete the file to reset
the cache. The cache currently never shrinks.

## JSON schema

Top-level fields (see `src/model.rs`, `schema_version` = 1). Cross references are indices into
the top-level arrays.

| Field | Content |
|---|---|
| `base`, `head`, `merge_base` | Revisions and resolved SHAs. |
| `stats` | Counts: files, hunks, symbols, clusters, additions, deletions, indexed files. |
| `commits` | Commits in `merge_base..head`, oldest first (`sha`, `summary`, `body`). |
| `files` | Changed files: `path`, `old_path`, `status`, `language`, `hunks`, `commits`, `parse_errors`. |
| `hunks` | Hunks with per-line `kind`/`old_line`/`new_line`/`text`, touched `symbols`, `commits`. |
| `symbols` | Changed symbols: `path`, `qualname`, `kind`, `change`, `moved_from`, `base`/`head` line ranges, `is_test`, `ref_count`, `hub`, head `signature` text, `summary` (`signature` changes with `kind`/`text`, `calls_added/removed`, `uses_added/removed`, `flow_added/removed`), `broken_calls`. |
| `edges` | `from` is linked to `to` (both changed symbols), with `kind` (`call`/`reference`/`mention`/`shared`), `via` (matched name or token for text links), `confidence`, `hub` and `call_check` (`{status: ok|broken|unknown, reason}`, set when `to`'s signature changed). |
| `clusters` | `title`, `subtitle`, ordered `symbols`, `hunks`, `commits`, `impact` (with `dangling` flag and `call_check`; broken first), `impact_total`. |
| `unclustered_files` | Changed files not represented by any change node (rare: e.g. a file whose only change cannot be attributed). Unsupported files appear as `file`-kind symbols. |

## Limitations

- Python only for now.
- Resolution is static and heuristic: no type inference, `self.x` does not follow inheritance,
  dynamic calls (`getattr`, dependency injection) are not seen.
- Formatting-only edits mark a symbol as modified.
- On very large ranges (dozens of commits) one dominant cluster can still form around a large
  connected refactor; tests attach to it rather than merging clusters.

## Adding a language

Implement the `Language` trait (`src/lang/mod.rs`) and register it in `Registry::new`:

- `matches` – which paths the language handles,
- `extract` – definitions, references and imports (typically tree-sitter queries, see
  `src/lang/python/*.scm`),
- `module_names` / `resolve_import` – how files map to importable names,
- optionally `scope_candidates` and `is_test_path`.

Bump `version()` whenever extraction output changes so cached facts are invalidated.

## Development

```sh
make test        # Rust + Lua (Lua tests need mini.nvim, see Makefile)
cargo fmt
cargo clippy --all-targets
```

End-to-end tests (`tests/analyze.rs`) build throwaway git repositories and run the full analysis.
