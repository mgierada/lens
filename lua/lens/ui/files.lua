---Opening files at a revision: real files when the revision is checked out, read-only
---`lens://<sha>/<path>` buffers otherwise.
local git = require("lens.git")

local M = {}

local SHORT_SHA = 8

---Read-only buffer holding `path` at `rev`; reused when already open.
---@param repo string
---@param rev string
---@param path string
---@return integer|nil buf, string|nil err
function M.revision_buffer(repo, rev, path)
  local name = ("lens://%s/%s"):format(rev:sub(1, SHORT_SHA), path)
  local existing = vim.fn.bufnr(name)
  if existing ~= -1 then
    return existing, nil
  end
  local lines, err = git.show(repo, rev, path)
  if not lines then
    return nil, err
  end
  local buf = vim.api.nvim_create_buf(true, true)
  vim.api.nvim_buf_set_name(buf, name)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  vim.bo[buf].bufhidden = "hide"
  local filetype = vim.filetype.match({ filename = path, contents = lines })
  if filetype then
    vim.bo[buf].filetype = filetype
  end
  return buf, nil
end

---Empty placeholder for the missing side of an added or deleted file.
---@param label string
---@return integer
function M.empty_buffer(label)
  local buf = vim.api.nvim_create_buf(false, true)
  pcall(vim.api.nvim_buf_set_name, buf, "lens://" .. label)
  vim.bo[buf].modifiable = false
  vim.bo[buf].bufhidden = "wipe"
  return buf
end

---Clamp and set the cursor.
---@param win integer
---@param line integer
function M.set_cursor(win, line)
  local count = vim.api.nvim_buf_line_count(vim.api.nvim_win_get_buf(win))
  vim.api.nvim_win_set_cursor(win, { math.max(1, math.min(line, count)), 0 })
  vim.cmd("normal! zz")
end

---Open `path` at the head revision in the previous tab (or a new one), like Diffview's `gf`.
---@param ctx { repo: string, head_sha: string, head_checked_out: boolean }
---@param path string
---@param line integer
function M.open(ctx, path, line)
  local buf, err
  if ctx.head_checked_out then
    buf = vim.fn.bufadd(ctx.repo .. "/" .. path)
    vim.fn.bufload(buf)
  else
    buf, err = M.revision_buffer(ctx.repo, ctx.head_sha, path)
  end
  if not buf then
    vim.notify("lens: cannot open " .. path .. ": " .. (err or "?"), vim.log.levels.ERROR)
    return
  end
  if vim.fn.tabpagenr("$") > 1 then
    vim.cmd("tabprevious")
  else
    vim.cmd("tabnew")
  end
  vim.api.nvim_win_set_buf(0, buf)
  vim.bo[buf].buflisted = true
  M.set_cursor(0, line)
end

return M
