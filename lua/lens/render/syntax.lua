---Tree-sitter highlighting of diff code in a builder.
---
---Each code block is parsed twice: the new side (context + added lines) and the old side
---(context + removed lines), so a parse never mixes both versions. Context lines take their
---highlights from the new side. Captures are mapped back to buffer rows, shifted past the diff
---prefix column.
local M = {}

local PREFIX_WIDTH = 1
local DEFAULT_PRIORITY = 100
local SKIP_CAPTURES = { spell = true, nospell = true, conceal = true }

---@type table<string, string|false>
local lang_cache = {}

---Tree-sitter language for a path, or nil when no parser is installed.
---@param path string
---@return string|nil
function M.lang_for(path)
  local ext = path:match("%.([^./]+)$") or path
  if lang_cache[ext] == nil then
    local filetype = vim.filetype.match({ filename = path })
    local lang = filetype and vim.treesitter.language.get_lang(filetype)
    local ok = lang and pcall(vim.treesitter.language.inspect, lang)
    if lang and not ok then
      ok = pcall(vim.treesitter.language.add, lang) and pcall(vim.treesitter.language.inspect, lang)
    end
    lang_cache[ext] = ok and lang or false
  end
  return lang_cache[ext] or nil
end

---@param metadata table
---@param capture integer
---@return integer
local function priority(metadata, capture)
  local value = metadata.priority or (metadata[capture] and metadata[capture].priority)
  return tonumber(value) or DEFAULT_PRIORITY
end

---Parse `lines` as one snippet and add highlights for lines with `apply = true`.
---@param b lens.Builder
---@param lang string
---@param lines { row: integer, text: string, apply: boolean }[]
local function highlight_snippet(b, lang, lines)
  if #lines == 0 then
    return
  end
  local text = table.concat(vim.tbl_map(function(l) return l.text end, lines), "\n")
  local ok, parser = pcall(vim.treesitter.get_string_parser, text, lang)
  if not ok or not pcall(parser.parse, parser, true) then
    return
  end
  parser:for_each_tree(function(tree, ltree)
    local query = vim.treesitter.query.get(ltree:lang(), "highlights")
    if not query then
      return
    end
    for capture, node, metadata in query:iter_captures(tree:root(), text) do
      local name = query.captures[capture]
      if not SKIP_CAPTURES[name] and name:sub(1, 1) ~= "_" then
        local group = "@" .. name .. "." .. ltree:lang()
        local start_row, start_col, end_row, end_col = node:range()
        for r = start_row, end_row do
          local line = lines[r + 1]
          if line and line.apply then
            local from = r == start_row and start_col or 0
            local to = r == end_row and end_col or #line.text
            if to > from then
              table.insert(b.highlights, {
                row = line.row,
                col = from + PREFIX_WIDTH,
                end_col = to + PREFIX_WIDTH,
                group = group,
                priority = priority(metadata, capture),
              })
            end
          end
        end
      end
    end
  end)
end

---Add syntax highlights for every code block recorded in the builder.
---@param b lens.Builder
function M.highlight(b)
  for _, block in ipairs(b.code) do
    local lang = M.lang_for(block.path)
    if lang then
      local new_side, old_side = {}, {}
      for _, line in ipairs(block.lines) do
        if line.kind ~= "removed" then
          table.insert(new_side, { row = line.row, text = line.text, apply = true })
        end
        if line.kind ~= "added" then
          table.insert(old_side, { row = line.row, text = line.text, apply = line.kind == "removed" })
        end
      end
      highlight_snippet(b, lang, new_side)
      highlight_snippet(b, lang, old_side)
    end
  end
end

return M
