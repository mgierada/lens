---Indexed view over a decoded report. Ids are the report's 0-based indices.
---@class lens.Model
---@field report table
---@field edges_out table<integer, table[]>
---@field edges_in table<integer, table[]>
---@field impact table<integer, table[]>
---@field cluster_of table<integer, integer>
---@field owned_hunks table<integer, integer[]> hunks shown under each symbol
---@field files_by_path table<string, table>
---@field hunks_by_path table<string, integer[]>
---@field commits_by_sha table<string, table>
local Model = {}
Model.__index = Model

local M = {}

---@param report table
---@return lens.Model
function M.new(report)
  local self = setmetatable({
    report = report,
    edges_out = {},
    edges_in = {},
    impact = {},
    cluster_of = {},
    owned_hunks = {},
    files_by_path = {},
    hunks_by_path = {},
    commits_by_sha = {},
  }, Model)
  self:_index_edges()
  self:_index_clusters()
  for _, file in ipairs(report.files) do
    self.files_by_path[file.path] = file
  end
  for _, hunk in ipairs(report.hunks) do
    self.hunks_by_path[hunk.path] = self.hunks_by_path[hunk.path] or {}
    table.insert(self.hunks_by_path[hunk.path], hunk.id)
  end
  for _, commit in ipairs(report.commits) do
    self.commits_by_sha[commit.sha] = commit
  end
  return self
end

function Model:_index_edges()
  for _, edge in ipairs(self.report.edges) do
    self.edges_out[edge.from] = self.edges_out[edge.from] or {}
    self.edges_in[edge.to] = self.edges_in[edge.to] or {}
    table.insert(self.edges_out[edge.from], edge)
    table.insert(self.edges_in[edge.to], edge)
  end
end

---Each hunk is shown once per cluster, under the first symbol (in review order) it touches.
function Model:_index_clusters()
  for _, cluster in ipairs(self.report.clusters) do
    local order = {}
    for position, id in ipairs(cluster.symbols) do
      self.cluster_of[id] = cluster.id
      order[id] = position
      self.owned_hunks[id] = {}
    end
    for _, hunk_id in ipairs(cluster.hunks) do
      local owner, best = cluster.symbols[1], math.huge
      for _, symbol in ipairs(self:hunk(hunk_id).symbols) do
        if order[symbol] and order[symbol] < best then
          owner, best = symbol, order[symbol]
        end
      end
      table.insert(self.owned_hunks[owner], hunk_id)
    end
    for _, entry in ipairs(cluster.impact) do
      self.impact[entry.target] = self.impact[entry.target] or {}
      table.insert(self.impact[entry.target], entry)
    end
  end
end

---@param id integer
---@return table
function Model:symbol(id)
  return self.report.symbols[id + 1]
end

---@param id integer
---@return table
function Model:hunk(id)
  return self.report.hunks[id + 1]
end

---@param id integer
---@return table|nil
function Model:cluster(id)
  return self.report.clusters[id + 1]
end

---@return integer
function Model:cluster_count()
  return #self.report.clusters
end

---Display name of a symbol (module-level changes include their file).
---@param id integer
---@return string
function Model:symbol_name(id)
  local symbol = self:symbol(id)
  if symbol.kind == "module" then
    return vim.fn.fnamemodify(symbol.path, ":t") .. " <module>"
  end
  if symbol.kind == "file" then
    return vim.fn.fnamemodify(symbol.path, ":t")
  end
  return symbol.qualname
end

---First head-side (or base-side) line of a symbol.
---@param id integer
---@return integer
function Model:symbol_line(id)
  local symbol = self:symbol(id)
  local range = symbol.head or symbol.base
  if range then
    return range.start
  end
  local hunk = self.owned_hunks[id] and self.owned_hunks[id][1]
  return hunk and self:hunk(hunk).new_start or 1
end

return M
