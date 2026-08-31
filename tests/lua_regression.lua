-- Run from the repository root:
--   wezterm --config-file tests/lua_regression.lua ls-fonts --text a >/dev/null
--
-- This is a WezTerm config file so the plugin modules run against WezTerm's
-- real embedded Lua API. WezTerm reports config errors but may still exit 0,
-- so the failure path below exits explicitly for CI and shell callers.
package.path = table.concat({
  './plugin/?.lua',
  './plugin/?/init.lua',
  package.path,
}, ';')

local wezterm = require 'wezterm'
local configmod = require 'sshmgr.config'
local profiles = require 'sshmgr.profiles'
local importer = require 'sshmgr.import'
local automation = require 'sshmgr.automation'
local panel = require 'sshmgr.panel'
local tui = require 'sshmgr.tui'
local util = require 'sshmgr.util'

local checked = 0
local function eq(actual, expected, label)
  checked = checked + 1
  assert(actual == expected, string.format(
    '%s: expected %q, got %q', label, tostring(expected), tostring(actual)
  ))
end

local tmpbase = os.tmpname()
os.remove(tmpbase)
local store_path = tmpbase .. '-store.lua'
local file_path = tmpbase .. '-profiles.lua'
local tabby_path = tmpbase .. '-tabby.yaml'
local snapshot_path = tmpbase .. '-snapshot.json'

local function cleanup()
  os.remove(store_path)
  os.remove(file_path)
  os.remove(tabby_path)
  os.remove(snapshot_path)
end

local function write(path, body)
  local f = assert(io.open(path, 'wb'))
  assert(f:write(body))
  assert(f:close())
end

local function cfg(opts)
  opts = opts or {}
  opts.profile_store = store_path
  local out = configmod.build(opts)
  out.automation.auto_host_key = false
  return out
end

local ok, why = xpcall(function()
  eq(type(tui.on_user_var), 'function', 'TUI bridge loads')

  local default_tui = configmod.build({}).ui.tui
  eq(default_tui.backend, 'auto', 'Rust-first auto backend default')
  eq(default_tui.command, nil, 'TUI command default')
  eq(
    tui.rust_tui_dir({ plugin_dir = '.' .. util.sep .. 'plugin' }),
    '.' .. util.sep .. 'tui-rust',
    'Rust TUI directory'
  )

  local custom_command = { 'custom manager.exe', '--native' }
  local custom_cfg = cfg {
    ui = {
      tui = {
        backend = 'rust',
        command = custom_command,
        cwd = '.',
      },
    },
  }
  local backends, backend_error = tui._backend_candidates({ plugin_dir = './plugin' }, custom_cfg)
  assert(backends, backend_error)
  eq(#backends, 1, 'explicit Rust backend count')
  eq(backends[1].name, 'rust', 'explicit Rust backend selected')
  eq(backends[1].command[1], 'custom manager.exe', 'Rust argv preserves spaces')
  custom_command[1] = 'changed-after-resolution'
  eq(backends[1].command[1], 'custom manager.exe', 'Rust argv is copied')

  local auto_backends, auto_backend_error = tui._backend_candidates(
    { plugin_dir = './missing/plugin' },
    cfg { ui = { tui = { backend = 'auto', command = { 'custom manager.exe' } } } }
  )
  assert(auto_backends, auto_backend_error)
  eq(#auto_backends, 1, 'auto custom native backend is not duplicated')
  eq(auto_backends[1].name, 'rust', 'auto custom native backend is Rust-compatible')

  local opentui_backends, opentui_backend_error = tui._backend_candidates(
    { plugin_dir = './missing/plugin' },
    cfg { ui = { tui = { backend = 'opentui', command = { 'bun', 'manager.tsx' } } } }
  )
  assert(opentui_backends, opentui_backend_error)
  eq(#opentui_backends, 1, 'explicit OpenTUI custom backend count')
  eq(opentui_backends[1].name, 'opentui', 'explicit OpenTUI custom backend retained')

  if os.getenv 'SSHMGR_TEST_RUST_BINARY' == '1' then
    local built_backends, built_backend_error = tui._backend_candidates(
      { plugin_dir = '.' .. util.sep .. 'plugin' },
      cfg { ui = { tui = { backend = 'rust' } } }
    )
    assert(built_backends, built_backend_error)
    eq(#built_backends, 1, 'local Rust build backend count')
    eq(built_backends[1].name, 'rust', 'local Rust build is discovered')
    eq(
      built_backends[1].command[1],
      '.' .. util.sep .. 'tui-rust' .. util.sep .. 'target' .. util.sep .. 'release'
        .. util.sep .. (util.is_windows and 'sshmgr-tui.exe' or 'sshmgr-tui'),
      'local Rust release binary path'
    )
  end
  eq(tui._copy_argv('bun run app'), nil, 'shell command string is refused')
  eq(tui._copy_argv({ [1] = 'bun', [3] = 'app.tsx' }), nil, 'sparse argv is refused')

  -- Inline/imported profiles have no editable `raw` record. Their sanitized
  -- SFTP view must therefore carry the normalized connection fields itself.
  local sftp_cfg = cfg()
  local inline_sftp = profiles.normalize({
    name = 'inline-sftp',
    env = { SSH_AUTH_SOCK = 'profile-agent.sock' },
    options = {
      host = 'sftp.test',
      user = 'deploy',
      auth = 'publicKey',
      private_keys = { '~/.ssh/id_ed25519' },
      password = 'literal-secret-must-not-leak',
      password_env = 'SFTP_PASSWORD',
      jump_host = 'bastion',
      ready_timeout = 45000,
      algorithms = { kex = { 'curve25519-sha256' } },
    },
  }, sftp_cfg)
  local password_sftp = profiles.normalize({
    name = 'ssh-alias',
    options = {
      host = 'ssh-alias',
      sftpHost = '192.0.2.44',
      auth = 'password',
      password_cmd = 'literal-command-must-not-leak',
    },
  }, sftp_cfg)
  local snapshot_ctx = {
    snapshot_path = snapshot_path,
    token = string.rep('a', 64),
    snapshot_seq = 0,
  }
  assert(tui.write_snapshot({
    cfg = sftp_cfg,
    profiles = function()
      return { inline_sftp, password_sftp }
    end,
  }, snapshot_ctx))
  local snapshot_text = assert(util.read_file(snapshot_path))
  assert(not snapshot_text:find('literal-secret-must-not-leak', 1, true), 'snapshot leaked password')
  assert(not snapshot_text:find('literal-command-must-not-leak', 1, true), 'snapshot leaked command')
  local snapshot = wezterm.serde.json_decode(snapshot_text)
  local snapshot_profile = assert(snapshot.profiles[1])
  eq(snapshot_profile.raw, nil, 'inline snapshot has no editable raw record')
  eq(snapshot_profile.sftp.host, 'sftp.test', 'SFTP host is normalized')
  eq(snapshot_profile.sftp.privateKeys[1], wezterm.home_dir .. '/.ssh/id_ed25519', 'SFTP key alias')
  eq(snapshot_profile.sftp.password_env, 'SFTP_PASSWORD', 'SFTP password environment hook')
  eq(snapshot_profile.sftp.identityAgent, 'profile-agent.sock', 'SFTP agent socket')
  eq(snapshot_profile.sftp.jumpHost, 'bastion', 'SFTP jump host alias')
  eq(snapshot_profile.sftp.readyTimeout, 45, 'SFTP ready timeout is seconds')
  eq(
    snapshot_profile.sftp.algorithms.kex[1],
    'curve25519-sha256',
    'SFTP reports ignored custom algorithms'
  )
  eq(snapshot_profile.sftp.ssh_options, nil, 'empty SSH options do not produce a warning')
  eq(snapshot_profile.sftp.password, nil, 'SFTP metadata has no plaintext password')
  local password_snapshot = assert(snapshot.profiles[2])
  eq(password_snapshot.sftp.host, '192.0.2.44', 'SFTP uses resolved SSH config hostname')
  eq(password_snapshot.sftp.identityAgent, nil, 'password auth does not inherit global agent')
  eq(password_snapshot.sftp.password_cmd, true, 'SFTP reports password command without its argv')
  eq(password_snapshot.sftp.ssh_options, nil, 'default SSH options stay absent')

  -- default_user is a Tabby compatibility setting, not a global SSH user.
  local c = cfg { default_user = 'deploy' }
  local inline = profiles.normalize({ name = 'inline', host = 'inline.test' }, c)
  eq(inline.options.user, nil, 'inline profile keeps OpenSSH user default')

  local converted = importer.convert_tabby({
    profiles = {
      { id = 'one', type = 'ssh', name = 'tabby', options = { host = 'tabby.test' } },
    },
  }, c)
  eq(converted[1].options.user, 'deploy', 'Tabby uses configured default user')
  c.default_user = false
  converted = importer.convert_tabby({
    profiles = {
      { id = 'two', type = 'ssh', name = 'tabby2', options = { host = 'tabby2.test' } },
    },
  }, c)
  eq(converted[1].options.user, nil, 'Tabby default user can be disabled')

  -- Explicit non-password authentication never receives the 60s password
  -- wait, even if a stale password source happens to exist.
  c = cfg()
  for _, auth in ipairs { 'agent', 'publicKey', 'keyboardInteractive' } do
    local p = profiles.normalize({
      name = auth,
      options = { host = auth .. '.test', auth = auth },
    }, c)
    eq(#automation.build_steps(p, c, { password = nil }), 0, auth .. ' skips capture')
    eq(#automation.build_steps(p, c, { password = 'stale' }), 0, auth .. ' skips autofill')
  end
  local password = profiles.normalize({
    name = 'password', options = { host = 'password.test', auth = 'password' },
  }, c)
  eq(#automation.build_steps(password, c, { password = nil }), 2, 'password capture plus rejection')

  eq(
    automation._password_was_rejected('Permission denied (publickey)'),
    false,
    'publickey denial is not a password reject'
  )
  eq(
    automation._password_was_rejected('Permission denied, please try again.\npassword:'),
    true,
    'retry prompt is a password reject'
  )
  eq(
    automation._password_was_rejected('Permission denied (password)'),
    true,
    'password method denial is a reject'
  )
  eq(
    automation._password_was_rejected('Permission denied (keyboard-interactive)'),
    true,
    'keyboard-interactive denial is a password reject for password profiles'
  )
  eq(
    automation._password_was_rejected(
      'sh: /root/private: Permission denied\nRemember to change your password:'
    ),
    false,
    'unrelated permission and password text is not a reject'
  )

  local empty_scripts = profiles.normalize({
    name = 'empty-scripts',
    options = {
      host = 'empty.test',
      auth = 'password',
      scripts = {
        { expect = '', send = '' },
        { expect = '', send = nil },
        '',
        { expect = '', prompt = 'OTP' },
        { expect = '', delay = 0.25 },
      },
    },
  }, c)
  local empty_steps = automation.build_steps(empty_scripts, c, { password = nil })
  eq(#empty_steps, 6, 'Enter, prompt and delay steps survive while a truly empty step is skipped')
  eq(empty_steps[3].send, '', 'explicit empty send remains an Enter step')
  eq(empty_steps[4].send, '', 'empty string shorthand remains an Enter step')
  eq(empty_steps[5].prompt, 'OTP', 'prompt-only step remains in order')
  eq(empty_steps[6].delay, 0.25, 'delay-only step remains in order')
  local mixed_scripts = profiles.normalize({
    name = 'mixed-scripts',
    options = {
      host = 'mixed.test',
      auth = 'password',
      scripts = {
        { expect = '', send = '' },
        { expect = 'Code:', send = '1234' },
      },
    },
  }, c)
  eq(
    #automation.build_steps(mixed_scripts, c, { password = 'x' }),
    3,
    'autofill keeps both an Enter step and a real script'
  )

  -- Drive automation.start with a deterministic clock and scheduler. These
  -- cover the persistence lifecycle rather than only the step builder.
  local function start_automation_case(text, options)
    options = options or {}
    local h = {
      now = 0,
      queue = {},
      text = text,
      sent = {},
      saved = 0,
      scheduled_after_save = false,
    }
    local fake_pane = {}
    function fake_pane:pane_id()
      return 9001
    end
    function fake_pane:get_lines_as_text()
      return h.text
    end
    function fake_pane:send_text(payload)
      table.insert(h.sent, payload)
      if options.on_send then
        options.on_send(h, payload)
      end
    end
    function fake_pane:window()
      return nil
    end

    local case_cfg = cfg()
    case_cfg.automation.poll_interval = 0.1
    case_cfg.automation.step_timeout = 0.2
    case_cfg.automation.session_timeout = 20
    local case_profile = profiles.normalize({
      name = 'automation-state',
      options = {
        host = 'state.test',
        auth = options.auth or 'password',
        scripts = options.scripts or {},
      },
    }, case_cfg)
    local context = {
      vars = {},
      resolve_pane = function()
        return fake_pane
      end,
      _now = function()
        return h.now
      end,
      _call_after = function(delay, callback)
        if h.saved > 0 then
          h.scheduled_after_save = true
        end
        table.insert(h.queue, { at = h.now + delay, callback = callback })
      end,
      _persist_password = function(_, _, password_value)
        h.saved = h.saved + 1
        h.saved_password = password_value
        return true
      end,
      on_done = function(done_ok, done_why)
        h.done = { ok = done_ok, why = done_why }
      end,
    }
    if not options.no_prompt_hook then
      context._prompt_user = function(_, callback)
        h.prompted = (h.prompted or 0) + 1
        local answer
        if not options.cancel_prompt then
          answer = options.prompt_line or 'candidate'
        end
        callback(answer)
      end
    end
    h.context = context

    function h:run_next()
      table.sort(self.queue, function(left, right)
        return left.at < right.at
      end)
      local item = table.remove(self.queue, 1)
      assert(item, 'automation test scheduler ran out of callbacks')
      self.now = item.at
      item.callback()
    end

    function h:run_until_done(limit)
      for _ = 1, limit do
        if self.done then
          return
        end
        self:run_next()
      end
      error('automation test did not finish')
    end

    automation.start(fake_pane, case_profile, case_cfg, context)
    return h
  end

  local later_failure = start_automation_case('state@test password:', {
    scripts = { { expect = 'never appears', send = 'later', timeout = 0.2 } },
  })
  later_failure:run_until_done(80)
  eq(later_failure.prompted, 1, 'real capture step prompts exactly once')
  eq(later_failure.sent[1], 'candidate\r', 'captured password is sent through the real step chain')
  eq(later_failure.done.ok, false, 'later script failure is reported')
  eq(later_failure.saved, 1, 'later script failure still saves the captured password')
  eq(later_failure.saved_password, 'candidate', 'the captured password is persisted')
  eq(later_failure.scheduled_after_save, false, 'no timer is scheduled after persistence')

  local rejected = start_automation_case('state@test password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  rejected:run_next()
  rejected.text = rejected.text
    .. '\nPermission denied, please try again.\nstate@test password:'
  rejected:run_until_done(5)
  eq(rejected.done.why, 'password rejected', 'password rejection stops automation')
  eq(rejected.saved, 0, 'a rejected password is not saved')
  eq(#rejected.sent, 1, 'scripts do not run in the retry password prompt')

  local replaced_rejection = start_automation_case('Password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  replaced_rejection:run_next()
  replaced_rejection.text = 'Permission denied, please try again.\nPassword:'
  replaced_rejection:run_until_done(5)
  eq(
    replaced_rejection.done.why,
    'password rejected',
    'replacement ending in an identical password prompt is rejected'
  )
  eq(replaced_rejection.saved, 0, 'replacement retry prompt is not persisted')
  eq(#replaced_rejection.sent, 1, 'replacement retry prompt does not run scripts')

  local keyboard_rejected = start_automation_case('state@test password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  keyboard_rejected:run_next()
  keyboard_rejected.text = keyboard_rejected.text
    .. '\nPermission denied (keyboard-interactive)'
  keyboard_rejected:run_until_done(5)
  eq(
    keyboard_rejected.done.why,
    'password rejected',
    'keyboard-interactive rejection stops password automation'
  )
  eq(keyboard_rejected.saved, 0, 'keyboard-interactive rejection is not persisted')
  eq(#keyboard_rejected.sent, 1, 'keyboard-interactive rejection does not run scripts')

  local reflow = start_automation_case(string.rep('banner ', 40) .. 'Password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  reflow:run_next()
  eq(reflow.sent[1], 'candidate\r', 'password step sends before reflow')
  reflow.text = 'short redraw'
  reflow:run_next()
  eq(reflow.saved, 0, 'screen shrink does not save early')
  eq(reflow.done, nil, 'screen shrink keeps the rejection observer active')
  eq(#reflow.sent, 1, 'screen shrink does not run the next script')
  reflow.text = 'Permission denied, please try again.\nPassword:'
  reflow:run_until_done(5)
  eq(reflow.done.why, 'password rejected', 'rejection after reflow is still detected')
  eq(reflow.saved, 0, 'rejected password after reflow is not saved')
  eq(#reflow.sent, 1, 'post-reflow rejection does not run scripts')

  local long_reflow = start_automation_case(string.rep('banner ', 40) .. 'Password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  long_reflow:run_next()
  long_reflow.text = string.rep('x', 600)
  long_reflow:run_next()
  eq(long_reflow.done, nil, 'equal-or-longer repaint keeps rejection observer active')
  eq(#long_reflow.sent, 1, 'equal-or-longer repaint does not run scripts')
  long_reflow.text = string.rep('y', 600)
    .. ' Permission denied, please try again. Password:'
  long_reflow:run_until_done(5)
  eq(long_reflow.done.why, 'password rejected', 'rejection in a longer repaint is detected')
  eq(long_reflow.saved, 0, 'rejection in a longer repaint is not persisted')

  local successful_repaint = start_automation_case(
    string.rep('login banner ', 40) .. 'Password:',
    {
      scripts = {
        { expect = 'READY>', send = 'after-repaint', delay = '0.25', timeout = 0.3 },
      },
    }
  )
  successful_repaint:run_next()
  successful_repaint.text = 'READY>' .. string.rep(' shell output', 60)
  successful_repaint:run_until_done(80)
  eq(successful_repaint.done.ok, true, 'successful long repaint completes login scripts')
  eq(
    successful_repaint.sent[2],
    'after-repaint\r',
    'post-repaint expect resets its cursor and accepts a string delay'
  )
  eq(successful_repaint.saved, 1, 'successful long repaint persists the password')

  local stale_rejection = start_automation_case(
    'Permission denied (keyboard-interactive)\nstate@test password:',
    { scripts = { { expect = '', send = '', delay = '0.25' } } }
  )
  stale_rejection:run_next()
  stale_rejection.text = 'Permission denied (keyboard-\ninteractive)\nstate@test pass\nword:'
  stale_rejection:run_next()
  eq(stale_rejection.done, nil, 'reflow does not rescan an old rejection')
  eq(stale_rejection.saved, 0, 'old rejection is not persisted before the observer ends')
  eq(#stale_rejection.sent, 1, 'old rejection does not stop or advance login scripts')
  stale_rejection.text = stale_rejection.text .. '\nstate@test $ '
  stale_rejection:run_until_done(80)
  eq(stale_rejection.done.ok, true, 'old rejection baseline can complete normally')
  eq(stale_rejection.saved, 1, 'successful attempt after old rejection is persisted')
  eq(stale_rejection.sent[2], '\r', 'empty send executes as Enter after observer timeout')

  local revealed_history = start_automation_case('state@test password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  revealed_history:run_next()
  revealed_history.text = 'Permission denied (keyboard-\ninteractive)\nstate@test pass\nword:'
  revealed_history:run_next()
  eq(revealed_history.done, nil, 'taller viewport does not treat revealed history as new output')
  eq(#revealed_history.sent, 1, 'revealed rejection history does not run scripts')
  revealed_history.text = revealed_history.text
    .. '\nPermission denied, please try again.\nPassword:'
  revealed_history:run_until_done(5)
  eq(revealed_history.done.why, 'password rejected', 'new output after revealed history is detected')
  eq(revealed_history.saved, 0, 'new rejection after revealed history is not persisted')

  local split_rejection = start_automation_case('state@test password:', {
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  split_rejection:run_next()
  split_rejection.text = string.rep('redraw ', 20)
    .. 'Permission denied, please try again.'
  split_rejection:run_next()
  eq(split_rejection.done, nil, 'partial rejection waits for the next pane poll')
  split_rejection.text = split_rejection.text .. '\nPassword:'
  split_rejection:run_until_done(5)
  eq(split_rejection.done.why, 'password rejected', 'split rejection is combined across polls')
  eq(split_rejection.saved, 0, 'split rejection is not persisted')

  local immediate_rejection = start_automation_case('state@test password:', {
    on_send = function(h)
      if #h.sent == 1 then
        h.text = h.text .. '\nPermission denied (keyboard-interactive)'
      end
    end,
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  immediate_rejection:run_until_done(5)
  eq(
    immediate_rejection.done.why,
    'password rejected',
    'observer baseline is captured before password output can arrive'
  )
  eq(immediate_rejection.saved, 0, 'immediate rejection is not persisted')
  eq(#immediate_rejection.sent, 1, 'immediate rejection does not run scripts')

  local ordinary_repaint = start_automation_case(string.rep('x', 500) .. 'ONE>', {
    auth = 'agent',
    scripts = {
      { expect = 'ONE>', send = 'first' },
      { expect = 'TWO>', send = 'second', timeout = 0.3 },
    },
  })
  ordinary_repaint:run_next()
  eq(ordinary_repaint.sent[1], 'first\r', 'first ordinary script runs')
  ordinary_repaint.text = 'TWO>' .. string.rep('y', 600)
  ordinary_repaint:run_until_done(10)
  eq(ordinary_repaint.done.ok, true, 'ordinary scripts survive a longer repaint')
  eq(ordinary_repaint.sent[2], 'second\r', 'second expect is remapped to the new screen')

  local no_gui_prompt = start_automation_case('', {
    auth = 'agent',
    no_prompt_hook = true,
    scripts = { { expect = '', prompt = 'OTP' } },
  })
  no_gui_prompt:run_until_done(5)
  eq(no_gui_prompt.done.why, 'cannot prompt', 'required prompt stops without a GUI')
  eq(#no_gui_prompt.sent, 0, 'required prompt is not skipped without a GUI')

  local cancelled = start_automation_case('state@test password:', {
    cancel_prompt = true,
    scripts = { { expect = '', send = 'must-not-run' } },
  })
  cancelled:run_until_done(5)
  eq(cancelled.done.why, 'password prompt cancelled', 'cancel stops password automation')
  eq(cancelled.saved, 0, 'cancelled password is not persisted')
  eq(#cancelled.sent, 0, 'cancel does not run scripts at the password prompt')

  -- `auth` has historically been optional when a password source itself
  -- makes the intent clear. Keep that shorthand without reintroducing waits
  -- for profiles that have neither an auth mode nor a password source.
  local implicit_password = profiles.normalize({
    name = 'implicit-password', options = { host = 'implicit.test', password = 'secret' },
  }, c)
  eq(
    #automation.build_steps(implicit_password, c, { password = 'secret' }),
    1,
    'implicit password source autofills'
  )
  local missing_env = profiles.normalize({
    name = 'missing-env', options = { host = 'missing-env.test', password_env = 'MISSING_SECRET' },
  }, c)
  eq(
    #automation.build_steps(missing_env, c, { password = nil }),
    2,
    'implicit unresolved password source captures'
  )

  -- Tabby commonly omits auth while still relying on password fallback.
  write(tabby_path, [[
profiles:
  - id: fallback
    type: ssh
    name: fallback
    options:
      host: fallback.test
]])
  c = cfg { import_tabby = tabby_path, default_user = false }
  local tabby = assert(profiles.find(profiles.load(c), 'tabby/fallback'))
  eq(#automation.build_steps(tabby, c, { password = nil }), 2, 'Tabby auth fallback captures')

  -- Exact group matching prevents the password for dev/db reaching prod/db.
  c = cfg()
  assert(panel.save_store(c, {
    { name = 'db', group = 'prod', options = { host = 'prod.test', auth = 'password' } },
    { name = 'db', group = 'dev', options = { host = 'dev.test', auth = 'password' } },
  }))
  if not wezterm.target_triple:find 'windows' then
    local stat = wezterm.target_triple:find 'darwin'
        and { 'stat', '-f', '%Lp', store_path }
      or { 'stat', '-c', '%a', store_path }
    local stat_ok, mode = wezterm.run_child_process(stat)
    assert(stat_ok, mode)
    eq(mode:match '%d+', '600', 'profile store mode is owner-only')
  end
  local dev = assert(profiles.find(profiles.load(c), 'dev/db'))
  assert(panel.persist_password(c, dev, 'dev-secret'))
  local stored = assert(panel.load_store(c))
  eq(stored[1].options.password, nil, 'same name in prod untouched')
  eq(stored[2].options.password, 'dev-secret', 'exact dev profile updated')

  -- An external profile is copied in full; the persisted entry must not lose
  -- connection behaviour while shadowing its source on the next load.
  os.remove(store_path)
  write(file_path, [[
return {
  {
    id = 'external/complex',
    name = 'complex',
    group = 'external',
    behaviorOnSessionEnd = 'reconnect',
    on_login = { 'tmux new -As main' },
    options = {
      host = 'complex.test',
      user = 'ops',
      auth = 'password',
      jumpHost = 'bastion',
      forwardedPorts = { { type = 'Local', spec = '15432:db:5432' } },
      scripts = { { expect = 'Code:', send = '1234' } },
    },
  },
}
]])
  c = cfg { profile_files = { file_path } }
  local external = assert(profiles.find(profiles.load(c), 'external/complex'))
  assert(panel.persist_password(c, external, 'external-secret'))
  stored = assert(panel.load_store(c))
  eq(stored[1].options.jumpHost, 'bastion', 'copy preserves jump host')
  eq(stored[1].options.forwardedPorts[1].spec, '15432:db:5432', 'copy preserves forwarding')
  eq(stored[1].options.scripts[1].expect, 'Code:', 'copy preserves scripts')
  eq(stored[1].on_login[1], 'tmux new -As main', 'copy preserves on_login')
  eq(stored[1].behaviorOnSessionEnd, 'reconnect', 'copy preserves session behaviour')

  -- Runtime-only values from a Lua profile file are refused instead of being
  -- stringified into a broken store entry.
  os.remove(store_path)
  write(file_path, [[
return {
  {
    id = 'external/runtime',
    name = 'runtime',
    options = { host = 'runtime.test', auth = 'password' },
    runtime_callback = function() end,
  },
}
]])
  c = cfg { profile_files = { file_path } }
  local runtime = assert(profiles.find(profiles.load(c), 'external/runtime'))
  local runtime_saved = panel.persist_password(c, runtime, 'must-not-stringify')
  eq(runtime_saved, false, 'runtime-only external profile is refused')
  eq(panel.load_store(c)[1], nil, 'serialization refusal keeps store empty')

  -- Inline entries precede the store and therefore cannot be safely shadowed.
  os.remove(store_path)
  c = cfg {
    profiles = {
      { name = 'inline-password', options = { host = 'inline.test', auth = 'password' } },
    },
  }
  inline = assert(profiles.find(profiles.load(c), 'inline-password'))
  local saved = panel.persist_password(c, inline, 'must-not-write')
  eq(saved, false, 'inline password persistence is refused')
  eq(panel.load_store(c)[1], nil, 'inline refusal creates no fragment')
end, tostring)

cleanup()
if not ok then
  io.stderr:write('ssh-manager Lua regression checks failed:\n' .. tostring(why) .. '\n')
  os.exit(1)
end
io.stderr:write(string.format('ssh-manager Lua regression checks: %d passed\n', checked))

local result = wezterm.config_builder()
result.keys = {
  { key = 'F24', mods = 'NONE', action = wezterm.action.Nop },
}
return result
