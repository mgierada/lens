---Shared rendering of change summaries and call checks (stack view and info panel).
local M = {}

local MAX_ITEMS = 8

---Segments for a `label: +a, +b, -c` line, or nil when there is nothing to show.
---@param indent string
---@param label string
---@param added string[]
---@param removed string[]
---@return (string|nil)[][]|nil
local function delta(indent, label, added, removed)
  local items = {}
  for _, name in ipairs(added or {}) do
    table.insert(items, { "+" .. name, "LensAdded" })
  end
  for _, name in ipairs(removed or {}) do
    table.insert(items, { "-" .. name, "LensDeleted" })
  end
  if #items == 0 then
    return nil
  end
  local segments = { { indent .. label .. ": ", "LensDim" } }
  for i = 1, math.min(#items, MAX_ITEMS) do
    if i > 1 then
      table.insert(segments, { ", " })
    end
    table.insert(segments, items[i])
  end
  if #items > MAX_ITEMS then
    table.insert(segments, { (" …+%d"):format(#items - MAX_ITEMS), "LensDim" })
  end
  return segments
end

---Lines (as segment lists) describing what changed inside a symbol.
---@param symbol table report symbol
---@param indent string
---@return (string|nil)[][][]
function M.lines(symbol, indent)
  local out = {}
  if symbol.change == "added" and symbol.signature then
    table.insert(out, { { indent .. "signature ", "LensDim" }, { symbol.signature } })
  end
  local summary = symbol.summary
  if summary then
    local texts = vim.tbl_map(function(c)
      return c.text
    end, summary.signature or {})
    if #texts > 0 then
      table.insert(out, {
        { indent .. "signature: ", "LensDim" },
        { table.concat(texts, " · "), "LensModified" },
      })
    end
    for _, row in ipairs({
      { "calls", summary.calls_added, summary.calls_removed },
      { "uses", summary.uses_added, summary.uses_removed },
      { "flow", summary.flow_added, summary.flow_removed },
    }) do
      local line = delta(indent, row[1], row[2], row[3])
      if line then
        table.insert(out, line)
      end
    end
  end
  if (symbol.broken_calls or 0) > 0 then
    local text = ("✗ %d call(s) no longer match the signature"):format(symbol.broken_calls)
    table.insert(out, { { indent .. text, "LensDangling" } })
  end
  return out
end

---Short marker and highlight for a call check.
---@param check table|nil
---@return string text, string group
function M.check(check)
  if not check then
    return "", "LensDim"
  end
  if check.status == "broken" then
    return "✗ " .. check.reason, "LensDangling"
  end
  if check.status == "unknown" then
    return "? " .. check.reason, "LensDim"
  end
  return "✓ matches new signature", "LensAdded"
end

return M
