-- Hand-written report following the CLI's JSON schema (schema_version 1).
local SHA1 = string.rep("a", 40)
local SHA2 = string.rep("b", 40)

local function line(kind, text, old, new)
  return { kind = kind, text = text, old_line = old, new_line = new }
end

return function()
  return {
    schema_version = 1,
    repo = "/tmp/repo",
    base = { rev = "main", sha = string.rep("1", 40) },
    head = { rev = "feature", sha = string.rep("2", 40) },
    merge_base = string.rep("1", 40),
    stats = { files = 5, hunks = 4, symbols = 4, clusters = 2, additions = 8, deletions = 4, indexed_files = 9 },
    commits = {
      { sha = SHA1, summary = "Add retry to fetch", body = "" },
      { sha = SHA2, summary = "Drop lonely", body = "" },
    },
    files = {
      { path = "pkg/http.py", status = "modified", language = "python", hunks = { 0 }, commits = { SHA1 }, parse_errors = false },
      { path = "pkg/api.py", status = "modified", language = "python", hunks = { 1 }, commits = { SHA1 }, parse_errors = false },
      { path = "tests/test_http.py", status = "added", language = "python", hunks = { 2 }, commits = { SHA1 }, parse_errors = false },
      { path = "pkg/other.py", status = "deleted", language = "python", hunks = { 3 }, commits = { SHA2 }, parse_errors = false },
      { path = "README.md", status = "modified", hunks = {}, commits = {}, parse_errors = false },
    },
    hunks = {
      {
        id = 0, path = "pkg/http.py", old_start = 1, old_lines = 2, new_start = 1, new_lines = 2,
        header = "", symbols = { 0 }, commits = { SHA1 },
        lines = {
          line("removed", "def fetch(url):", 1, nil),
          line("added", "def fetch(url, retries=3):", nil, 1),
          line("context", "    return get(url)", 2, 2),
        },
      },
      {
        id = 1, path = "pkg/api.py", old_start = 5, old_lines = 2, new_start = 5, new_lines = 2,
        header = "class Client:", symbols = { 1 }, commits = { SHA1 },
        lines = {
          line("context", "    def load(self, url):", 5, 5),
          line("removed", "        return fetch(url)", 6, nil),
          line("added", "        return fetch(url, retries=1)", nil, 6),
        },
      },
      {
        id = 2, path = "tests/test_http.py", old_start = 0, old_lines = 0, new_start = 1, new_lines = 2,
        header = "", symbols = { 2 }, commits = { SHA1 },
        lines = {
          line("added", "def test_fetch():", nil, 1),
          line("added", "    assert fetch('u')", nil, 2),
        },
      },
      {
        id = 3, path = "pkg/other.py", old_start = 1, old_lines = 2, new_start = 0, new_lines = 0,
        header = "", symbols = { 3 }, commits = {},
        lines = {
          line("removed", "def lonely():", 1, nil),
          line("removed", "    return 0", 2, nil),
        },
      },
    },
    symbols = {
      { id = 0, path = "pkg/http.py", qualname = "fetch", kind = "function", change = "modified",
        base = { start = 1, ["end"] = 2 }, head = { start = 1, ["end"] = 2 }, is_test = false, ref_count = 3, hub = false,
        signature = "(url, retries=3)", broken_calls = 1,
        summary = { signature = { { kind = "param_added", text = "+retries=3" } }, calls_added = {}, calls_removed = {},
          uses_added = {}, uses_removed = {}, flow_added = { "raise ValueError" }, flow_removed = {} } },
      { id = 1, path = "pkg/api.py", qualname = "Client.load", kind = "method", change = "modified",
        base = { start = 5, ["end"] = 6 }, head = { start = 5, ["end"] = 6 }, is_test = false, ref_count = 0, hub = false },
      { id = 2, path = "tests/test_http.py", qualname = "test_fetch", kind = "function", change = "added",
        head = { start = 1, ["end"] = 2 }, is_test = true, ref_count = 0, hub = false },
      { id = 3, path = "pkg/other.py", qualname = "lonely", kind = "function", change = "deleted",
        base = { start = 1, ["end"] = 2 }, is_test = false, ref_count = 0, hub = false },
    },
    edges = {
      { from = 1, to = 0, kind = "call", confidence = "high", hub = false },
      { from = 2, to = 0, kind = "call", confidence = "low", hub = false,
        call_check = { status = "broken", reason = "line 2: missing required argument 'url'" } },
    },
    clusters = {
      {
        id = 0, title = "fetch (+2)", subtitle = "pkg/ · http.py, api.py +1 · fetch", symbols = { 0, 1, 2 }, hunks = { 0, 1, 2 }, commits = { SHA1 },
        impact = { { path = "pkg/cli.py", qualname = "main", line = 5, target = 0, confidence = "high", dangling = false,
            call_check = { status = "ok" } } },
        impact_total = 1,
      },
      { id = 1, title = "lonely", subtitle = "pkg/ · other.py", symbols = { 3 }, hunks = { 3 }, commits = { SHA2 }, impact = {}, impact_total = 0 },
    },
    unclustered_files = { "README.md" },
  }
end
