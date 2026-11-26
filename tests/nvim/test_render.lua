local fixture = require("tests.nvim.fixture")
local config = require("lens.config")
local info = require("lens.render.info")
local model_mod = require("lens.model")
local panel = require("lens.render.panel")
local runner = require("lens.runner")
local stack = require("lens.render.stack")

local eq = MiniTest.expect.equality
local T = MiniTest.new_set()

local icons = config.defaults.icons

local function model()
  return model_mod.new(fixture())
end

---@param built lens.Builder
---@param needle string
---@return integer|nil 1-based line containing needle
local function find(built, needle)
  for i, text in ipairs(built.lines) do
    if text:find(needle, 1, true) then
      return i
    end
  end
  return nil
end

T["model"] = MiniTest.new_set()

T["model"]["assigns each hunk to its first symbol in review order"] = function()
  local m = model()
  eq(m.owned_hunks[0], { 0 })
  eq(m.owned_hunks[1], { 1 })
  eq(m.cluster_of[2], 0)
  eq(m.cluster_of[3], 1)
end

T["model"]["indexes edges and impact by symbol"] = function()
  local m = model()
  eq(#m.edges_in[0], 2)
  eq(m.edges_out[1][1].to, 0)
  eq(m.impact[0][1].qualname, "main")
end

T["panel"] = MiniTest.new_set()

T["panel"]["lists clusters and expands the current one"] = function()
  local built = panel.render(model(), { current = 0, expanded = { [0] = true }, viewed = {} }, icons)
  eq(find(built, "pkg/ · http.py, api.py +1 · fetch") ~= nil, true)
  local cluster_line = find(built, "[1] fetch (+2)")
  eq(built.items[cluster_line], { kind = "cluster", cluster = 0 })
  local symbol_line = find(built, "Client.load")
  eq(built.items[symbol_line].symbol, 1)
  eq(find(built, "lonely") ~= nil, true)
  eq(find(built, "test_fetch") ~= nil, true)
  eq(built.items[find(built, "README.md")], { kind = "file", path = "README.md" })
end

T["panel"]["collapsed clusters hide symbols and viewed clusters are marked"] = function()
  local state = { current = 0, expanded = {}, viewed = { [3] = true } }
  local built = panel.render(model(), state, icons)
  eq(find(built, "Client.load"), nil)
  eq(find(built, "lonely " .. icons.viewed) ~= nil, true)
end

T["stack"] = MiniTest.new_set()

T["stack"]["shows symbols in review order with relations and hunks"] = function()
  local built = stack.render(model(), 0, { expanded = {}, viewed = {} }, icons, function() end)
  local fetch = find(built, "fetch  function modified")
  local load = find(built, "Client.load  method")
  local test = find(built, "test_fetch  function added")
  eq(fetch < load and load < test, true)
  eq(find(built, "← used by Client.load, test_fetch? ✗") ~= nil, true)
  eq(find(built, "  pkg/ · http.py, api.py +1 · fetch") ~= nil, true)
  eq(find(built, "signature: +retries=3") ~= nil, true)
  eq(find(built, "flow: +raise ValueError") ~= nil, true)
  eq(find(built, "✗ 1 call(s) no longer match the signature") ~= nil, true)
  eq(find(built, "1 unchanged referrer(s)") ~= nil, true)
  eq(find(built, "Add retry to fetch") ~= nil, true)
  local added = find(built, "+def fetch(url, retries=3):")
  eq(built.items[added], { kind = "line", hunk = 0, symbol = 0, path = "pkg/http.py", line = 1 })
  local groups = {}
  for _, hl in ipairs(built.line_highlights) do
    groups[hl.group] = (groups[hl.group] or 0) + 1
  end
  eq(groups, { LensAdd = 4, LensDelete = 2 })
end

T["stack"]["expanded symbols show the full head body with removals interleaved"] = function()
  local file = { "def fetch(url, retries=3):", "    return get(url)", "", "def get(url):" }
  local read = function(path)
    eq(path, "pkg/http.py")
    return file
  end
  local built = stack.render(model(), 0, { expanded = { [0] = true }, viewed = {} }, icons, read)
  local header = find(built, "@@ fetch (full body) @@")
  eq(built.lines[header + 1], "-def fetch(url):")
  eq(built.lines[header + 2], "+def fetch(url, retries=3):")
  eq(built.lines[header + 3], "     return get(url)")
  eq(find(built, "def get(url)"), nil)
end

T["stack"]["renders an empty report"] = function()
  local report = fixture()
  report.clusters = {}
  local built = stack.render(model_mod.new(report), 0, { expanded = {}, viewed = {} }, icons, function() end)
  eq(built.lines, { "No clusters: nothing to review." })
end

T["info"] = MiniTest.new_set()

T["info"]["shows relations, impact and commits"] = function()
  local built = info.render(model(), 0)
  eq(find(built, "used by:") ~= nil, true)
  eq(find(built, "by name only") ~= nil, true)
  local impact = find(built, "pkg/cli.py:5 main")
  eq(built.items[impact], { kind = "impact", path = "pkg/cli.py", line = 5 })
  eq(find(built, "pkg/cli.py:5 main  ✓ matches new signature") ~= nil, true)
  eq(find(built, "✗ line 2: missing required argument 'url'") ~= nil, true)
  eq(find(built, "signature: +retries=3") ~= nil, true)
  eq(find(built, "Add retry to fetch") ~= nil, true)
end

T["runner"] = MiniTest.new_set()

T["runner"]["builds the CLI command"] = function()
  local cmd = runner.command("lens", "main..x", "/repo", { "--no-blame" })
  eq(cmd, { "lens", "analyze", "main..x", "--repo", "/repo", "--format", "json", "--no-blame" })
end

T["runner"]["decodes JSON null as nil"] = function()
  local report = runner.decode('{"a": null, "b": [1]}')
  eq(report, { b = { 1 } })
  local _, err = runner.decode("not json")
  eq(type(err), "string")
end


T["syntax"] = MiniTest.new_set()

local function snippet_builder(lines)
  local builder = require("lens.render.builder")
  local prefix = { added = "+", removed = "-", context = " " }
  local b = builder.new()
  b:begin_code("example.lua")
  for _, entry in ipairs(lines) do
    local row = b:add({ { prefix[entry[1]] .. entry[2] } })
    b:code_line(row, entry[1], entry[2])
  end
  return b
end

local function groups_at(b, row, col)
  local found = {}
  for _, h in ipairs(b.highlights) do
    if h.row == row and h.col == col then
      table.insert(found, h.group)
    end
  end
  return found
end

T["syntax"]["highlights old and new sides past the diff prefix"] = function()
  local syntax = require("lens.render.syntax")
  local b = snippet_builder({
    { "removed", "local a = 1" },
    { "added", "local b = 'x'" },
    { "context", "return a" },
  })
  syntax.highlight(b)
  eq(vim.tbl_contains(groups_at(b, 0, 1), "@keyword.lua"), true)
  eq(vim.tbl_contains(groups_at(b, 1, 1), "@keyword.lua"), true)
  eq(vim.tbl_contains(groups_at(b, 1, 11), "@string.lua"), true)
  -- Context lines are highlighted from the new side only.
  eq(#groups_at(b, 2, 1), 1)
end

T["syntax"]["skips files without a parser"] = function()
  local syntax = require("lens.render.syntax")
  eq(syntax.lang_for("notes.unknownext"), nil)
end


T["stack"]["groups text links by kind"] = function()
  local report = fixture()
  table.insert(report.edges, { from = 1, to = 3, kind = "mention", via = "other.py", confidence = "low", hub = false })
  table.insert(report.edges, { from = 0, to = 3, kind = "shared", via = "cycle_59", confidence = "low", hub = false })
  local relations = stack.relations(model_mod.new(report), 3)
  local by_label = {}
  for _, r in ipairs(relations) do
    by_label[r.label] = r.names
  end
  eq(by_label["← mentioned by"], { "Client.load" })
  eq(by_label["≈ shares"], { "cycle_59 with fetch" })
end


T["graph"] = MiniTest.new_set()

T["graph"]["embeds raw report JSON and focus into the template"] = function()
  local graph = require("lens.graph")
  local html = graph.page('{"clusters":[],"note":"</script>"}', 2)
  eq(html:find("const REPORT = {\"clusters\":[]", 1, true) ~= nil, true)
  eq(html:find("const FOCUS = 2;", 1, true) ~= nil, true)
  eq(html:find("\\u003c/script>", 1, true) ~= nil, true)
  eq(html:find("/*LENS_REPORT*/", 1, true), nil)
end

return T
