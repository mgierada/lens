---Small synchronous git helpers (all calls are short-lived).
local M = {}

---@param repo string
---@param args string[]
---@return string|nil stdout, string|nil err
local function run(repo, args)
  local cmd = vim.list_extend({ "git", "-C", repo }, args)
  local result = vim.system(cmd, { text = true }):wait()
  if result.code ~= 0 then
    return nil, vim.trim(result.stderr or "")
  end
  return result.stdout, nil
end

---Top-level directory of the repository containing `dir`.
---@param dir string
---@return string|nil
function M.root(dir)
  local out = run(dir, { "rev-parse", "--show-toplevel" })
  return out and vim.trim(out) or nil
end

---Commit SHA of `rev`, or nil when it does not resolve.
---@param repo string
---@param rev string
---@return string|nil
function M.resolve(repo, rev)
  local out = run(repo, { "rev-parse", "--verify", "--quiet", rev .. "^{commit}" })
  return out and vim.trim(out) or nil
end

---Best guess at the branch a feature branch is compared against.
---@param repo string
---@return string
function M.default_base(repo)
  local remote_head = run(repo, { "rev-parse", "--abbrev-ref", "origin/HEAD" })
  if remote_head and vim.trim(remote_head) ~= "origin/HEAD" then
    return vim.trim(remote_head)
  end
  for _, candidate in ipairs({ "main", "master" }) do
    if M.resolve(repo, candidate) then
      return candidate
    end
  end
  return "main"
end

---Lines of `path` at `rev`.
---@param repo string
---@param rev string
---@param path string
---@return string[]|nil lines, string|nil err
function M.show(repo, rev, path)
  local out, err = run(repo, { "show", rev .. ":" .. path })
  if not out then
    return nil, err
  end
  local lines = vim.split(out, "\n", { plain = true })
  if lines[#lines] == "" then
    table.remove(lines)
  end
  return lines, nil
end

---Local and remote branch names, for command completion.
---@param repo string
---@return string[]
function M.refs(repo)
  local out = run(repo, { "for-each-ref", "--format=%(refname:short)", "refs/heads", "refs/remotes" })
  return out and vim.split(vim.trim(out), "\n", { plain = true }) or {}
end

return M
