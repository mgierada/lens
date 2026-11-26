---lens.nvim: structure-aware branch review.
local config = require("lens.config")
local git = require("lens.git")
local highlights = require("lens.highlights")
local runner = require("lens.runner")
local view = require("lens.ui.view")

local M = {}

---@param opts lens.Config|nil
function M.setup(opts)
  config.setup(opts)
  highlights.setup()
end

---@param message string
---@param level integer|nil
local function notify(message, level)
  vim.notify("lens: " .. message, level or vim.log.levels.INFO)
end

---Run the CLI for `range` in `repo`, calling `on_report` with the decoded report.
---@param repo string
---@param range string
---@param on_report fun(report: table, json: string)
local function analyze(repo, range, on_report)
  local binary = runner.find_binary(config.options.cmd)
  if not binary then
    notify("binary not found; run `cargo build --release` in the plugin dir or set `cmd`", vim.log.levels.ERROR)
    return
  end
  notify("analysing " .. range .. " …")
  runner.run(runner.command(binary, range, repo, config.options.args), function(err, report, json)
    if err or not report then
      notify(err or "no report", vim.log.levels.ERROR)
      return
    end
    on_report(report, json)
  end)
end

---Open a review of `range` (e.g. "main..feature"; defaults to the configured/detected base).
---@param range string|nil
function M.open(range)
  highlights.setup()
  local repo = git.root(vim.fn.getcwd())
  if not repo then
    notify("not inside a git repository", vim.log.levels.ERROR)
    return
  end
  if not range or range == "" then
    range = config.options.base or git.default_base(repo)
  end
  analyze(repo, range, function(report, json)
    view.open(report, view.context(report, repo, range), json)
  end)
end

---Re-run the analysis of the open review.
function M.refresh()
  local session = view.session
  if not session then
    notify("no review open", vim.log.levels.WARN)
    return
  end
  analyze(session.ctx.repo, session.ctx.range, view.replace)
end

function M.close()
  view.close()
end

---Branch names for command completion.
---@param lead string
---@return string[]
function M.complete(lead)
  local repo = git.root(vim.fn.getcwd())
  if not repo then
    return {}
  end
  local prefix, rest = lead:match("^(.-%.%.%.?)(.*)$")
  prefix, rest = prefix or "", rest or lead
  local matches = {}
  for _, ref in ipairs(git.refs(repo)) do
    if vim.startswith(ref, rest) then
      table.insert(matches, prefix .. ref)
    end
  end
  return matches
end

return M
