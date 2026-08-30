-- sshmgr.ui -- picker, quick-connect, command palette and tab decoration
local wezterm = require 'wezterm'
local act = wezterm.action
local util = require 'sshmgr.util'
local session = require 'sshmgr.session'

local M = {}

local function target_string(p)
  local o = p.options
  local s = ''
  if o.user then
    s = o.user .. '@'
  end
  s = s .. (o.host or '?')
  if o.port and o.port ~= 22 then
    s = s .. ':' .. tostring(o.port)
  end
  return s
end

--- Coloured, aligned label for the InputSelector.
local function label_for(p, cfg, name_width)
  local dim = cfg.ui.dim_color or '#6c7086'
  local icon = p.icon or cfg.ui.default_icon or ''
  local name = p.name
  if p.group ~= '' then
    name = p.group .. '/' .. name
  end
  local pad = string.rep(' ', math.max(1, name_width - #name + 2))

  local segs = {}
  if icon ~= '' then
    if p.color then
      table.insert(segs, { Foreground = { Color = p.color } })
    end
    table.insert(segs, { Text = icon .. ' ' })
    table.insert(segs, 'ResetAttributes')
  end
  if p.color then
    table.insert(segs, { Foreground = { Color = p.color } })
  end
  table.insert(segs, { Text = name })
  table.insert(segs, 'ResetAttributes')
  table.insert(segs, { Foreground = { Color = dim } })
  table.insert(segs, { Text = pad .. target_string(p) })
  if p.options.jumpHost and p.options.jumpHost ~= '' then
    table.insert(segs, { Text = '  via ' .. tostring(p.options.jumpHost) })
  end
  if #(p.options.forwardedPorts or {}) > 0 then
    table.insert(segs, { Text = string.format('  [%d fwd]', #p.options.forwardedPorts) })
  end
  table.insert(segs, 'ResetAttributes')
  return wezterm.format(segs)
end

--- Build the InputSelector choices for the current profile list.
function M.choices(profiles, cfg)
  local width = 0
  for _, p in ipairs(profiles) do
    local n = #p.name + (p.group ~= '' and (#p.group + 1) or 0)
    if n > width then
      width = n
    end
  end
  local choices = {}
  for _, p in ipairs(profiles) do
    table.insert(choices, { id = p.id, label = label_for(p, cfg, width) })
  end
  return choices
end

--- Command-palette-style overlay: type to filter, Enter to connect.
function M.picker_action(state, where)
  return wezterm.action_callback(function(window, pane)
    local profiles = state.profiles()
    local cfg = state.cfg
    if #profiles == 0 then
      window:toast_notification('wezterm ssh-manager', 'no SSH profiles configured', nil, 4000)
      return
    end
    local choices = {}
    for _, p in ipairs(profiles) do
      local name = p.group ~= '' and (p.group .. '/' .. p.name) or p.name
      table.insert(choices, {
        id = p.id,
        label = name .. '   ' .. target_string(p),
      })
    end
    window:perform_action(
      act.InputSelector {
        title = 'SSH',
        choices = choices,
        fuzzy = true,
        fuzzy_description = 'SSH: ',
        action = wezterm.action_callback(function(win, pn, id)
          if not id then
            return
          end
          local profile = state.find(profiles, id)
          if not profile then
            util.err('profile %q vanished', tostring(id))
            return
          end
          session.connect(profile, cfg, profiles, state.find, win, pn, where)
        end),
      },
      pane
    )
  end)
end

--- An action that asks for `[user@]host[:port]` and connects ad hoc.
function M.quick_connect_action(state, where)
  return wezterm.action_callback(function(window, pane)
    window:perform_action(
      act.PromptInputLine {
        description = wezterm.format {
          { Attribute = { Intensity = 'Bold' } },
          { Foreground = { AnsiColor = 'Aqua' } },
          { Text = 'ssh · [user@]host[:port]' },
        },
        action = wezterm.action_callback(function(win, pn, line)
          if not line or util.trim(line) == '' then
            return
          end
          local profiles = state.profiles()
          -- an existing profile name wins over treating it as a hostname
          local existing = state.find(profiles, util.trim(line))
          local profile = existing
          if not profile then
            local t = util.parse_target(util.trim(line))
            profile = state.normalize({
              name = util.trim(line),
              group = 'ad-hoc',
              options = { host = t.host, user = t.user, port = t.port },
            }, state.cfg)
          end
          session.connect(profile, state.cfg, profiles, state.find, win, pn, where)
        end),
      },
      pane
    )
  end)
end

--- Entries contributed to the command palette (CTRL-SHIFT-P).
function M.palette_entries(state)
  local cfg = state.cfg
  if not cfg.ui.command_palette then
    return {}
  end
  local out = {}
  local profiles = state.profiles()
  for _, p in ipairs(profiles) do
    local label = p.group ~= '' and (p.group .. '/' .. p.name) or p.name
    table.insert(out, {
      brief = 'SSH  ' .. label .. '  ' .. target_string(p),
      doc = target_string(p),
      icon = 'md_server_network',
      action = wezterm.action_callback(function(win, pn)
        local list = state.profiles()
        session.connect(state.find(list, p.id) or p, cfg, list, state.find, win, pn, cfg.default_where)
      end),
    })
  end
  table.insert(out, {
    brief = 'SSH: pick a connection…',
    doc = 'fuzzy search overlay',
    icon = 'md_lan_connect',
    action = M.picker_action(state, cfg.default_where),
  })
  table.insert(out, {
    brief = 'SSH: open manager…',
    doc = 'persistent TUI: groups on the left, connections on the right',
    icon = 'md_playlist_edit',
    action = require('sshmgr.tui').open_action(state),
  })
  table.insert(out, {
    brief = 'SSH: quick connect (user@host)…',
    icon = 'md_console_network',
    action = M.quick_connect_action(state, cfg.default_where),
  })
  table.insert(out, {
    brief = 'SSH: convert Tabby config.yaml to a profile file…',
    doc = 'writes ssh_profiles.lua next to your wezterm.lua',
    icon = 'md_file_export',
    action = wezterm.action_callback(function(win)
      local ok, msg = require('sshmgr.export').tabby {
        from = cfg.import_tabby ~= false and cfg.import_tabby or true,
      }
      win:toast_notification('wezterm ssh-manager', msg, nil, ok and 6000 or 10000)
      if not ok then
        util.err('%s', msg)
      end
    end),
  })
  return out
end

--- `launch_menu` entries. These run the plain ssh command: wezterm exposes no
--- hook for panes created by the launcher, so login scripts do not run for
--- these. Use the picker / palette / sshmgr.connect() for the full flow.
function M.launch_menu_entries(state)
  local out = {}
  local cfg = state.cfg
  local profiles = state.profiles()
  local argvmod = require 'sshmgr.argv'
  for _, p in ipairs(profiles) do
    local ok, argv = pcall(argvmod.build, p, cfg, profiles, state.find)
    if ok then
      table.insert(out, {
        label = 'ssh: ' .. (p.group ~= '' and (p.group .. '/' .. p.name) or p.name),
        args = argvmod.wrap_for_session_end(argv, p.behaviorOnSessionEnd, p),
      })
    end
  end
  return out
end

--- format-tab-title helper: tint the tab using the profile colour.
function M.decorate_tab(tab, cfg)
  local info = session.tab_info(tab.tab_id)
  if not info then
    return nil
  end
  local title = tab.tab_title
  if not title or title == '' then
    title = (info.icon or cfg.ui.default_icon or '') .. ' ' .. info.name
  end
  local fg = info.color or (tab.is_active and '#ffffff' or nil)
  local segs = {}
  if fg then
    table.insert(segs, { Foreground = { Color = fg } })
  end
  if tab.is_active then
    table.insert(segs, { Attribute = { Intensity = 'Bold' } })
  end
  table.insert(segs, { Text = ' ' .. title .. ' ' })
  return segs
end

return M
