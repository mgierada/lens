---Review session: tab layout (panel | stack / info), state and user actions.
local config = require("lens.config")
local diff = require("lens.ui.diff")
local files = require("lens.ui.files")
local git = require("lens.git")
local info = require("lens.render.info")
local model_mod = require("lens.model")
local panel = require("lens.render.panel")
local stack = require("lens.render.stack")

local api = vim.api
local M = {}
local ns = api.nvim_create_namespace("lens")
local group = api.nvim_create_augroup("lens", { clear = true })

---@class lens.Context
---@field repo string
---@field range string
---@field merge_base string
---@field head_sha string
---@field head_checked_out boolean

---@class lens.Session
---@field model lens.Model
---@field ctx lens.Context
---@field tab integer
---@field bufs table<string, integer>
---@field wins table<string, integer>
---@field items table<string, table<integer, table>>
---@field current integer cluster shown in the stack view
---@field expanded_clusters table<integer, boolean>
---@field expanded_symbols table<string, boolean> keyed by stable symbol key
---@field viewed table<string, boolean> keyed by stable symbol key, survives refresh
---@field info_symbol integer|nil
---@field file_cache table<string, string[]|false>
---@field json string|nil raw report JSON from the CLI

---@type lens.Session|nil
M.session = nil

---@param report table
---@param repo string
---@param range string
---@return lens.Context
function M.context(report, repo, range)
  return {
    repo = repo,
    range = range,
    merge_base = report.merge_base,
    head_sha = report.head.sha,
    head_checked_out = git.resolve(repo, "HEAD") == report.head.sha,
  }
end

---Identity of a symbol that is stable across re-analysis.
---@param symbol table
---@return string
local function stable_key(symbol)
  return symbol.path .. "::" .. symbol.qualname
end

---Translate a stable-key set into a set of current symbol ids.
---@param s lens.Session
---@param keys table<string, boolean>
---@return table<integer, boolean>
local function ids_from_keys(s, keys)
  local ids = {}
  for _, symbol in ipairs(s.model.report.symbols) do
    ids[symbol.id] = keys[stable_key(symbol)] or nil
  end
  return ids
end

-- Layout -------------------------------------------------------------------------------------

---@param name string
---@param filetype string
---@return integer
local function make_buffer(name, filetype)
  local buf = api.nvim_create_buf(false, true)
  pcall(api.nvim_buf_set_name, buf, "lens://" .. name)
  vim.bo[buf].filetype = filetype
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].swapfile = false
  vim.bo[buf].modifiable = false
  return buf
end

---@param win integer
---@param cursorline boolean
local function style_window(win, cursorline)
  local wo = vim.wo[win]
  wo.number = false
  wo.relativenumber = false
  wo.signcolumn = "no"
  wo.foldcolumn = "0"
  wo.wrap = false
  wo.list = false
  wo.spell = false
  wo.cursorline = cursorline
end

---@param s lens.Session
local function open_info_window(s)
  local height = config.options.info.height
  s.wins.info = api.nvim_open_win(s.bufs.info, false, { split = "below", win = s.wins.stack, height = height })
  style_window(s.wins.info, true)
  vim.wo[s.wins.info].winfixheight = true
end

---@param s lens.Session
local function create_layout(s)
  vim.cmd("tabnew")
  s.tab = api.nvim_get_current_tabpage()
  local placeholder = api.nvim_get_current_buf()
  s.bufs = {
    panel = make_buffer("panel", "lens_panel"),
    stack = make_buffer("stack", "lens_stack"),
    info = make_buffer("info", "lens_info"),
  }
  s.wins = { stack = api.nvim_get_current_win() }
  api.nvim_win_set_buf(s.wins.stack, s.bufs.stack)
  style_window(s.wins.stack, true)
  local width = config.options.panel.width
  s.wins.panel = api.nvim_open_win(s.bufs.panel, false, { split = "left", win = s.wins.stack, width = width })
  style_window(s.wins.panel, true)
  vim.wo[s.wins.panel].winfixwidth = true
  if config.options.info.enabled then
    open_info_window(s)
  end
  if api.nvim_buf_is_valid(placeholder) then
    api.nvim_buf_delete(placeholder, { force = true })
  end
end

---@param s lens.Session
---@param name string
---@return boolean
local function window_open(s, name)
  return s.wins[name] ~= nil and api.nvim_win_is_valid(s.wins[name])
end

-- Rendering ----------------------------------------------------------------------------------

---Apply a builder, keeping the cursor line of windows showing the buffer.
---@param s lens.Session
---@param name string
---@param built lens.Builder
local function apply(s, name, built)
  local cursor = window_open(s, name) and api.nvim_win_get_cursor(s.wins[name]) or nil
  built:apply(s.bufs[name], ns)
  s.items[name] = built.items
  if cursor then
    local line = math.min(cursor[1], math.max(1, #built.lines))
    api.nvim_win_set_cursor(s.wins[name], { line, 0 })
  end
end

---@param s lens.Session
---@param path string
---@return string[]|nil
local function read_head_file(s, path)
  if s.file_cache[path] == nil then
    s.file_cache[path] = git.show(s.ctx.repo, s.ctx.head_sha, path) or false
  end
  return s.file_cache[path] or nil
end

function M.render_panel()
  local s = assert(M.session)
  local state = { current = s.current, expanded = s.expanded_clusters, viewed = ids_from_keys(s, s.viewed) }
  apply(s, "panel", panel.render(s.model, state, config.options.icons))
end

function M.render_stack()
  local s = assert(M.session)
  local state = { expanded = ids_from_keys(s, s.expanded_symbols), viewed = ids_from_keys(s, s.viewed) }
  local read_file = function(path)
    return read_head_file(s, path)
  end
  apply(s, "stack", stack.render(s.model, s.current, state, config.options.icons, read_file))
end

---@param id integer|nil
function M.render_info(id)
  local s = assert(M.session)
  s.info_symbol = id
  if window_open(s, "info") then
    apply(s, "info", info.render(s.model, id))
  end
end

-- Cursor helpers -----------------------------------------------------------------------------

---Name of the lens window holding the cursor, if any.
---@param s lens.Session
---@return string|nil
local function focused(s)
  local win = api.nvim_get_current_win()
  for name, w in pairs(s.wins) do
    if w == win then
      return name
    end
  end
  return nil
end

---Item under the cursor in the focused lens window.
---@return table|nil item, string|nil window
local function current_item()
  local s = M.session
  local name = s and focused(s)
  if not s or not name then
    return nil, nil
  end
  local line = api.nvim_win_get_cursor(s.wins[name])[1]
  return s.items[name] and s.items[name][line], name
end

---Move the stack cursor to the header of `symbol`.
---@param s lens.Session
---@param symbol integer
local function focus_symbol(s, symbol)
  for line, item in pairs(s.items.stack or {}) do
    if item.kind == "symbol" and item.symbol == symbol then
      api.nvim_win_set_cursor(s.wins.stack, { line, 0 })
      api.nvim_win_call(s.wins.stack, function()
        vim.cmd("normal! zt")
      end)
      return
    end
  end
end

-- Actions ------------------------------------------------------------------------------------

---Show a cluster in the stack view, optionally jumping to one of its symbols.
---@param id integer
---@param symbol integer|nil
function M.show_cluster(id, symbol)
  local s = assert(M.session)
  local count = s.model:cluster_count()
  if count == 0 then
    M.render_stack()
    M.render_panel()
    return
  end
  s.current = math.max(0, math.min(id, count - 1))
  s.expanded_clusters[s.current] = true
  M.render_stack()
  M.render_panel()
  local cluster = s.model:cluster(s.current)
  local target = symbol or (cluster and cluster.symbols[1])
  api.nvim_win_set_cursor(s.wins.stack, { 1, 0 })
  if symbol then
    focus_symbol(s, symbol)
  end
  M.render_info(target)
end

---Next/previous cluster.
---@param step integer
function M.step_cluster(step)
  local s = assert(M.session)
  M.show_cluster(s.current + step)
end

---Move to the next/previous stack line whose item has `kind`.
---@param kind string
---@param step integer
function M.jump(kind, step)
  local s = assert(M.session)
  api.nvim_set_current_win(s.wins.stack)
  local line = api.nvim_win_get_cursor(s.wins.stack)[1] + step
  local last = api.nvim_buf_line_count(s.bufs.stack)
  while line >= 1 and line <= last do
    local item = s.items.stack[line]
    if item and item.kind == kind then
      api.nvim_win_set_cursor(s.wins.stack, { line, 0 })
      return
    end
    line = line + step
  end
end

---Path and head line the item under the cursor refers to.
---@return string|nil path, integer line
local function item_location()
  local s = assert(M.session)
  local item = current_item()
  if not item then
    return nil, 1
  end
  if item.path then
    return item.path, item.line or 1
  end
  if item.symbol then
    return s.model:symbol(item.symbol).path, s.model:symbol_line(item.symbol)
  end
  return nil, 1
end

function M.open_diff()
  local s = assert(M.session)
  local path, line = item_location()
  local file = path and s.model.files_by_path[path]
  if not file then
    vim.notify("lens: nothing to diff here", vim.log.levels.INFO)
    return
  end
  diff.open(s.ctx, file, line, config.options.diff_tool)
end

function M.open_file()
  local s = assert(M.session)
  local path, line = item_location()
  if not path then
    return
  end
  files.open(s.ctx, path, line)
end

---Toggle showing a symbol's full body instead of its hunks.
function M.toggle_expand()
  local s = assert(M.session)
  local item = current_item()
  if not item or not item.symbol then
    return
  end
  local symbol = s.model:symbol(item.symbol)
  if not symbol.head then
    vim.notify("lens: deleted symbols have no head body to expand", vim.log.levels.INFO)
    return
  end
  local key = stable_key(symbol)
  s.expanded_symbols[key] = not s.expanded_symbols[key] or nil
  M.render_stack()
  focus_symbol(s, item.symbol)
end

---Toggle viewed on a symbol, or on every symbol of a cluster.
function M.toggle_viewed()
  local s = assert(M.session)
  local item = current_item()
  if not item then
    return
  end
  local ids = {}
  if item.kind == "cluster" then
    ids = s.model:cluster(item.cluster).symbols
  elseif item.symbol then
    ids = { item.symbol }
  end
  local all_viewed = #ids > 0
  for _, id in ipairs(ids) do
    all_viewed = all_viewed and s.viewed[stable_key(s.model:symbol(id))] == true
  end
  for _, id in ipairs(ids) do
    s.viewed[stable_key(s.model:symbol(id))] = not all_viewed or nil
  end
  M.render_panel()
  M.render_stack()
end

function M.hover()
  local s = assert(M.session)
  local item = current_item()
  if not item or not item.symbol then
    return
  end
  local built = info.render(s.model, item.symbol)
  local buf = api.nvim_create_buf(false, true)
  built:apply(buf, ns)
  vim.bo[buf].bufhidden = "wipe"
  local width = 20
  for _, line in ipairs(built.lines) do
    width = math.max(width, vim.fn.strdisplaywidth(line))
  end
  local win = api.nvim_open_win(buf, false, {
    relative = "cursor",
    row = 1,
    col = 0,
    width = math.min(width, math.floor(vim.o.columns * 0.8)),
    height = math.min(#built.lines, math.floor(vim.o.lines * 0.6)),
    border = "rounded",
    style = "minimal",
  })
  api.nvim_create_autocmd({ "CursorMoved", "BufLeave", "WinLeave" }, {
    group = group,
    once = true,
    callback = function()
      if api.nvim_win_is_valid(win) then
        api.nvim_win_close(win, true)
      end
    end,
  })
end

function M.toggle_info()
  local s = assert(M.session)
  if window_open(s, "info") then
    api.nvim_win_close(s.wins.info, true)
    s.wins.info = nil
    return
  end
  -- The info buffer is wiped with its window; recreate it.
  if not api.nvim_buf_is_valid(s.bufs.info) then
    s.bufs.info = make_buffer("info", "lens_info")
    M.set_keymaps(s.bufs.info, "info")
  end
  open_info_window(s)
  M.render_info(s.info_symbol)
end

---Cycle focus panel → stack → info.
function M.cycle_focus()
  local s = assert(M.session)
  local order = { "panel", "stack", "info" }
  local name = focused(s) or "info"
  local start = vim.fn.index(order, name) + 1
  for offset = 1, #order do
    local next_name = order[(start + offset - 1) % #order + 1]
    if window_open(s, next_name) then
      api.nvim_set_current_win(s.wins[next_name])
      return
    end
  end
end

---`select` key: context dependent.
function M.select()
  local s = assert(M.session)
  local item, window = current_item()
  if not item then
    return
  end
  if item.kind == "cluster" and window == "panel" then
    if s.current == item.cluster then
      s.expanded_clusters[item.cluster] = not s.expanded_clusters[item.cluster] or nil
      M.render_panel()
    else
      M.show_cluster(item.cluster)
    end
  elseif item.kind == "file" then
    M.open_diff()
  elseif item.kind == "impact" then
    M.open_file()
  elseif item.kind == "symbol" and window ~= "stack" then
    M.show_cluster(s.model.cluster_of[item.symbol], item.symbol)
    api.nvim_set_current_win(s.wins.stack)
  elseif item.kind == "symbol" then
    M.toggle_expand()
  elseif item.kind == "hunk" or item.kind == "line" then
    M.open_diff()
  end
end

---Open the interactive graph in the browser, focused on the current cluster.
function M.open_graph()
  local s = assert(M.session)
  require("lens.graph").open(s.json or vim.json.encode(s.model.report), s.current)
end

---Replace the report (after a refresh), keeping viewed marks and the current position.
---@param report table
function M.replace(report, json)
  local s = assert(M.session)
  s.json = json
  s.model = model_mod.new(report)
  s.ctx = M.context(report, s.ctx.repo, s.ctx.range)
  s.expanded_clusters = {}
  s.file_cache = {}
  M.show_cluster(s.current)
end

function M.close()
  local s = M.session
  M.session = nil
  if not s or not api.nvim_tabpage_is_valid(s.tab) then
    return
  end
  if #api.nvim_list_tabpages() == 1 then
    vim.cmd("tabnew")
  end
  api.nvim_set_current_tabpage(s.tab)
  vim.cmd("tabclose")
end

-- Wiring -------------------------------------------------------------------------------------

---@param buf integer
---@param window string
function M.set_keymaps(buf, window)
  local keys = config.options.keymaps
  local actions = {
    { keys.select, M.select, "Select" },
    { keys.next_cluster, function() M.step_cluster(1) end, "Next cluster" },
    { keys.prev_cluster, function() M.step_cluster(-1) end, "Previous cluster" },
    { keys.next_hunk, function() M.jump("hunk", 1) end, "Next hunk" },
    { keys.prev_hunk, function() M.jump("hunk", -1) end, "Previous hunk" },
    { keys.next_symbol, function() M.jump("symbol", 1) end, "Next symbol" },
    { keys.prev_symbol, function() M.jump("symbol", -1) end, "Previous symbol" },
    { keys.open_diff, M.open_diff, "Side-by-side diff" },
    { keys.open_file, M.open_file, "Open file at head" },
    { keys.hover, M.hover, "Symbol details" },
    { keys.expand, M.toggle_expand, "Expand symbol body" },
    { keys.toggle_info, M.toggle_info, "Toggle info panel" },
    { keys.toggle_viewed, M.toggle_viewed, "Toggle viewed" },
    { keys.focus, M.cycle_focus, "Cycle focus" },
    { keys.refresh, function() require("lens").refresh() end, "Refresh analysis" },
    { keys.close, M.close, "Close lens" },
    { keys.graph, M.open_graph, "Open graph in browser" },
  }
  for _, action in ipairs(actions) do
    if action[1] and action[1] ~= "" then
      vim.keymap.set("n", action[1], action[2], { buffer = buf, nowait = true, desc = "lens: " .. action[3] })
    end
  end
  vim.b[buf].lens_window = window
end

---@param s lens.Session
local function set_autocmds(s)
  api.nvim_clear_autocmds({ group = group })
  for _, name in ipairs({ "panel", "stack" }) do
    api.nvim_create_autocmd("CursorMoved", {
      group = group,
      buffer = s.bufs[name],
      callback = function()
        local item = current_item()
        if item and item.symbol and item.symbol ~= s.info_symbol then
          M.render_info(item.symbol)
        end
      end,
    })
  end
  api.nvim_create_autocmd("TabClosed", {
    group = group,
    callback = function()
      if M.session == s and not api.nvim_tabpage_is_valid(s.tab) then
        M.session = nil
      end
    end,
  })
end

---Open a review tab for a report.
---@param report table
---@param ctx lens.Context
function M.open(report, ctx, json)
  M.close()
  ---@type lens.Session
  local s = {
    model = model_mod.new(report),
    ctx = ctx,
    tab = 0,
    bufs = {},
    wins = {},
    items = {},
    current = 0,
    expanded_clusters = {},
    expanded_symbols = {},
    viewed = {},
    info_symbol = nil,
    file_cache = {},
    json = json,
  }
  M.session = s
  create_layout(s)
  for name, buf in pairs(s.bufs) do
    M.set_keymaps(buf, name)
  end
  set_autocmds(s)
  M.show_cluster(0)
  api.nvim_set_current_win(s.wins.panel)
end

return M
