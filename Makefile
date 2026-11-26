.PHONY: build test test-rust test-lua lint

build:
	cargo build --release

test: test-rust test-lua

test-rust:
	cargo test

# Needs mini.nvim: MINI_PATH, ./deps/mini.nvim or lazy.nvim's install dir.
# The end-to-end case uses target/release/lens and is skipped when it is not built.
test-lua: build
	nvim --headless --noplugin -u tests/nvim/minimal_init.lua \
		-c "lua MiniTest.run({ collect = { find_files = function() return vim.fn.globpath('tests/nvim', 'test_*.lua', true, true) end } })"

lint:
	cargo fmt --check
	cargo clippy --all-targets
