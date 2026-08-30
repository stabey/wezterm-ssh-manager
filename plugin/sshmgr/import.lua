-- sshmgr.import -- pull profiles out of ~/.ssh/config and Tabby's config.yaml
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'

local M = {}

--- Every literal `Host` stanza wezterm can see, as profiles.
--- The generated profiles deliberately carry almost no -o overrides: ssh will
--- read ~/.ssh/config itself, so duplicating the options here would only cause
--- drift.
function M.from_ssh_config(cfg)
  local out = {}
  local ok, hosts = pcall(wezterm.enumerate_ssh_hosts)
  if not ok then
    util.err('cannot read ssh config: %s', tostring(hosts))
    return out
  end
  for host, opts in pairs(hosts) do
    local keys = {}
    for k in (opts.identityfile or ''):gmatch '%S+' do
      table.insert(keys, k)
    end
    table.insert(out, {
      name = host,
      group = cfg.ssh_config_group or 'ssh_config',
      id = 'ssh_config/' .. host,
      options = {
        -- use the alias, not the resolved hostname, so ssh applies the stanza
        host = host,
        -- The integrated ssh2/SFTP client does not read ~/.ssh/config, so it
        -- needs the already-resolved HostName alongside the OpenSSH alias.
        sftpHost = opts.hostname,
        user = opts.user,
        port = tonumber(opts.port),
        privateKeys = #keys > 0 and keys or nil,
      },
      weight = -10,
    })
  end
  return out
end

local function tabby_config_path()
  if util.is_windows then
    local appdata = os.getenv 'APPDATA'
    if appdata then
      return appdata .. '\\tabby\\config.yaml'
    end
  elseif wezterm.target_triple:find 'darwin' then
    return wezterm.home_dir .. '/Library/Application Support/tabby/config.yaml'
  end
  return wezterm.home_dir .. '/.config/tabby/config.yaml'
end

M.tabby_config_path = tabby_config_path

--- Some Tabby versions store the per-provider defaults as a bare options table
--- rather than a profile-shaped one. Normalise both into { options = {...} }.
local function as_profile_shaped(t)
  if type(t) ~= 'table' then
    return {}
  end
  if t.options ~= nil then
    return t
  end
  for _, k in ipairs { 'host', 'port', 'user', 'auth', 'privateKeys', 'scripts', 'jumpHost' } do
    if t[k] ~= nil then
      return { options = t }
    end
  end
  return t
end

--- Render a group id as a `parent/child` path.
local function group_path(by_id, id, seen)
  local g = by_id[id]
  if not g then
    return nil
  end
  seen = seen or {}
  if seen[id] then
    return g.name or id
  end
  seen[id] = true
  local own = g.name or id
  if g.parentGroupId then
    local parent = group_path(by_id, g.parentGroupId, seen)
    if parent then
      return parent .. '/' .. own
    end
  end
  return own
end

--- Read and decode Tabby's config.yaml. Returns doc, resolved_path.
function M.read_tabby(path)
  if path == nil or path == true then
    path = tabby_config_path()
  end
  path = util.expand_path(path)
  local data = util.read_file(path)
  if not data then
    return nil, path, 'file not found'
  end
  local ok, doc = pcall(wezterm.serde.yaml_decode, data)
  if not ok or type(doc) ~= 'table' then
    return nil, path, 'cannot parse yaml: ' .. tostring(doc)
  end
  return doc, path
end

--- Convert a decoded Tabby document into profile entries.
--- Returns list, report where report describes what could not be carried over.
function M.convert_tabby(doc, cfg)
  local report = {
    skipped = {},        -- { name = ..., type = ... }
    needs_password = {}, -- profiles whose auth is 'password'
    vault_enabled = doc.vault ~= nil and doc.vault ~= false,
    groups = {},
  }

  -- Tabby writes `groups`; some older/exported files use `profileGroups`.
  local raw_groups = doc.groups or doc.profileGroups or {}
  local by_id = {}
  for _, g in ipairs(raw_groups) do
    if g.id then
      by_id[g.id] = g
    end
  end

  local global_defaults = as_profile_shaped((doc.profileDefaults or {}).ssh)

  local id_to_name = {}
  for _, p in ipairs(doc.profiles or {}) do
    if p.id and p.name then
      id_to_name[p.id] = p.name
    end
  end

  local out = {}
  for _, p in ipairs(doc.profiles or {}) do
    if p.type ~= 'ssh' then
      table.insert(report.skipped, { name = p.name or p.id, type = p.type or '?' })
    else
      local entry = {
        id = 'tabby/' .. (p.id or p.name),
        name = p.name or (p.options or {}).host,
        icon = p.icon,
        color = p.color,
        weight = tonumber(p.weight) or nil,
        behaviorOnSessionEnd = p.behaviorOnSessionEnd,
        options = util.deep_copy(p.options or {}),
      }

      -- profile > group defaults > global defaults
      local gdef = as_profile_shaped((by_id[p.group] or {}).defaults and by_id[p.group].defaults.ssh)
      util.defaults_into(entry, { options = gdef.options, color = gdef.color, icon = gdef.icon })
      util.defaults_into(entry, {
        options = global_defaults.options,
        color = global_defaults.color,
        icon = global_defaults.icon,
      })

      local gname = p.group and group_path(by_id, p.group) or nil
      entry.group = gname or (p.group and tostring(p.group)) or 'tabby'
      report.groups[entry.group] = true

      local o = entry.options
      -- Tabby keeps passwords in the OS keychain / vault, never in the yaml.
      o.password = nil
      -- Match Tabby's SSH default without changing inline, OpenSSH-config or
      -- ad-hoc profiles. A nil cfg is used by the one-shot exporter and keeps
      -- the historical Tabby-compatible default.
      local default_user = cfg == nil and 'root' or cfg.default_user
      if (o.user == nil or o.user == '') and default_user then
        o.user = default_user
      end
      if o.jumpHost and id_to_name[o.jumpHost] then
        o.jumpHost = id_to_name[o.jumpHost]
      end
      -- Tabby writes JS regexes; mark them so normalize_scripts translates.
      for _, sc in ipairs(o.scripts or {}) do
        if type(sc) == 'table' then
          sc.flavor = 'js'
        end
      end
      if o.auth == 'password' then
        table.insert(report.needs_password, entry.name)
      end

      table.insert(out, entry)
    end
  end

  return out, report
end

--- Read Tabby's config.yaml and convert its `ssh` profiles.
function M.from_tabby(cfg)
  local doc, path, err = M.read_tabby(cfg.import_tabby)
  if not doc then
    util.warn('tabby config: %s (%s)', err, path)
    return {}
  end
  wezterm.add_to_config_reload_watch_list(path)

  local list, report = M.convert_tabby(doc, cfg)
  util.log('imported %d ssh profile(s) from %s', #list, path)
  if #report.needs_password > 0 then
    util.warn(
      '%d profile(s) use password auth; Tabby stores those in the OS keychain, '
        .. 'not in config.yaml -- configure password_cmd/password_env for: %s',
      #report.needs_password,
      table.concat(report.needs_password, ', ')
    )
  end
  return list
end

return M
