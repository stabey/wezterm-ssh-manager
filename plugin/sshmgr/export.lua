-- sshmgr.export -- one-shot conversion of Tabby's config.yaml into a profile
-- file this plugin can read.
--
-- You do not need this to use Tabby's config: `import_tabby = true` reads it
-- live. Export is for people who want to leave Tabby behind and own a plain
-- file they can edit and put in git.
local _MODNAME, THIS_FILE = ...

local wezterm = require 'wezterm'
local util = require 'sshmgr.util'
local importer = require 'sshmgr.import'
local caps = require 'sshmgr.caps'
local serialize = require 'sshmgr.serialize'
local nf = wezterm.nerdfonts

-- Tabby uses Font Awesome class names; translate the common ones.
local ICON_MAP = {
  ['fas fa-server'] = nf.md_server,
  ['fas fa-desktop'] = nf.md_desktop_classic,
  ['fas fa-database'] = nf.md_database,
  ['fas fa-network-wired'] = nf.md_lan,
  ['fas fa-shield-alt'] = nf.md_shield_lock,
  ['fas fa-lock'] = nf.md_lock,
  ['fas fa-cloud'] = nf.md_cloud,
  ['fas fa-cube'] = nf.md_cube_outline,
  ['fas fa-cubes'] = nf.md_kubernetes,
  ['fas fa-hdd'] = nf.md_harddisk,
  ['fas fa-terminal'] = nf.md_console,
  ['fab fa-linux'] = nf.md_linux,
  ['fab fa-ubuntu'] = nf.md_ubuntu,
  ['fab fa-redhat'] = nf.md_redhat,
  ['fab fa-centos'] = nf.md_centos,
  ['fab fa-docker'] = nf.md_docker,
  ['fab fa-windows'] = nf.md_microsoft_windows,
  ['fab fa-apple'] = nf.md_apple,
  ['fab fa-raspberry-pi'] = nf.md_raspberry_pi,
}

local function slug(s)
  return (tostring(s):upper():gsub('[^%w]+', '_'):gsub('^_+', ''):gsub('_+$', ''))
end

--- The credential-store entry Tabby used for this profile.
---
--- keytar's target name is "<service>/<account>". Tabby writes the yaml without
--- `port` when it is 22, but savePassword runs after the session fills in the
--- default, so the Windows credential is always "ssh@host:port/user" --
--- including `:22`. Looking up `ssh@host/user` returns not-found.
--- Computed from the *raw* tabby options, before we strip a redundant port == 22.
local function keytar_target(host, port, user)
  local p = tonumber(port)
  if not p or p <= 0 then
    p = 22
  end
  return string.format('ssh@%s:%d/%s', tostring(host), math.floor(p), tostring(user or ''))
end

--- Enumerate credential-manager target names via credman.ps1 -List -NamesOnly.
local function list_credman_targets(path)
  if not path then
    return {}
  end
  local argv = {
    'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
    '-File', path, '-List', '-NamesOnly',
  }
  local ok, success, stdout, stderr = pcall(wezterm.run_child_process, argv)
  stdout = tostring(stdout or '')
  stderr = tostring(stderr or '')
  -- PowerShell 5.1 often emits UTF-16LE on a captured pipe.
  stdout = stdout:gsub('%z', ''):gsub('^\239\187\191', ''):gsub('^\255\254', ''):gsub('^\254\255', '')
  stderr = stderr:gsub('%z', '')
  if not ok or not success then
    util.warn('could not list credential-manager targets: %s', stderr ~= '' and stderr or tostring(success))
    return {}
  end
  local set = {}
  for t in stdout:gmatch('ssh@%S+') do
    set[t] = true
  end
  local n = 0
  for _ in pairs(set) do n = n + 1 end
  util.log('credman listed %d target(s) (stdout %d bytes)', n, #stdout)
  if n == 0 then
    util.warn('credman -List produced no targets; sample=%q stderr=%q', stdout:sub(1, 160), stderr:sub(1, 160))
  end
  return set
end

--- Locate tools/credman.ps1 inside this checkout.
local function credman_source()
  if type(THIS_FILE) ~= 'string' then
    return nil
  end
  local root = THIS_FILE:match '^(.*)[/\\]plugin[/\\]sshmgr[/\\]export%.lua$'
  if not root then
    return nil
  end
  local path = root .. util.sep .. 'tools' .. util.sep .. 'credman.ps1'
  return util.file_exists(path) and path or nil
end

local M = {}

local function default_ssh()
  return util.is_windows and 'ssh.exe' or 'ssh'
end

--- Turn the imported entries into something worth writing to disk.
--- Returns profiles, report.
local function prepare(entries, opts)
  local report = {
    regex_failed = {}, regex_ok = 0, icons_dropped = {},
    password = {}, algos_dropped = {},
  }

  for _, p in ipairs(entries) do
    p.id = nil -- ids are derived from group/name; a stale tabby id helps nobody

    if p.icon then
      local mapped = ICON_MAP[p.icon]
      if mapped then
        p.icon = mapped
      else
        table.insert(report.icons_dropped, p.icon)
        p.icon = nil
      end
    end
    if p.weight == 0 then
      p.weight = nil
    end
    if p.behaviorOnSessionEnd == 'auto' or p.behaviorOnSessionEnd == 'close' then
      p.behaviorOnSessionEnd = nil
    end

    local o = p.options or {}

    -- must be computed before the port == 22 strip below
    local cred_target = keytar_target(o.host, o.port, o.user)

    -- keepalive/timeout come out of tabby in milliseconds
    for _, k in ipairs { 'keepaliveInterval', 'readyTimeout' } do
      local v = tonumber(o[k])
      if v and v > 300 then
        o[k] = math.floor(v / 1000 + 0.5)
      end
    end

    -- strip the fields tabby writes at their default value
    if o.port == 22 then
      o.port = nil
    end
    for _, k in ipairs { 'x11', 'skipBanner', 'agentForward', 'reuseSession' } do
      if o[k] == false then
        o[k] = nil
      end
    end
    -- Tabby's algorithm panel defaults to selecting every entry in its menu,
    -- and those names come from the ssh2 javascript library. OpenSSH rejects
    -- the whole list on one unknown name, so filter against the real client.
    if o.algorithms then
      local any = false
      for k, v in pairs(o.algorithms) do
        if type(v) == 'table' and #v > 0 and opts.filter_algorithms ~= false then
          local kept, dropped = caps.filter(v, k, opts.ssh_binary or default_ssh())
          if #dropped > 0 then
            report.algos_dropped[k] = report.algos_dropped[k] or {}
            for _, name in ipairs(dropped) do
              report.algos_dropped[k][name] = true
            end
          end
          v = kept
          o.algorithms[k] = kept
        end
        if type(v) ~= 'table' or #v == 0 then
          o.algorithms[k] = nil
        else
          any = true
        end
      end
      if not any then
        o.algorithms = nil
      end
    end
    for _, k in ipairs { 'privateKeys', 'forwardedPorts', 'scripts' } do
      if type(o[k]) == 'table' and #o[k] == 0 then
        o[k] = nil
      end
    end

    -- translate the JS regexes now, so the written file is self-contained
    for _, sc in ipairs(o.scripts or {}) do
      if type(sc) == 'table' then
        sc.flavor = nil
        if sc.isRegex then
          local converted, why = util.js_regex_to_lua(sc.expect or '')
          if converted then
            sc.expect = converted
            report.regex_ok = report.regex_ok + 1
          else
            sc.isRegex = nil
            table.insert(report.regex_failed, {
              profile = p.name,
              pattern = sc.expect,
              why = why,
            })
          end
        end
      end
    end

    -- Tabby often leaves `auth` unset (try keys, then password) but still
    -- saves the password in the credential store. Wire a hook whenever the
    -- matching target exists, not only when auth is explicitly 'password'.
    local want_password = o.auth == 'password' or (opts.known_cred_targets or {})[cred_target]
    if want_password and opts.password_mode ~= 'none' then
      o.password = nil
      local entry = { profile = p.name, target = cred_target }
      if opts.credman_path then
        o.password_cmd = {
          'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
          '-File', opts.credman_path, '-Target', cred_target,
        }
        entry.mode = 'credman'
      else
        o.password_env = 'WEZTERM_SSH_' .. slug(p.name)
        entry.mode = 'env'
        entry.env = o.password_env
      end
      table.insert(report.password, entry)
    end
  end

  util.sort_profiles(entries)
  return entries, report
end

local function header(path, src, report, tabby_report, credman_out)
  local L = {}
  local function add(fmt, ...)
    table.insert(L, select('#', ...) > 0 and string.format(fmt, ...) or fmt)
  end
  add '-- Generated by wezterm-ssh-manager from a Tabby configuration.'
  add('-- source: %s', src)
  add('-- date:   %s', wezterm.strftime '%Y-%m-%d %H:%M:%S')
  add '--'
  add '-- Use it from your wezterm.lua:'
  add('--   profile_files = { %q },', path)
  add '--'

  if #tabby_report.skipped > 0 then
    local names = {}
    for _, s in ipairs(tabby_report.skipped) do
      table.insert(names, string.format('%s (%s)', s.name, s.type))
    end
    add('-- Skipped %d non-ssh profile(s): %s', #tabby_report.skipped, table.concat(names, ', '))
    add '--'
  end

  if tabby_report.vault_enabled then
    add '-- VAULT IS ENABLED IN TABBY'
    add '-- Your passwords are inside Tabby\'s own encrypted vault (config.yaml ->'
    add '-- vault.contents, AES with your master passphrase), not the OS credential'
    add '-- store. They cannot be read from here. Open Tabby, disable the vault so'
    add '-- it writes secrets back to the credential store, then re-run the export --'
    add '-- or set each password by hand.'
    add '--'
  end

  if #report.password > 0 then
    local credman = report.password[1].mode == 'credman'
    add '-- PASSWORDS'
    add '-- Tabby never writes SSH passwords into config.yaml; keytar puts them in'
    add '-- the Windows Credential Manager under the target "ssh@host[:port]/user".'
    if credman then
      add '-- Each password profile below reads its password straight back out of the'
      add '-- credential store at connect time via tools/credman.ps1, so nothing has'
      add '-- to be retyped and no password ends up in this file.'
      add '--'
      add '-- Check what is actually stored:'
      add('--   powershell -NoProfile -ExecutionPolicy Bypass -File %s -List', credman_out or 'credman.ps1')
      add '-- Read one:'
      add('--   powershell -NoProfile -ExecutionPolicy Bypass -File %s -Target %q',
        credman_out or 'credman.ps1', report.password[1].target)
    else
      add '-- credman.ps1 was not found next to this export, so each profile got a'
      add '-- `password_env` hook instead: set that variable and it works. Swap it for'
      add '-- `password_cmd = {...}` to read from a password manager.'
    end
    add '--'
    for _, e in ipairs(report.password) do
      if e.mode == 'credman' then
        add('--   %-26s %s', e.profile, e.target)
      else
        add('--   %-26s $env:%s   (was: %s)', e.profile, e.env, e.target)
      end
    end
    add '--'
  end

  local any_algo = false
  for _ in pairs(report.algos_dropped) do
    any_algo = true
  end
  if any_algo then
    add '-- ALGORITHMS DROPPED'
    add '-- Tabby\'s algorithm panel selects every entry in its menu by default, and'
    add '-- that menu comes from the ssh2 javascript library. OpenSSH validates these'
    add '-- lists strictly and exits before connecting if one name is unknown, which'
    add '-- looks like a pane that flashes open and closes. These names are not known'
    add '-- to your ssh client and were removed:'
    for kind, names in pairs(report.algos_dropped) do
      local list = {}
      for n in pairs(names) do
        table.insert(list, n)
      end
      table.sort(list)
      add('--   %-14s %s', kind, table.concat(list, ', '))
    end
    add '-- Legacy algorithms your client does still support (3des-cbc, ssh-rsa,'
    add '-- hmac-md5, group1-sha1, ...) were kept, so old devices still connect.'
    add '--'
  end

  if #report.regex_failed > 0 then
    add '-- LOGIN SCRIPT REGEXES THAT NEED A LOOK'
    add '-- These used JS regex features lua patterns do not have. They were'
    add '-- downgraded to a plain substring match, which usually still works --'
    add '-- but check them.'
    for _, r in ipairs(report.regex_failed) do
      add('--   %s: %q  (%s)', r.profile, r.pattern, r.why)
    end
    add '--'
  end

  if #report.icons_dropped > 0 then
    local seen, uniq = {}, {}
    for _, i in ipairs(report.icons_dropped) do
      if not seen[i] then
        seen[i] = true
        table.insert(uniq, i)
      end
    end
    add(
      '-- Dropped %d Font Awesome icon name(s) with no nerdfont equivalent: %s',
      #uniq,
      table.concat(uniq, ', ')
    )
    add '-- Pick replacements from wezterm.nerdfonts if you want them back.'
    add '--'
  end

  return table.concat(L, '\n')
end

--- Convert Tabby's config.yaml into a profile file.
---
---   sshmgr.export_tabby {
---     from  = true,                       -- or an explicit config.yaml path
---     to    = '~/.config/wezterm/ssh_profiles.lua',
---     format = 'lua',                     -- 'lua' | 'yaml' | 'json'
---     force = false,                      -- overwrite an existing file
---     password_mode = 'credman',          -- 'credman' | 'env' | 'none'
---     filter_algorithms = true,
---   }
---
--- Returns ok, message.
function M.tabby(opts)
  opts = opts or {}
  local doc, src, err = importer.read_tabby(opts.from)
  if not doc then
    return false, string.format('cannot read tabby config: %s (%s)', err, src)
  end

  local format = opts.format or 'lua'
  local to = util.expand_path(
    opts.to or (wezterm.config_dir .. util.sep .. 'ssh_profiles.' .. (format == 'lua' and 'lua' or format))
  )

  if util.file_exists(to) and not opts.force then
    return false,
      string.format('%s already exists; pass force = true to overwrite it', to)
  end

  local entries, tabby_report = importer.convert_tabby(doc)
  if #entries == 0 then
    return false, string.format('no ssh profiles found in %s', src)
  end

  -- Put a copy of the credential-manager reader next to the generated file, so
  -- the password hooks in it point at something that will still be there if the
  -- plugin checkout moves.
  local run = {}
  for k, v in pairs(opts) do
    run[k] = v
  end
  local mode = opts.password_mode or (util.is_windows and 'credman' or 'env')
  if mode == 'credman' then
    local from = credman_source()
    local out_dir = to:match('^(.*)[/\\][^/\\]+$') or '.'
    local dest = out_dir .. util.sep .. 'credman.ps1'
    if from and (from ~= dest) then
      local data = util.read_file(from)
      local f = data and io.open(dest, 'wb')
      if f then
        f:write(data)
        f:close()
        run.credman_path = dest
      end
    elseif util.file_exists(dest) then
      run.credman_path = dest
    end
    if not run.credman_path then
      util.warn 'could not place credman.ps1 next to the export; falling back to password_env hooks'
    else
      run.known_cred_targets = list_credman_targets(run.credman_path)
    end
  end

  local profiles, report = prepare(entries, run)
  opts = run

  local body
  if format == 'lua' then
    body = header(to, src, report, tabby_report, opts.credman_path)
      .. '\n\nreturn {\n  profiles = '
      .. serialize.encode(profiles, 1)
      .. ',\n}\n'
  elseif format == 'yaml' then
    body = wezterm.serde.yaml_encode { profiles = profiles }
  elseif format == 'json' then
    body = wezterm.serde.json_encode_pretty { profiles = profiles }
  else
    return false, 'unknown format: ' .. tostring(format)
  end

  local f, ferr = io.open(to, 'wb')
  if not f then
    return false, string.format('cannot write %s: %s', to, tostring(ferr))
  end
  f:write(body)
  f:close()

  local msg = string.format(
    '%d profile(s) -> %s  (%d skipped, %d password hook(s), %d regex(es) need review)',
    #profiles,
    to,
    #tabby_report.skipped,
    #report.password,
    #report.regex_failed
  )
  util.log('%s', msg)
  return true, msg, { path = to, report = report, tabby = tabby_report }
end

M.prepare = prepare

return M
