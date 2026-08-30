-- sshmgr.argv -- translate a Tabby-shaped profile into an OpenSSH argv
local util = require 'sshmgr.util'
local caps = require 'sshmgr.caps'

local M = {}

-- Tabby SSHAlgorithmType -> ssh_config keyword
local ALGO_KEY = {
  kex = 'KexAlgorithms',
  cipher = 'Ciphers',
  hmac = 'MACs',
  serverHostKey = 'HostKeyAlgorithms',
}

local AUTH_OPTS = {
  password = {
    PreferredAuthentications = 'keyboard-interactive,password',
    PubkeyAuthentication = 'no',
  },
  publicKey = {
    PreferredAuthentications = 'publickey',
    IdentitiesOnly = 'yes',
  },
  agent = {
    PreferredAuthentications = 'publickey',
  },
  keyboardInteractive = {
    PreferredAuthentications = 'keyboard-interactive',
    PubkeyAuthentication = 'no',
  },
}

--- Render a ForwardedPortConfig into the string ssh expects after -L/-R/-D.
local function forward_spec(f)
  if f.spec then
    return f.spec
  end
  local bind = f.host
  if bind == '' then
    bind = nil
  end
  if f.type == 'Dynamic' then
    if bind then
      return string.format('%s:%d', bind, f.port)
    end
    return tostring(f.port)
  end
  local left = bind and string.format('%s:%d', bind, f.port) or tostring(f.port)
  return string.format('%s:%s:%d', left, f.targetAddress or 'localhost', f.targetPort or f.port)
end

local FORWARD_FLAG = { Local = '-L', Remote = '-R', Dynamic = '-D' }

--- Resolve `options.jumpHost`. Accepts a profile id/name (resolved against
--- `all_profiles`), a raw `[user@]host[:port]`, or a comma separated chain.
local function resolve_jump(spec, all_profiles, find)
  local parts = util.split(tostring(spec), ',')
  local out = {}
  for _, part in ipairs(parts) do
    part = util.trim(part)
    if part ~= '' then
      local hop = find and find(all_profiles, part) or nil
      if hop then
        local ho = hop.options
        local s = ho.host
        if ho.user then
          s = ho.user .. '@' .. s
        end
        if ho.port then
          s = s .. ':' .. tostring(ho.port)
        end
        table.insert(out, s)
      else
        table.insert(out, part)
      end
    end
  end
  return table.concat(out, ',')
end

--- Build the ProxyCommand for a SOCKS5 / HTTP proxy.
local function proxy_command_for(o, cfg)
  local host, port, kind
  if o.socksProxyHost then
    host, port, kind = o.socksProxyHost, o.socksProxyPort or 1080, 'socks5'
  elseif o.httpProxyHost then
    host, port, kind = o.httpProxyHost, o.httpProxyPort or 8080, 'http'
  else
    return nil
  end
  local tmpl = cfg.proxy_command_template
    or 'ncat --proxy %{proxy_host}:%{proxy_port} --proxy-type %{proxy_type} %h %p'
  return (tmpl:gsub('%%{([%w_]+)}', {
    proxy_host = host,
    proxy_port = tostring(port),
    proxy_type = kind,
  }))
end

--- Produce the argv array for `profile`.
--- Returns argv, meta where meta carries information the session layer needs.
function M.build(profile, cfg, all_profiles, find)
  local o = profile.options
  local argv = { profile.ssh_binary or cfg.ssh_binary }
  local opts = {} -- ssh_config style -o overrides, later wins

  local function set(k, v)
    if v ~= nil then
      opts[k] = tostring(v)
    end
  end

  for k, v in pairs(cfg.default_ssh_options or {}) do
    set(k, v)
  end

  ---------------------------------------------------------------------------
  -- authentication
  ---------------------------------------------------------------------------
  if o.auth and AUTH_OPTS[o.auth] then
    for k, v in pairs(AUTH_OPTS[o.auth]) do
      set(k, v)
    end
  end
  if o.auth == 'agent' or o.agentForward then
    set('IdentityAgent', o.identityAgent)
  end
  local pubkey_ok = o.auth ~= 'agent' and o.auth ~= 'password' and o.auth ~= 'keyboardInteractive'
  if pubkey_ok and o.privateKeys and #o.privateKeys > 0 then
    for _, key in ipairs(o.privateKeys) do
      table.insert(argv, '-i')
      table.insert(argv, key)
    end
    if o.auth == 'publicKey' then
      set('IdentitiesOnly', 'yes')
    end
  end
  if o.agentForward then
    table.insert(argv, '-A')
  end

  ---------------------------------------------------------------------------
  -- connection tuning
  ---------------------------------------------------------------------------
  if o.port then
    table.insert(argv, '-p')
    table.insert(argv, tostring(o.port))
  end
  if o.user then
    table.insert(argv, '-l')
    table.insert(argv, o.user)
  end
  set('ServerAliveInterval', o.keepaliveInterval)
  set('ServerAliveCountMax', o.keepaliveCountMax)
  set('ConnectTimeout', o.readyTimeout)
  if o.x11 then
    table.insert(argv, o.x11_trusted and '-Y' or '-X')
  end
  if o.skipBanner then
    set('LogLevel', 'QUIET')
  end
  if o.compression or (o.algorithms and o.algorithms.compression) then
    local c = o.compression
    if c == nil and o.algorithms then
      local list = o.algorithms.compression
      c = type(list) == 'table' and list[1] ~= 'none' or list == true
    end
    set('Compression', c and 'yes' or 'no')
  end

  ---------------------------------------------------------------------------
  -- host key policy
  ---------------------------------------------------------------------------
  local policy = profile.host_key_policy or cfg.host_key_policy
  if policy == 'accept-new' or policy == 'yes' then
    set('StrictHostKeyChecking', policy == 'yes' and 'no' or 'accept-new')
  end

  ---------------------------------------------------------------------------
  -- algorithms
  --
  -- Filtered against what this ssh client actually supports. OpenSSH rejects
  -- the whole list if a single name is unknown and exits before connecting,
  -- and Tabby exports routinely carry names from the ssh2 javascript library
  -- that OpenSSH has never had.
  ---------------------------------------------------------------------------
  local filter_algos = cfg.filter_algorithms ~= false
  for tabby_key, ssh_key in pairs(ALGO_KEY) do
    local list = o.algorithms and o.algorithms[tabby_key]
    if type(list) == 'string' and list ~= '' then
      list = util.split(list, ',')
    end
    if type(list) == 'table' and #list > 0 then
      local dropped = {}
      if filter_algos then
        list, dropped = caps.filter(list, tabby_key, argv[1])
      end
      if #dropped > 0 then
        util.warn(
          '%s: %s does not support %s; dropped from %s',
          profile.name,
          argv[1],
          table.concat(dropped, ', '),
          ssh_key
        )
      end
      if list and #list > 0 then
        set(ssh_key, table.concat(list, ','))
      elseif #dropped > 0 then
        util.warn(
          '%s: no usable %s left after filtering; omitting the option so ssh uses its defaults',
          profile.name,
          ssh_key
        )
      end
    end
  end

  ---------------------------------------------------------------------------
  -- jump host / proxies
  ---------------------------------------------------------------------------
  if o.jumpHost and o.jumpHost ~= '' then
    local j = resolve_jump(o.jumpHost, all_profiles, find)
    if j ~= '' then
      table.insert(argv, '-J')
      table.insert(argv, j)
    end
  end
  if o.proxyCommand and o.proxyCommand ~= '' then
    set('ProxyCommand', o.proxyCommand)
  else
    local pc = proxy_command_for(o, cfg)
    if pc then
      set('ProxyCommand', pc)
    end
  end

  ---------------------------------------------------------------------------
  -- port forwarding
  ---------------------------------------------------------------------------
  for _, f in ipairs(o.forwardedPorts or {}) do
    local flag = FORWARD_FLAG[f.type]
    if flag then
      table.insert(argv, flag)
      table.insert(argv, forward_spec(f))
    end
  end

  ---------------------------------------------------------------------------
  -- connection sharing (Tabby: reuseSession). Not available on Win32 OpenSSH.
  ---------------------------------------------------------------------------
  if o.reuseSession then
    if util.is_windows then
      util.warn(
        'profile %q sets reuseSession, but ControlMaster is not supported by the Windows OpenSSH client; ignoring',
        profile.name
      )
    else
      set('ControlMaster', 'auto')
      set('ControlPath', o.controlPath or '~/.ssh/wezterm-%r@%h:%p')
      set('ControlPersist', o.controlPersist or '10m')
    end
  end

  ---------------------------------------------------------------------------
  -- raw passthrough (profile.ssh_options always wins)
  ---------------------------------------------------------------------------
  for k, v in pairs(profile.ssh_options or {}) do
    set(k, v)
  end

  local keys = {}
  for k in pairs(opts) do
    table.insert(keys, k)
  end
  table.sort(keys)
  for _, k in ipairs(keys) do
    table.insert(argv, '-o')
    table.insert(argv, k .. '=' .. opts[k])
  end

  util.tbl_concat_into(argv, profile.extra_args)

  ---------------------------------------------------------------------------
  -- destination + optional remote command
  ---------------------------------------------------------------------------
  local remote_cmd = profile.remote_command
  if not remote_cmd and profile.cwd then
    remote_cmd = string.format('cd %s && exec "$SHELL" -l', util.sh_quote(profile.cwd))
  end
  if remote_cmd then
    table.insert(argv, '-t')
  end
  table.insert(argv, o.host)
  if remote_cmd then
    table.insert(argv, remote_cmd)
  end

  return argv, { ssh_options = opts, has_remote_command = remote_cmd ~= nil }
end

--- Wrap `argv` so the pane survives (or retries) the ssh process exiting.
--- `behavior` is Tabby's behaviorOnSessionEnd: 'close' | 'keep' | 'reconnect'.
function M.wrap_for_session_end(argv, behavior, profile)
  if behavior == nil or behavior == 'close' or behavior == 'auto' then
    return argv
  end

  if util.is_windows then
    -- Everything below is single-quoted on purpose: the whole script travels
    -- as one argv element through wezterm -> CreateProcess -> powershell, and
    -- a double quote anywhere in it would have to survive two layers of
    -- escaping. Splatting via a variable ($a) is also deliberate --
    -- `& $exe @('a','b')` does not reliably unroll into separate arguments.
    local quoted = {}
    for _, a in ipairs(argv) do
      table.insert(quoted, util.ps_quote(a))
    end
    local exe = quoted[1]
    local rest = table.concat(quoted, ',', 2, #quoted)
    local prelude = string.format('$ErrorActionPreference=%s; $a=@(%s); ', util.ps_quote 'Continue', rest)
    local call = string.format('& %s @a', exe)
    local label = util.ps_quote(profile and profile.name or 'session')
    local script
    if behavior == 'reconnect' then
      script = string.format(
        '%swhile ($true) { %s; Write-Host %s; Write-Host '
          .. '([string]::Format(%s, %s)) -ForegroundColor Yellow; Start-Sleep -Seconds 3 }',
        prelude,
        call,
        util.ps_quote '',
        util.ps_quote '[ssh-manager] {0} disconnected, reconnecting in 3s (Ctrl-C to stop)',
        label
      )
    else
      script = string.format(
        '%s%s; Write-Host %s; Read-Host %s',
        prelude,
        call,
        util.ps_quote '',
        util.ps_quote '[ssh-manager] session ended - press Enter to close'
      )
    end
    return { 'powershell.exe', '-NoLogo', '-NoProfile', '-Command', script }
  end

  local quoted = {}
  for _, a in ipairs(argv) do
    table.insert(quoted, util.sh_quote(a))
  end
  local call = table.concat(quoted, ' ')
  local label = util.sh_quote(profile and profile.name or 'session')
  local script
  if behavior == 'reconnect' then
    script = 'while true; do '
      .. call
      .. '; printf \'\\n[ssh-manager] %s disconnected, reconnecting in 3s (Ctrl-C to stop)\\n\' '
      .. label
      .. '; sleep 3; done'
  else
    script = call
      .. '; printf \'\\n[ssh-manager] session ended - press Enter to close\'; read -r _'
  end
  return { 'sh', '-c', script }
end

--- The process names wezterm should not warn about when closing this pane.
function M.close_confirmation_names(profiles, cfg)
  local names = {}
  local want = false
  for _, p in ipairs(profiles) do
    if p.options.warnOnClose == false then
      want = true
    end
  end
  if want then
    names = { util.is_windows and 'ssh.exe' or 'ssh' }
  end
  return names
end

return M
