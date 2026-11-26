---@class lens.Keymaps
---@field select string
---@field next_cluster string
---@field prev_cluster string
---@field next_hunk string
---@field prev_hunk string
---@field next_symbol string
---@field prev_symbol string
---@field open_diff string
---@field open_file string
---@field hover string
---@field expand string
---@field toggle_info string
---@field toggle_viewed string
---@field focus string
---@field refresh string
---@field close string
---@field graph string

---@class lens.Config
---@field cmd string|nil Path to the `lens` binary; auto-detected when nil.
---@field base string|nil Default base revision; auto-detected (origin/HEAD, main, master) when nil.
---@field args string[] Extra CLI arguments, e.g. { "--no-blame" }.
---@field panel { width: integer }
---@field info { height: integer, enabled: boolean }
---@field diff_tool "auto"|"diffview"|"builtin"
---@field icons table<string, string>
---@field keymaps lens.Keymaps

local M = {}

---@type lens.Config
M.defaults = {
  cmd = nil,
  base = nil,
  args = {},
  panel = { width = 42 },
  info = { height = 12, enabled = true },
  diff_tool = "auto",
  icons = {
    added = "+",
    modified = "~",
    deleted = "-",
    moved = ">",
    expanded = "▾",
    collapsed = "▸",
    viewed = "✓",
  },
  keymaps = {
    select = "<CR>",
    next_cluster = "]c",
    prev_cluster = "[c",
    next_hunk = "]h",
    prev_hunk = "[h",
    next_symbol = "]s",
    prev_symbol = "[s",
    open_diff = "gd",
    open_file = "gf",
    hover = "K",
    expand = "e",
    toggle_info = "i",
    toggle_viewed = "x",
    focus = "<Tab>",
    refresh = "R",
    close = "q",
    graph = "gG",
  },
}

---@type lens.Config
M.options = vim.deepcopy(M.defaults)

---Merge user options over the defaults.
---@param opts table|nil
function M.setup(opts)
  M.options = vim.tbl_deep_extend("force", vim.deepcopy(M.defaults), opts or {})
end

return M
