-- sshmgr.caps -- ask the local ssh client what it actually supports
--
-- Tabby's algorithm panel is a multi-select that defaults to *everything*, and
-- its menu comes from the ssh2 javascript library, so an exported profile
-- routinely lists names OpenSSH has never heard of (arcfour, blowfish-cbc,
-- bare aes128-gcm, hmac-ripemd160, ext-info-c, kex-strict-*, ...).
--
-- OpenSSH validates these lists strictly: one unknown name and it exits before
-- connecting, which shows up as a pane that flashes open and closes. So we ask
-- the client itself, via `ssh -Q`, and drop what it does not know.
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'

local M = {}

-- Tabby algorithm type -> the `ssh -Q` queries that answer it, best first.
-- HostKeyAlgorithms is the correct query for -o HostKeyAlgorithms but only
-- exists on OpenSSH 8.5+; key-sig and key are the older spellings.
local QUERIES = {
  cipher = { 'cipher' },
  hmac = { 'mac' },
  kex = { 'kex' },
  serverHostKey = { 'HostKeyAlgorithms', 'key-sig', 'key' },
}

local GLOBAL_KEY = 'sshmgr_caps'

local function cache_get(key)
  local c = wezterm.GLOBAL[GLOBAL_KEY]
  if type(c) ~= 'table' then
    return nil
  end
  return c[key]
end

local function cache_put(key, value)
  local c = wezterm.GLOBAL[GLOBAL_KEY]
  if type(c) ~= 'table' then
    c = {}
  end
  c[key] = value
  wezterm.GLOBAL[GLOBAL_KEY] = c
end

--- Run `ssh -Q <query>` and return the names it prints.
local function query(ssh_binary, q)
  local ok, success, stdout = pcall(wezterm.run_child_process, { ssh_binary, '-Q', q })
  if not ok or not success or not stdout then
    return nil
  end
  local names = {}
  for line in tostring(stdout):gmatch '[^\r\n]+' do
    line = util.trim(line)
    if line ~= '' then
      table.insert(names, line)
    end
  end
  if #names == 0 then
    return nil
  end
  return names
end

--- The set of algorithm names `ssh_binary` accepts for `kind`.
--- Returns a table keyed by name, or nil when we could not find out (no ssh on
--- PATH, sandboxed, ancient client). Cached for the lifetime of the process,
--- so config reloads do not re-spawn ssh.
function M.supported(ssh_binary, kind)
  local queries = QUERIES[kind]
  if not queries then
    return nil
  end
  local key = ssh_binary .. '|' .. kind
  local cached = cache_get(key)
  if cached == false then
    return nil
  elseif type(cached) == 'table' then
    local set = {}
    for _, n in ipairs(cached) do
      set[n] = true
    end
    return set
  end

  for _, q in ipairs(queries) do
    local names = query(ssh_binary, q)
    if names then
      cache_put(key, names)
      local set = {}
      for _, n in ipairs(names) do
        set[n] = true
      end
      return set
    end
  end

  cache_put(key, false)
  util.warn(
    "could not ask %s what %s algorithms it supports; leaving the list unfiltered",
    ssh_binary,
    kind
  )
  return nil
end

--- Drop names `ssh_binary` does not know from an algorithm list.
--- Returns filtered, dropped. `filtered` is nil when nothing usable is left --
--- the caller must then omit the option entirely rather than emit an empty one,
--- which OpenSSH also rejects.
---
--- A leading '+', '-' or '^' on the first element is OpenSSH's
--- append/remove/prepend syntax and applies to the whole value, so it is
--- carried across. Entries containing glob characters are passed through
--- untouched because they are patterns, not names.
function M.filter(list, kind, ssh_binary)
  if type(list) ~= 'table' or #list == 0 then
    return nil, {}
  end
  local set = M.supported(ssh_binary, kind)
  if not set then
    return list, {}
  end

  local prefix = ''
  local first = tostring(list[1])
  local p = first:sub(1, 1)
  if p == '+' or p == '-' or p == '^' then
    prefix = p
  end

  local kept, dropped = {}, {}
  for i, raw in ipairs(list) do
    local name = tostring(raw)
    if i == 1 and prefix ~= '' then
      name = name:sub(2)
    end
    if name == '' then
      -- skip
    elseif name:find '[*?]' or set[name] then
      table.insert(kept, name)
    else
      table.insert(dropped, name)
    end
  end

  if #kept == 0 then
    return nil, dropped
  end
  kept[1] = prefix .. kept[1]
  return kept, dropped
end

return M
