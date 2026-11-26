-- Minimal init for headless Lua tests: this plugin + mini.test only.
local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h:h:h")
vim.opt.runtimepath:prepend(root)
-- Lets tests `require("tests.nvim.fixture")`.
package.path = root .. "/?.lua;" .. package.path

local candidates = {
  os.getenv("MINI_PATH") or "",
  root .. "/deps/mini.nvim",
  vim.fn.stdpath("data") .. "/lazy/mini.nvim",
}
for _, path in ipairs(candidates) do
  if path ~= "" and vim.uv.fs_stat(path) then
    vim.opt.runtimepath:prepend(path)
    break
  end
end

vim.o.swapfile = false
require("mini.test").setup()
