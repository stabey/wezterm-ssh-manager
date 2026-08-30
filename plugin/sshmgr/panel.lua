-- sshmgr.panel -- edit profiles without opening the config file
--
-- wezterm's lua cannot draw its own UI: the only interactive surfaces are
-- InputSelector (a list), PromptInputLine (one line of text) and Confirmation
-- (yes/no). So this is a menu, not a form -- a stack of pickers where each
-- choice opens the next screen. Everything is written straight back to the
-- editable profile file, and the profile list is re-read in place, so no
-- config reload is needed.
local wezterm = require 'wezterm'
local act = wezterm.action
local util = require 'sshmgr.util'
local serialize = require 'sshmgr.serialize'

local M = {}

local BACK = '\1back'
local NEW = '\1new'
local RELOAD = '\1reload'

---------------------------------------------------------------------------
-- the editable store
---------------------------------------------------------------------------

function M.store_path(cfg)
  return util.expand_path(
    cfg.profile_store or (wezterm.config_dir .. util.sep .. 'ssh_profiles.lua')
  )
end

local DEFAULT_HEADER = [[
-- SSH profiles, managed by wezterm-ssh-manager (SSH Manager TUI).
-- Hand edits are preserved, but comments inside the table are not: the panel
-- rewrites everything below the header when you change something here.
]]

--- Read the store. Returns list, header_comment.
function M.load_store(cfg)
  local path = M.store_path(cfg)
  local data = util.read_file(path)
  if not data then
    return {}, DEFAULT_HEADER
  end

  -- keep whatever comment block the file starts with (the tabby exporter
  -- writes its migration report there and it would be a shame to lose it)
  local header = data:match '^(%s*%-%-[^\n]*\n[%s\n]*)' and data:match '^(.-)\n%s*return%s' or nil
  if header and not header:match '^%s*%-%-' then
    header = nil
  end

  local chunk, err = load(data, '@' .. path, 't')
  if not chunk then
    util.err('cannot parse %s: %s', path, err)
    return nil, nil, err
  end
  local ok, decoded = pcall(chunk)
  if not ok or type(decoded) ~= 'table' then
    util.err('cannot evaluate %s: %s', path, tostring(decoded))
    return nil, nil, tostring(decoded)
  end
  return decoded.profiles or decoded, header or DEFAULT_HEADER
end

local function store_key(p)
  local name = tostring(p.name or '')
  if p.group and p.group ~= '' then
    return p.group .. '/' .. name
  end
  return name
end

local function canonical_id(p)
  if p.id ~= nil and tostring(p.id) ~= '' then
    return tostring(p.id)
  end
  local options = p.options or {}
  local name = tostring(p.name or options.host or p.host or '')
  local group = tostring(p.group or '')
  return group ~= '' and (group .. '/' .. name) or name
end

local function same_store_profile(stored, profile)
  -- Require the exact group/name pair in every case; explicit ids must also
  -- agree. Never fall back to a bare name because different groups commonly
  -- reuse it.
  local stored_options = stored.options or {}
  local profile_options = profile.options or {}
  local same_group_name = tostring(stored.group or '') == tostring(profile.group or '')
    and tostring(stored.name or stored_options.host or stored.host or '')
      == tostring(profile.name or profile_options.host or profile.host or '')
  if not same_group_name then
    return false
  end
  if stored.id ~= nil and profile.id ~= nil then
    return tostring(stored.id) == tostring(profile.id)
  end
  return canonical_id(stored) == canonical_id(profile)
end

local function put_password(entry, password)
  -- Preserve a deliberately flat profile instead of leaving an obsolete
  -- top-level password beside a newer options.password value.
  if entry.options == nil then
    entry.password = password
    if entry.auth == nil then
      entry.auth = 'password'
    end
  else
    entry.options.password = password
    if entry.options.auth == nil then
      entry.options.auth = 'password'
    end
  end
end

local function copy_for_store(profile, password, origin)
  local copy = util.deep_copy((origin and origin.raw) or profile)
  put_password(copy, password)
  return copy
end

--- Write `password` onto the exact store entry for `profile`. Profiles from
--- imported files may be copied in full so the store shadows them without
--- losing jump hosts, keys, forwarding or scripts. Inline/ad-hoc profiles
--- cannot be safely shadowed because inline entries load first.
function M.persist_password(cfg, profile, password)
  if type(password) ~= 'string' or password == '' then
    return false, 'empty password'
  end
  local list, header = M.load_store(cfg)
  if not list then
    return false, 'cannot read profile store'
  end
  local matches = {}
  for i, p in ipairs(list) do
    if same_store_profile(p, profile) then
      table.insert(matches, i)
    end
  end
  if #matches > 1 then
    return false, string.format('ambiguous profile identity for %s', canonical_id(profile))
  end
  local idx = matches[1]
  if idx then
    put_password(list[idx], password)
  else
    local origin = require('sshmgr.profiles').origin(profile)
    local kind = type(origin) == 'table' and origin.kind or nil
    if kind == 'inline' or kind == nil then
      return false,
        'profile is not safely writable; copy it to the profile store or configure a password source'
    end
    local copy = copy_for_store(profile, password, origin)
    table.insert(list, copy)
  end
  local ok, err = M.save_store(cfg, list, header)
  if ok then
    wezterm.GLOBAL.sshmgr_invalidate = true
  end
  return ok, err
end

function M.save_store(cfg, list, header)
  local path = M.store_path(cfg)

  -- serialize.encode intentionally renders unknown Lua values for display,
  -- but a profile store must round-trip exactly. Refuse functions, userdata,
  -- sparse/mixed-key tables, cycles and non-finite numbers before touching the
  -- current file.
  local seen = {}
  local function serializable(value, at)
    local kind = type(value)
    if kind == 'nil' or kind == 'string' or kind == 'boolean' then
      return true
    end
    if kind == 'number' then
      if value ~= value or value == math.huge or value == -math.huge then
        return false, at .. ' contains a non-finite number'
      end
      return true
    end
    if kind ~= 'table' then
      return false, string.format('%s contains unsupported %s value', at, kind)
    end
    if seen[value] then
      return false, at .. ' contains a table cycle'
    end
    if getmetatable(value) ~= nil then
      return false, at .. ' contains a table with a metatable'
    end
    seen[value] = true
    local array_len = #value
    local has_numeric, has_string = false, false
    for key, child in pairs(value) do
      if type(key) == 'number' then
        has_numeric = true
        if key % 1 ~= 0 or key < 1 or key > array_len then
          seen[value] = nil
          return false, at .. ' contains a sparse or mixed numeric key'
        end
      elseif type(key) == 'string' then
        has_string = true
      else
        seen[value] = nil
        return false, at .. ' contains an unsupported table key'
      end
      if has_numeric and has_string then
        seen[value] = nil
        return false, at .. ' mixes array and object keys'
      end
      local ok, err = serializable(child, string.format('%s[%s]', at, tostring(key)))
      if not ok then
        seen[value] = nil
        return false, err
      end
    end
    seen[value] = nil
    return true
  end

  local safe, unsafe = serializable(list, 'profiles')
  if not safe then
    return false, 'cannot save profile store: ' .. unsafe
  end

  local body = (header or DEFAULT_HEADER)
    .. '\nreturn {\n  profiles = '
    .. serialize.encode(list, 1)
    .. ',\n}\n'
  local suffix = string.format('.tmp-%d-%d', os.time(), math.random(100000, 999999))
  local tmp = path .. suffix
  local function discard(candidate)
    if not util.file_exists(candidate) then
      return ''
    end
    local removed, remove_err = os.remove(candidate)
    if removed then
      return ''
    end
    return string.format('; could not remove %s: %s', candidate, tostring(remove_err))
  end
  local f, ferr = io.open(tmp, 'wb')
  if not f then
    return false, string.format('cannot write %s: %s', tmp, tostring(ferr))
  end

  -- The temp file exists but is still empty here. Tighten its mode before any
  -- password reaches disk. Windows keeps the ACL inherited from the store
  -- directory; POSIX gets an explicit owner-only mode independent of umask.
  if not util.is_windows then
    local pok, chmod_ok, _, chmod_err = pcall(
      wezterm.run_child_process,
      { 'chmod', '600', tmp }
    )
    if not pok or not chmod_ok then
      f:close()
      return false, string.format(
        'cannot protect %s: %s%s',
        tmp,
        tostring(chmod_err or chmod_ok),
        discard(tmp)
      )
    end
  end

  local wrote, werr = f:write(body)
  if wrote then
    f:flush()
  end
  local closed, cerr = f:close()
  if not wrote or not closed then
    return false, string.format('cannot write %s: %s%s', tmp, tostring(werr or cerr), discard(tmp))
  end

  -- POSIX rename replaces the destination atomically. Standard Lua on
  -- Windows cannot replace an existing file, so use a same-directory backup
  -- only as a fallback; the watched destination is still never partial.
  local renamed, rerr = os.rename(tmp, path)
  if not renamed and util.file_exists(path) then
    local backup = path .. suffix .. '.bak'
    local moved, merr = os.rename(path, backup)
    if not moved then
      return false, string.format('cannot replace %s: %s%s', path, tostring(merr or rerr), discard(tmp))
    end
    renamed, rerr = os.rename(tmp, path)
    if renamed then
      local removed, remove_err = os.remove(backup)
      if not removed then
        util.warn('saved %s but could not remove backup %s: %s', path, backup, tostring(remove_err))
      end
    else
      local restored, restore_err = os.rename(backup, path)
      if not restored then
        return false, string.format(
          'cannot replace %s (%s); original retained at %s (%s)%s',
          path,
          tostring(rerr),
          backup,
          tostring(restore_err),
          discard(tmp)
        )
      end
    end
  end
  if not renamed then
    return false, string.format('cannot replace %s: %s%s', path, tostring(rerr), discard(tmp))
  end
  return true, path
end

---------------------------------------------------------------------------
-- dotted path access, so a field spec can point at options.host
---------------------------------------------------------------------------

local function get_path(t, path)
  local cur = t
  for part in path:gmatch '[^.]+' do
    if type(cur) ~= 'table' then
      return nil
    end
    cur = cur[part]
  end
  return cur
end

local function set_path(t, path, value)
  local parts = {}
  for part in path:gmatch '[^.]+' do
    table.insert(parts, part)
  end
  local cur = t
  for i = 1, #parts - 1 do
    if type(cur[parts[i]]) ~= 'table' then
      cur[parts[i]] = {}
    end
    cur = cur[parts[i]]
  end
  cur[parts[#parts]] = value
end

---------------------------------------------------------------------------
-- field specs
---------------------------------------------------------------------------

local NIL = '(未设置)'

local FIELDS = {
  { key = 'name', label = '名称', kind = 'text' },
  { key = 'group', label = '分组', kind = 'text', hint = '用 / 分隔层级' },
  { key = 'options.host', label = '主机', kind = 'text' },
  { key = 'options.port', label = '端口', kind = 'number' },
  { key = 'options.user', label = '用户名', kind = 'text' },
  {
    key = 'options.auth',
    label = '认证方式',
    kind = 'enum',
    values = { 'agent', 'publicKey', 'password', 'keyboardInteractive' },
  },
  { key = 'options.password', label = '密码', kind = 'password', hint = '明文写入配置文件；已有密码时留空不改' },
  { key = 'options.privateKeys', label = '私钥路径', kind = 'list', hint = '密钥文件路径，不是密码' },
  { key = 'options.password_env', label = '密码环境变量', kind = 'text' },
  { key = 'options.jumpHost', label = '跳板机', kind = 'text', hint = 'profile 名字，或 user@host:port' },
  { key = 'options.agentForward', label = 'Agent 转发', kind = 'bool' },
  { key = 'options.x11', label = 'X11 转发', kind = 'bool' },
  { key = 'options.skipBanner', label = '跳过 banner', kind = 'bool' },
  { key = 'options.keepaliveInterval', label = '保活间隔', kind = 'number', hint = '秒' },
  { key = 'options.readyTimeout', label = '连接超时', kind = 'number', hint = '秒' },
  { key = 'options.forwardedPorts', label = '端口转发', kind = 'forwards' },
  { key = 'options.scripts', label = '登录脚本', kind = 'scripts' },
  { key = 'on_login', label = '登录后命令', kind = 'list', hint = '等到提示符后依次执行，逗号分隔' },
  {
    key = 'behaviorOnSessionEnd',
    label = '断开后行为',
    kind = 'enum',
    values = { 'close', 'keep', 'reconnect' },
  },
  { key = 'color', label = '颜色', kind = 'text', hint = '#rrggbb' },
  { key = 'icon', label = '图标', kind = 'text', hint = '一个字符，例如 nerdfont 图标' },
}

local function fmt(v, kind)
  if v == nil or v == '' then
    return NIL
  end
  if kind == 'bool' then
    return v and '是' or '否'
  end
  if kind == 'list' then
    return table.concat(v, ', ')
  end
  if kind == 'scripts' then
    return string.format('%d 步', #v)
  end
  if kind == 'forwards' then
    return string.format('%d 条', #v)
  end
  return tostring(v)
end

---------------------------------------------------------------------------
-- primitives
---------------------------------------------------------------------------

local function menu(window, pane, o)
  window:perform_action(
    act.InputSelector {
      title = o.title,
      choices = o.choices,
      fuzzy = o.fuzzy or false,
      description = o.description or '⏎ 选择    / 搜索    Esc 返回',
      fuzzy_description = (o.title or 'ssh') .. ': ',
      action = wezterm.action_callback(function(win, pn, id)
        if not id then
          if o.on_cancel then
            o.on_cancel(win, pn)
          end
          return
        end
        o.on_pick(win, pn, id)
      end),
    },
    pane
  )
end

local function ask(window, pane, o)
  local action = {
    description = wezterm.format {
      { Attribute = { Intensity = 'Bold' } },
      { Foreground = { AnsiColor = 'Aqua' } },
      { Text = o.description },
    },
    action = wezterm.action_callback(function(win, pn, line)
      o.on_done(win, pn, line)
    end),
  }
  if o.initial and o.initial ~= '' then
    action.initial_value = o.initial
  end
  window:perform_action(act.PromptInputLine(action), pane)
end

local function toast(window, msg, ms)
  pcall(function()
    window:toast_notification('wezterm ssh-manager', msg, nil, ms or 3000)
  end)
end

---------------------------------------------------------------------------
-- screens
---------------------------------------------------------------------------

--- `state` is the plugin singleton: { cfg, profiles(), find, reload() }
function M.open(state)
  local cfg = state.cfg

  local commit -- forward decls
  local main_menu, profile_menu, edit_field, scripts_menu, script_menu,
    forwards_menu, forward_menu

  --- Persist `list` and make the change visible without a config reload.
  commit = function(window, list, header)
    local ok, err = M.save_store(cfg, list, header)
    if not ok then
      util.err('%s', err)
      toast(window, err, 8000)
      return false
    end
    state.reload()
    return true
  end

  ---------------------------------------------------------------------------
  main_menu = function(window, pane)
    local list, header = M.load_store(cfg)
    if not list then
      toast(window, '配置文件有语法错误，先手动修一下：' .. M.store_path(cfg), 10000)
      return
    end

    local in_store = {}
    for _, p in ipairs(list) do
      in_store[(p.group and p.group ~= '' and (p.group .. '/') or '') .. (p.name or '')] = true
    end

    local choices = {
      { id = NEW, label = wezterm.format {
        { Foreground = { AnsiColor = 'Green' } },
        { Text = '＋  新建连接' },
      } },
    }
    for i, p in ipairs(list) do
      local name = (p.group and p.group ~= '' and (p.group .. '/') or '') .. (p.name or '?')
      local o = p.options or {}
      table.insert(choices, {
        id = 'e' .. i,
        label = wezterm.format {
          { Text = string.format('%-34s', name) },
          { Foreground = { Color = cfg.ui.dim_color or '#6c7086' } },
          { Text = (o.user and (o.user .. '@') or '') .. tostring(o.host or '?') },
        },
      })
    end

    -- profiles that come from somewhere else cannot be edited in place
    for _, p in ipairs(state.profiles()) do
      local key = (p.group ~= '' and (p.group .. '/') or '') .. p.name
      if not in_store[key] then
        table.insert(choices, {
          id = 'r' .. p.id,
          label = wezterm.format {
            { Foreground = { Color = cfg.ui.dim_color or '#6c7086' } },
            { Text = string.format('%-34s%s  (只读)', key, tostring(p.options.host or '')) },
          },
        })
      end
    end

    table.insert(choices, { id = RELOAD, label = '↻  重新读取配置文件' })

    menu(window, pane, {
      title = 'SSH  ·  连接管理',
      choices = choices,
      fuzzy = true,
      description = string.format('%d 条可编辑    ⏎ 编辑    / 搜索    Esc 关闭', #list),
      on_pick = function(win, pn, id)
        if id == NEW then
          ask(win, pn, {
            description = '新连接： [user@]host[:port]',
            on_done = function(w2, p2, line)
              if not line or util.trim(line) == '' then
                return main_menu(w2, p2)
              end
              local t = util.parse_target(util.trim(line))
              local entry = { name = t.host, options = { host = t.host } }
              if t.user then
                entry.options.user = t.user
              end
              if t.port then
                entry.options.port = t.port
              end
              table.insert(list, entry)
              if commit(w2, list, header) then
                profile_menu(w2, p2, #list)
              end
            end,
          })
        elseif id == RELOAD then
          state.reload()
          toast(win, '已重新读取')
          main_menu(win, pn)
        elseif id:sub(1, 1) == 'e' then
          profile_menu(win, pn, tonumber(id:sub(2)))
        elseif id:sub(1, 1) == 'r' then
          local src = state.find(state.profiles(), id:sub(2))
          if not src then
            return main_menu(win, pn)
          end
          window:perform_action(
            act.Confirmation {
              message = string.format(
                '「%s」来自导入或 wezterm.lua，不能就地编辑。复制一份到 %s ？',
                src.name,
                M.store_path(cfg)
              ),
              action = wezterm.action_callback(function(w2, p2)
                local copy = util.deep_copy(src)
                copy.id = nil
                copy.options.scripts = copy.options.scripts or nil
                table.insert(list, copy)
                if commit(w2, list, header) then
                  profile_menu(w2, p2, #list)
                end
              end),
              cancel = wezterm.action_callback(function(w2, p2)
                main_menu(w2, p2)
              end),
            },
            pn
          )
        end
      end,
    })
  end

  ---------------------------------------------------------------------------
  profile_menu = function(window, pane, idx)
    local list, header = M.load_store(cfg)
    if not list or not list[idx] then
      return main_menu(window, pane)
    end
    local p = list[idx]

    local choices = {}
    for i, spec in ipairs(FIELDS) do
      local v = get_path(p, spec.key)
      local shown = fmt(v, spec.kind)
      table.insert(choices, {
        id = 'f' .. i,
        label = wezterm.format {
          { Text = string.format('%-16s', spec.label) },
          { Foreground = { Color = v == nil and (cfg.ui.dim_color or '#6c7086') or '#a6e3a1' } },
          { Text = shown },
        },
      })
    end
    table.insert(choices, { id = 'connect', label = '▶  连接' })
    table.insert(choices, { id = 'dup', label = '⧉  复制一份' })
    table.insert(choices, { id = 'del', label = wezterm.format {
      { Foreground = { AnsiColor = 'Red' } },
      { Text = '🗑  删除' },
    } })
    table.insert(choices, { id = BACK, label = '←  返回' })

    menu(window, pane, {
      title = 'SSH  ·  ' .. tostring(p.name or ''),
      choices = choices,
      description = '⏎ 编辑该项    Esc 返回',
      on_cancel = main_menu,
      on_pick = function(win, pn, id)
        if id == BACK then
          return main_menu(win, pn)
        elseif id == 'connect' then
          local session = require 'sshmgr.session'
          local all = state.profiles()
          local key = (p.group and p.group ~= '' and (p.group .. '/') or '') .. tostring(p.name)
          local target = state.find(all, key) or state.find(all, tostring(p.name))
          if target then
            session.connect(target, cfg, all, state.find, win, pn, cfg.default_where)
          else
            toast(win, '找不到 ' .. key .. '，可能还没保存', 5000)
          end
        elseif id == 'dup' then
          local copy = util.deep_copy(p)
          copy.name = tostring(copy.name or 'profile') .. '-copy'
          table.insert(list, copy)
          if commit(win, list, header) then
            profile_menu(win, pn, #list)
          end
        elseif id == 'del' then
          win:perform_action(
            act.Confirmation {
              message = string.format('删除「%s」？', tostring(p.name)),
              action = wezterm.action_callback(function(w2, p2)
                table.remove(list, idx)
                commit(w2, list, header)
                main_menu(w2, p2)
              end),
              cancel = wezterm.action_callback(function(w2, p2)
                profile_menu(w2, p2, idx)
              end),
            },
            pn
          )
        elseif id:sub(1, 1) == 'f' then
          edit_field(win, pn, idx, tonumber(id:sub(2)))
        end
      end,
    })
  end

  ---------------------------------------------------------------------------
  edit_field = function(window, pane, idx, fidx)
    local list, header = M.load_store(cfg)
    if not list or not list[idx] then
      return main_menu(window, pane)
    end
    local p = list[idx]
    local spec = FIELDS[fidx]
    local cur = get_path(p, spec.key)

    local function store(value)
      set_path(p, spec.key, value)
      if commit(window, list, header) then
        profile_menu(window, pane, idx)
      end
    end

    if spec.kind == 'bool' then
      return menu(window, pane, {
        title = spec.label,
        choices = { { id = 'y', label = '是' }, { id = 'n', label = '否' }, { id = 'x', label = NIL } },
        on_cancel = function(w, p2)
          profile_menu(w, p2, idx)
        end,
        on_pick = function(_, _, id)
          store(id == 'y' and true or (id == 'n' and false or nil))
        end,
      })
    end

    if spec.kind == 'enum' then
      local choices = {}
      for _, v in ipairs(spec.values) do
        table.insert(choices, { id = v, label = v })
      end
      table.insert(choices, { id = '\1nil', label = NIL })
      return menu(window, pane, {
        title = spec.label,
        choices = choices,
        on_cancel = function(w, p2)
          profile_menu(w, p2, idx)
        end,
        on_pick = function(_, _, id)
          store(id ~= '\1nil' and id or nil)
        end,
      })
    end

    if spec.kind == 'scripts' then
      return scripts_menu(window, pane, idx)
    end
    if spec.kind == 'forwards' then
      return forwards_menu(window, pane, idx)
    end

    local initial
    if spec.kind == 'list' then
      initial = type(cur) == 'table' and table.concat(cur, ', ') or nil
    elseif cur ~= nil then
      initial = tostring(cur)
    end

    ask(window, pane, {
      description = spec.label .. (spec.hint and ('   (' .. spec.hint .. ')') or '') .. '    留空=清除',
      initial = initial,
      on_done = function(_, _, line)
        if line == nil then
          return profile_menu(window, pane, idx)
        end
        line = util.trim(line)
        if line == '' then
          return store(nil)
        end
        if spec.kind == 'number' then
          local n = tonumber(line)
          if not n then
            toast(window, spec.label .. ' 需要是数字', 4000)
            return profile_menu(window, pane, idx)
          end
          return store(n)
        end
        if spec.kind == 'list' then
          local items = {}
          for _, item in ipairs(util.split(line, ',')) do
            item = util.trim(item)
            if item ~= '' then
              table.insert(items, item)
            end
          end
          return store(#items > 0 and items or nil)
        end
        store(line)
      end,
    })
  end

  ---------------------------------------------------------------------------
  scripts_menu = function(window, pane, idx)
    local list, header = M.load_store(cfg)
    if not list or not list[idx] then
      return main_menu(window, pane)
    end
    local p = list[idx]
    p.options = p.options or {}
    local steps = p.options.scripts or {}

    local choices = {}
    for i, s in ipairs(steps) do
      local expect = type(s) == 'table' and s.expect or ''
      local send = type(s) == 'table' and s.send or tostring(s)
      table.insert(choices, {
        id = 's' .. i,
        label = wezterm.format {
          { Text = string.format('%d. ', i) },
          { Foreground = { AnsiColor = 'Yellow' } },
          { Text = (expect == '' or expect == nil) and '(立即)' or ('等 ' .. tostring(expect)) },
          'ResetAttributes',
          { Text = '  →  ' .. tostring(send or '') },
        },
      })
    end
    table.insert(choices, { id = NEW, label = '＋  添加一步' })
    table.insert(choices, { id = BACK, label = '←  返回' })

    menu(window, pane, {
      title = '登录脚本  ·  ' .. tostring(p.name),
      choices = choices,
      description = '按顺序执行；expect 留空 = 立即发送    Esc 返回',
      on_cancel = function(w, p2)
        profile_menu(w, p2, idx)
      end,
      on_pick = function(win, pn, id)
        if id == BACK then
          return profile_menu(win, pn, idx)
        end
        if id == NEW then
          table.insert(steps, { expect = '', send = '' })
          p.options.scripts = steps
          if commit(win, list, header) then
            script_menu(win, pn, idx, #steps)
          end
          return
        end
        script_menu(win, pn, idx, tonumber(id:sub(2)))
      end,
    })
  end

  ---------------------------------------------------------------------------
  script_menu = function(window, pane, idx, sidx)
    local list, header = M.load_store(cfg)
    if not list or not list[idx] then
      return main_menu(window, pane)
    end
    local p = list[idx]
    local steps = p.options.scripts or {}
    local s = steps[sidx]
    if type(s) == 'string' then
      s = { expect = '', send = s }
      steps[sidx] = s
    end
    if not s then
      return scripts_menu(window, pane, idx)
    end

    local function save_and_back()
      if commit(window, list, header) then
        script_menu(window, pane, idx, sidx)
      end
    end

    menu(window, pane, {
      title = string.format('第 %d 步', sidx),
      choices = {
        { id = 'expect', label = string.format('%-12s%s', '等待文本', s.expect ~= '' and s.expect or '(立即执行)') },
        { id = 'send', label = string.format('%-12s%s', '发送', tostring(s.send or NIL)) },
        { id = 'isRegex', label = string.format('%-12s%s', '正则匹配', s.isRegex and '是 (Lua pattern)' or '否 (子串)') },
        { id = 'optional', label = string.format('%-12s%s', '可跳过', s.optional and '是' or '否') },
        { id = 'hide', label = string.format('%-12s%s', '日志隐藏', s.hide and '是' or '否') },
        { id = 'up', label = '↑  上移' },
        { id = 'down', label = '↓  下移' },
        { id = 'del', label = '🗑  删除这一步' },
        { id = BACK, label = '←  返回' },
      },
      on_cancel = function(w, p2)
        scripts_menu(w, p2, idx)
      end,
      on_pick = function(win, pn, id)
        if id == BACK then
          return scripts_menu(win, pn, idx)
        elseif id == 'isRegex' or id == 'optional' or id == 'hide' then
          s[id] = not s[id] or nil
          return save_and_back()
        elseif id == 'del' then
          table.remove(steps, sidx)
          commit(win, list, header)
          return scripts_menu(win, pn, idx)
        elseif id == 'up' and sidx > 1 then
          steps[sidx], steps[sidx - 1] = steps[sidx - 1], steps[sidx]
          commit(win, list, header)
          return script_menu(win, pn, idx, sidx - 1)
        elseif id == 'down' and sidx < #steps then
          steps[sidx], steps[sidx + 1] = steps[sidx + 1], steps[sidx]
          commit(win, list, header)
          return script_menu(win, pn, idx, sidx + 1)
        elseif id == 'up' or id == 'down' then
          return script_menu(win, pn, idx, sidx)
        end
        ask(win, pn, {
          description = id == 'expect'
              and '等待屏幕上出现的文本    留空 = 立即执行'
            or '要发送的内容    支持 ${password} ${user} ${host} 和 \\n \\t',
          initial = tostring(s[id] or ''),
          on_done = function(w2, p2, line)
            if line ~= nil then
              s[id] = line
            end
            if commit(w2, list, header) then
              script_menu(w2, p2, idx, sidx)
            end
          end,
        })
      end,
    })
  end

  ---------------------------------------------------------------------------
  forwards_menu = function(window, pane, idx)
    local list, header = M.load_store(cfg)
    if not list or not list[idx] then
      return main_menu(window, pane)
    end
    local p = list[idx]
    p.options = p.options or {}
    local fwds = p.options.forwardedPorts or {}

    local choices = {}
    for i, f in ipairs(fwds) do
      local desc
      if type(f) == 'string' then
        desc = f
      elseif f.type == 'Dynamic' then
        desc = string.format('SOCKS  %s:%s', f.host or '*', tostring(f.port))
      else
        desc = string.format(
          '%-7s %s:%s → %s:%s',
          f.type or 'Local',
          f.host or '*',
          tostring(f.port),
          tostring(f.targetAddress),
          tostring(f.targetPort)
        )
      end
      table.insert(choices, { id = 'p' .. i, label = string.format('%d. %s', i, desc) })
    end
    table.insert(choices, { id = NEW, label = '＋  添加转发' })
    table.insert(choices, { id = BACK, label = '←  返回' })

    menu(window, pane, {
      title = '端口转发  ·  ' .. tostring(p.name),
      choices = choices,
      on_cancel = function(w, p2)
        profile_menu(w, p2, idx)
      end,
      on_pick = function(win, pn, id)
        if id == BACK then
          return profile_menu(win, pn, idx)
        end
        if id == NEW then
          return menu(win, pn, {
            title = '转发类型',
            choices = {
              { id = 'Local', label = 'Local   本地端口 → 远端服务  (-L)' },
              { id = 'Remote', label = 'Remote  远端端口 → 本地服务  (-R)' },
              { id = 'Dynamic', label = 'Dynamic SOCKS 代理          (-D)' },
            },
            on_cancel = function(w2, p2)
              forwards_menu(w2, p2, idx)
            end,
            on_pick = function(w2, p2, kind)
              ask(w2, p2, {
                description = kind == 'Dynamic' and '本地端口，例如 1080'
                  or '本地端口:目标主机:目标端口，例如 15432:db.example.com:5432',
                on_done = function(w3, p3, line)
                  if not line or util.trim(line) == '' then
                    return forwards_menu(w3, p3, idx)
                  end
                  local parts = util.split(util.trim(line), ':')
                  local entry
                  if kind == 'Dynamic' then
                    entry = { type = kind, host = '127.0.0.1', port = tonumber(parts[1]) }
                  else
                    entry = {
                      type = kind,
                      host = '127.0.0.1',
                      port = tonumber(parts[1]),
                      targetAddress = parts[2],
                      targetPort = tonumber(parts[3]),
                    }
                  end
                  if not entry.port then
                    toast(w3, '端口要是数字', 4000)
                    return forwards_menu(w3, p3, idx)
                  end
                  table.insert(fwds, entry)
                  p.options.forwardedPorts = fwds
                  commit(w3, list, header)
                  forwards_menu(w3, p3, idx)
                end,
              })
            end,
          })
        end
        forward_menu(win, pn, idx, tonumber(id:sub(2)))
      end,
    })
  end

  ---------------------------------------------------------------------------
  forward_menu = function(window, pane, idx, fidx)
    local list, header = M.load_store(cfg)
    if not list or not list[idx] then
      return main_menu(window, pane)
    end
    local fwds = list[idx].options.forwardedPorts or {}
    menu(window, pane, {
      title = '转发 ' .. tostring(fidx),
      choices = {
        { id = 'del', label = '🗑  删除' },
        { id = BACK, label = '←  返回' },
      },
      on_cancel = function(w, p2)
        forwards_menu(w, p2, idx)
      end,
      on_pick = function(win, pn, id)
        if id == 'del' then
          table.remove(fwds, fidx)
          commit(win, list, header)
        end
        forwards_menu(win, pn, idx)
      end,
    })
  end

  return main_menu
end

--- Key assignment / palette action that opens the panel.
function M.action(state)
  return wezterm.action_callback(function(window, pane)
    M.open(state)(window, pane)
  end)
end

return M
