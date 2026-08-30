-- sshmgr.util -- small helpers shared by the other modules
local wezterm = require 'wezterm'

local M = {}

M.sep = package.config:sub(1, 1)
M.is_windows = wezterm.target_triple:find 'windows' ~= nil

function M.log(fmt, ...)
  wezterm.log_info('[ssh-manager] ' .. string.format(fmt, ...))
end

function M.warn(fmt, ...)
  wezterm.log_warn('[ssh-manager] ' .. string.format(fmt, ...))
end

function M.err(fmt, ...)
  wezterm.log_error('[ssh-manager] ' .. string.format(fmt, ...))
end

function M.is_array(t)
  return type(t) == 'table' and (#t > 0 or next(t) == nil)
end

--- Deep copy a plain lua value.
function M.deep_copy(v)
  if type(v) ~= 'table' then
    return v
  end
  local out = {}
  for k, vv in pairs(v) do
    out[k] = M.deep_copy(vv)
  end
  return out
end

--- Merge `src` into `dst` recursively. Values already present in `dst` win.
--- Arrays are treated as scalars (not concatenated) so a profile can fully
--- override a group default.
function M.defaults_into(dst, src)
  if type(src) ~= 'table' then
    return dst
  end
  dst = dst or {}
  for k, v in pairs(src) do
    if dst[k] == nil then
      dst[k] = M.deep_copy(v)
    elseif type(dst[k]) == 'table' and type(v) == 'table' and not M.is_array(v) then
      M.defaults_into(dst[k], v)
    end
  end
  return dst
end

function M.trim(s)
  return (s:gsub('^%s+', ''):gsub('%s+$', ''))
end

--- Split on a plain separator.
function M.split(s, sep)
  local out = {}
  local pos = 1
  while true do
    local a, b = s:find(sep, pos, true)
    if not a then
      table.insert(out, s:sub(pos))
      return out
    end
    table.insert(out, s:sub(pos, a - 1))
    pos = b + 1
  end
end

--- Read a whole file, returns nil if it cannot be opened.
function M.read_file(path)
  local f = io.open(path, 'rb')
  if not f then
    return nil
  end
  local data = f:read '*a'
  f:close()
  return data
end

function M.file_exists(path)
  local f = io.open(path, 'rb')
  if f then
    f:close()
    return true
  end
  return false
end

function M.path_join(...)
  return table.concat({ ... }, M.sep)
end

--- Expand a leading `~` and `$VAR` / `%VAR%` references in a path.
function M.expand_path(p)
  if type(p) ~= 'string' then
    return p
  end
  p = p:gsub('^~', wezterm.home_dir)
  p = p:gsub('%$([%w_]+)', function(name)
    return os.getenv(name) or ('$' .. name)
  end)
  p = p:gsub('%%([%w_]+)%%', function(name)
    return os.getenv(name) or ('%' .. name .. '%')
  end)
  return p
end

--- `${...}` template expansion used by login-script `send` values and titles.
--- Supported: ${name} from `vars`, ${env:NAME} from the environment.
function M.expand_vars(s, vars)
  if type(s) ~= 'string' then
    return s
  end
  return (s:gsub('%${([^}]+)}', function(key)
    local env = key:match '^env:(.+)$'
    if env then
      return os.getenv(env) or ''
    end
    local v = vars[key]
    if v == nil then
      return '${' .. key .. '}'
    end
    return tostring(v)
  end))
end

--- Interpret the usual backslash escapes inside a `send` payload.
function M.unescape(s)
  if type(s) ~= 'string' then
    return s
  end
  local map = { n = '\n', r = '\r', t = '\t', e = '\27', a = '\7', ['0'] = '\0', ['\\'] = '\\' }
  return (s:gsub('\\(.)', function(c)
    return map[c] or ('\\' .. c)
  end))
end

--- Monotonic-ish wall clock in fractional seconds.
function M.now()
  return os.time() + (os.clock() % 1)
end

--- Parse `[user@]host[:port]` into its parts.
function M.parse_target(spec)
  local out = {}
  local rest = spec
  local user, r = rest:match '^([^@]+)@(.+)$'
  if user then
    out.user = user
    rest = r
  end
  -- bracketed IPv6: [::1]:22
  local v6, port = rest:match '^%[(.+)%]:(%d+)$'
  if v6 then
    out.host, out.port = v6, tonumber(port)
    return out
  end
  local v6only = rest:match '^%[(.+)%]$'
  if v6only then
    out.host = v6only
    return out
  end
  local h, p = rest:match '^([^:]+):(%d+)$'
  if h then
    out.host, out.port = h, tonumber(p)
    return out
  end
  out.host = rest
  return out
end

--- Quote a single argument for a POSIX `sh -c` string.
function M.sh_quote(arg)
  return "'" .. tostring(arg):gsub("'", "'\\''") .. "'"
end

--- Quote a single argument for a PowerShell single-quoted string literal.
function M.ps_quote(arg)
  return "'" .. tostring(arg):gsub("'", "''") .. "'"
end

function M.tbl_concat_into(dst, src)
  for _, v in ipairs(src or {}) do
    table.insert(dst, v)
  end
  return dst
end

--- Sort helper producing a stable order: weight desc, then group, then name.
function M.sort_profiles(list)
  table.sort(list, function(a, b)
    local aw, bw = a.weight or 0, b.weight or 0
    if aw ~= bw then
      return aw > bw
    end
    local ag, bg = a.group or '', b.group or ''
    if ag ~= bg then
      return ag < bg
    end
    return (a.name or '') < (b.name or '')
  end)
  return list
end

---------------------------------------------------------------------------
-- JavaScript regex -> Lua pattern
--
-- Tabby's login scripts store `expect` as a JS RegExp when `isRegex` is set.
-- Lua has patterns, not regexes, so we translate the subset that actually
-- shows up in login prompts and refuse the rest rather than silently
-- mis-matching. Returns pattern, or nil plus the construct we choked on.
---------------------------------------------------------------------------
local JS_CLASS = {
  d = '%d', D = '%D', w = '%w', W = '%W', s = '%s', S = '%S',
}
-- characters that are literal in a regex but magic in a lua pattern
local LUA_MAGIC = '^$()%.[]*+-?'

function M.js_regex_to_lua(re)
  local out = {}
  local i, n = 1, #re
  local in_class = false

  while i <= n do
    local c = re:sub(i, i)

    if c == '\\' then
      local nxt = re:sub(i + 1, i + 1)
      if nxt == '' then
        return nil, 'trailing backslash'
      end
      local cls = JS_CLASS[nxt]
      if cls then
        if in_class then
          -- lua allows %d etc. inside a set
          table.insert(out, cls)
        else
          table.insert(out, cls)
        end
      elseif nxt == 'b' or nxt == 'B' then
        return nil, '\\' .. nxt .. ' (word boundary)'
      elseif nxt == 'n' then
        table.insert(out, '\n')
      elseif nxt == 'r' then
        table.insert(out, '\r')
      elseif nxt == 't' then
        table.insert(out, '\t')
      elseif LUA_MAGIC:find(nxt, 1, true) then
        table.insert(out, '%' .. nxt)
      else
        table.insert(out, nxt)
      end
      i = i + 2
    elseif in_class then
      if c == ']' then
        in_class = false
        table.insert(out, ']')
      elseif c == '%' then
        table.insert(out, '%%')
      else
        table.insert(out, c)
      end
      i = i + 1
    elseif c == '[' then
      in_class = true
      table.insert(out, '[')
      i = i + 1
      if re:sub(i, i) == '^' then
        table.insert(out, '^')
        i = i + 1
      end
    elseif c == '|' then
      return nil, '| (alternation)'
    elseif c == '(' then
      if re:sub(i, i + 2) == '(?:' then
        return nil, '(?: (non-capturing group)'
      end
      return nil, '( (group)'
    elseif c == ')' then
      return nil, ') (group)'
    elseif c == '{' then
      return nil, '{n,m} (counted repetition)'
    elseif c == '%' then
      table.insert(out, '%%')
      i = i + 1
    elseif c == '-' then
      -- literal in a regex, but the lazy quantifier in a lua pattern
      table.insert(out, '%-')
      i = i + 1
    elseif c == '+' or c == '*' then
      -- lazy quantifiers: JS `+?`/`*?` -> lua `-` only exists for `*?`
      if re:sub(i + 1, i + 1) == '?' then
        if c == '*' then
          table.insert(out, '-')
          i = i + 2
        else
          return nil, '+? (lazy plus)'
        end
      else
        table.insert(out, c)
        i = i + 1
      end
    else
      table.insert(out, c)
      i = i + 1
    end
  end

  if in_class then
    return nil, 'unterminated ['
  end
  return table.concat(out)
end

return M
