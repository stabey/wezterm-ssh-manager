-- sshmgr.tui -- spawn the OpenTUI/Textual manager tab and dispatch OSC commands
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'
local panel = require 'sshmgr.panel'
local session = require 'sshmgr.session'

local M = {}

local attached
local _, THIS_FILE = ...
local GLOBAL_PREV = 'sshmgr_tui_prev'
local GLOBAL_SESSIONS = 'sshmgr_tui_sessions_v2'

local PIP_HINT = 'python -m pip install textual==8.2.8'
local GLOBAL_TAB = 'sshmgr_tui_tab'
local MAX_REQUEST_BYTES = 4 * 1024 * 1024

---------------------------------------------------------------------------
-- helpers
---------------------------------------------------------------------------

local function toast(window, msg, ms)
  if window and window.toast_notification then
    pcall(function()
      window:toast_notification('wezterm ssh-manager', msg, nil, ms or 5000)
    end)
  end
  util.log('%s', msg)
end

local function json_encode(v)
  local ok, s = pcall(wezterm.serde.json_encode, v)
  if ok and type(s) == 'string' then
    return s
  end
  return nil, tostring(s)
end

local function json_decode(s)
  if type(s) ~= 'string' or s == '' then
    return nil
  end
  local ok, v = pcall(wezterm.serde.json_decode, s)
  if ok and type(v) == 'table' then
    return v
  end
  if wezterm.base64 and wezterm.base64.decode then
    local dok, raw = pcall(wezterm.base64.decode, s)
    if dok and type(raw) == 'string' then
      ok, v = pcall(wezterm.serde.json_decode, raw)
      if ok and type(v) == 'table' then
        return v
      end
    end
  end
  return nil
end

local function sessions()
  local current = wezterm.GLOBAL[GLOBAL_SESSIONS]
  if type(current) ~= 'table' then
    current = {}
  end
  return current
end

local function save_sessions(current)
  wezterm.GLOBAL[GLOBAL_SESSIONS] = current
end

local function pane_id(pane)
  if not pane or not pane.pane_id then
    return nil
  end
  local ok, id = pcall(function()
    return pane:pane_id()
  end)
  if ok and id ~= nil then
    return tostring(id)
  end
  return nil
end

local function resolve_pane(id)
  if id == nil or not wezterm.mux or not wezterm.mux.get_pane then
    return nil
  end
  local ok, pane = pcall(wezterm.mux.get_pane, tonumber(id) or id)
  if ok then
    return pane
  end
  return nil
end

local function session_for_pane(pane)
  local id = pane_id(pane)
  if not id then
    return nil, nil
  end
  return sessions()[id], id
end

local function store_key(p)
  local name = tostring(p.name or '')
  if p.group and p.group ~= '' then
    return p.group .. '/' .. name
  end
  return name
end

local function profile_id(p)
  if p.id ~= nil and tostring(p.id) ~= '' then
    return tostring(p.id)
  end
  return store_key(p)
end

local function jsonable(v, depth)
  depth = depth or 0
  if depth > 12 then
    return nil
  end
  local tv = type(v)
  if tv == 'string' or tv == 'number' or tv == 'boolean' then
    return v
  end
  if tv ~= 'table' then
    return nil
  end
  local array = true
  local n = 0
  for k in pairs(v) do
    if type(k) ~= 'number' then
      array = false
      break
    end
    n = n + 1
  end
  if array then
    local out = {}
    for i, x in ipairs(v) do
      out[i] = jsonable(x, depth + 1)
    end
    return out
  end
  local out = {}
  for k, x in pairs(v) do
    if type(k) == 'string' then
      local y = jsonable(x, depth + 1)
      if y ~= nil then
        out[k] = y
      end
    end
  end
  return out
end

local function strip_password(p)
  local c = util.deep_copy(p)
  c.password = nil
  if type(c.options) == 'table' then
    c.options.password = nil
  end
  return c
end

local function has_secret(p)
  local o = (p and p.options) or p or {}
  if type(o.password) == 'string' and o.password ~= '' then
    return true
  end
  if type(p.password) == 'string' and p.password ~= '' then
    return true
  end
  if o.password_env and o.password_env ~= '' then
    return true
  end
  if o.password_cmd ~= nil then
    return true
  end
  return false
end

local function dir_of(path)
  if type(path) ~= 'string' then
    return nil
  end
  return path:match '^(.*)[/\\][^/\\]+$'
end

local function basename(path)
  if type(path) ~= 'string' then
    return ''
  end
  return path:match '([^/\\]+)$' or path
end

local function copy_argv(argv)
  if type(argv) ~= 'table' or #argv == 0 then
    return nil, 'ui.tui.command must be a non-empty argv array'
  end
  local out = {}
  for i, value in ipairs(argv) do
    if type(value) ~= 'string' or value == '' then
      return nil, string.format('ui.tui.command[%d] must be a non-empty string', i)
    end
    out[i] = value
  end
  for key in pairs(argv) do
    if type(key) ~= 'number' or key < 1 or key > #out or key ~= math.floor(key) then
      return nil, 'ui.tui.command must be a dense argv array'
    end
  end
  return out
end

M._copy_argv = copy_argv

function M.plugin_dir(state)
  if state and state.plugin_dir then
    return state.plugin_dir
  end
  if type(THIS_FILE) == 'string' then
    -- .../plugin/sshmgr/tui.lua -> .../plugin
    local sshmgr_dir = dir_of(THIS_FILE)
    local plugin_dir = dir_of(sshmgr_dir)
    if plugin_dir then
      return plugin_dir
    end
  end
  local sep = package.config:sub(1, 1)
  for entry in (package.path .. ';'):gmatch '(.-);' do
    local dir = entry:match '^(.*)[/\\]%?%.lua$'
    if dir then
      local f = io.open(dir .. sep .. 'sshmgr' .. sep .. 'tui.lua', 'r')
      if f then
        f:close()
        return dir
      end
    end
  end
  return nil
end

function M.tui_dir(state)
  local plugin_dir = M.plugin_dir(state)
  if not plugin_dir then
    return nil
  end
  local root = dir_of(plugin_dir)
  if not root then
    return nil
  end
  return root .. util.sep .. 'tui'
end

function M.opentui_dir(state)
  local plugin_dir = M.plugin_dir(state)
  local root = plugin_dir and dir_of(plugin_dir) or nil
  return root and (root .. util.sep .. 'tui-opentui') or nil
end

local function tui_exists(state)
  local dir = M.tui_dir(state)
  if not dir then
    return false, nil
  end
  local main = dir .. util.sep .. 'sshmgr_tui' .. util.sep .. '__main__.py'
  return util.file_exists(main), dir
end

---------------------------------------------------------------------------
-- python + textual
---------------------------------------------------------------------------

local function probe_python(prefix)
  local cmd = {}
  for _, a in ipairs(prefix) do
    table.insert(cmd, a)
  end
  table.insert(cmd, '-c')
  table.insert(cmd, 'import textual,sys; sys.stdout.write("ok")')
  local ok, success = pcall(wezterm.run_child_process, cmd)
  return ok and success
end

local function python_candidates(cfg)
  local out = {}
  local explicit = cfg.ui and cfg.ui.tui and cfg.ui.tui.python
  if type(explicit) == 'string' and explicit ~= '' then
    table.insert(out, { explicit })
  elseif type(explicit) == 'table' then
    table.insert(out, explicit)
  end
  table.insert(out, { 'python' })
  table.insert(out, { 'python3' })
  if util.is_windows then
    table.insert(out, { 'py', '-3' })
    local la = os.getenv 'LOCALAPPDATA'
    if la then
      for _, ver in ipairs { 'Python314', 'Python313', 'Python312', 'Python311', 'Python310' } do
        table.insert(out, { la .. '\\Programs\\Python\\' .. ver .. '\\python.exe' })
      end
    end
  end
  return out
end

function M.python(cfg)
  for _, prefix in ipairs(python_candidates(cfg)) do
    if probe_python(prefix) then
      return prefix
    end
  end
  return nil
end

local function python_exists_without_textual(cfg)
  local function probe_plain(prefix)
    local cmd = {}
    for _, a in ipairs(prefix) do
      table.insert(cmd, a)
    end
    table.insert(cmd, '-c')
    table.insert(cmd, 'import sys; sys.stdout.write(sys.version)')
    local ok, success = pcall(wezterm.run_child_process, cmd)
    return ok and success
  end
  for _, prefix in ipairs(python_candidates(cfg)) do
    if probe_plain(prefix) then
      return prefix
    end
  end
  return nil
end

local function opentui_command(state, cfg)
  local tui_cfg = (cfg.ui and cfg.ui.tui) or {}
  if tui_cfg.command ~= nil then
    local command, err = copy_argv(tui_cfg.command)
    if not command then
      return nil, err
    end
    local cwd = tui_cfg.cwd and util.expand_path(tui_cfg.cwd)
      or dir_of(M.plugin_dir(state))
      or M.opentui_dir(state)
    return {
      name = 'opentui',
      command = command,
      cwd = cwd,
      process_names = { basename(command[1]):lower() },
    }
  end

  local root = M.opentui_dir(state)
  if not root then
    return nil, 'cannot locate tui-opentui'
  end

  local executables
  if util.is_windows then
    executables = { 'sshmgr-tui.exe', 'sshmgr-tui-windows-x64.exe' }
  elseif wezterm.target_triple:find('darwin', 1, true)
    and wezterm.target_triple:find('aarch64', 1, true)
  then
    executables = { 'sshmgr-tui', 'sshmgr-tui-macos-arm64' }
  elseif wezterm.target_triple:find('darwin', 1, true) then
    executables = { 'sshmgr-tui', 'sshmgr-tui-macos-x64' }
  else
    executables = { 'sshmgr-tui' }
  end
  for _, exe in ipairs(executables) do
    for _, rel in ipairs {
      { 'dist', exe },
      { 'bin', exe },
      { exe },
    } do
      local path = root
      for _, part in ipairs(rel) do
        path = path .. util.sep .. part
      end
      if util.file_exists(path) then
        return {
          name = 'opentui',
          command = { path },
          cwd = root,
          process_names = { exe:lower(), 'sshmgr-tui', 'sshmgr-tui.exe' },
        }
      end
    end
  end

  local entry
  for _, rel in ipairs {
    { 'src', 'index.tsx' },
    { 'src', 'index.ts' },
    { 'src', 'main.ts' },
    { 'src', 'main.tsx' },
    { 'index.tsx' },
    { 'index.ts' },
  } do
    local path = root
    for _, part in ipairs(rel) do
      path = path .. util.sep .. part
    end
    if util.file_exists(path) then
      entry = path
      break
    end
  end
  if not entry then
    return nil, 'tui-opentui has no compiled binary or TypeScript entrypoint'
  end

  local bun = type(tui_cfg.bun) == 'string' and tui_cfg.bun ~= '' and tui_cfg.bun or 'bun'
  local call_ok, success = pcall(wezterm.run_child_process, { bun, '--version' })
  if not call_ok or not success then
    return nil, 'tui-opentui source exists but Bun is unavailable'
  end
  return {
    name = 'opentui',
    -- `wezterm.run_child_process` has no cwd option. Keeping --cwd in the
    -- argv makes the helper modes load bunfig.toml's Solid JSX preload too.
    command = { bun, 'run', '--cwd', root, entry },
    cwd = root,
    process_names = { basename(bun):lower(), 'bun', 'bun.exe' },
  }
end

local function textual_command(state, cfg)
  local ok_tui, tui_dir = tui_exists(state)
  if not ok_tui then
    return nil, '找不到 Textual TUI（仓库根下的 tui/sshmgr_tui）'
  end
  local py = M.python(cfg)
  if not py then
    if python_exists_without_textual(cfg) then
      return nil, '未安装 textual。运行：' .. PIP_HINT
    end
    return nil, '找不到 Python。安装 Python 3 后重试，或设置 ui.tui.python'
  end
  local command = {}
  util.tbl_concat_into(command, py)
  util.tbl_concat_into(command, { '-u', '-m', 'sshmgr_tui' })
  return {
    name = 'textual',
    command = command,
    python = py,
    cwd = tui_dir,
    process_names = { 'python', 'python.exe', 'python3', 'py', 'py.exe' },
    env = {
      PYTHONPATH = tui_dir,
      PYTHONIOENCODING = 'utf-8',
      PYTHONUTF8 = '1',
    },
  }
end

local function backend_candidates(state, cfg)
  local tui_cfg = (cfg.ui and cfg.ui.tui) or {}
  local requested = tostring(tui_cfg.backend or 'auto'):lower()
  if requested ~= 'auto' and requested ~= 'opentui' and requested ~= 'textual' then
    return nil, 'ui.tui.backend must be auto, opentui or textual'
  end

  local out, errors = {}, {}
  if requested == 'auto' or requested == 'opentui' then
    local backend, err = opentui_command(state, cfg)
    if backend then
      table.insert(out, backend)
    else
      table.insert(errors, 'OpenTUI: ' .. tostring(err))
    end
  end
  if requested == 'auto' or requested == 'textual' then
    local backend, err = textual_command(state, cfg)
    if backend then
      table.insert(out, backend)
    else
      table.insert(errors, 'Textual: ' .. tostring(err))
    end
  end
  if #out == 0 then
    return nil, table.concat(errors, '; ')
  end
  return out, table.concat(errors, '; ')
end

M._backend_candidates = backend_candidates

local CREATE_RUNTIME = [[
import json, os, secrets, tempfile
path = tempfile.mkdtemp(prefix="wezterm-sshmgr-")
try:
    os.chmod(path, 0o700)
except OSError:
    pass
print(json.dumps({"runtime_dir": path, "token": secrets.token_hex(32)}), end="")
]]

local function create_runtime(backend)
  local cmd = {}
  if backend.name == 'opentui' then
    util.tbl_concat_into(cmd, backend.command)
    table.insert(cmd, '--create-runtime')
  else
    util.tbl_concat_into(cmd, backend.python)
    util.tbl_concat_into(cmd, { '-c', CREATE_RUNTIME })
  end
  local call_ok, success, stdout, stderr = pcall(wezterm.run_child_process, cmd)
  if not call_ok or not success then
    return nil, string.format(
      '%s cannot create private TUI runtime: %s',
      backend.name,
      tostring(stderr or success)
    )
  end
  local made = json_decode(stdout)
  if not made
    or type(made.runtime_dir) ~= 'string'
    or made.runtime_dir == ''
    or type(made.token) ~= 'string'
    or not made.token:match('^[0-9a-f]+$')
    or #made.token ~= 64
  then
    return nil, backend.name .. ' returned an invalid TUI runtime'
  end
  return {
    runtime_dir = made.runtime_dir,
    snapshot_path = made.runtime_dir .. util.sep .. 'snapshot.json',
    token = made.token,
    backend = backend.name,
    command = util.deep_copy(backend.command),
    python = backend.python and util.deep_copy(backend.python) or nil,
    process_names = util.deep_copy(backend.process_names or {}),
    last_seq = 0,
    snapshot_seq = 0,
  }
end

local CLEAN_RUNTIME = [[
import os, pathlib, re, sys
directory = pathlib.Path(sys.argv[1])
request = re.compile(r"request-[1-9][0-9]*-[0-9a-f]{32}[.]json$")
try:
    children = list(directory.iterdir())
except OSError:
    children = []
for child in children:
    if (child.name == "snapshot.json"
            or child.name.startswith("snapshot.json.tmp-")
            or request.fullmatch(child.name)):
        try:
            child.unlink()
        except OSError:
            pass
try:
    directory.rmdir()
except OSError:
    pass
]]

local function discard_runtime(ctx)
  if not ctx or type(ctx.runtime_dir) ~= 'string' then
    return
  end
  local cmd = {}
  if ctx.backend == 'opentui' and type(ctx.command) == 'table' then
    util.tbl_concat_into(cmd, ctx.command)
    util.tbl_concat_into(cmd, { '--cleanup-runtime', ctx.runtime_dir })
  else
    util.tbl_concat_into(cmd, ctx.python or {})
    if #cmd > 0 then
      util.tbl_concat_into(cmd, { '-c', CLEAN_RUNTIME, ctx.runtime_dir })
    end
  end
  if #cmd == 0 then
    return
  end
  pcall(wezterm.run_child_process, cmd)
end

local function prune_sessions(current)
  current = current or sessions()
  local changed = false
  for id, ctx in pairs(current) do
    if not resolve_pane(id) then
      discard_runtime(ctx)
      current[id] = nil
      changed = true
    end
  end
  if changed then
    save_sessions(current)
  end
  return current
end

---------------------------------------------------------------------------
-- snapshot
---------------------------------------------------------------------------

local REPLACE_FILE = [[
import os, sys
os.replace(sys.argv[1], sys.argv[2])
]]

local function atomic_snapshot(ctx, body)
  if type(ctx) ~= 'table' or type(ctx.snapshot_path) ~= 'string' then
    return nil, 'TUI session has no snapshot path'
  end
  ctx.snapshot_seq = (tonumber(ctx.snapshot_seq) or 0) + 1
  local suffix = tostring(ctx.snapshot_seq)
  if type(ctx.token) == 'string' then
    suffix = suffix .. '-' .. ctx.token:sub(1, 12)
  end
  local tmp = ctx.snapshot_path .. '.tmp-' .. suffix
  local f, ferr = io.open(tmp, 'wb')
  if not f then
    return nil, string.format('cannot write %s: %s', tmp, tostring(ferr))
  end
  local wrote, werr = f:write(body)
  local closed, cerr = f:close()
  if not wrote or not closed then
    os.remove(tmp)
    return nil, string.format('cannot write snapshot: %s', tostring(werr or cerr))
  end

  local moved, merr = os.rename(tmp, ctx.snapshot_path)
  if not moved and ctx.backend == 'opentui' and type(ctx.command) == 'table' then
    local cmd = {}
    util.tbl_concat_into(cmd, ctx.command)
    util.tbl_concat_into(cmd, { '--replace-file', tmp, ctx.snapshot_path })
    local call_ok, success, _, stderr = pcall(wezterm.run_child_process, cmd)
    moved = call_ok and success
    merr = stderr or merr
  elseif not moved and type(ctx.python) == 'table' and #ctx.python > 0 then
    local cmd = {}
    util.tbl_concat_into(cmd, ctx.python)
    util.tbl_concat_into(cmd, { '-c', REPLACE_FILE, tmp, ctx.snapshot_path })
    local call_ok, success, _, stderr = pcall(wezterm.run_child_process, cmd)
    moved = call_ok and success
    merr = stderr or merr
  end
  if not moved then
    os.remove(tmp)
    return nil, string.format('cannot replace %s: %s', ctx.snapshot_path, tostring(merr))
  end
  return ctx.snapshot_path
end

local function snapshot_json(state)
  local cfg = state.cfg
  local store_list = panel.load_store(cfg) or {}
  local in_store = {}
  for _, p in ipairs(store_list) do
    in_store[profile_id(p)] = p
  end

  local groups_set, groups = {}, {}
  local profiles = {}
  for _, p in ipairs(state.profiles()) do
    local key = profile_id(p)
    if p.group and p.group ~= '' and not groups_set[p.group] then
      groups_set[p.group] = true
      table.insert(groups, p.group)
    end
    local o = p.options or {}
    local stored = in_store[key]
    -- Every source gets a normalised, password-free connection view. Imported
    -- and inline profiles do not have `raw`, but the OpenTUI SFTP client still
    -- needs their resolved key paths, jump host and timeout settings.
    local resolved_agent = o.identityAgent or (p.env and p.env.SSH_AUTH_SOCK)
    if not resolved_agent and (o.auth == nil or o.auth == '' or o.auth == 'agent') then
      resolved_agent = os.getenv 'SSH_AUTH_SOCK'
    end
    local sftp = {
      host = o.sftpHost or o.host,
      user = o.user,
      port = o.port or 22,
      auth = o.auth,
      privateKeys = util.deep_copy(o.privateKeys or {}),
      password_env = o.password_env,
      identityAgent = resolved_agent,
      jumpHost = o.jumpHost,
      proxyCommand = o.proxyCommand,
      readyTimeout = o.readyTimeout,
      keepaliveInterval = o.keepaliveInterval,
      keepaliveCountMax = o.keepaliveCountMax,
      host_key_policy = p.host_key_policy or cfg.host_key_policy,
      ssh_options = util.deep_copy(p.ssh_options or {}),
    }
    local entry = {
      id = p.id,
      name = p.name,
      group = p.group or '',
      editable = stored ~= nil,
      source = stored and 'store' or 'import',
      host = o.host,
      user = o.user,
      port = o.port or 22,
      auth = o.auth,
      has_password = has_secret(stored or p),
      jumpHost = o.jumpHost or '',
      icon = type(p.icon) == 'string' and p.icon or '',
      color = type(p.color) == 'string' and p.color or '',
      sftp = jsonable(sftp),
    }
    if stored then
      entry.raw = jsonable(strip_password(stored))
    end
    table.insert(profiles, jsonable(entry))
  end
  table.sort(groups)

  local snap = {
    store_path = panel.store_path(cfg),
    default_where = cfg.default_where or 'tab',
    groups = groups,
    profiles = profiles,
  }
  local body, err = json_encode(snap)
  if not body then
    return nil, 'cannot encode snapshot: ' .. tostring(err)
  end
  return body
end

function M.snapshot_path(pane)
  local ctx = pane and session_for_pane(pane) or nil
  if ctx then
    return ctx.snapshot_path
  end
  for _, candidate in pairs(sessions()) do
    if type(candidate) == 'table' and type(candidate.snapshot_path) == 'string' then
      return candidate.snapshot_path
    end
  end
  return nil
end

-- With an explicit context this initializes one newly-created TUI. Without a
-- context it refreshes every live manager, which also keeps old panes working
-- after a wezterm configuration reload.
function M.write_snapshot(state, target)
  local body, err = snapshot_json(state)
  if not body then
    return nil, err
  end
  if target then
    return atomic_snapshot(target, body)
  end

  local current = prune_sessions(sessions())
  local first_path
  local errors = {}
  for id, ctx in pairs(current) do
    if resolve_pane(id) then
      local path, write_err = atomic_snapshot(ctx, body)
      current[id] = ctx
      first_path = first_path or path
      if not path then
        table.insert(errors, tostring(write_err))
      end
    else
      current[id] = nil
    end
  end
  save_sessions(current)
  if #errors > 0 then
    return nil, table.concat(errors, '; ')
  end
  return first_path or true
end

---------------------------------------------------------------------------
-- find / spawn the manager tab
---------------------------------------------------------------------------

local function tab_title(cfg)
  return (cfg.ui and cfg.ui.tui and cfg.ui.tui.tab_title) or 'SSH Manager'
end

local function tab_is_tui(tab, title, manager_pane, ctx)
  local ok, got = pcall(function()
    return tab:get_title()
  end)
  if not ok or got ~= title then
    return false
  end
  if not manager_pane then
    return false
  end
  local same_tab = false
  pcall(function()
    same_tab = manager_pane:tab():tab_id() == tab:tab_id()
  end)
  if not same_tab then
    return false
  end
  local proc = ''
  pcall(function()
    proc = manager_pane:get_foreground_process_name() or ''
  end)
  proc = proc:lower()
  if proc == '' then
    return true
  end
  for _, name in ipairs((ctx and ctx.process_names) or {}) do
    name = tostring(name):lower()
    if name ~= '' and proc:find(name, 1, true) ~= nil then
      return true
    end
  end
  return proc:find('python', 1, true) ~= nil
    or proc:find('sshmgr', 1, true) ~= nil
    or proc:find('bun', 1, true) ~= nil
    or proc:find('node', 1, true) ~= nil
    or proc:find('py.exe', 1, true) ~= nil
    or proc:find('/py', 1, true) ~= nil
end

local function find_manager_tab(window, title)
  local saved = wezterm.GLOBAL[GLOBAL_TAB]
  local current = prune_sessions(sessions())
  local function registered(tab)
    local tab_id = tab:tab_id()
    for id, ctx in pairs(current) do
      if type(ctx) == 'table' and ctx.tab_id == tab_id then
        local manager_pane = resolve_pane(id)
        if tab_is_tui(tab, title, manager_pane, ctx) then
          return tab, ctx, manager_pane, id
        end
      end
    end
    return nil
  end
  local function scan(mux_win)
    if not mux_win or not mux_win.tabs then
      return nil
    end
    for _, tab in ipairs(mux_win:tabs()) do
      if saved and tab:tab_id() == saved then
        local found, ctx, manager_pane, id = registered(tab)
        if found then
          return found, ctx, manager_pane, id
        end
      end
    end
    for _, tab in ipairs(mux_win:tabs()) do
      local found, ctx, manager_pane, id = registered(tab)
      if found then
        return found, ctx, manager_pane, id
      end
    end
    return nil
  end

  local mux_win = window and window:mux_window()
  local tab, ctx, manager_pane, id = scan(mux_win)
  if tab then
    return tab, ctx, manager_pane, id
  end
  local ok, windows = pcall(wezterm.mux.all_windows)
  if ok then
    for _, w in ipairs(windows) do
      tab, ctx, manager_pane, id = scan(w)
      if tab then
        return tab, ctx, manager_pane, id
      end
    end
  end
  return nil
end

local function spawn_tui(state, window, previous)
  local cfg = state.cfg
  local mux_win = window and window:mux_window()
  if not mux_win then
    return nil, 'no mux window'
  end

  local backends, discovery = backend_candidates(state, cfg)
  if not backends then
    return nil, discovery
  end
  local title = tab_title(cfg)
  local errors = {}
  if discovery and discovery ~= '' then
    table.insert(errors, discovery)
  end

  for _, backend in ipairs(backends) do
    local ctx, err = create_runtime(backend)
    if ctx then
      if previous then
        ctx.previous_tab_id = previous.tab_id
        ctx.previous_pane_id = previous.pane_id
      end
      local snap
      snap, err = M.write_snapshot(state, ctx)
      if snap then
        local args = util.deep_copy(backend.command)
        util.tbl_concat_into(args, { '--snapshot', snap })
        local env = util.deep_copy(backend.env or {})
        env.WEZTERM_SSHMGR_SESSION_TOKEN = ctx.token
        local tab, pane
        local spawn_ok, spawn_err = pcall(function()
          tab, pane = mux_win:spawn_tab {
            args = args,
            cwd = backend.cwd,
            set_environment_variables = env,
          }
        end)
        if spawn_ok and tab then
          local id = pane_id(pane)
          if id then
            ctx.tab_id = tab:tab_id()
            ctx.pane_id = id
            local current = sessions()
            current[id] = ctx
            save_sessions(current)
            util.log(
              '%s TUI spawned tab=%s pane=%s',
              backend.name,
              tostring(ctx.tab_id),
              id
            )
            pcall(function()
              tab:set_title(title)
            end)
            wezterm.GLOBAL[GLOBAL_TAB] = tab:tab_id()
            return tab, pane
          end
          spawn_err = 'spawned TUI pane has no pane id'
        end
        err = 'spawn failed: ' .. tostring(spawn_err)
      end
      discard_runtime(ctx)
    end
    table.insert(errors, backend.name .. ': ' .. tostring(err))
  end

  return nil, table.concat(errors, '; ')
end

local function remember_prev(window, pane)
  local tab = window and window:active_tab()
  if not tab then
    return nil
  end
  if session_for_pane(pane) then
    local old = wezterm.GLOBAL[GLOBAL_PREV]
    return type(old) == 'table' and old or nil
  end
  local previous = {
    tab_id = tab:tab_id(),
    pane_id = pane_id(pane or tab:active_pane()),
  }
  wezterm.GLOBAL[GLOBAL_PREV] = previous
  return previous
end

local function activate_tab_id(tab_id)
  if not tab_id then
    return false
  end
  local ok, windows = pcall(wezterm.mux.all_windows)
  if not ok then
    return false
  end
  for _, w in ipairs(windows) do
    for _, tab in ipairs(w:tabs()) do
      if tab:tab_id() == tab_id then
        pcall(function()
          tab:activate()
        end)
        return true
      end
    end
  end
  return false
end

local function activate_manager(window, ctx)
  local tab
  if ctx and ctx.tab_id then
    local ok, found = pcall(wezterm.mux.get_tab, ctx.tab_id)
    if ok then
      tab = found
    end
  end
  if not tab then
    local cfg = attached and attached.cfg
    local title = tab_title(cfg or { ui = {} })
    tab = find_manager_tab(window, title)
  end
  if tab then
    pcall(function()
      tab:activate()
    end)
    return true
  end
  return activate_tab_id(wezterm.GLOBAL[GLOBAL_TAB])
end

local function activate_previous(ctx)
  local previous_tab = ctx and ctx.previous_tab_id
  local previous_pane = ctx and resolve_pane(ctx.previous_pane_id)
  local activated = activate_tab_id(previous_tab)
  if previous_pane then
    pcall(function()
      previous_pane:activate()
    end)
    activated = true
  end
  if activated then
    return true
  end
  local old = wezterm.GLOBAL[GLOBAL_PREV]
  if type(old) == 'table' then
    return activate_tab_id(old.tab_id)
  end
  return activate_tab_id(old)
end

function M.open(state, window, pane)
  attached = state
  local cfg = state.cfg
  local title = tab_title(cfg)

  local previous = remember_prev(window, pane)
  local existing, ctx = find_manager_tab(window, title)
  if existing then
    if previous then
      ctx.previous_tab_id = previous.tab_id
      ctx.previous_pane_id = previous.pane_id
      local current = sessions()
      current[tostring(ctx.pane_id)] = ctx
      save_sessions(current)
    end
    pcall(function()
      existing:activate()
    end)
    local ok, err = M.write_snapshot(state, ctx)
    if not ok then
      toast(window, err, 8000)
    end
    return existing
  end
  local tab, err = spawn_tui(state, window, previous)
  if not tab then
    util.err('open TUI: %s', tostring(err))
    toast(window, err or '无法打开 SSH Manager', 10000)
    return nil
  end
  return tab
end

function M.open_action(state)
  return wezterm.action_callback(function(window, pane)
    M.open(state, window, pane)
  end)
end

function M.attach(state)
  attached = state
end

---------------------------------------------------------------------------
-- incoming commands from the TUI
---------------------------------------------------------------------------

local function commit_store(window, list, header)
  local cfg = attached.cfg
  local ok, err = panel.save_store(cfg, list, header)
  if not ok then
    toast(window, err, 8000)
    return false
  end
  attached.reload()
  local snap, serr = M.write_snapshot(attached)
  if not snap then
    toast(window, serr, 8000)
  end
  return true
end

local function find_store_index(list, id)
  if not id then
    return nil
  end
  for i, p in ipairs(list) do
    -- A bare-name fallback is ambiguous when groups reuse names (for
    -- example `prod/db` and `dev/db`). Normalised profiles identify an entry
    -- either by its explicit id or by the exact group/name store key.
    if store_key(p) == id or p.id == id then
      return i
    end
  end
  return nil
end

local function preserve_password(old, raw)
  if not old then
    return
  end
  local old_pw = old.password or (old.options and old.options.password)
  if type(old_pw) ~= 'string' or old_pw == '' then
    return
  end
  local raw_options = type(raw.options) == 'table' and raw.options or nil
  if raw.password == nil and (not raw_options or raw_options.password == nil) then
    if type(old.password) == 'string' and old.password ~= '' then
      raw.password = old_pw
    else
      raw.options = raw_options or {}
      raw.options.password = old_pw
    end
  end
end

local function connection_origin(ctx, pane, where)
  if type(where) == 'string' and where:match '^split_' then
    local previous = ctx and resolve_pane(ctx.previous_pane_id)
    if previous then
      return previous, where
    end
    util.warn('TUI split requested without a live previous pane; using a new tab')
    return nil, 'tab'
  end
  return pane, where
end

local function do_connect(window, pane, ctx, id, where)
  local list = attached.profiles()
  local profile = attached.find(list, id)
  if not profile then
    toast(window, '找不到连接 ' .. tostring(id), 5000)
    return
  end
  pane, where = connection_origin(ctx, pane, where or attached.cfg.default_where)
  session.connect(
    profile,
    attached.cfg,
    list,
    attached.find,
    window,
    pane,
    where
  )
end

local function do_quick(window, pane, ctx, target, where)
  if type(target) ~= 'string' or util.trim(target) == '' then
    return
  end
  pane, where = connection_origin(ctx, pane, where or attached.cfg.default_where)
  local list = attached.profiles()
  local existing = attached.find(list, util.trim(target))
  if existing then
    session.connect(existing, attached.cfg, list, attached.find, window, pane, where)
    return
  end
  local t = util.parse_target(util.trim(target))
  local profile = attached.normalize({
    name = util.trim(target),
    group = 'ad-hoc',
    options = { host = t.host, user = t.user, port = t.port },
  }, attached.cfg)
  session.connect(profile, attached.cfg, list, attached.find, window, pane, where)
end

local function do_upsert(window, msg)
  local list, header = panel.load_store(attached.cfg)
  if not list then
    toast(window, '配置文件有语法错误：' .. panel.store_path(attached.cfg), 8000)
    return
  end
  local raw = msg.raw
  if type(raw) ~= 'table' then
    toast(window, 'upsert 缺少 raw', 4000)
    return
  end
  raw.name = raw.name or (raw.options and raw.options.host) or 'unnamed'
  local idx = find_store_index(list, msg.id) or find_store_index(list, store_key(raw))
  if idx then
    preserve_password(list[idx], raw)
    list[idx] = raw
  else
    table.insert(list, raw)
  end
  commit_store(window, list, header)
end

local function do_delete(window, id)
  local list, header = panel.load_store(attached.cfg)
  if not list then
    toast(window, '配置文件有语法错误', 8000)
    return
  end
  local idx = find_store_index(list, id)
  if not idx then
    toast(window, 'store 里没有 ' .. tostring(id), 4000)
    return
  end
  table.remove(list, idx)
  if commit_store(window, list, header) then
    toast(window, '已删除 ' .. tostring(id), 2500)
  end
end

local function do_copy_in(window, id)
  local list, header = panel.load_store(attached.cfg)
  if not list then
    toast(window, '配置文件有语法错误', 8000)
    return
  end
  local src = attached.find(attached.profiles(), id)
  if not src then
    toast(window, '找不到 ' .. tostring(id), 4000)
    return
  end
  if find_store_index(list, store_key(src)) then
    toast(window, '已经在可编辑 store 里了', 3000)
    return
  end
  local copy = strip_password(src)
  copy.id = nil
  table.insert(list, copy)
  if commit_store(window, list, header) then
    toast(window, '已复制到 ' .. panel.store_path(attached.cfg), 4000)
  end
end

local function consume_request(ctx, id, envelope)
  if type(envelope) ~= 'table'
    or envelope.v ~= 2
    or type(envelope.token) ~= 'string'
    or envelope.token ~= ctx.token
  then
    return nil, 'invalid envelope'
  end

  local seq = tonumber(envelope.seq)
  if not seq or seq < 1 or seq ~= math.floor(seq) then
    return nil, 'invalid sequence'
  end
  if seq <= (tonumber(ctx.last_seq) or 0) then
    return nil, 'replayed sequence'
  end

  local request = envelope.request
  if type(request) ~= 'string' then
    return nil, 'missing request reference'
  end
  local file_seq, nonce = request:match '^request%-([1-9][0-9]*)%-([0-9a-f]+)%.json$'
  if tonumber(file_seq) ~= seq or not nonce or #nonce ~= 32 then
    return nil, 'invalid request reference'
  end
  if type(ctx.runtime_dir) ~= 'string' or ctx.runtime_dir == '' then
    return nil, 'missing runtime directory'
  end

  -- Claim the sequence before touching the file. Duplicate events are sent to
  -- multiple mux clients in some configurations; only the first may consume it.
  ctx.last_seq = seq
  local current = sessions()
  current[id] = ctx
  save_sessions(current)

  -- `request` is a strict basename, never an arbitrary path. Read one byte
  -- beyond the limit, then unlink before decoding or executing the command.
  local path = ctx.runtime_dir .. util.sep .. request
  local f, ferr = io.open(path, 'rb')
  if not f then
    return nil, 'cannot open one-shot request: ' .. tostring(ferr)
  end
  local raw = f:read(MAX_REQUEST_BYTES + 1)
  f:close()
  local removed, rerr = os.remove(path)
  if not removed then
    return nil, 'cannot remove one-shot request: ' .. tostring(rerr)
  end
  if type(raw) ~= 'string' or #raw > MAX_REQUEST_BYTES then
    return nil, 'request is empty or too large'
  end

  local msg = json_decode(raw)
  if type(msg) ~= 'table'
    or msg._session ~= ctx.token
    or tonumber(msg._seq) ~= seq
    or type(msg.op) ~= 'string'
  then
    return nil, 'invalid request body'
  end
  msg._session = nil
  msg._seq = nil
  return msg
end

function M.on_user_var(window, pane, name, value)
  if name ~= 'sshmgr' then
    return
  end
  -- protocol.py clears the UserVar immediately after its wake-up envelope.
  if value == nil or value == '' then
    return
  end
  if not attached or not attached.cfg then
    util.warn('TUI command received before attach')
    return
  end
  local ctx, id = session_for_pane(pane)
  if not ctx then
    util.warn('ignored sshmgr command from an unregistered pane')
    return
  end
  local ok, err = pcall(function()
    local envelope = json_decode(value)
    local msg, request_err = consume_request(ctx, id, envelope)
    if not msg then
      util.warn('ignored invalid sshmgr TUI request: %s', tostring(request_err))
      return
    end
    local op = msg.op
    if op == 'connect' then
      local where = msg.where or attached.cfg.default_where
      do_connect(window, pane, ctx, msg.id, where)
      if where ~= 'window' then
        wezterm.time.call_after(0.05, function()
          activate_manager(window, ctx)
        end)
      end
    elseif op == 'quick' then
      local where = msg.where or attached.cfg.default_where
      do_quick(window, pane, ctx, msg.target, where)
      if where ~= 'window' then
        wezterm.time.call_after(0.05, function()
          activate_manager(window, ctx)
        end)
      end
    elseif op == 'hide' then
      activate_previous(ctx)
    elseif op == 'upsert' then
      do_upsert(window, msg)
    elseif op == 'delete' then
      do_delete(window, msg.id)
    elseif op == 'copy_in' then
      do_copy_in(window, msg.id)
    elseif op == 'reload' then
      attached.reload()
      local snap, serr = M.write_snapshot(attached)
      if not snap then
        toast(window, serr, 8000)
      end
    else
      util.warn('unknown sshmgr op %q', tostring(op))
    end
  end)
  if not ok then
    util.err('TUI command failed: %s', tostring(err))
    toast(window, 'TUI 命令失败：' .. tostring(err), 8000)
  end
end

return M
