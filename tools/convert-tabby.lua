-- Standalone one-shot converter: Tabby config.yaml -> wezterm-ssh-manager profile file.
--
-- Run it with wezterm as the lua host (no extra dependencies):
--
--   Windows (PowerShell):
--     wezterm --config-file .\tools\convert-tabby.lua show-keys > $null
--   macOS / Linux:
--     wezterm --config-file ./tools/convert-tabby.lua show-keys > /dev/null
--
-- Everything it has to tell you goes to stderr, so redirecting stdout hides
-- the key-table noise that `show-keys` prints.
--
-- Edit the three settings below if the defaults are not what you want.

local wezterm = require 'wezterm'

local FROM = true -- true = Tabby's default path, or an explicit config.yaml path
local TO = nil -- nil = <wezterm config dir>/ssh_profiles.lua
local FORCE = false -- true to overwrite an existing output file

---------------------------------------------------------------------------

local ROOT = wezterm.config_dir
-- Locate the plugin: this file lives at <checkout>/tools/convert-tabby.lua.
-- `--config-file` makes config_dir = tools/, so the modules are next door.
-- Prefer that over a possibly-stale wezterm.plugin clone.
local function add_path(dir)
  package.path = dir .. '/?.lua;' .. dir .. '/?/init.lua;' .. package.path
end

add_path(ROOT .. '/../plugin')
add_path(ROOT .. '/plugin')

if not pcall(require, 'sshmgr.export') then
  local ok, list = pcall(wezterm.plugin.list)
  if ok then
    for _, p in ipairs(list) do
      if p.plugin_dir then
        add_path(p.plugin_dir .. '/plugin')
      end
    end
  end
  add_path(ROOT)
end

local ok, export = pcall(require, 'sshmgr.export')
if not ok then
  wezterm.log_error [[
convert-tabby: could not find the plugin modules.
Run it from the plugin checkout, or add the path by hand near the top of this file:
  package.path = '/path/to/wezterm-ssh-manager/plugin/?.lua;'
              .. '/path/to/wezterm-ssh-manager/plugin/?/init.lua;' .. package.path
]]
  return {}
end

local done, msg = export.tabby { from = FROM, to = TO, force = FORCE }
if done then
  wezterm.log_error('convert-tabby: OK -- ' .. msg)
else
  wezterm.log_error('convert-tabby: FAILED -- ' .. msg)
end

return {}
