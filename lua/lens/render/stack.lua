---Main view: every hunk of one cluster, grouped by symbol in review order.
local builder = require("lens.render.builder")
local hl = require("lens.highlights")
local summary = require("lens.render.summary")
local syntax = require("lens.render.syntax")

local M = {}

local SHORT_SHA = 8
local PREFIX = { added = "+", removed = "-", context = " " }
local LINE_GROUP = { added = "LensAdd", removed = "LensDelete" }

---@class lens.StackState
---@field expanded table<integer, boolean> symbols shown as their full body
---@field viewed table<integer, boolean>

---@alias lens.ReadFile fun(path: string): string[]|nil

---@param model lens.Model
---@param cluster_id integer
---@param state lens.StackState
---@param icons table<string, string>
---@param read_file lens.ReadFile head-revision file contents, used for expanded symbols
---@return lens.Builder
function M.render(model, cluster_id, state, icons, read_file)
  local b = builder.new()
  local cluster = model:cluster(cluster_id)
  if not cluster then
    b:add({ { "No clusters: nothing to review.", "LensDim" } })
    return b
  end
  b:add({
    { ("[%d/%d] "):format(cluster.id + 1, model:cluster_count()), "LensDim" },
    { cluster.title, "LensTitle" },
  }, { kind = "cluster", cluster = cluster.id })
  if cluster.subtitle and cluster.subtitle ~= "" then
    b:add({ { "  " .. cluster.subtitle, "LensDim" } }, { kind = "cluster", cluster = cluster.id })
  end
  for _, sha in ipairs(cluster.commits) do
    local commit = model.commits_by_sha[sha]
    b:add({
      { "  " .. sha:sub(1, SHORT_SHA) .. " ", "LensCommit" },
      { commit and commit.summary or "", "LensDim" },
    }, { kind = "commit", sha = sha })
  end
  for _, id in ipairs(cluster.symbols) do
    b:blank()
    M.render_symbol(b, model, id, state, icons, read_file)
  end
  syntax.highlight(b)
  return b
end

---@param b lens.Builder
---@param model lens.Model
---@param id integer
---@param state lens.StackState
---@param icons table<string, string>
---@param read_file lens.ReadFile
function M.render_symbol(b, model, id, state, icons, read_file)
  local symbol = model:symbol(id)
  local item = { kind = "symbol", symbol = id, path = symbol.path, line = model:symbol_line(id) }
  b:add({
    { icons[symbol.change] .. " ", hl.for_change(symbol.change) },
    { model:symbol_name(id), state.viewed[id] and "LensViewed" or "LensSymbol" },
    { ("  %s %s  "):format(symbol.kind, symbol.change), "LensDim" },
    { ("%s:%d"):format(symbol.path, item.line), "LensPath" },
    { state.viewed[id] and (" " .. icons.viewed) or "", "LensAdded" },
  }, item)
  M.render_relations(b, model, id)
  local lines = state.expanded[id] and symbol.head and read_file(symbol.path)
  if lines then
    M.render_expanded(b, model, id, lines)
  else
    for _, hunk_id in ipairs(model.owned_hunks[id] or {}) do
      M.render_hunk(b, model, hunk_id, id)
    end
  end
end

---Calls, callers, move origin and impact summary under a symbol header.
---@param b lens.Builder
---@param model lens.Model
---@param id integer
function M.render_relations(b, model, id)
  local item = { kind = "relations", symbol = id }
  local symbol = model:symbol(id)
  if symbol.moved_from then
    local from = symbol.moved_from
    b:add({ { "    moved from ", "LensDim" }, { from.path .. ":" .. from.qualname, "LensPath" } }, item)
  end
  for _, segments in ipairs(summary.lines(symbol, "    ")) do
    b:add(segments, item)
  end
  for _, relation in ipairs(M.relations(model, id)) do
    b:add({ { "    " .. relation.label .. " ", "LensDim" }, { table.concat(relation.names, ", ") } }, item)
  end
  local impact = model.impact[id]
  if impact and #impact > 0 then
    local dangling = 0
    for _, entry in ipairs(impact) do
      dangling = dangling + (entry.dangling and 1 or 0)
    end
    local text = ("    ! %d unchanged referrer(s)"):format(#impact)
    local suffix = dangling > 0 and (" · %d dangling"):format(dangling) or ""
    b:add({ { text, "LensImpact" }, { suffix, "LensDangling" } }, item)
  end
end

---Relation lines for a symbol: code links (uses / used by), name mentions and shared tokens.
---@param model lens.Model
---@param id integer
---@return { label: string, names: string[] }[]
function M.relations(model, id)
  local groups = {
    { label = "→ uses", names = {} },
    { label = "← used by", names = {} },
    { label = "→ mentions", names = {} },
    { label = "← mentioned by", names = {} },
    { label = "≈ shares", names = {} },
  }
  local function name(edge, other)
    local text = model:symbol_name(other)
    if edge.kind == "shared" then
      return ("%s with %s"):format(edge.via or "?", text)
    end
    if edge.confidence == "low" and (edge.kind == "call" or edge.kind == "reference") then
      text = text .. "?"
    end
    if edge.call_check and edge.call_check.status == "broken" then
      text = text .. " ✗"
    end
    return text
  end
  local function add(edge, other, outgoing)
    local index = ({ call = 1, reference = 1, mention = 3, shared = 5 })[edge.kind] or 1
    if index ~= 5 and not outgoing then
      index = index + 1
    end
    table.insert(groups[index].names, name(edge, other))
  end
  for _, edge in ipairs(model.edges_out[id] or {}) do
    add(edge, edge.to, true)
  end
  for _, edge in ipairs(model.edges_in[id] or {}) do
    add(edge, edge.from, false)
  end
  return vim.tbl_filter(function(g)
    return #g.names > 0
  end, groups)
end

---@param b lens.Builder
---@param model lens.Model
---@param hunk_id integer
---@param symbol_id integer
function M.render_hunk(b, model, hunk_id, symbol_id)
  local hunk = model:hunk(hunk_id)
  local header = ("@@ -%d,%d +%d,%d @@ %s"):format(
    hunk.old_start,
    hunk.old_lines,
    hunk.new_start,
    hunk.new_lines,
    hunk.header
  )
  local hunk_item = { kind = "hunk", hunk = hunk_id, symbol = symbol_id, path = hunk.path }
  hunk_item.line = hunk.new_start
  local row = b:add({ { header, "LensHunkHeader" } }, hunk_item)
  if #hunk.commits > 0 then
    local shas = vim.tbl_map(function(sha)
      return sha:sub(1, SHORT_SHA)
    end, hunk.commits)
    b:virt(row, { { table.concat(shas, " "), "LensCommit" } })
  end
  b:begin_code(hunk.path)
  for _, line in ipairs(hunk.lines) do
    M.render_line(b, line.kind, line.text, {
      kind = "line",
      hunk = hunk_id,
      symbol = symbol_id,
      path = hunk.path,
      line = line.new_line or hunk.new_start,
    })
  end
end

---@param b lens.Builder
---@param kind string
---@param text string
---@param item table
function M.render_line(b, kind, text, item)
  local row = b:add({ { PREFIX[kind] .. text } }, item)
  b:code_line(row, kind, text)
  if LINE_GROUP[kind] then
    b:line_highlight(row, LINE_GROUP[kind])
  end
end

---Full head-side body of a symbol with additions marked and removals interleaved.
---@param b lens.Builder
---@param model lens.Model
---@param id integer
---@param file_lines string[]
function M.render_expanded(b, model, id, file_lines)
  local symbol = model:symbol(id)
  local marks = M.line_marks(model, symbol.path)
  b:begin_code(symbol.path)
  local first, last = symbol.head.start, symbol.head["end"]
  b:add({ { ("@@ %s (full body) @@"):format(symbol.qualname), "LensHunkHeader" } }, {
    kind = "hunk",
    symbol = id,
    path = symbol.path,
    line = first,
  })
  for n = first, last + 1 do
    for _, removed in ipairs(marks.removed_before[n] or {}) do
      M.render_line(b, "removed", removed, { kind = "line", symbol = id, path = symbol.path, line = n })
    end
    if n <= last and file_lines[n] then
      local kind = marks.added[n] and "added" or "context"
      M.render_line(b, kind, file_lines[n], { kind = "line", symbol = id, path = symbol.path, line = n })
    end
  end
end

---Added head lines and removed lines keyed by the head line they precede, for one file.
---@param model lens.Model
---@param path string
---@return { added: table<integer, boolean>, removed_before: table<integer, string[]> }
function M.line_marks(model, path)
  local added, removed_before = {}, {}
  for _, hunk_id in ipairs(model.hunks_by_path[path] or {}) do
    local hunk = model:hunk(hunk_id)
    local pending = {}
    for _, line in ipairs(hunk.lines) do
      if line.kind == "removed" then
        table.insert(pending, line.text)
      else
        if #pending > 0 then
          removed_before[line.new_line] = pending
          pending = {}
        end
        added[line.new_line] = line.kind == "added" or nil
      end
    end
    if #pending > 0 then
      removed_before[hunk.new_start + hunk.new_lines] = pending
    end
  end
  return { added = added, removed_before = removed_before }
end

return M
