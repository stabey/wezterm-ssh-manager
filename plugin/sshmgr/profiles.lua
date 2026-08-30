-- sshmgr.profiles -- load, normalise and index the profile list
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'

local M = {}
local ORIGINS = setmetatable({}, { __mode = 'k' })

--- Return loader metadata for a normalised profile without serialising that
--- metadata when callers copy the profile into another source.
function M.origin(profile)
  return ORIGINS[profile]
end

-- Option keys that Tabby spells in camelCase. We accept snake_case too so a
-- hand-written lua profile can look like the rest of a wezterm config.
local ALIASES = {
  private_keys = 'privateKeys',
  keepalive_interval = 'keepaliveInterval',
  keepalive_count_max = 'keepaliveCountMax',
  ready_timeout = 'readyTimeout',
  skip_banner = 'skipBanner',
  jump_host = 'jumpHost',
  agent_forward = 'agentForward',
  warn_on_close = 'warnOnClose',
  proxy_command = 'proxyCommand',
  forwarded_ports = 'forwardedPorts',
  socks_proxy_host = 'socksProxyHost',
  socks_proxy_port = 'socksProxyPort',
  http_proxy_host = 'httpProxyHost',
  http_proxy_port = 'httpProxyPort',
  reuse_session = 'reuseSession',
  login_scripts = 'scripts',
}

local function apply_aliases(options)
  for from, to in pairs(ALIASES) do
    if options[from] ~= nil and options[to] == nil then
      options[to] = options[from]
      options[from] = nil
    end
  end
  return options
end

--- Tabby stores keepalive/readyTimeout in milliseconds; ssh wants seconds.
--- Anything above 300 is assumed to be milliseconds.
local function to_seconds(v)
  v = tonumber(v)
  if not v then
    return nil
  end
  if v > 300 then
    return math.floor(v / 1000 + 0.5)
  end
  return math.floor(v)
end

function M.normalize_scripts(scripts)
  local out = {}
  for _, s in ipairs(scripts or {}) do
    if type(s) == 'string' then
      -- shorthand: a bare string is an unconditional command
      table.insert(out, { expect = '', send = s })
    elseif type(s) == 'table' then
      local step = {
        expect = s.expect or '',
        send = s.send,
        isRegex = s.isRegex or s.is_regex or false,
        -- 'lua' (default) or 'js'. Tabby stores JS regexes; wezterm's lua only
        -- has lua patterns, so those get translated on the way in.
        flavor = s.flavor or s.regex_flavor or 'lua',
        optional = s.optional or false,
        raw = s.raw or false, -- do not append a newline
        hide = s.hide or false, -- do not echo the payload into the log
        prompt = s.prompt, -- ask the user instead of sending a literal
        timeout = s.timeout,
        delay = s.delay, -- extra pause before sending
      }
      if step.isRegex and step.flavor == 'js' then
        local converted, why = util.js_regex_to_lua(step.expect)
        if converted then
          step.expect = converted
        else
          util.warn(
            'login script expect %q uses %s, which lua patterns cannot express; '
              .. 'falling back to a plain substring match',
            step.expect,
            why
          )
          step.isRegex = false
        end
        step.flavor = 'lua'
      end
      table.insert(out, step)
    end
  end
  return out
end

local function normalize_forwards(list)
  local out = {}
  for _, f in ipairs(list or {}) do
    if type(f) == 'string' then
      -- "L 8080:localhost:80" / "D 1080" / "R 9000:localhost:9000"
      local kind, rest = f:match '^([LRDlrd])%s+(.+)$'
      if kind then
        local t = ({ l = 'Local', r = 'Remote', d = 'Dynamic' })[kind:lower()]
        table.insert(out, { type = t, spec = rest })
      end
    elseif type(f) == 'table' then
      table.insert(out, {
        type = f.type or 'Local',
        host = f.host,
        port = f.port,
        targetAddress = f.targetAddress or f.target_address,
        targetPort = f.targetPort or f.target_port,
        description = f.description,
        spec = f.spec,
      })
    end
  end
  return out
end

--- Bring one raw profile entry into canonical shape.
function M.normalize(raw, cfg)
  local p = util.deep_copy(raw)

  -- Allow the flat form: { name='x', host='y', user='z', scripts={...} }
  p.options = p.options or {}
  for _, k in ipairs {
    'host', 'sftpHost', 'port', 'user', 'auth', 'password', 'password_env', 'password_cmd',
    'privateKeys', 'private_keys', 'identityAgent', 'jumpHost', 'jump_host', 'agentForward',
    'agent_forward', 'x11', 'scripts', 'login_scripts', 'forwardedPorts',
    'forwarded_ports', 'proxyCommand', 'proxy_command', 'algorithms',
    'skipBanner', 'skip_banner', 'warnOnClose', 'warn_on_close', 'reuseSession',
    'reuse_session', 'keepaliveInterval', 'keepalive_interval',
    'keepaliveCountMax', 'keepalive_count_max', 'readyTimeout', 'ready_timeout',
    'socksProxyHost', 'socks_proxy_host', 'socksProxyPort', 'socks_proxy_port',
    'httpProxyHost', 'http_proxy_host', 'httpProxyPort', 'http_proxy_port',
  } do
    if p[k] ~= nil and p.options[k] == nil then
      p.options[k] = p[k]
      p[k] = nil
    end
  end

  -- group defaults, then global defaults
  if p.group and cfg.groups[p.group] then
    util.defaults_into(p, cfg.groups[p.group])
  end
  util.defaults_into(p, cfg.defaults)

  apply_aliases(p.options)

  local o = p.options
  o.host = o.host or p.name
  o.port = tonumber(o.port) or nil
  if type(o.privateKeys) == 'string' then
    o.privateKeys = { o.privateKeys }
  end
  for i, k in ipairs(o.privateKeys or {}) do
    o.privateKeys[i] = util.expand_path(k)
  end
  o.keepaliveInterval = to_seconds(o.keepaliveInterval)
  o.readyTimeout = to_seconds(o.readyTimeout)
  o.keepaliveCountMax = tonumber(o.keepaliveCountMax)
  o.scripts = M.normalize_scripts(o.scripts)
  o.forwardedPorts = normalize_forwards(o.forwardedPorts)

  p.name = p.name or o.host or 'unnamed'
  p.id = p.id or ((p.group and (p.group .. '/') or '') .. p.name)
  p.group = p.group or ''
  p.weight = tonumber(p.weight) or 0
  p.behaviorOnSessionEnd = p.behaviorOnSessionEnd or p.on_session_end or 'close'
  p.ssh_options = p.ssh_options or {}
  p.extra_args = p.extra_args or {}
  p.env = p.env or {}

  return p
end

--- Decode one profile file. Returns a list (possibly empty). Store writes are
--- atomic, so every source can safely participate in config-reload watching.
local function load_file(path, watch)
  path = util.expand_path(path)
  local data = util.read_file(path)
  if not data then
    util.warn('profile file not found: %s', path)
    return {}
  end
  local lower = path:lower()
  local ok, decoded
  if lower:match '%.lua$' then
    local chunk, err = load(data, '@' .. path, 't')
    if not chunk then
      util.err('cannot parse %s: %s', path, err)
      return {}
    end
    ok, decoded = pcall(chunk)
  elseif lower:match '%.json$' then
    ok, decoded = pcall(wezterm.serde.json_decode, data)
  elseif lower:match '%.ya?ml$' then
    ok, decoded = pcall(wezterm.serde.yaml_decode, data)
  elseif lower:match '%.toml$' then
    ok, decoded = pcall(wezterm.serde.toml_decode, data)
  else
    util.err('unknown profile file type: %s', path)
    return {}
  end
  if not ok then
    util.err('cannot decode %s: %s', path, tostring(decoded))
    return {}
  end
  if watch ~= false then
    wezterm.add_to_config_reload_watch_list(path)
  end
  if type(decoded) ~= 'table' then
    return {}
  end
  -- accept either a bare list or { profiles = {...}, groups = {...} }
  if decoded.profiles then
    return decoded.profiles, decoded.groups
  end
  return decoded
end

--- Build the full, normalised, sorted profile list for `cfg`.
function M.load(cfg)
  local raw = {}

  -- Keep the origin on the normalised in-memory profile. Password capture
  -- needs to distinguish Tabby profiles (where auth is commonly omitted but
  -- password fallback is intentional) from ordinary OpenSSH/inline profiles.
  -- The marker is private and is stripped before a profile is copied into the
  -- editable store.
  local function append(list, kind, path)
    for _, entry in ipairs(list or {}) do
      local copy = util.deep_copy(entry)
      copy._sshmgr_origin = {
        kind = kind,
        path = path,
        -- Retain the source shape out-of-band. If a non-store profile later
        -- needs a password saved, panel.lua can copy this instead of a
        -- normalised object containing expanded/default/runtime values.
        raw = util.deep_copy(entry),
      }
      table.insert(raw, copy)
    end
  end

  append(cfg.profiles, 'inline')

  -- the panel's store is always read, even if the user never listed it
  local files = {}
  local store = require('sshmgr.panel').store_path(cfg)
  -- Watching the final path is safe because panel.save_store writes a
  -- complete temporary file and renames it into place. This also makes hand
  -- edits visible without having to use the panel's explicit reload action.
  if util.file_exists(store) then
    table.insert(files, store)
  else
    -- Some watcher backends can also observe creation of a missing path.
    pcall(wezterm.add_to_config_reload_watch_list, store)
  end
  for _, path in ipairs(cfg.profile_files or {}) do
    if util.expand_path(path) ~= store then
      table.insert(files, path)
    end
  end

  for _, path in ipairs(files) do
    local expanded = util.expand_path(path)
    local list, groups = load_file(path, true)
    if groups then
      for k, v in pairs(groups) do
        cfg.groups[k] = cfg.groups[k] or v
      end
    end
    append(list, expanded == store and 'store' or 'file', expanded)
  end

  if cfg.import_ssh_config then
    local importer = require 'sshmgr.import'
    append(importer.from_ssh_config(cfg), 'ssh_config')
  end

  if cfg.import_tabby then
    local importer = require 'sshmgr.import'
    append(importer.from_tabby(cfg), 'tabby')
  end

  local out, seen = {}, {}
  for _, r in ipairs(raw) do
    local ok, p = pcall(M.normalize, r, cfg)
    if not ok then
      util.err('bad profile entry: %s', tostring(p))
    elseif seen[p.id] then
      util.warn('duplicate profile id %q ignored', p.id)
    else
      local origin = p._sshmgr_origin
      p._sshmgr_origin = nil
      ORIGINS[p] = origin
      seen[p.id] = true
      table.insert(out, p)
    end
  end
  return util.sort_profiles(out)
end

--- Look a profile up by id, then by name, then case-insensitively by name.
function M.find(list, key)
  if type(key) == 'table' then
    return key
  end
  for _, p in ipairs(list) do
    if p.id == key then
      return p
    end
  end
  for _, p in ipairs(list) do
    if p.name == key then
      return p
    end
  end
  local lk = tostring(key):lower()
  for _, p in ipairs(list) do
    if p.name:lower() == lk or p.id:lower() == lk then
      return p
    end
  end
  return nil
end

return M
