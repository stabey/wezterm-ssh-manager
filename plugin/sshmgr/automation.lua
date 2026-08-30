-- sshmgr.automation -- Tabby-style "login scripts" (expect / send) for a pane
--
-- Tabby feeds every byte of the session through a matcher; wezterm gives us a
-- rendered screen instead, so we poll the pane and match against everything
-- that appeared since the previous step completed. In practice that behaves
-- the same for login sequences, and it additionally survives the remote side
-- redrawing the line.
local wezterm = require 'wezterm'
local act = wezterm.action
local util = require 'sshmgr.util'

local M = {}

--- Grab the tail of the pane as plain text.
local function pane_text(pane, lines)
  local ok, text = pcall(pane.get_lines_as_text, pane, lines)
  if not ok then
    return nil
  end
  return text
end

--- Find `step.expect` in `text` at or after `from`.
--- Returns the index just past the match, or nil.
local function match_at(text, step, from)
  if step.expect == nil or step.expect == '' then
    return from -- unconditional: fires immediately
  end
  local ok, _, e = pcall(string.find, text, step.expect, from, not step.isRegex)
  if ok and e then
    return e + 1
  end
  return nil
end

M._match_at = match_at

--- Count password-attempt failures in a pane fragment. Key-auth's
--- "Permission denied (publickey)" must not count. `keyboard-interactive` is
--- included because password profiles prefer that OpenSSH method first.
local function password_rejection_counts(region)
  -- Pane text may insert a newline in the middle of any word on resize. Remove
  -- layout whitespace before parsing so `pass\nword` remains `password`.
  local lower = (region:lower():gsub('%s+', ''))
  local from = 1
  local count = 0
  local retry_count = 0
  while true do
    local denial_start = lower:find('permissiondenied', from, true)
    if not denial_start then
      return count, retry_count
    end

    -- Bound one event so unrelated text much later in scrollback cannot turn
    -- an ordinary filesystem error into an authentication failure. This also
    -- works on whitespace-normalised text used by the reflow-safe observer.
    local next_denial = lower:find('permissiondenied', denial_start + 1, true)
    local event_end = math.min((next_denial or (#lower + 1)) - 1, denial_start + 512)
    local event = lower:sub(denial_start, event_end)
    local method_prefix = event:sub(1, 256)
    local explicit_method = method_prefix:find('%([^)]*password[^)]*%)')
      or method_prefix:find('%([^)]*keyboard%-interactive[^)]*%)')
    local retry = event:find('pleasetryagain', 1, true)
    local retry_prompt = retry and event:find('password:', retry + 1, true)
    if explicit_method or retry_prompt then
      count = count + 1
    end
    if retry_prompt then
      retry_count = retry_count + 1
    end
    from = denial_start + #'permissiondenied'
  end
end

local function password_rejection_count(region)
  local count = password_rejection_counts(region)
  return count
end

local function password_was_rejected(region)
  return password_rejection_count(region) > 0
end

M._password_was_rejected = password_was_rejected

--- Ignore line wrapping when comparing pane snapshots. The rendered pane can
--- insert/remove newlines on resize even though no remote output arrived.
local function normalize_screen(text)
  return ((text or ''):gsub('%s+', ''))
end

--- Return text appended since the previous rendered snapshot. A terminal may
--- discard lines from the front, so align the current prefix with a suffix of
--- the previous snapshot. `continuous=false` means the screen was replaced;
--- callers must treat the current contents as a new baseline, not old output.
--- The third return value is text revealed before an intact old snapshot.
local function screen_delta(previous, current)
  if previous == '' then
    return current, true
  end
  if current:sub(1, #previous) == previous then
    return current:sub(#previous + 1), true
  end
  if current == '' then
    return '', false
  end

  -- A taller viewport can reveal older scrollback at the front while keeping
  -- the prior snapshot intact. Only the suffix after that snapshot is new.
  local contained_at
  local contained_from = 1
  while true do
    local found = current:find(previous, contained_from, true)
    if not found then
      break
    end
    contained_at = found
    contained_from = found + 1
  end
  if contained_at then
    return current:sub(contained_at + #previous), true, current:sub(1, contained_at - 1)
  end

  local anchor_len = math.min(64, #current)
  local anchor = current:sub(1, anchor_len)
  local search_from = 1
  local best = 0
  while true do
    local found = previous:find(anchor, search_from, true)
    if not found then
      break
    end
    local overlap = #previous - found + 1
    if overlap <= #current
      and overlap > best
      and previous:sub(found) == current:sub(1, overlap)
    then
      best = overlap
    end
    search_from = found + 1
  end
  if best > 0 then
    return current:sub(best + 1), true
  end
  return '', false
end

local function last_plain_find(text, needle)
  local last
  local from = 1
  while true do
    local found = text:find(needle, from, true)
    if not found then
      return last
    end
    last = found
    from = found + 1
  end
end

local function raw_index_after_compact(text, compact_index)
  if compact_index <= 0 then
    return 1
  end
  local seen = 0
  for i = 1, #text do
    if not text:sub(i, i):match('%s') then
      seen = seen + 1
      if seen == compact_index then
        return i + 1
      end
    end
  end
  return #text + 1
end

--- Relocate a raw pane cursor after resize/reflow. Prefer the complete text
--- before the old cursor; if the viewport cropped it, use a bounded suffix.
local function remap_cursor(previous, consumed, current)
  local prefix = normalize_screen(previous:sub(1, math.max(consumed - 1, 0)))
  if prefix == '' then
    return 1
  end
  local compact = normalize_screen(current)
  local found = last_plain_find(compact, prefix)
  local matched = #prefix
  if not found then
    local anchor_len = math.min(256, #prefix)
    while anchor_len >= 32 and not found do
      found = last_plain_find(compact, prefix:sub(-anchor_len))
      matched = anchor_len
      anchor_len = math.floor(anchor_len / 2)
    end
  end
  if not found then
    return 1
  end
  return raw_index_after_compact(current, found + matched - 1)
end

local function describe(step)
  if step.expect == '' then
    return 'send'
  end
  return string.format('expect %q', step.expect)
end

--- Whether the built-in password expect/prompt steps apply to this profile.
--- Explicit key/agent/kbd-interactive authentication must never wait for a
--- password prompt. With an omitted auth method, an explicit password source
--- preserves the existing shorthand; Tabby also keeps its key-then-password
--- fallback behaviour.
local function wants_password_automation(profile, ctx)
  local options = profile.options or {}
  if options.auth == 'password' then
    return true
  end
  if options.auth ~= nil then
    return false
  end
  -- Preserve the long-standing shorthand where a profile omits `auth` but
  -- explicitly configures a password source. A resolved global provider is
  -- represented by ctx.password; option-based sources also count as intent
  -- when resolution failed and a prompt is therefore still useful.
  if (ctx and type(ctx.password) == 'string' and ctx.password ~= '')
    or (type(options.password) == 'string' and options.password ~= '')
    or (type(options.password_env) == 'string' and options.password_env ~= '')
    or options.password_cmd ~= nil
  then
    return true
  end
  local origin = require('sshmgr.profiles').origin(profile)
  return type(origin) == 'table' and origin.kind == 'tabby'
end

M._wants_password_automation = wants_password_automation

--- Build the effective step list: injected steps first, then the profile's.
function M.build_steps(profile, cfg, ctx)
  local a = cfg.automation
  local steps = {}

  if a.auto_host_key and (profile.host_key_policy or cfg.host_key_policy) == 'ask' then
    table.insert(steps, {
      expect = 'Are you sure you want to continue connecting',
      -- the password prompt can only appear after the host key was accepted,
      -- so seeing it cancels this wait instead of blocking on the timeout
      skip_if = 'assword',
      send = 'yes',
      optional = true,
      timeout = 8,
      _label = 'host key',
    })
  end

  local wants_password = wants_password_automation(profile, ctx)
  if a.auto_password and wants_password and ctx.password then
    table.insert(steps, {
      expect = 'assword',
      send = '${password}',
      optional = true,
      hide = true,
      -- slow servers can take well past the 25s step_timeout to even show
      -- the prompt (observed >39s on a busy host), so give this one room
      timeout = math.max(tonumber(profile.options.readyTimeout) or 30, 60),
      _label = 'password',
    })
  elseif a.auto_password
      and wants_password
      and a.save_passwords ~= false
      and not ctx.password
  then
    local who = (profile.options.user and (profile.options.user .. '@') or '')
      .. (profile.options.host or profile.name or 'ssh')
    table.insert(steps, {
      expect = 'assword',
      prompt = '密码  ' .. who .. '  （登录成功后会记住）',
      optional = true,
      hide = true,
      _capture_password = true,
      timeout = math.max(tonumber(profile.options.readyTimeout) or 30, 60),
      _label = 'password-capture',
    })
    table.insert(steps, {
      expect = 'Permission denied',
      optional = true,
      timeout = 5,
      _password_rejected = true,
      send = nil,
      _label = 'password-reject',
    })
  end

  -- tolerate a hand-built profile that never went through profiles.normalize
  local scripts = require('sshmgr.profiles').normalize_scripts(profile.options.scripts)
  for _, s in ipairs(scripts) do
    local active = s.send ~= nil
      or s.prompt ~= nil
      or (s.expect and s.expect ~= '')
      or (tonumber(s.delay) or 0) > 0
    if active then
      table.insert(steps, s)
    end
  end

  -- `on_login` convenience: wait for a shell prompt, then run these verbatim.
  local after = profile.on_login or profile.options.on_login
  if type(after) == 'string' then
    after = { after }
  end
  if type(after) == 'table' and #after > 0 then
    table.insert(steps, {
      expect = a.ready_pattern,
      isRegex = true,
      send = nil,
      optional = true,
      timeout = a.ready_timeout,
      _label = 'wait for prompt',
    })
    for _, cmd in ipairs(after) do
      table.insert(steps, { expect = '', send = cmd, delay = 0.05 })
    end
  end

  return steps
end

--- Kick off the state machine for `pane`.
--- `ctx` = {
---   password    = string|nil,
---   vars        = table,
---   on_done     = function(ok, why),
---   resolve_pane = function(pane_id) -> pane   (defaults to wezterm.mux.get_pane)
--- }
function M.start(pane, profile, cfg, ctx)
  local a = cfg.automation
  if not a.enabled then
    return
  end
  local resolve = ctx.resolve_pane or wezterm.mux.get_pane
  -- Private dependency hooks keep the state machine deterministic in tests.
  local schedule = ctx._call_after or wezterm.time.call_after
  local now = ctx._now or util.now
  local persist_password = ctx._persist_password
    or require('sshmgr.panel').persist_password

  local steps = M.build_steps(profile, cfg, ctx)
  if #steps == 0 then
    if ctx.on_done then
      ctx.on_done(true, 'no steps')
    end
    return
  end

  local pane_id = pane:pane_id()
  local vars = ctx.vars or {}
  vars.password = ctx.password
  vars.user = profile.options.user or ''
  vars.host = profile.options.host or ''
  vars.port = tostring(profile.options.port or 22)
  vars.name = profile.name

  local state = {
    idx = 1,
    consumed = 1,
    started = now(),
    step_started = now(),
    cursor_text = nil,
    reject_screen = nil,
    reject_output = '',
  }

  local function maybe_save_password()
    if a.save_passwords == false or ctx.password_saved then
      return
    end
    if not ctx.captured_password or ctx.password_rejected then
      return
    end
    local pok, perr = persist_password(cfg, profile, ctx.captured_password)
    if pok then
      util.log('%s: password saved to profile store', profile.name)
      ctx.password_saved = true
    else
      util.warn('%s: failed to save password: %s', profile.name, tostring(perr))
    end
  end

  local function finish(ok, why)
    if ok then
      util.log('%s: login script complete', profile.name)
      maybe_save_password()
    else
      util.warn('%s: login script stopped (%s)', profile.name, why)
      -- login scripts (e.g. wait for bash-5.1) can fail after a good password
      -- on fish hosts; still persist if we never saw a reject.
      maybe_save_password()
    end
    if ctx.on_done then
      pcall(ctx.on_done, ok, why)
    end
  end

  local tick

  local function advance(p, next_reject_baseline)
    state.idx = state.idx + 1
    state.step_started = now()
    if steps[state.idx] and steps[state.idx]._password_rejected then
      -- Snapshot immediately before sending the candidate password. Only output
      -- added after this baseline may reject this attempt; scrollback can hold
      -- failures from an older attempt.
      local baseline = next_reject_baseline
        or pane_text(p, a.scan_lines)
        or ''
      state.reject_screen = normalize_screen(baseline)
      state.reject_output = ''
    else
      state.reject_screen = nil
      state.reject_output = ''
    end
  end

  local function send_and_advance(p, step)
    local payload = step.send
    if payload ~= nil then
      payload = util.expand_vars(payload, vars)
      payload = util.unescape(payload)
      if not step.raw then
        payload = payload .. '\r'
      end
      p:send_text(payload)
      if not step.hide then
        util.log('%s: sent %q', profile.name, (step.send or ''):sub(1, 60))
      else
        util.log('%s: sent <hidden>', profile.name)
      end
    end
    advance(p)
  end

  --- A `prompt` step asks the user rather than sending a canned value.
  local function ask_user(p, step)
    local function accept_line(line)
      local ok2, target = pcall(resolve, pane_id)
      if not ok2 or not target then
        return finish(false, 'pane closed')
      end
      if line == nil or line == '' then
        if step._capture_password or not step.optional then
          if step._capture_password then
            ctx.captured_password = nil
          end
          return finish(false, step._capture_password and 'password prompt cancelled'
            or 'prompt cancelled')
        end
        advance(target)
        return schedule(a.poll_interval, tick)
      end
      if step._capture_password then
        ctx.captured_password = line
        vars.password = line
      end
      local reject_baseline
      if steps[state.idx + 1] and steps[state.idx + 1]._password_rejected then
        reject_baseline = pane_text(target, a.scan_lines) or ''
      end
      target:send_text(line .. (step.raw and '' or '\r'))
      if step.hide or step._capture_password then
        util.log('%s: sent <hidden>', profile.name)
      end
      advance(target, reject_baseline)
      schedule(a.poll_interval, tick)
    end

    if ctx._prompt_user then
      return ctx._prompt_user(step.prompt, accept_line)
    end

    local mux_win = p:window()
    local gui = mux_win and mux_win:gui_window()
    if not gui then
      if step._capture_password or not step.optional then
        if step._capture_password then
          ctx.captured_password = nil
        end
        return finish(false, step._capture_password and 'cannot prompt for password'
          or 'cannot prompt')
      end
      util.warn('%s: cannot prompt (no gui window); skipping step', profile.name)
      advance(p)
      schedule(a.poll_interval, tick)
      return
    end
    gui:perform_action(
      act.PromptInputLine {
        description = wezterm.format {
          { Attribute = { Intensity = 'Bold' } },
          { Foreground = { AnsiColor = 'Fuchsia' } },
          { Text = step.prompt },
        },
        action = wezterm.action_callback(function(_, _, line)
          accept_line(line)
        end),
      },
      p
    )
  end

  tick = function()
    local ok, p = pcall(resolve, pane_id)
    if not ok or not p then
      return finish(false, 'pane closed')
    end

    if now() - state.started > a.session_timeout then
      return finish(false, 'session timeout')
    end

    local step = steps[state.idx]
    if not step then
      return finish(true, 'done')
    end

    local text = pane_text(p, a.scan_lines) or ''
    if state.cursor_text ~= nil
      and text:sub(1, #state.cursor_text) ~= state.cursor_text
    then
      state.consumed = remap_cursor(state.cursor_text, state.consumed, text)
    end
    state.cursor_text = text

    -- A step with skip_if is abandoned the moment that pattern shows up:
    -- the password prompt means the host-key question is already past, and
    -- waiting out its timeout would only delay sending the password.
    if step.skip_if and text:find(step.skip_if, 1, true) then
      util.log('%s: skipping %s (%q visible)', profile.name, describe(step), step.skip_if)
      advance(p)
      return schedule(a.poll_interval, tick)
    end

    if step._password_rejected then
      local current = normalize_screen(text)
      local previous = state.reject_screen
      if previous == nil then
        state.reject_screen = current
        previous = current
      end
      local added, continuous, prepended = screen_delta(previous, current)
      local rejected = false
      if continuous then
        state.reject_output = (state.reject_output .. added):sub(-2048)
        rejected = password_was_rejected(state.reject_output)
        if not rejected and prepended and prepended ~= '' then
          -- If the old prompt was replaced by a retry plus an identical new
          -- prompt, the baseline appears at the tail rather than disappearing.
          -- A complete retry pair in that prefix is still post-send evidence.
          local _, prepended_retries = password_rejection_counts(prepended .. previous)
          rejected = prepended_retries > 0
        end
      else
        -- A clear/repaint can arrive in the same polling interval as a denial.
        -- Accept only newly introduced evidence; otherwise re-baseline so an
        -- old visible denial is never attributed to this password attempt.
        local current_rejections, current_retries = password_rejection_counts(current)
        local _, previous_retries = password_rejection_counts(previous)
        -- A method-list denial can re-enter the viewport when it grows. With
        -- no shared anchor, only the stronger retry-message + prompt sequence
        -- is useful evidence that this screen replacement contains a failure.
        rejected = current_retries > previous_retries
        -- Preserve an incomplete event across the next poll only when the new
        -- baseline has no complete rejection that could belong to scrollback.
        state.reject_output = current_rejections == 0 and current:sub(-512) or ''
      end
      state.reject_screen = current
      if rejected then
        ctx.password_rejected = true
        ctx.captured_password = nil
        util.warn('%s: password rejected, not saving', profile.name)
        return finish(false, 'password rejected')
      end
    else
      local hit = match_at(text, step, state.consumed)
      if hit then
        state.consumed = hit
        if step.prompt then
          return ask_user(p, step)
        end
        local delay = tonumber(step.delay) or 0
        if delay > 0 then
          schedule(delay, function()
            local ok2, p2 = pcall(resolve, pane_id)
            if ok2 and p2 then
              send_and_advance(p2, step)
            end
            schedule(a.poll_interval, tick)
          end)
          return
        end
        send_and_advance(p, step)
        return schedule(a.poll_interval, tick)
      end
    end

    local budget = step.timeout or a.step_timeout
    if now() - state.step_started > budget then
      if step.optional then
        util.log('%s: optional step timed out (%s), skipping', profile.name, describe(step))
        advance(p)
        return schedule(a.poll_interval, tick)
      end
      return finish(false, string.format('timed out waiting for %s', describe(step)))
    end

    schedule(a.poll_interval, tick)
  end

  schedule(a.poll_interval, tick)
end

return M
