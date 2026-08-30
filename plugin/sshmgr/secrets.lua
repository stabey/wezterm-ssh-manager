-- sshmgr.secrets -- resolve a profile password without hard-coding it
local wezterm = require 'wezterm'
local util = require 'sshmgr.util'

local M = {}

--- Resolve the password for `profile`, in priority order:
---   1. cfg.password_provider(profile)
---   2. options.password_cmd   (argv array, or string run through a shell)
---   3. options.password_env   (name of an environment variable)
---   4. options.password       (literal -- avoid in files you commit)
--- Returns nil when nothing is configured.
function M.resolve(profile, cfg)
  local o = profile.options or {}

  if type(cfg.password_provider) == 'function' then
    local ok, v = pcall(cfg.password_provider, profile)
    if ok and type(v) == 'string' and v ~= '' then
      return v
    elseif not ok then
      util.err('password_provider failed for %q: %s', profile.name, tostring(v))
    end
  end

  if o.password_cmd then
    local argv
    if type(o.password_cmd) == 'table' then
      argv = o.password_cmd
    else
      argv = {}
      util.tbl_concat_into(argv, cfg.password_cmd_shell)
      table.insert(argv, o.password_cmd)
    end
    local ok, success, stdout, stderr = pcall(wezterm.run_child_process, argv)
    if ok and success then
      local v = (stdout or ''):gsub('[\r\n]+$', '')
      if v ~= '' then
        return v
      end
    else
      util.err('password_cmd failed for %q: %s', profile.name, tostring(stderr or success))
    end
  end

  if o.password_env then
    local v = os.getenv(o.password_env)
    if v and v ~= '' then
      return v
    end
    util.warn('password_env %q is empty for profile %q', o.password_env, profile.name)
  end

  if type(o.password) == 'string' and o.password ~= '' then
    return o.password
  end

  return nil
end

return M
