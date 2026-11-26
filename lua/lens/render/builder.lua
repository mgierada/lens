---Accumulates buffer lines with highlights, virtual text and per-line items, then applies them.
---Renderers are pure: they only produce a Builder; `apply` is the only side effect.
---@class lens.Builder
---@field lines string[]
---@field highlights { row: integer, col: integer, end_col: integer, group: string, priority: integer|nil }[]
---@field line_highlights { row: integer, group: string }[]
---@field virtual { row: integer, chunks: string[][] }[]
---@field items table<integer, table> item per 1-based line number
---@field code lens.CodeBlock[] diff code, grouped into blocks that are syntax-highlighted together

---@class lens.CodeLine
---@field row integer 0-based buffer row
---@field kind "added"|"removed"|"context"
---@field text string code without the diff prefix

---@class lens.CodeBlock
---@field path string
---@field lines lens.CodeLine[]
local Builder = {}
Builder.__index = Builder

local M = {}

---@return lens.Builder
function M.new()
  return setmetatable(
    { lines = {}, highlights = {}, line_highlights = {}, virtual = {}, items = {}, code = {} },
    Builder
  )
end

---Append a line made of `{ text, group|nil }` segments.
---@param segments (string|nil)[][]
---@param item table|nil
---@return integer row 0-based row of the new line
function Builder:add(segments, item)
  local text, col = {}, 0
  local row = #self.lines
  for _, segment in ipairs(segments) do
    local chunk, group = segment[1], segment[2]
    if group and #chunk > 0 then
      table.insert(self.highlights, { row = row, col = col, end_col = col + #chunk, group = group })
    end
    table.insert(text, chunk)
    col = col + #chunk
  end
  table.insert(self.lines, table.concat(text))
  self.items[row + 1] = item
  return row
end

---Append an empty line.
function Builder:blank()
  self:add({ { "" } })
end

---Highlight a whole line (full window width).
---@param row integer
---@param group string
function Builder:line_highlight(row, group)
  table.insert(self.line_highlights, { row = row, group = group })
end

---Attach end-of-line virtual text.
---@param row integer
---@param chunks string[][]
function Builder:virt(row, chunks)
  table.insert(self.virtual, { row = row, chunks = chunks })
end

---Start a new block of diff code from `path` (e.g. one hunk or one expanded body).
---@param path string
function Builder:begin_code(path)
  table.insert(self.code, { path = path, lines = {} })
end

---Record that `row` shows `text` (without its one-character diff prefix) in the current block.
---@param row integer
---@param kind "added"|"removed"|"context"
---@param text string
function Builder:code_line(row, kind, text)
  local block = self.code[#self.code]
  if block then
    table.insert(block.lines, { row = row, kind = kind, text = text })
  end
end

---Write the content into a (non-modifiable) buffer.
---@param buf integer
---@param ns integer
function Builder:apply(buf, ns)
  vim.bo[buf].modifiable = true
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, self.lines)
  vim.bo[buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  for _, hl in ipairs(self.line_highlights) do
    vim.api.nvim_buf_set_extmark(buf, ns, hl.row, 0, { line_hl_group = hl.group })
  end
  for _, hl in ipairs(self.highlights) do
    local opts = { end_col = hl.end_col, hl_group = hl.group, priority = hl.priority }
    vim.api.nvim_buf_set_extmark(buf, ns, hl.row, hl.col, opts)
  end
  for _, v in ipairs(self.virtual) do
    vim.api.nvim_buf_set_extmark(buf, ns, v.row, 0, { virt_text = v.chunks, virt_text_pos = "eol" })
  end
end

return M
