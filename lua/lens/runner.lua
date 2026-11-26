---Runs the `lens` CLI and decodes its JSON report.
local M = {}

---Directory of this plugin checkout (two levels above lua/lens/).
---@return string
local function plugin_root()
  local source = debug.getinfo(1, "S").source:sub(2)
  return vim.fn.fnamemodify(source, ":p:h:h:h")
end

---Locate the binary: configured path, this checkout's release build, then PATH.
---A plugin-local build wins over PATH because unrelated tools are also called `lens`.
---@param cmd string|nil
---@return string|nil
function M.find_binary(cmd)
  if cmd then
    return cmd
  end
  local built = plugin_root() .. "/target/release/lens"
  if vim.uv.fs_stat(built) then
    return built
  end
  if vim.fn.executable("lens") == 1 then
    return "lens"
  end
  return nil
end

---CLI arguments for an analysis.
---@param binary string
---@param range string e.g. "main..feature" or "main"
---@param repo string
---@param extra string[]
---@return string[]
function M.command(binary, range, repo, extra)
  local cmd = { binary, "analyze", range, "--repo", repo, "--format", "json" }
  return vim.list_extend(cmd, extra or {})
end

---Decode a report, mapping JSON null to nil.
---@param json string
---@return table|nil report, string|nil err
function M.decode(json)
  local ok, report = pcall(vim.json.decode, json, { luanil = { object = true, array = true } })
  if not ok then
    return nil, "invalid JSON from lens: " .. tostring(report)
  end
  return report, nil
end

---Run an analysis asynchronously; `callback(err, report, json)` runs on the main loop.
---`json` is the raw output, kept for the graph page (re-encoding would turn `[]` into `{}`).
---@param cmd string[]
---@param callback fun(err: string|nil, report: table|nil, json: string|nil)
function M.run(cmd, callback)
  vim.system(cmd, { text = true }, function(result)
    vim.schedule(function()
      if result.code ~= 0 then
        local message = vim.trim(result.stderr ~= "" and result.stderr or result.stdout or "")
        callback(message ~= "" and message or ("lens exited with " .. result.code), nil)
        return
      end
      local report, err = M.decode(result.stdout)
      callback(err, report, result.stdout)
    end)
  end)
end

return M
