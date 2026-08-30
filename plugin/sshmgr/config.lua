-- sshmgr.config -- plugin-wide options and their defaults
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'

local M = {}

M.defaults = {
  ---------------------------------------------------------------------------
  -- Where profiles come from
  ---------------------------------------------------------------------------
  -- Inline list of profiles (see README for the schema).
  profiles = {},
  -- Group defaults keyed by group name, e.g. `{ ['prod'] = { options = {...} } }`
  groups = {},
  -- Options merged into *every* profile before its own values are applied.
  defaults = {},
  -- Username used only when an imported Tabby SSH profile omits `user`.
  -- Tabby's own SSH configDefaults use root. Set this to another name or
  -- false to leave the user unset and let OpenSSH choose. Inline profiles,
  -- ~/.ssh/config entries and ad-hoc connections are never changed by it.
  default_user = 'root',
  -- The one profile file the panel edits. Always loaded, whether or not it is
  -- also listed in profile_files. nil => <wezterm config dir>/ssh_profiles.lua
  -- -- which is $HOME if the config is ~/.wezterm.lua, not ~/.config/wezterm.
  profile_store = nil,
  -- Extra files to load profiles from. `.lua` returns a table, `.json`/`.yaml`
  -- /`.yml`/`.toml` are decoded with wezterm.serde.
  profile_files = {},
  -- Pull `Host` stanzas out of ~/.ssh/config and expose them as profiles.
  import_ssh_config = false,
  ssh_config_group = 'ssh_config',
  -- Import Tabby's own config.yaml (its SSH profiles + login scripts).
  -- `true` uses the default per-OS location, or pass an explicit path.
  import_tabby = false,

  ---------------------------------------------------------------------------
  -- How a connection is launched
  ---------------------------------------------------------------------------
  -- Path to the OpenSSH client. On Windows the built-in client lives in
  -- C:\Windows\System32\OpenSSH\ssh.exe and is on PATH by default.
  ssh_binary = nil, -- nil => "ssh.exe" on Windows, "ssh" elsewhere
  -- Default placement when connecting: 'tab' | 'window' | 'split_right' | 'split_down'
  default_where = 'tab',
  -- Applied to every profile unless it sets its own.
  default_ssh_options = {
    ServerAliveInterval = '30',
    ServerAliveCountMax = '3',
  },
  -- Answer the "authenticity of host ... can't be established" prompt.
  -- 'ask' (default), 'accept-new' or 'yes'.
  host_key_policy = 'ask',
  -- Drop algorithm names the local ssh client does not know (queried once via
  -- `ssh -Q` and cached). Tabby's algorithm panel defaults to selecting every
  -- entry in its menu, and those names come from the ssh2 javascript library --
  -- OpenSSH rejects the whole list on one unknown name and exits before
  -- connecting, which looks like a pane that flashes open and closes.
  filter_algorithms = true,
  -- Used to build a ProxyCommand for profiles that set socksProxy*/httpProxy*.
  proxy_command_template = 'ncat --proxy %{proxy_host}:%{proxy_port} --proxy-type %{proxy_type} %h %p',

  ---------------------------------------------------------------------------
  -- Post-login automation (Tabby "Login scripts")
  ---------------------------------------------------------------------------
  automation = {
    enabled = true,
    -- How often the pane is sampled while a login script is running.
    poll_interval = 0.15,
    -- How many lines of the pane are inspected for `expect` matches.
    scan_lines = 120,
    -- Per-step timeout, seconds. A required step that times out aborts the
    -- rest of the script; an `optional` step is simply skipped.
    step_timeout = 25,
    -- Hard ceiling for the whole script.
    session_timeout = 180,
    -- Injected before user scripts when a password is resolvable. If none is
    -- stored, a prompt asks for it; unless we saw a password reject, the
    -- value is written back even if later login scripts fail (e.g. waiting
    -- for a bash prompt on a fish host).
    auto_password = true,
    save_passwords = true,
    -- Injected when host_key_policy ~= 'ask'.
    auto_host_key = true,
    -- Prompt regex used by `on_login` / `${ready}` to decide the shell is up.
    -- Lua pattern, matched against the tail of the pane.
    ready_pattern = '[%$#>%%][ ]?$',
    ready_timeout = 30,
  },

  ---------------------------------------------------------------------------
  -- Secrets
  ---------------------------------------------------------------------------
  -- function(profile) -> string|nil ; consulted before options.password
  password_provider = nil,
  -- Command template used to resolve `options.password_cmd` entries that are
  -- given as a plain string rather than an argv array.
  password_cmd_shell = nil, -- nil => {'cmd.exe','/c'} on Windows, {'sh','-c'}

  ---------------------------------------------------------------------------
  -- UI
  ---------------------------------------------------------------------------
  ui = {
    -- Show the picker in fuzzy mode straight away.
    fuzzy = true,
    title = 'SSH  ·  connections',
    default_icon = wezterm.nerdfonts.md_server_network,
    group_icon = wezterm.nerdfonts.md_folder_network,
    -- Colour used for the dim part of the label.
    dim_color = '#6c7086',
    -- Rename the tab to the profile name once connected.
    set_tab_title = true,
    tab_title_format = '{icon} {name}',
    -- Tint the tab bar entry with `profile.color`.
    -- Off by default: wezterm only ever calls the FIRST registered
    -- `format-tab-title` handler, so enabling this would shadow one of your
    -- own. Either set it to true, or call `sshmgr.decorate_tab(tab)` from
    -- your handler and fall through when it returns nil.
    color_tabs = false,
    -- Add "SSH: <name>" entries to the command palette (CTRL-SHIFT-P).
    -- Same caveat as color_tabs: set to false and call
    -- `sshmgr.palette_entries()` from your own handler if you have one.
    command_palette = true,
    -- Also add the profiles to `config.launch_menu`. Launcher-spawned panes
    -- run the raw ssh command -- wezterm exposes no hook for them, so login
    -- scripts do not run for those.
    launch_menu = false,
    -- Toast when a login script finishes or fails.
    notify = true,
    -- Persistent manager tab (Ctrl+Shift+S).
    tui = {
      -- 'auto' prefers the bundled OpenTUI implementation and falls back to
      -- Textual. Use 'opentui' or 'textual' to force one backend.
      backend = 'auto',
      -- Optional argv prefix for OpenTUI, for example
      -- { 'bun', 'run', 'C:/dev/wezterm-ssh-manager/tui-opentui/src/index.tsx' }.
      -- The launcher appends `--snapshot <path>` and never invokes a shell.
      -- The same command must implement `--create-runtime`,
      -- `--cleanup-runtime <dir>` and `--replace-file <from> <to>`.
      command = nil,
      -- Optional working directory for a custom OpenTUI command.
      cwd = nil,
      bun = nil, -- explicit Bun executable used for bundled TypeScript source
      python = nil, -- explicit interpreter, e.g. [[C:/Python314/python.exe]]
      tab_title = 'SSH Manager',
    },
  },

  ---------------------------------------------------------------------------
  -- Key bindings. Set any of these to false to skip registering it.
  ---------------------------------------------------------------------------
  keys = {
    picker = { key = 's', mods = 'CTRL|SHIFT' }, -- TUI manager
    selector = { key = 'e', mods = 'CTRL|SHIFT' }, -- InputSelector 模糊搜索
    panel = { key = 'e', mods = 'CTRL|SHIFT|ALT' }, -- TUI manager
    picker_new_window = { key = 'S', mods = 'CTRL|SHIFT|ALT' }, -- selector, new window
    quick_connect = { key = 'p', mods = 'CTRL|SHIFT|ALT' },
    reconnect_tab = false, -- e.g. { key = 'r', mods = 'CTRL|SHIFT|ALT' }
  },

  -- Called with (profile, pane, window) right after the pane is spawned.
  on_spawn = nil,
  -- Called with (profile, pane, window) once the login script has finished.
  on_ready = nil,
}

--- Merge user options over the defaults.
function M.build(opts)
  local cfg = util.deep_copy(M.defaults)
  if type(opts) == 'table' then
    -- callbacks are functions and must not be deep-copied through GLOBAL
    for k, v in pairs(opts) do
      if type(v) == 'table' and type(cfg[k]) == 'table' and not util.is_array(v) then
        for kk, vv in pairs(v) do
          cfg[k][kk] = vv
        end
      else
        cfg[k] = v
      end
    end
  end
  if not cfg.ssh_binary then
    cfg.ssh_binary = util.is_windows and 'ssh.exe' or 'ssh'
  end
  if not cfg.password_cmd_shell then
    cfg.password_cmd_shell = util.is_windows and { 'cmd.exe', '/c' } or { 'sh', '-c' }
  end
  return cfg
end

return M
