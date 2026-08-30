-- Example ~/.config/wezterm/wezterm.lua  (Windows: C:\Users\<you>\.config\wezterm\wezterm.lua)
local wezterm = require 'wezterm'
local config = wezterm.config_builder()

---------------------------------------------------------------------------
-- ordinary wezterm settings
---------------------------------------------------------------------------
config.default_prog = { 'pwsh.exe', '-NoLogo' }
config.font = wezterm.font_with_fallback { 'JetBrainsMono Nerd Font', 'Consolas' }
config.color_scheme = 'Catppuccin Mocha'
config.window_close_confirmation = 'AlwaysPrompt'

---------------------------------------------------------------------------
-- ssh manager
---------------------------------------------------------------------------
-- Install directly from the public repository. See README section 1 for local
-- clone and offline alternatives.
local sshmgr = wezterm.plugin.require 'https://github.com/stabey/wezterm-ssh-manager'

sshmgr.apply_to_config(config, {
  -- Where connections open by default.
  default_where = 'tab',

  -- Values every profile inherits.
  defaults = {
    options = {
      keepaliveInterval = 30,
      keepaliveCountMax = 3,
    },
  },

  -- Per-group defaults. A profile with `group = 'prod'` picks these up.
  groups = {
    prod = {
      color = '#f38ba8',
      icon = wezterm.nerdfonts.md_server_security,
      options = {
        user = 'ops',
        auth = 'publicKey',
        privateKeys = { '~/.ssh/id_ed25519' },
        jumpHost = 'bastion',
      },
    },
    lab = {
      color = '#a6e3a1',
      icon = wezterm.nerdfonts.md_flask,
    },
  },

  profiles = {
    -- the jump host itself
    {
      name = 'bastion',
      group = 'infra',
      icon = wezterm.nerdfonts.md_shield_lock,
      options = {
        host = 'bastion.example.com',
        port = 2222,
        user = 'jump',
        auth = 'agent',
      },
    },

    -- production web box: forwards the DB port and drops you into tmux
    {
      name = 'web-1',
      group = 'prod',
      options = {
        host = '198.51.100.11',
        forwardedPorts = {
          { type = 'Local', host = '127.0.0.1', port = 15432,
            targetAddress = 'db.example.com', targetPort = 5432, description = 'postgres' },
        },
        scripts = {
          { expect = '%[ops@', isRegex = true, send = 'tmux new -As main', optional = true },
        },
      },
    },

    -- an old box that only takes passwords; the password comes from 1Password
    {
      name = 'legacy',
      group = 'prod',
      behaviorOnSessionEnd = 'reconnect',
      options = {
        host = 'legacy.example.com',
        user = 'root',
        auth = 'password',
        password_cmd = { 'op', 'read', 'op://example-vault/example-server/password' },
        skipBanner = true,
        algorithms = {
          kex = { 'diffie-hellman-group14-sha1' },
          serverHostKey = { 'ssh-rsa' },
        },
      },
      on_login = { 'cd /var/log', 'tail -f messages' },
    },

    -- flat shorthand
    { name = 'nas', group = 'lab', host = '192.0.2.9', user = 'admin' },
  },

  -- The file the SSH Manager TUI (CTRL-SHIFT-S) edits. Defaults to
  -- <wezterm.config_dir>/ssh_profiles.lua, which is the *home directory*
  -- if your config is ~/.wezterm.lua -- pin it if the profiles live elsewhere.
  profile_store = '~/.config/wezterm/ssh_profiles.lua',
  -- Extra files to load (hot-reloaded). Same path as the store is fine; it is
  -- only read once.
  profile_files = { '~/.config/wezterm/ssh_profiles.lua' },

  -- Bring hosts over from ~/.ssh/config and from Tabby.
  import_ssh_config = true,
  -- import_tabby = true,

  ui = {
    color_tabs = true,   -- safe here: this config has no format-tab-title of its own
    launch_menu = true,
  },

  keys = {
    picker = { key = 's', mods = 'CTRL|SHIFT' },
    picker_new_window = { key = 'S', mods = 'CTRL|SHIFT|ALT' },
    quick_connect = { key = 'p', mods = 'CTRL|SHIFT|ALT' },
    reconnect_tab = { key = 'r', mods = 'CTRL|SHIFT|ALT' },
  },

  on_ready = function(profile)
    wezterm.log_info('connected to ' .. profile.name)
  end,
})

-- Dedicated hotkeys for the hosts you open twenty times a day.
table.insert(config.keys, {
  key = '1',
  mods = 'CTRL|SHIFT|ALT',
  action = sshmgr.connect_action('prod/web-1', 'tab'),
})
table.insert(config.keys, {
  key = '2',
  mods = 'CTRL|SHIFT|ALT',
  action = sshmgr.connect_action('prod/web-1', 'split_right'),
})

return config
