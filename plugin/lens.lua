if vim.g.loaded_lens then
  return
end
vim.g.loaded_lens = true

vim.api.nvim_create_user_command("Lens", function(cmd)
  require("lens").open(cmd.args)
end, {
  nargs = "?",
  desc = "Review branch changes as logical clusters (e.g. :Lens main..feature)",
  complete = function(lead)
    return require("lens").complete(lead)
  end,
})

vim.api.nvim_create_user_command("LensRefresh", function()
  require("lens").refresh()
end, { desc = "Re-run the lens analysis" })

vim.api.nvim_create_user_command("LensClose", function()
  require("lens").close()
end, { desc = "Close the lens review tab" })
