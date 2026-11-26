---Interactive graph page: fills the CLI's HTML template (assets/graph.html) with the report and
---opens it in the browser.
local M = {}

---@return string
local function template_path()
  local source = debug.getinfo(1, "S").source:sub(2)
  return vim.fn.fnamemodify(source, ":p:h:h:h") .. "/assets/graph.html"
end

---Replace the first occurrence of `needle` literally.
---@param text string
---@param needle string
---@param value string
---@return string
local function replace(text, needle, value)
  local start, finish = text:find(needle, 1, true)
  if not start then
    return text
  end
  return text:sub(1, start - 1) .. value .. text:sub(finish + 1)
end

---Page HTML for a report given as raw JSON (as produced by the CLI).
---`<` is escaped so code in the report cannot end the script element.
---@param json string
---@param focus integer|nil cluster to focus
---@return string|nil html, string|nil err
function M.page(json, focus)
  local file = io.open(template_path(), "r")
  if not file then
    return nil, "graph template not found at " .. template_path()
  end
  local template = file:read("*a")
  file:close()
  local html = replace(template, "/*LENS_REPORT*/null", (json:gsub("<", "\\u003c")))
  return replace(html, "/*LENS_FOCUS*/null", focus and tostring(focus) or "null"), nil
end

---Write the page to a temporary file and open it with the system handler.
---@param json string
---@param focus integer|nil
function M.open(json, focus)
  local html, err = M.page(json, focus)
  if not html then
    vim.notify("lens: " .. err, vim.log.levels.ERROR)
    return
  end
  local path = vim.fn.tempname() .. ".html"
  vim.fn.writefile(vim.split(html, "\n", { plain = true }), path)
  local _, open_err = vim.ui.open(path)
  if open_err then
    vim.notify("lens: could not open browser: " .. open_err .. " (page: " .. path .. ")", vim.log.levels.ERROR)
  end
end

return M
