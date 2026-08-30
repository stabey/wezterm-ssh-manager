-- sshmgr.session -- spawn a profile into a tab/window/split and drive it
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'
local argvmod = require 'sshmgr.argv'
local secrets = require 'sshmgr.secrets'
local automation = require 'sshmgr.automation'

local M = {}

local GLOBAL_TABS = 'sshmgr_tabs'

local function remember_tab(tab_id, profile)
  local tabs = wezterm.GLOBAL[GLOBAL_TABS] or {}
  tabs[tostring(tab_id)] = {
    id = profile.id,
    name = profile.name,
    color = profile.color,
    icon = profile.icon,
    group = profile.group,
    host = profile.options.host,
    user = profile.options.user,
  }
  wezterm.GLOBAL[GLOBAL_TABS] = tabs
end

function M.tab_info(tab_id)
  local tabs = wezterm.GLOBAL[GLOBAL_TABS] or {}
  return tabs[tostring(tab_id)]
end

local function format_title(profile, cfg)
  local fmt = cfg.ui.tab_title_format or '{name}'
  return (fmt:gsub('{(%w+)}', {
    icon = profile.icon or cfg.ui.default_icon or '',
    name = profile.name,
    host = profile.options.host or '',
    user = profile.options.user or '',
    group = profile.group ~= '' and profile.group or '',
  }))
end

local SPLIT_DIR = {
  split_right = 'Right',
  split_left = 'Left',
  split_down = 'Bottom',
  split_up = 'Top',
  Right = 'Right',
  Left = 'Left',
  Down = 'Bottom',
  Bottom = 'Bottom',
  Up = 'Top',
  Top = 'Top',
}

--- Spawn `profile`.
--- where: 'tab' | 'window' | 'split_right' | 'split_left' | 'split_down' | 'split_up'
---        or SplitPane directions: 'Right' | 'Left' | 'Down' | 'Up' | 'Bottom' | 'Top'
--- Returns pane, tab (or nil, err)
function M.connect(profile, cfg, all_profiles, find, window, pane, where)
  where = where or cfg.default_where or 'tab'

  local ok, argv, meta = pcall(argvmod.build, profile, cfg, all_profiles, find)
  if not ok then
    util.err('failed to build ssh command for %q: %s', profile.name, tostring(argv))
    return nil, argv
  end

  local password = secrets.resolve(profile, cfg)
  if profile.options.auth == 'password' and not password then
    util.warn('%q uses password auth but no password source is configured; ssh will prompt', profile.name)
  end

  local spawn_argv = argvmod.wrap_for_session_end(argv, profile.behaviorOnSessionEnd, profile)

  local spawn_opts = {
    args = spawn_argv,
    label = 'ssh: ' .. profile.name,
  }
  if next(profile.env) ~= nil then
    spawn_opts.set_environment_variables = profile.env
  end
  if profile.local_cwd then
    spawn_opts.cwd = util.expand_path(profile.local_cwd)
  end
  if profile.domain then
    spawn_opts.domain = { DomainName = profile.domain }
  end

  util.log('%s -> %s', profile.name, table.concat(argv, ' '))

  local new_pane, new_tab
  local spawn_ok, spawn_err = pcall(function()
    if where == 'window' then
      local tab, p = wezterm.mux.spawn_window(spawn_opts)
      new_tab, new_pane = tab, p
    elseif SPLIT_DIR[where] then
      local base = pane or (window and window:active_pane())
      if not base then
        error 'no pane to split'
      end
      spawn_opts.direction = SPLIT_DIR[where]
      new_pane = base:split(spawn_opts)
      new_tab = new_pane:tab()
    else
      local mux_win = window and window:mux_window() or (pane and pane:window())
      if not mux_win then
        local tab, p = wezterm.mux.spawn_window(spawn_opts)
        new_tab, new_pane = tab, p
      else
        local tab, p = mux_win:spawn_tab(spawn_opts)
        new_tab, new_pane = tab, p
      end
    end
  end)

  if not spawn_ok or not new_pane then
    util.err('spawn failed for %q: %s', profile.name, tostring(spawn_err))
    return nil, spawn_err
  end

  if new_tab then
    if cfg.ui.set_tab_title then
      pcall(function()
        new_tab:set_title(format_title(profile, cfg))
      end)
    end
    remember_tab(new_tab:tab_id(), profile)
  end

  if type(cfg.on_spawn) == 'function' then
    pcall(cfg.on_spawn, profile, new_pane, window)
  end

  local auto_ctx = {
    password = password,
    vars = { profile = profile.name },
  }
  auto_ctx.on_done = function(done_ok, why)
    if cfg.ui.notify and window then
      local msg
      if done_ok and auto_ctx.password_saved then
        msg = profile.name .. ': ready（已记住密码）'
      elseif done_ok then
        msg = profile.name .. ': ready'
      else
        msg = profile.name .. ': ' .. why
      end
      pcall(function()
        window:toast_notification('wezterm ssh-manager', msg, nil, done_ok and 3000 or 6000)
      end)
    end
    if done_ok and type(cfg.on_ready) == 'function' then
      pcall(cfg.on_ready, profile, new_pane, window)
    end
  end
  automation.start(new_pane, profile, cfg, auto_ctx)

  return new_pane, new_tab, meta
end

--- Build the argv without spawning; handy for debugging from the debug overlay.
function M.preview(profile, cfg, all_profiles, find)
  local argv = argvmod.build(profile, cfg, all_profiles, find)
  return table.concat(argv, ' '), argv
end

return M
