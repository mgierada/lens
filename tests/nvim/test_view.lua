local fixture = require("tests.nvim.fixture")
local runner = require("lens.runner")
local view = require("lens.ui.view")

local eq = MiniTest.expect.equality
local api = vim.api

local function ctx()
  return {
    repo = "/tmp/repo",
    range = "main..feature",
    merge_base = string.rep("1", 40),
    head_sha = string.rep("2", 40),
    head_checked_out = false,
  }
end

local function lines(name)
  return api.nvim_buf_get_lines(view.session.bufs[name], 0, -1, false)
end

local function contains(list, needle)
  for _, text in ipairs(list) do
    if text:find(needle, 1, true) then
      return true
    end
  end
  return false
end

local T = MiniTest.new_set({
  hooks = {
    post_case = function()
      view.close()
    end,
  },
})

T["opens a tab with panel, stack and info"] = function()
  local tabs = #api.nvim_list_tabpages()
  view.open(fixture(), ctx())
  eq(#api.nvim_list_tabpages(), tabs + 1)
  eq(api.nvim_get_current_win(), view.session.wins.panel)
  eq(contains(lines("panel"), "[1] fetch (+2)"), true)
  eq(contains(lines("stack"), "[1/2] fetch (+2)"), true)
  eq(contains(lines("info"), "fetch  function modified"), true)
end

T["navigates clusters and symbols"] = function()
  view.open(fixture(), ctx())
  view.step_cluster(1)
  eq(view.session.current, 1)
  eq(contains(lines("stack"), "lonely"), true)
  view.step_cluster(1)
  eq(view.session.current, 1)
  view.show_cluster(0, 1)
  local line = api.nvim_win_get_cursor(view.session.wins.stack)[1]
  eq(view.session.items.stack[line], {
    kind = "symbol",
    symbol = 1,
    path = "pkg/api.py",
    line = 5,
  })
  view.jump("hunk", 1)
  line = api.nvim_win_get_cursor(view.session.wins.stack)[1]
  eq(view.session.items.stack[line].kind, "hunk")
end

T["toggles viewed for a whole cluster from the panel"] = function()
  view.open(fixture(), ctx())
  local panel_items = view.session.items.panel
  for line, item in pairs(panel_items) do
    if item.kind == "cluster" and item.cluster == 0 then
      api.nvim_win_set_cursor(view.session.wins.panel, { line, 0 })
    end
  end
  view.toggle_viewed()
  eq(contains(lines("panel"), "fetch (+2) ✓"), true)
  view.toggle_viewed()
  eq(contains(lines("panel"), "fetch (+2) ✓"), false)
end

T["toggles the info window"] = function()
  view.open(fixture(), ctx())
  view.toggle_info()
  eq(view.session.wins.info, nil)
  view.toggle_info()
  eq(api.nvim_win_is_valid(view.session.wins.info), true)
  eq(contains(lines("info"), "fetch"), true)
end

T["close removes the tab and session"] = function()
  local tabs = #api.nvim_list_tabpages()
  view.open(fixture(), ctx())
  view.close()
  eq(view.session, nil)
  eq(#api.nvim_list_tabpages(), tabs)
end

T["end to end with the real binary"] = function()
  local binary = runner.find_binary(nil)
  if not binary or binary == "lens" then
    MiniTest.skip("release binary not built")
  end
  local repo = vim.fn.tempname()
  vim.fn.mkdir(repo .. "/pkg", "p")
  local function git(...)
    local cmd = { "git", "-C", repo, "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", ... }
    local result = vim.system(cmd, { text = true }):wait()
    eq(result.code, 0)
  end
  local function write(path, text)
    vim.fn.writefile(vim.split(text, "\n"), repo .. "/" .. path)
  end
  git("init", "-q", "-b", "main")
  write("pkg/a.py", "def f():\n    return 1\n\n\ndef g():\n    return f()")
  git("add", "-A")
  git("commit", "-q", "-m", "base")
  git("checkout", "-q", "-b", "feature")
  write("pkg/a.py", "def f():\n    return 2\n\n\ndef g():\n    return f() + 1")
  git("commit", "-qam", "Change f and g")

  local report
  local cmd = runner.command(binary, "main..feature", repo, { "--no-cache" })
  runner.run(cmd, function(err, result)
    eq(err, nil)
    report = result
  end)
  eq(vim.wait(10000, function() return report ~= nil end), true)
  view.open(report, view.context(report, repo, "main..feature"))
  eq(view.session.ctx.head_checked_out, true)
  eq(contains(lines("stack"), "Change f and g"), true)
  eq(contains(lines("stack"), "+    return f() + 1"), true)
  eq(contains(lines("panel"), "[1] Change f and g"), true)
  vim.fn.delete(repo, "rf")
end

return T
