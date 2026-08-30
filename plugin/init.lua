-- wezterm-ssh-manager
--
-- A Tabby-style SSH connection manager for wezterm.
--
--   local sshmgr = wezterm.plugin.require 'https://github.com/stabey/wezterm-ssh-manager'
--   sshmgr.apply_to_config(config, { profiles = { ... } })
--
-- See README.md for the profile schema.

-- Lua hands a required chunk `(modname, path_of_this_file)` in `...`. That is
-- how we find our own directory; wezterm's lua has no `debug` library.
local _MODNAME, THIS_FILE = ...

local wezterm = require 'wezterm'

---------------------------------------------------------------------------
-- Bootstrap: wezterm only puts `<plugin>/plugin/init.lua` on package.path,
-- so the sibling modules need the directory adding by hand.
---------------------------------------------------------------------------
local function bootstrap_package_path()
  if pcall(require, 'sshmgr.util') then
    return true
  end

  local sep = package.config:sub(1, 1)

  local function try(dir)
    if not dir then
      return false
    end
    local f = io.open(dir .. sep .. 'sshmgr' .. sep .. 'util.lua', 'r')
    if not f then
      return false
    end
    f:close()
    package.path = table.concat({
      dir .. sep .. '?.lua',
      dir .. sep .. '?' .. sep .. 'init.lua',
      package.path,
    }, ';')
    return (pcall(require, 'sshmgr.util'))
  end

  -- Preferred: the path the loader used for this very file.
  if type(THIS_FILE) == 'string' and try(THIS_FILE:match '^(.*)[/\\][^/\\]+$') then
    return true
  end

  -- Fall back to asking the plugin registry. Note this is only a fallback:
  -- wezterm.plugin.list() raises if *any* directory under plugins/ is not a
  -- git checkout, and wezterm.glob/read_dir are async so they cannot be called
  -- from inside wezterm.plugin.require at all.
  local ok, list = pcall(wezterm.plugin.list)
  if ok then
    for _, p in ipairs(list) do
      if p.plugin_dir and try(p.plugin_dir .. sep .. 'plugin') then
        return true
      end
    end
  end

  return false
end

if not bootstrap_package_path() then
  wezterm.log_error [[
[ssh-manager] could not locate the plugin's lua modules.
If you vendored the plugin instead of using wezterm.plugin.require, add its
`plugin` directory to package.path before requiring it:
  package.path = '/path/to/wezterm-ssh-manager/plugin/?.lua;'
              .. '/path/to/wezterm-ssh-manager/plugin/?/init.lua;' .. package.path
]]
  error 'ssh-manager: module path bootstrap failed'
end

local util = require 'sshmgr.util'
local cfgmod = require 'sshmgr.config'
local profiles_mod = require 'sshmgr.profiles'
local session = require 'sshmgr.session'
local ui = require 'sshmgr.ui'
local argvmod = require 'sshmgr.argv'

local M = {}

-- Per-lua-state singleton. wezterm builds a fresh lua state for every config
-- evaluation, so this is naturally reset on reload.
local state = {
  cfg = nil,
  list = nil,
  registered = false,
  plugin_dir = type(THIS_FILE) == 'string' and THIS_FILE:match '^(.*)[/\\][^/\\]+$' or nil,
}

function state.profiles()
  if wezterm.GLOBAL.sshmgr_invalidate then
    wezterm.GLOBAL.sshmgr_invalidate = nil
    state.list = nil
  end
  if not state.list then
    state.list = profiles_mod.load(state.cfg)
  end
  return state.list
end

function state.reload()
  state.list = nil
  return state.profiles()
end

state.find = profiles_mod.find
state.normalize = profiles_mod.normalize

---------------------------------------------------------------------------
-- Public API
---------------------------------------------------------------------------

--- Configure the plugin and wire it into `config`.
function M.apply_to_config(config, opts)
  state.cfg = cfgmod.build(opts)
  state.list = nil
  local cfg = state.cfg
  local tui = require 'sshmgr.tui'
  tui.attach(state)
  local list = state.profiles()

  util.log('%d profile(s) loaded', #list)
  pcall(function()
    tui.write_snapshot(state)
  end)

  ---------------------------------------------------------------------------
  -- key bindings
  ---------------------------------------------------------------------------
  config.keys = config.keys or {}
  local function bind(spec, action)
    if not spec then
      return
    end
    table.insert(config.keys, { key = spec.key, mods = spec.mods, action = action })
  end
  bind(cfg.keys.picker, tui.open_action(state))
  bind(cfg.keys.panel, tui.open_action(state))
  bind(cfg.keys.selector, wezterm.action.ActivateCommandPalette)
  bind(cfg.keys.picker_new_window, ui.picker_action(state, 'window'))
  bind(cfg.keys.quick_connect, ui.quick_connect_action(state, cfg.default_where))
  if cfg.keys.reconnect_tab then
    bind(cfg.keys.reconnect_tab, M.reconnect_action())
  end

  ---------------------------------------------------------------------------
  -- launcher entries
  ---------------------------------------------------------------------------
  if cfg.ui.launch_menu then
    config.launch_menu = config.launch_menu or {}
    for _, entry in ipairs(ui.launch_menu_entries(state)) do
      table.insert(config.launch_menu, entry)
    end
  end

  ---------------------------------------------------------------------------
  -- close confirmation (Tabby: warnOnClose)
  ---------------------------------------------------------------------------
  local skip = argvmod.close_confirmation_names(list, cfg)
  if #skip > 0 then
    config.skip_close_confirmation_for_processes_named = config.skip_close_confirmation_for_processes_named
      or {
        'bash', 'sh', 'zsh', 'fish', 'tmux', 'nu',
        'cmd.exe', 'pwsh.exe', 'powershell.exe',
      }
    util.tbl_concat_into(config.skip_close_confirmation_for_processes_named, skip)
  end

  ---------------------------------------------------------------------------
  -- events
  --
  -- NOTE: wezterm invokes only the FIRST registered handler for
  -- `augment-command-palette` and `format-tab-title`. Registering ours would
  -- shadow one you wrote yourself, so tab colouring is opt-in and both are
  -- also exposed as plain functions you can call from your own handler.
  ---------------------------------------------------------------------------
  if not state.registered then
    state.registered = true

    wezterm.on('user-var-changed', function(window, pane, name, value)
      require('sshmgr.tui').on_user_var(window, pane, name, value)
    end)

    if cfg.ui.command_palette == true then
      wezterm.on('augment-command-palette', function()
        return ui.palette_entries(state)
      end)
    end

    if cfg.ui.color_tabs == true then
      wezterm.on('format-tab-title', function(tab)
        return ui.decorate_tab(tab, state.cfg)
      end)
    end
  end

  return config
end

--- Open the SSH Manager TUI tab.
function M.panel()
  return require('sshmgr.tui').open_action(state)
end

--- One-shot conversion of Tabby's config.yaml into a profile file this plugin
--- can read. See sshmgr.export.tabby for the options.
---
---   sshmgr.export_tabby { to = '~/.config/wezterm/ssh_profiles.lua' }
---
--- Returns ok, message, details.
function M.export_tabby(opts)
  return require('sshmgr.export').tabby(opts)
end

--- Force the profile list to be rebuilt (e.g. after editing a profile file).
function M.reload()
  return state.reload()
end

--- The normalised profile list.
function M.profiles()
  return state.profiles()
end

--- Look up one profile by id or name.
function M.get(key)
  return profiles_mod.find(state.profiles(), key)
end

--- Connect to `key` (a profile id/name or a profile table).
--- `where` is 'tab' | 'window' | 'split_right' | 'split_down'.
function M.connect(window, pane, key, where)
  local list = state.profiles()
  local profile = profiles_mod.find(list, key)
  if not profile then
    util.err('unknown profile: %s', tostring(key))
    return nil, 'unknown profile'
  end
  return session.connect(profile, state.cfg, list, profiles_mod.find, window, pane, where)
end

--- An action you can drop straight into `config.keys`.
function M.connect_action(key, where)
  return wezterm.action_callback(function(window, pane)
    M.connect(window, pane, key, where)
  end)
end

--- Fuzzy InputSelector. `where` overrides `default_where`.
function M.picker(where)
  return ui.picker_action(state, where or state.cfg.default_where)
end

--- Prompt for `[user@]host[:port]` and connect.
function M.quick_connect(where)
  return ui.quick_connect_action(state, where or state.cfg.default_where)
end

--- Split the current pane. If this tab is an SSH session, open the same profile
--- in the new pane (password + login scripts still run). Otherwise a normal split.
--- `direction`: 'Right' | 'Left' | 'Down' | 'Up'
function M.split_action(direction)
  direction = direction or 'Right'
  return wezterm.action_callback(function(window, pane)
    local tab = window:active_tab()
    local info = tab and session.tab_info(tab:tab_id())
    if info and info.id then
      M.connect(window, pane, info.id, direction)
      return
    end
    window:perform_action(
      wezterm.action.SplitPane {
        direction = direction,
        size = { Percent = 50 },
      },
      pane
    )
  end)
end

--- Re-run the profile that owns the active tab, in place of the active pane's tab.
function M.reconnect_action(where)
  return wezterm.action_callback(function(window, pane)
    local tab = window:active_tab()
    local info = session.tab_info(tab:tab_id())
    if not info then
      window:toast_notification('wezterm ssh-manager', 'this tab is not an ssh session', nil, 3000)
      return
    end
    M.connect(window, pane, info.id, where or state.cfg.default_where)
  end)
end

--- Command palette entries, for use inside your own `augment-command-palette`.
function M.palette_entries()
  return ui.palette_entries(state)
end

--- Tab decoration, for use inside your own `format-tab-title`.
--- Returns nil for tabs that are not ssh sessions so you can fall through.
function M.decorate_tab(tab)
  return ui.decorate_tab(tab, state.cfg)
end

--- Information recorded for an ssh tab, or nil.
M.tab_info = session.tab_info

--- The exact ssh command line a profile would run. Useful from the debug
--- overlay (CTRL-SHIFT-L):  sshmgr.command_for 'prod-web-1'
function M.command_for(key)
  local list = state.profiles()
  local profile = profiles_mod.find(list, key)
  if not profile then
    return nil, 'unknown profile'
  end
  return session.preview(profile, state.cfg, list, profiles_mod.find)
end

M.util = util

return M
