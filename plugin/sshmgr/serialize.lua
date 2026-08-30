-- sshmgr.serialize -- render a lua value as readable, re-loadable lua source
local M = {}

local KEYWORDS = {
  ['and'] = true, ['break'] = true, ['do'] = true, ['else'] = true,
  ['elseif'] = true, ['end'] = true, ['false'] = true, ['for'] = true,
  ['function'] = true, ['goto'] = true, ['if'] = true, ['in'] = true,
  ['local'] = true, ['nil'] = true, ['not'] = true, ['or'] = true,
  ['repeat'] = true, ['return'] = true, ['then'] = true, ['true'] = true,
  ['until'] = true, ['while'] = true,
}

-- Preferred key order, so a generated file reads like a hand-written one.
local ORDER = {}
for i, k in ipairs {
  'id', 'name', 'group', 'icon', 'color', 'weight', 'behaviorOnSessionEnd',
  'host_key_policy', 'domain', 'ssh_binary', 'env', 'ssh_options', 'extra_args',
  'remote_command', 'cwd', 'on_login', 'options',
  -- inside options:
  'host', 'sftpHost', 'port', 'user', 'auth', 'password', 'password_env', 'password_cmd',
  'privateKeys', 'identityAgent', 'jumpHost', 'agentForward', 'x11', 'skipBanner',
  'keepaliveInterval', 'keepaliveCountMax', 'readyTimeout', 'algorithms',
  'proxyCommand', 'socksProxyHost', 'socksProxyPort', 'httpProxyHost',
  'httpProxyPort', 'forwardedPorts', 'reuseSession', 'warnOnClose', 'scripts',
  -- inside a login script step:
  'expect', 'isRegex', 'flavor', 'send', 'optional', 'timeout', 'raw', 'hide',
  'delay', 'prompt',
  -- inside a forwarded port:
  'type', 'targetAddress', 'targetPort', 'description',
} do
  ORDER[k] = i
end

local function quote(s)
  local escaped = s
    :gsub('\\', '\\\\')
    :gsub("'", "\\'")
    :gsub('\n', '\\n')
    :gsub('\r', '\\r')
    :gsub('\t', '\\t')
    :gsub('%c', function(c)
      return string.format('\\%d', c:byte())
    end)
  return "'" .. escaped .. "'"
end

local function is_ident(k)
  return type(k) == 'string' and k:match '^[%a_][%w_]*$' ~= nil and not KEYWORDS[k]
end

local function is_array(t)
  local n = 0
  for k in pairs(t) do
    if type(k) ~= 'number' then
      return false
    end
    n = n + 1
  end
  return n == #t
end

local function sorted_keys(t)
  local keys = {}
  for k in pairs(t) do
    table.insert(keys, k)
  end
  table.sort(keys, function(a, b)
    local ao, bo = ORDER[a], ORDER[b]
    if ao and bo then
      return ao < bo
    end
    if ao then
      return true
    end
    if bo then
      return false
    end
    return tostring(a) < tostring(b)
  end)
  return keys
end

--- True when a table is small and flat enough to sit on one line.
local function fits_inline(t)
  if not is_array(t) or #t == 0 or #t > 8 then
    return false
  end
  local len = 0
  for _, v in ipairs(t) do
    if type(v) == 'table' then
      return false
    end
    len = len + #tostring(v) + 4
  end
  return len <= 76
end

local function encode(v, indent, out)
  local pad = string.rep('  ', indent)
  local tv = type(v)

  if tv == 'string' then
    table.insert(out, quote(v))
  elseif tv == 'number' or tv == 'boolean' then
    table.insert(out, tostring(v))
  elseif tv == 'nil' then
    table.insert(out, 'nil')
  elseif tv ~= 'table' then
    table.insert(out, quote(tostring(v)))
  elseif next(v) == nil then
    table.insert(out, '{}')
  elseif fits_inline(v) then
    local parts = {}
    for _, item in ipairs(v) do
      local buf = {}
      encode(item, indent, buf)
      table.insert(parts, table.concat(buf))
    end
    table.insert(out, '{ ' .. table.concat(parts, ', ') .. ' }')
  else
    table.insert(out, '{\n')
    if is_array(v) then
      for _, item in ipairs(v) do
        table.insert(out, pad .. '  ')
        encode(item, indent + 1, out)
        table.insert(out, ',\n')
      end
    else
      for _, k in ipairs(sorted_keys(v)) do
        table.insert(out, pad .. '  ')
        if is_ident(k) then
          table.insert(out, k .. ' = ')
        else
          table.insert(out, '[' .. quote(tostring(k)) .. '] = ')
        end
        encode(v[k], indent + 1, out)
        table.insert(out, ',\n')
      end
    end
    table.insert(out, pad .. '}')
  end
end

--- Serialise `value` as lua source. `indent` is the starting depth.
function M.encode(value, indent)
  local out = {}
  encode(value, indent or 0, out)
  return table.concat(out)
end

return M
