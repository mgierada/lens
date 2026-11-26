---Details for one symbol: relations, impact and commits. Used by the info panel and hover.
local builder = require("lens.render.builder")
local hl = require("lens.highlights")
local summary = require("lens.render.summary")

local M = {}

local SHORT_SHA = 8

---@param model lens.Model
---@param id integer|nil
---@return lens.Builder
function M.render(model, id)
  local b = builder.new()
  if not id then
    b:add({ { "Move the cursor onto a symbol to see its details.", "LensDim" } })
    return b
  end
  local symbol = model:symbol(id)
  local range = symbol.head or symbol.base
  b:add({
    { model:symbol_name(id), "LensSymbol" },
    { ("  %s "):format(symbol.kind), "LensDim" },
    { symbol.change, hl.for_change(symbol.change) },
    { "  " .. symbol.path .. (range and (":%d-%d"):format(range.start, range["end"]) or ""), "LensPath" },
  }, { kind = "symbol", symbol = id })
  if symbol.moved_from then
    local from = symbol.moved_from
    b:add({ { "moved from ", "LensDim" }, { from.path .. ":" .. from.qualname, "LensPath" } })
  end
  for _, segments in ipairs(summary.lines(symbol, "")) do
    b:add(segments)
  end
  if symbol.hub then
    b:add({ { ("hub: %d referrers, does not merge clusters"):format(symbol.ref_count), "LensDim" } })
  end
  M.render_edges(b, model, "uses", model.edges_out[id], "to")
  M.render_edges(b, model, "used by", model.edges_in[id], "from")
  M.render_impact(b, model.impact[id])
  M.render_commits(b, model, id)
  return b
end

---@param b lens.Builder
---@param model lens.Model
---@param label string
---@param edges table[]|nil
---@param field "to"|"from"
function M.render_edges(b, model, label, edges, field)
  if not edges or #edges == 0 then
    return
  end
  b:add({ { label .. ":", "LensTitle" } })
  for _, edge in ipairs(edges) do
    local other = edge[field]
    local code = edge.kind == "call" or edge.kind == "reference"
    local detail = edge.kind .. (edge.via and (" " .. edge.via) or "")
    if code and edge.confidence == "low" then
      detail = detail .. ", by name only"
    end
    local check_text, check_group = summary.check(edge.call_check)
    b:add({
      { "  " .. model:symbol_name(other) },
      { "  " .. model:symbol(other).path, "LensPath" },
      { "  (" .. detail .. ")", "LensDim" },
      { check_text ~= "" and ("  " .. check_text) or "", check_group },
    }, { kind = "symbol", symbol = other })
  end
end

---@param b lens.Builder
---@param impact table[]|nil
function M.render_impact(b, impact)
  if not impact or #impact == 0 then
    return
  end
  b:add({ { ("unchanged referrers (%d):"):format(#impact), "LensTitle" } })
  for _, entry in ipairs(impact) do
    local check_text, check_group = summary.check(entry.call_check)
    b:add({
      { ("  %s:%d "):format(entry.path, entry.line), "LensPath" },
      { entry.qualname or "<module>" },
      { entry.dangling and "  dangling: name no longer exists" or "", "LensDangling" },
      { check_text ~= "" and ("  " .. check_text) or "", check_group },
    }, { kind = "impact", path = entry.path, line = entry.line })
  end
end

---Commits behind the hunks this symbol owns.
---@param b lens.Builder
---@param model lens.Model
---@param id integer
function M.render_commits(b, model, id)
  local seen, shas = {}, {}
  for _, hunk_id in ipairs(model.owned_hunks[id] or {}) do
    for _, sha in ipairs(model:hunk(hunk_id).commits) do
      if not seen[sha] then
        seen[sha] = true
        table.insert(shas, sha)
      end
    end
  end
  if #shas == 0 then
    return
  end
  b:add({ { "commits:", "LensTitle" } })
  for _, sha in ipairs(shas) do
    local commit = model.commits_by_sha[sha]
    b:add({
      { "  " .. sha:sub(1, SHORT_SHA) .. " ", "LensCommit" },
      { commit and commit.summary or "" },
    }, { kind = "commit", sha = sha })
  end
end

return M
