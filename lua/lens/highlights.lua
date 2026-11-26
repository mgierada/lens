---Highlight groups, all linked to standard groups so colorschemes apply.
local M = {}

M.groups = {
  LensTitle = "Title",
  LensDim = "Comment",
  LensPath = "Directory",
  LensCommit = "Constant",
  LensHunkHeader = "DiffText",
  LensAdd = "DiffAdd",
  LensDelete = "DiffDelete",
  LensAdded = "DiagnosticOk",
  LensModified = "DiagnosticWarn",
  LensDeleted = "DiagnosticError",
  LensMoved = "DiagnosticInfo",
  LensSymbol = "Function",
  LensCurrent = "CursorLine",
  LensViewed = "Comment",
  LensImpact = "DiagnosticHint",
  LensDangling = "DiagnosticError",
}

---Highlight group for a change kind.
---@param change string
---@return string
function M.for_change(change)
  return ({
    added = "LensAdded",
    modified = "LensModified",
    deleted = "LensDeleted",
    moved = "LensMoved",
  })[change] or "LensDim"
end

function M.setup()
  for group, link in pairs(M.groups) do
    vim.api.nvim_set_hl(0, group, { link = link, default = true })
  end
end

return M
