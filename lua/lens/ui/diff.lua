---Side-by-side diff of one file: Diffview when available, built-in `diffthis` otherwise.
local files = require("lens.ui.files")

local M = {}

---@param tool "auto"|"diffview"|"builtin"
---@return boolean
local function use_diffview(tool)
  if tool == "builtin" then
    return false
  end
  local available = vim.fn.exists(":DiffviewOpen") == 2
  if tool == "diffview" and not available then
    vim.notify("lens: diffview.nvim not found, using built-in diff", vim.log.levels.WARN)
  end
  return available
end

---Buffer for one side, or an empty placeholder when the file does not exist there.
---@param ctx table
---@param rev string
---@param path string|nil
---@param label string
---@return integer
local function side(ctx, rev, path, label)
  if not path then
    return files.empty_buffer(label)
  end
  local buf = files.revision_buffer(ctx.repo, rev, path)
  return buf or files.empty_buffer(label)
end

---@param ctx { repo: string, merge_base: string, head_sha: string }
---@param file table report file entry
---@param line integer head-side line to focus
---@param tool "auto"|"diffview"|"builtin"
function M.open(ctx, file, line, tool)
  if use_diffview(tool) then
    local range = ctx.merge_base .. ".." .. ctx.head_sha
    vim.cmd(("DiffviewOpen %s -- %s"):format(range, vim.fn.fnameescape(file.path)))
    return
  end
  local old_path = file.status ~= "added" and (file.old_path or file.path) or nil
  local new_path = file.status ~= "deleted" and file.path or nil
  vim.cmd("tabnew")
  local placeholder = vim.api.nvim_get_current_buf()
  local right = vim.api.nvim_get_current_win()
  vim.api.nvim_win_set_buf(right, side(ctx, ctx.head_sha, new_path, "deleted/" .. file.path))
  local left_buf = side(ctx, ctx.merge_base, old_path, "added/" .. file.path)
  local left = vim.api.nvim_open_win(left_buf, false, { split = "left", win = right })
  for _, win in ipairs({ left, right }) do
    vim.api.nvim_win_call(win, function()
      vim.cmd("diffthis")
    end)
    local buf = vim.api.nvim_win_get_buf(win)
    vim.keymap.set("n", "q", "<cmd>tabclose<cr>", { buffer = buf, desc = "Close lens diff" })
  end
  if vim.api.nvim_buf_is_valid(placeholder) then
    vim.api.nvim_buf_delete(placeholder, { force = true })
  end
  vim.api.nvim_set_current_win(right)
  files.set_cursor(right, line)
end

return M
