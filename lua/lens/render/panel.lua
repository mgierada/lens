---Left panel: cluster tree plus files without symbol-level analysis.
local builder = require("lens.render.builder")
local hl = require("lens.highlights")

local M = {}

---@class lens.PanelState
---@field current integer|nil cluster shown in the main view
---@field expanded table<integer, boolean> clusters unfolded in the tree
---@field viewed table<integer, boolean> symbols marked as viewed

---@param cluster table
---@param viewed table<integer, boolean>
---@return boolean
local function cluster_viewed(cluster, viewed)
  for _, id in ipairs(cluster.symbols) do
    if not viewed[id] then
      return false
    end
  end
  return #cluster.symbols > 0
end

---@param model lens.Model
---@param state lens.PanelState
---@param icons table<string, string>
---@return lens.Builder
function M.render(model, state, icons)
  local b = builder.new()
  local report = model.report
  b:add({ { "LENS ", "LensTitle" }, { report.base.rev .. "..." .. report.head.rev, "LensTitle" } })
  local stats = report.stats
  b:add({
    {
      ("%d clusters · %d symbols · +%d -%d"):format(
        stats.clusters,
        stats.symbols,
        stats.additions,
        stats.deletions
      ),
      "LensDim",
    },
  })
  b:blank()
  for _, cluster in ipairs(report.clusters) do
    M.render_cluster(b, model, cluster, state, icons)
  end
  if #report.unclustered_files > 0 then
    b:blank()
    b:add({ { ("Not analysed (%d)"):format(#report.unclustered_files), "LensTitle" } })
    for _, path in ipairs(report.unclustered_files) do
      b:add({ { "    " }, { path, "LensPath" } }, { kind = "file", path = path })
    end
  end
  return b
end

---@param b lens.Builder
---@param model lens.Model
---@param cluster table
---@param state lens.PanelState
---@param icons table<string, string>
function M.render_cluster(b, model, cluster, state, icons)
  local open = state.expanded[cluster.id]
  local done = cluster_viewed(cluster, state.viewed)
  local row = b:add({
    { (open and icons.expanded or icons.collapsed) .. " " },
    { ("[%d] "):format(cluster.id + 1), "LensDim" },
    { cluster.title, done and "LensViewed" or "LensSymbol" },
    { done and (" " .. icons.viewed) or "", "LensAdded" },
  }, { kind = "cluster", cluster = cluster.id })
  if state.current == cluster.id then
    b:line_highlight(row, "LensCurrent")
  end
  if not open then
    return
  end
  if cluster.subtitle and cluster.subtitle ~= "" then
    b:add({ { "    " .. cluster.subtitle, "LensDim" } }, { kind = "cluster", cluster = cluster.id })
  end
  for _, id in ipairs(cluster.symbols) do
    local symbol = model:symbol(id)
    local viewed = state.viewed[id]
    local named_by_file = symbol.kind == "module" or symbol.kind == "file"
    local file = named_by_file and "" or ("  " .. vim.fn.fnamemodify(symbol.path, ":t"))
    b:add({
      { "    " },
      { icons[symbol.change] .. " ", hl.for_change(symbol.change) },
      { model:symbol_name(id), viewed and "LensViewed" or nil },
      { file, "LensDim" },
      { (symbol.broken_calls or 0) > 0 and " ✗" or "", "LensDangling" },
      { viewed and (" " .. icons.viewed) or "", "LensAdded" },
    }, { kind = "symbol", cluster = cluster.id, symbol = id })
  end
end

return M
