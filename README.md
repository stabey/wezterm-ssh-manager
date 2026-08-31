# wezterm-ssh-manager

给 WezTerm（Windows / macOS / Linux）的 SSH 连接管理插件，配置模型对齐 [Tabby](https://github.com/Eugeny/tabby) 的 SSH Profile，
**包含登录后自动执行命令（Tabby 的 Login scripts）**。

当前正式 Release：**v1.0.0（OpenTUI 版）**；`main` 分支已切换为计划中的 v1.1.0 Rust TUI，
当前三平台构建见 [GitHub Actions](https://github.com/stabey/wezterm-ssh-manager/actions/workflows/opentui.yml)。

这是社区维护项目，与 WezTerm 或 Tabby 官方无隶属关系。

连接本身是纯 Lua + 系统自带的 OpenSSH 客户端
（Windows 10/11 自带 `C:\Windows\System32\OpenSSH\ssh.exe`）。
管理界面默认使用 Rust + Ratatui，SSH Manager 和双栏 SFTP 在同一个常驻 tab 中；
原有 OpenTUI + Solid 与 Python Textual 界面保留为自动回退（见下面「TUI 依赖」）。

> 📋 [`docs/runbook.html`](docs/runbook.html) 是原有 Windows/Tabby 迁移的历史验收手册；
> 其中 Step 8 已补上当前 Rust TUI 与 SFTP 的 Windows 实机检查项。

```
Ctrl+Shift+S            SSH Manager TUI（分组 / 编辑）
Ctrl+Shift+E            真正的命令面板搜索框（与 Ctrl+Shift+P 相同 UI），输入主机/名称过滤 SSH
Ctrl+Shift+Alt+S        同上，但在新窗口连接
Ctrl+Shift+Alt+E        同 Ctrl+Shift+S（TUI）
Ctrl+Shift+Alt+P        快捷连接：输入 user@host:port
Enter / 空格 / 双击     TUI 里连接（新 tab）
Ctrl+Enter              TUI 里新窗口连接
Ctrl+Shift+P            命令面板 "SSH: xxx" 直连；也可搜 pick / manager
```

---

## 1. 安装

公开仓库：<https://github.com/stabey/wezterm-ssh-manager>

### 1.1 从公开仓库安装（推荐）

```lua
local sshmgr = wezterm.plugin.require 'https://github.com/stabey/wezterm-ssh-manager'
sshmgr.apply_to_config(config, { profiles = { ... } })
```

更新方式见 §1.1.1。

### 1.1.1 关于「自动更新」

**wezterm 不会自动更新插件。** `wezterm.plugin.require` 只在插件目录不存在时 clone 一次，
之后每次启动都是直接 `require` 已有的 checkout，永远不碰网络。实测：

```
改了上游 → 只调 require        → 还是旧版本
改了上游 → update_all() + require → 新版本
```

想更新必须显式调 `wezterm.plugin.update_all()`。几个要注意的地方：

* **它是同步阻塞的**（`lua.create_function`，不是 async）。写在 `wezterm.lua` 顶层的话，
  每次启动和每次 reload 都会卡在网络 I/O 上。建议挂到按键或者启动后延迟触发：

  ```lua
  config.keys = {
    {
      key = 'u', mods = 'CTRL|SHIFT|ALT',
      action = wezterm.action_callback(function(window)
        wezterm.plugin.update_all()
        wezterm.reload_configuration()
        window:toast_notification('wezterm', 'plugins updated', nil, 3000)
      end),
    },
  }
  ```

* **它不会自动 reload 配置。** 已经加载的 lua 还是旧代码，必须自己
  `wezterm.reload_configuration()`（或按 `Ctrl+Shift+R`）。

* **失败是静默的。** `update_all` 内部是
  `match p.update() { Ok => log::info, Err => log::error }`，错误只进日志，不会抛给 lua。
  网络、代理或 GitHub 不可达时，界面上看不出来。日志在
  `%USERPROFILE%\.local\share\wezterm\wezterm-gui-log-*.txt`。

* **插件目录里有一个非 git 目录，整个 `update_all()` 和 `wezterm.plugin.list()` 就全废。**
  `list_plugins()` 对 `plugins/` 下每个目录都 `Repository::open`，一个失败就整体 `?` 返回错误
  （比如一次失败的 clone 留下的临时目录）。报错长这样：
  `could not find repository at '.../plugins/xxx'; class=Repository (6)`。
  解决办法就是把那个目录删掉。

* **更新走的是 fast-forward，冲突了会尝试 merge。** 如果你手改过
  `plugins/<编码后的URL>/` 里的文件，更新可能会失败或留下 merge 状态。要重置就直接删掉那个
  目录，下次启动会重新 clone。

* 按 §1.1 从公开 URL 安装时，执行 `update_all()` 后 reload 即可。若按 §1.2 使用本地
  clone，更新是两步：先让本地 clone 跟 GitHub 同步，再让 wezterm 从本地 clone 同步。

  ```bash
  git -C C:/dev/wezterm-ssh-manager pull
  ```
  ```lua
  wezterm.plugin.update_all(); wezterm.reload_configuration()
  ```

### 1.2 本地 clone + `file://`（开发或离线使用）

先把公开仓库拉到本地，再让 WezTerm 从本地路径安装：

```bash
git clone https://github.com/stabey/wezterm-ssh-manager C:/dev/wezterm-ssh-manager
```

```lua
local sshmgr = wezterm.plugin.require 'file:///C:/dev/wezterm-ssh-manager'
sshmgr.apply_to_config(config, { profiles = { ... } })
```

注意 `file://` 后面跟的是**三条斜杠 + 盘符**：`file:///C:/dev/...`（正斜杠，不是反斜杠）。

### 1.3 直接放进配置目录（不走 git）

把 `plugin/` 下的东西拷到 wezterm 配置目录，或者放任意目录后手动加 `package.path`：

```lua
local P = 'C:/Users/me/wezterm-ssh-manager/plugin'
package.path = P .. '/?.lua;' .. P .. '/?/init.lua;' .. package.path
local sshmgr = require 'init'
```

`~/.config/wezterm/?.lua` 和 `~/.config/wezterm/?/init.lua` 本来就在 wezterm 的
`package.path` 里，所以把 `plugin/sshmgr/` 整个丢进 `~/.config/wezterm/sshmgr/`、
`plugin/init.lua` 丢成 `~/.config/wezterm/sshmgr_init.lua` 也能直接 `require`。

只复制 `plugin/` 时，纯 Lua 的连接功能和 `Ctrl+Shift+E` 连接选择器仍可用，但
`Ctrl+Shift+S` Manager 会提示缺少 TUI 后端。默认 Rust 管理器和 SFTP 需要仓库根目录的
`tui-rust/` 以及本平台编译产物；OpenTUI、Python 回退分别位于 `tui-opentui/`、`tui/`
（推荐直接安装完整仓库）。

使用公开 URL 或 `file://` 时，WezTerm 会 clone 到
`%APPDATA%\wezterm\plugins\<编码后的URL>\`（Windows）、
`~/.local/share/wezterm/plugins/`（Linux）、
`~/Library/Application Support/wezterm/plugins/`（macOS）。
仓库根目录必须有 `plugin/init.lua`——本仓库就是这个结构。

### TUI 依赖

SSH Manager 默认是常驻 tab 里的 Rust + [Ratatui](https://ratatui.rs/) 应用，支持
macOS arm64/x64 和 Windows x64。普通 SSH 连接、密码保存和登录脚本仍由 Lua 插件执行；
SFTP 会在原生 TUI 进程中另建一条 SSH 会话。

`wezterm.plugin.require` 只 clone 仓库源码，不会自动下载 GitHub Release 附件。Manager/SFTP
要可用，需要下面四种后端之一：Rust 预编译程序、本地 Rust 构建、OpenTUI/Bun，或
Python Textual 回退。

#### 使用 Rust 预编译程序

`main` 每次相关提交都会在 [Rust TUI workflow](https://github.com/stabey/wezterm-ssh-manager/actions/workflows/opentui.yml)
生成以下 Actions artifact；后续 `v*` tag 会把同样的归档和 `SHA256SUMS` 发布到 Release：

- Apple Silicon：`sshmgr-tui-<版本>-macos-arm64.tar.gz`
- Intel Mac：`sshmgr-tui-<版本>-macos-x64.tar.gz`
- Windows x64：`sshmgr-tui-<版本>-windows-x64.zip`

Actions 下载得到的外层 artifact 还要再解压一次。然后把归档目录中的
`sshmgr-tui-macos-arm64`、`sshmgr-tui-macos-x64` 或 `sshmgr-tui-windows-x64.exe`
复制到当前插件 checkout 的 `tui-rust/dist/`，保持原文件名；macOS 上还要保留可执行位。
公开 URL 安装可在 WezTerm
debug overlay 运行 `wezterm.plugin.list()` 找到这个 checkout 的 `plugin_dir`；本地 clone
则直接使用仓库目录。重载配置后，Lua 会优先找到这个预编译程序。

现有 [v1.0.0 Release](https://github.com/stabey/wezterm-ssh-manager/releases/tag/v1.0.0)
是上一代 OpenTUI 构建，不是 Rust 构建；它仍可放进 `tui-opentui/dist/` 作为回退。

这些社区构建尚未做 Apple/Windows 代码签名。系统若阻止首次运行，请先核对校验和，再通过
系统安全界面允许该程序；不希望运行未签名二进制时，请按下节从源码构建。

从源码构建 Rust TUI 需要 Rust 1.88 或更新版本：

```bash
cd tui-rust
cargo test --locked
cargo build --locked --release
```

本机构建会生成 `tui-rust/target/release/sshmgr-tui[.exe]`，Lua 可以直接发现，无需改名。
Lua 的 `auto` 顺序是 Rust 预编译程序/本机构建、OpenTUI 编译产物或 Bun 源码、Python Textual。
也可以显式指定后端或一个不经过 shell 的 argv：

```lua
sshmgr.apply_to_config(config, {
  ui = { tui = {
    backend = 'auto', -- 'auto' | 'rust' | 'opentui' | 'textual'
    -- command = { 'C:/tools/sshmgr-tui-windows-x64.exe' },
    -- cwd = 'C:/tools',
    -- bun = 'C:/Users/me/.bun/bin/bun.exe',
  } },
})
```

仓库内的 `.github/workflows/opentui.yml`（保留旧文件名）会在 macOS arm64、macOS Intel
和 Windows x64 分别执行格式检查、Clippy、测试、release 构建和 helper 协议冒烟测试，
上传包含项目声明、锁文件和逐 crate 许可证的 `tar.gz` / `zip`。推送 `v*` tag 时还会创建
带 `SHA256SUMS` 的 GitHub Release。Windows 的最终键鼠、resize 和中文宽度仍需 WezTerm 实机确认。

Rust 不可用时，`auto` 会继续尝试原有 OpenTUI。需要从源码运行 OpenTUI 时：

```bash
cd tui-opentui
bun install
bun run typecheck
bun test
bun run build
```

OpenTUI 和 Bun 也不可用时，`auto` 最后尝试旧的 Python Textual 界面：

```bash
python -m pip install textual==8.2.8
```

TUI 与 Lua 之间的命令使用每个 Manager pane 独立的短期会话：OSC UserVar 只携带
一次性请求引用并会立即清空，密码等命令内容保存在临时目录中，由 Lua 读取后立即删除。
macOS/Linux 会主动收紧目录权限；Windows 依赖当前 `%TEMP%` 的继承 ACL。

指定 Textual 的解释器：

```lua
sshmgr.apply_to_config(config, {
  ui = { tui = { python = [[C:/Users/me/AppData/Local/Programs/Python/Python314/python.exe]] } },
})
```

SSH Manager 按键：

```
Tab / 方向键     左右栏
/                过滤
Enter            新 tab 连接（Manager 不关）
Ctrl+Enter       新窗口连接
e                编辑（只读导入项会先复制进 store）
n                新建
d                删除
p                快捷连接 user@host
r                重新读取配置
1                SSH Manager
2 / s            为当前主机打开 SFTP
q / Esc          回到上一个 tab（TUI 进程留着）
```

SFTP 是真实的双栏文件管理器，左侧本地、右侧远端：

```
Tab              切换本地 / 远端栏
↑↓ / j k         选择；Enter 进入目录；Backspace 上级
u / d            上传当前本地文件 / 下载当前远端文件
m / r / x        新建目录 / 改名 / 递归删除
F5 / c           刷新 / 取消当前连接或传输
Esc              返回 SSH Manager
```

SFTP 当前支持密码、`password_env`、多把未加密私钥、OpenSSH Agent 和单跳 `jumpHost`。为了不把已保存
密码复制进 snapshot，密码 profile 每次新建 SFTP 会话时会在界面里再询问一次；
`password_cmd`、keyboard-interactive、ProxyCommand/SOCKS/HTTP proxy 和多跳链会明确提示
不支持。上传/下载首版只接受普通文件；目录和符号链接需先打包，目录本身可浏览、创建、
改名和递归删除。

首版 SFTP 尚未接入 `known_hosts`/host key 校验，也会忽略自定义 algorithms 和任意
`ssh_options`（界面会给 warning）；加密私钥没有 passphrase 输入框。若手动强制使用
OpenTUI 0.5.9 回退，它的 `input` 没有密码遮罩，密码会在当前终端输入框里显示。
这个边界适合个人环境，不应当作面向多用户的安全 SFTP 客户端。

## 2. 最小配置

```lua
sshmgr.apply_to_config(config, {
  profiles = {
    { name = 'nas', host = '192.0.2.9', user = 'admin' },

    {
      name = 'web-1',
      group = 'prod',
      color = '#f38ba8',
      icon = wezterm.nerdfonts.md_server,
      options = {
        host = '198.51.100.11',
        user = 'ops',
        auth = 'publicKey',
        privateKeys = { '~/.ssh/id_ed25519' },
        jumpHost = 'bastion',            -- 可以直接写另一个 profile 的名字
        forwardedPorts = {
          { type = 'Local', host = '127.0.0.1', port = 15432,
            targetAddress = 'db.example.com', targetPort = 5432 },
        },
        -- 登录后自动执行
        scripts = {
          'sudo -i',
          { expect = '%[root@', isRegex = true, send = 'tmux new -As main', optional = true },
        },
      },
    },
  },
})
```

profile 支持两种写法：**扁平写法**（`host` / `user` 直接写在顶层，适合简单条目）
和 **Tabby 写法**（放在 `options = {}` 里）。两者可以混用。
键名同时接受 Tabby 的 camelCase（`privateKeys`）和 snake_case（`private_keys`）。

---

## 3. Tabby 选项对照表

`options` 里的每一项都对应 Tabby SSH Profile 设置面板里的一个开关。

| Tabby 字段 | 本插件 | 生成的 ssh 参数 |
|---|---|---|
| Host | `host` | 位置参数 |
| Port | `port` | `-p` |
| Username | `user` | `-l` |
| Authentication method | `auth` = `'password'` \| `'publicKey'` \| `'agent'` \| `'keyboardInteractive'` | `-o PreferredAuthentications=…` / `-o PubkeyAuthentication=no` |
| Password | `password` / `password_env` / `password_cmd` | 见 §5，通过 expect 自动填 |
| Private keys | `privateKeys = {…}` | `-i`（+ `-o IdentitiesOnly=yes`） |
| Keep-alive interval | `keepaliveInterval`（毫秒或秒都行） | `-o ServerAliveInterval` |
| Keep-alive count max | `keepaliveCountMax` | `-o ServerAliveCountMax` |
| Ready timeout | `readyTimeout` | `-o ConnectTimeout` |
| X11 forwarding | `x11 = true`（`x11_trusted = true` 走 `-Y`） | `-X` / `-Y` |
| Skip banner | `skipBanner = true` | `-o LogLevel=QUIET` |
| Jump host | `jumpHost = 'profile名'` 或 `'user@host:port'`，逗号可串多跳 | `-J` |
| Agent forwarding | `agentForward = true` | `-A` |
| Warn on close | `warnOnClose = false` | 把 `ssh` 加进 `skip_close_confirmation_for_processes_named` |
| Algorithms | `algorithms = { kex/cipher/hmac/serverHostKey/compression = {…} }` | `-o KexAlgorithms/Ciphers/MACs/HostKeyAlgorithms/Compression` |
| Proxy command | `proxyCommand = '…'` | `-o ProxyCommand` |
| Forwarded ports | `forwardedPorts = {…}` | `-L` / `-R` / `-D` |
| SOCKS proxy | `socksProxyHost` / `socksProxyPort` | `-o ProxyCommand=ncat --proxy … --proxy-type socks5 %h %p` |
| HTTP proxy | `httpProxyHost` / `httpProxyPort` | 同上，`--proxy-type http` |
| Reuse session | `reuseSession = true` | `-o ControlMaster=auto …`（**Windows 不支持，见 §8**） |
| Login scripts | `scripts = {…}` | 见 §4 |
| Behavior on session end | `behaviorOnSessionEnd = 'close'\|'keep'\|'reconnect'`（profile 顶层） | 包一层本地 shell，见 §6 |
| Name / Group / Icon / Color / Weight | 同名字段，写在 profile 顶层 | 影响 TUI 列表与 tab 显示 |

还有几个 Tabby 没有、但 OpenSSH 有的口子：

```lua
{
  name = 'x',
  options = { host = 'h' },

  ssh_options = { StrictHostKeyChecking = 'accept-new' },  -- 任意 -o 直通，优先级最高
  extra_args  = { '-vvv' },                                -- 任意额外参数
  env         = { LANG = 'en_US.UTF-8' },                  -- 本地 ssh 进程的环境变量
  remote_command = 'sudo -i',                              -- 用 -t 跑远端命令而不是登录 shell
  cwd = '/var/log',                                        -- 登录后 cd 过去（等价于 remote_command）
  host_key_policy = 'accept-new',                          -- 覆盖全局
  domain = 'my-mux-domain',                                -- 改成在某个 wezterm mux domain 里 spawn
  ssh_binary = 'C:/Program Files/Git/usr/bin/ssh.exe',      -- 换一个 ssh 客户端
}
```

---

## 4. 登录后自动命令（Tabby Login scripts）

这是重点。语义跟 Tabby 一致：一串 `{ expect, send }` **顺序**执行。

```lua
scripts = {
  -- expect 为空 = 无条件立刻发送
  'sudo -i',
  { expect = '', send = 'export TERM=xterm-256color' },

  -- 等到屏幕上出现这个子串，再发送
  { expect = 'assword', send = '${password}', optional = true, hide = true },

  -- isRegex = true 时 expect 按 Lua pattern 解释（不是 JS 正则，见下）
  { expect = '%[root@[^%]]+%]#', isRegex = true, send = 'tmux new -As main' },

  -- 找不到就跳过；不写 optional 则超时后中止后续脚本
  { expect = 'Verification code', prompt = '输入 2FA 验证码', optional = true },
}
```

字段：

| 字段 | 说明 |
|---|---|
| `expect` | 要等的文本。空串 = 立刻执行 |
| `send` | 要发送的内容，默认自动补一个回车 |
| `isRegex` | `true` 时 `expect` 按 **Lua pattern** 匹配 |
| `flavor` | `'js'` 时先把 JS 正则转成 Lua pattern（Tabby 导入时自动设置） |
| `optional` | 超时后跳过而不是中止 |
| `timeout` | 这一步的超时秒数，默认 `automation.step_timeout`（25s） |
| `raw` | `true` 时不补回车 |
| `hide` | `true` 时日志里不打印发送内容（密码用） |
| `delay` | 匹配到之后再等几秒才发送 |
| `prompt` | 有这个字段时不发 `send`，而是弹 `PromptInputLine` 问用户，把输入发过去（2FA / OTP 用） |

`send` 里支持变量替换和转义：

```
${password} ${user} ${host} ${port} ${name} ${env:VAR}
\n \r \t \e
```

### 4.1 更省事的写法：`on_login`

不想写 expect 的话，用 `on_login` —— 它会先等到 shell 提示符（`automation.ready_pattern`），
再把命令一条条发过去：

```lua
{
  name = 'web-1',
  options = { host = '198.51.100.11', user = 'ops' },
  on_login = {
    'cd /srv/app',
    'source .env',
    'docker compose logs -f --tail=100',
  },
}
```

### 4.2 Lua pattern ≠ JS 正则（重要）

WezTerm 的 Lua 里没有正则引擎，只有 Lua pattern。区别：

| 你想要 | JS 正则 | Lua pattern |
|---|---|---|
| 转义 | `\.` `\$` | `%.` `%$` |
| 数字 / 单词 / 空白 | `\d` `\w` `\s` | `%d` `%w` `%s` |
| 惰性匹配 | `.*?` | `.-` |
| 或 | `(a\|b)` | **不支持** |
| 重复 n 次 | `a{2,3}` | **不支持** |
| 分组 | `(?:…)` | **不支持** |

从 Tabby 导入时，插件会自动把能转的 JS 正则转成 Lua pattern；
转不了的（`|`、`{n,m}`、`\b`、分组）会打一条 warning 并**降级为普通子串匹配**。
自己写的时候直接写 Lua pattern 就好；想写 JS 语法就加 `flavor = 'js'`。

### 4.3 实现方式与 Tabby 的差别

Tabby 直接在 SSH 流上逐字节匹配；WezTerm 给的是渲染后的屏幕，所以本插件是
**轮询 pane 的文本**（默认每 150ms 取最后 120 行）。对登录序列来说行为等价，
而且远端重绘一行也不会漏匹配。可调：

```lua
automation = {
  poll_interval  = 0.15,   -- 采样间隔（秒）
  scan_lines     = 120,    -- 每次看多少行
  step_timeout   = 25,     -- 单步默认超时
  session_timeout= 180,    -- 整个脚本的上限
  ready_pattern  = '[%$#>%%][ ]?$',
  ready_timeout  = 30,
  auto_password  = true,   -- 密码认证或已配置密码源时，自动应答 "password:"
  auto_host_key  = true,   -- 自动应答 "Are you sure you want to continue connecting"
}
```

---

## 5. 密码怎么放

Windows 的 OpenSSH 没法从命令行传密码，所以密码是通过 §4 的 expect/send 机制填进去的
（跟你手打是一样的）。按优先级取值：

```lua
-- 1. 全局回调，最灵活
password_provider = function(profile)
  return my_vault[profile.id]
end,

-- 2. 外部命令（推荐：1Password / pass / gopass / Windows 凭据管理器脚本）
options = { password_cmd = { 'op', 'read', 'op://example-vault/example-server/password' } },
options = { password_cmd = 'pwsh -c "(Get-Secret web1 -AsPlainText)"' },  -- 字符串走 shell

-- 3. 环境变量
options = { password_env = 'PROD_WEB1_PW' },

-- 4. 明文（别提交到 git）
options = { password = 'hunter2' },
```

从 Tabby 迁过来的话不用手工配：转换器会自动挂上读 Windows 凭据管理器的
`password_cmd`，见 §7.4。

日志里密码永远打印为 `<hidden>`。

> 更推荐的做法仍然是公钥 + ssh-agent：`auth = 'agent'`，
> Windows 上把 `ssh-agent` 服务设成自动启动即可。

---

## 6. 会话结束的行为

对应 Tabby 的 *Behavior on session end*，写在 profile 顶层：

```lua
behaviorOnSessionEnd = 'close'      -- 默认，ssh 退出 → pane 关闭
behaviorOnSessionEnd = 'keep'       -- 保留 pane，等你回车再关
behaviorOnSessionEnd = 'reconnect'  -- 断线 3 秒后自动重连（Ctrl-C 停止）
```

`keep` / `reconnect` 的实现是把 ssh 包在一层本地 shell 里
（Windows 用 `powershell.exe -NoProfile -Command`，其它平台用 `sh -c`），
参数都做了引号转义。登录脚本照常工作。

---

## 7. 从 Tabby 迁移

两条路，按你要不要继续留着 Tabby 选。

### 7.1 直接读 Tabby 的 config.yaml（不用转换）

```lua
sshmgr.apply_to_config(config, {
  import_tabby = true,
  -- 或者写死路径：
  -- import_tabby = 'C:/Users/me/AppData/Roaming/tabby/config.yaml',
})
```

每次 reload 都重新读一遍，Tabby 里改了这边也跟着变，文件还会自动加进 wezterm 的
reload 监听。适合过渡期两边都在用。

### 7.2 一次性转换成静态配置文件（推荐）

生成一个你自己拥有、能编辑、能进 git 的 `ssh_profiles.lua`：

**方式 A — 命令面板**：`Ctrl+Shift+P` → `SSH: convert Tabby config.yaml to a profile file…`
默认写到 `<wezterm 配置目录>/ssh_profiles.lua`。

**方式 B — debug overlay**：`Ctrl+Shift+L`，然后

```lua
> wezterm.plugin.require('file:///C:/dev/wezterm-ssh-manager').export_tabby { to = 'C:/Users/me/.config/wezterm/ssh_profiles.lua' }
```

**方式 C — 命令行**（不用开 GUI，拿 wezterm 当 Lua 解释器跑）：

```bash
wezterm --config-file ./tools/convert-tabby.lua show-keys > /dev/null
```

Windows PowerShell 里把 `> /dev/null` 换成 `> $null`。转换结果打在 stderr 上。

参数：

```lua
sshmgr.export_tabby {
  from   = true,        -- true = Tabby 默认路径，或写具体的 config.yaml 路径
  to     = nil,         -- 默认 <wezterm 配置目录>/ssh_profiles.lua
  format = 'lua',       -- 'lua' | 'yaml' | 'json'
  force  = false,       -- 目标文件已存在时必须显式设 true 才覆盖
  password_stubs = true,
}
-- 返回 ok, message, details
```

生成完把它挂上去就行：

```lua
sshmgr.apply_to_config(config, {
  profile_files = { '~/.config/wezterm/ssh_profiles.lua' },
})
```

### 7.3 转换过程做了什么

| Tabby | 转换后 |
|---|---|
| `groups[].name` + `parentGroupId` | 展开成 `group = '父组/子组'` 路径 |
| `profileDefaults.ssh` / `groups[].defaults.ssh` | 按 Tabby 的优先级**摊平**进每个 profile（profile > 组默认 > 全局默认） |
| `group` / `jumpHost` 里的 profile id | 还原成人类可读的名字 |
| `keepaliveInterval` / `readyTimeout`（毫秒） | 换算成秒 |
| Font Awesome 图标名 | 映射到 nerdfonts；映射不到的丢掉并在文件头里列出来 |
| `isRegex` 的 JS 正则 | 转成 Lua pattern；转不了的降级成子串匹配并在文件头里列出来让你人工看 |
| `port: 22` / `x11: false` 这类默认值 | 删掉，保持文件干净 |
| `type` 不是 `ssh` 的 profile | 跳过，文件头里列出来 |

> **注意 Tabby 的组默认值不会向上继承。** `getProviderProfileGroupDefaults()` 只看
> profile 自己那个组，不看父组——转换器跟 Tabby 保持一致，所以转换前后行为不变。
> 如果你原本以为父组的设置生效了，转换后看到的才是 Tabby 真实的行为。

### 7.4 密码：直接读 Windows 凭据管理器

**Tabby 从来不把 SSH 密码写进 config.yaml。** keytar 把它写进 **Windows 凭据管理器**，
条目类型是 `CRED_TYPE_GENERIC`，target 名字是 `<service>/<account>`：

```
ssh@<主机>:<端口>/<用户名>
例：ssh@203.0.113.23:22/deploy
```

Tabby 的 yaml 在 port 为 22 时经常不写这个字段，但存进凭据管理器时已经填了默认端口，
所以 target **始终带端口**（包括 `:22`）。按 `ssh@host/user` 去读会找不到。

所以密码是**拿得到的**，不用手工重输。转换器在 Windows 上会给每个能对上凭据的
profile 挂上钩子（不限于 yaml 里写了 `auth: password` 的——Tabby 经常不写 auth，
先试密钥再试密码，但密码已经存在凭据管理器里）：

```lua
password_cmd = {
  'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
  '-File', 'C:/Users/me/.config/wezterm/credman.ps1',
  '-Target', 'ssh@203.0.113.23:22/deploy',
},
```

`credman.ps1` 会被自动拷到生成文件旁边。这样**密码始终留在凭据管理器里**，不进配置文件、
不进 git，日志里也只显示 `<hidden>`。

先自己验证一下再连：

```powershell
# 看看凭据管理器里到底有哪些（只列 target 和用户名，不显示密码）
powershell -NoProfile -ExecutionPolicy Bypass -File .\credman.ps1 -List

# 读一条
powershell -NoProfile -ExecutionPolicy Bypass -File .\credman.ps1 -Target 'ssh@203.0.113.23:22/deploy'

# 或者按字段拼
powershell -NoProfile -ExecutionPolicy Bypass -File .\credman.ps1 -Host_ 203.0.113.23 -Port 22 -User deploy
```

几个实现上的坑，脚本里都处理了：

* **keytar 写的是密码的 UTF-8 原始字节**（`CredentialBlob = password.data()`），不是
  Windows 惯例的 UTF-16LE。按 Unicode 解会得到乱码。脚本默认按 UTF-8 解，同时靠交替
  NUL 字节识别出真·UTF-16 的条目，所以手工用 `cmdkey` 加的条目也能读。
* **target 里的端口取的是 Tabby 的原始 `options.port`，没写就按 22。**
  转换器在删掉冗余的 `port = 22` **之前**就算好 target，避免钩子对不上凭据。
* 密码走 PowerShell 成功管道（`Write-Output`），因为 `wezterm.run_child_process`
  捕获的是管道而不是 `[Console]::Out`。调用方会去掉结尾换行。

`password_mode` 可选 `'credman'`（Windows 默认）、`'env'`（生成 `password_env` 钩子）、
`'none'`（什么都不加）。

> **如果你在 Tabby 里开了 Vault**，密码就不在凭据管理器里了，而是在 `config.yaml` 的
> `vault.contents`（AES 加密，密钥是你的主密码），这个读不了。转换器会检测到并在文件头
> 提示你：先在 Tabby 里关掉 Vault（它会把 secret 写回凭据管理器），再重新转换。

### 7.5 其它来源

```lua
sshmgr.apply_to_config(config, {
  -- ~/.ssh/config 里的 Host 段
  import_ssh_config = true,
  ssh_config_group = 'ssh_config',

  -- 额外的 profile 文件：.lua / .json / .yaml / .toml 都行，会被加入 reload 监听
  profile_files = {
    '~/.config/wezterm/ssh_profiles.lua',
    '~/work/team-hosts.yaml',
  },
})
```

### 7.6 算法列表（重要）

Tabby 的算法设置面板**默认全选**,而那个菜单来自 ssh2 这个 JS 库,里面很多名字
OpenSSH 根本不认识。OpenSSH 对 `-o Ciphers=` / `MACs` / `KexAlgorithms` /
`HostKeyAlgorithms` 是严格校验的——**列表里只要有一个不认识的名字就直接 exit,连都不连**,
表现就是 pane 一闪就关。

```console
$ ssh -G -o Ciphers=aes128-ctr,arcfour,blowfish-cbc host   # exit 255
$ ssh -G -o Ciphers= host                                  # exit 255 (空的也不行)
```

典型的问题名字:`arcfour*`、`blowfish-cbc`、`cast128-cbc`、不带 `@openssh.com` 后缀的
`aes128-gcm`/`aes256-gcm`、`hmac-ripemd160`、`hmac-sha2-*-96`、
`diffie-hellman-group15/17-sha512`,以及 `ext-info-c`/`ext-info-s`/`kex-strict-*`
——最后这几个是协议内部的伪算法,压根不是合法的 `KexAlgorithms` 取值。

**插件会自动处理**,不需要你手动改文件:

* 用 `ssh -Q cipher|mac|kex|HostKeyAlgorithms` 问**你本机这个** ssh 客户端支持什么,
  结果按进程缓存(config reload 不会重复 spawn)。
* 不认识的名字剔掉,认识的保留——包括 `3des-cbc`、`ssh-rsa`、`hmac-md5`、
  `diffie-hellman-group1-sha1` 这些默认关闭但能显式开启的遗留算法,所以连老设备的能力不丢。
* 一个都不剩时**整个选项省掉**(而不是发一个空的),让 ssh 用自己的默认值。
* OpenSSH 的 `+`/`-`/`^` 前缀语法(`Ciphers=+3des-cbc`)会保留;带 `*`/`?` 的通配符不过滤。
* 每次剔除都会写日志:`[ssh-manager] <profile>: ssh does not support <名字>; dropped from <选项>`。

`argv.lua`(每次连接)和 `export.lua`(转换时)两边都做,所以重新跑一次 Tabby 转换不会
把问题带回来。转换生成的文件头部会列出被剔除的名字。

不想要这个行为就 `filter_algorithms = false`。

> 因为是拿本机 `ssh -Q` 实测,不是硬编码黑名单,所以你换 OpenSSH 版本、或者
> `ssh_binary` 指向 Git for Windows 自带的 ssh,结果会跟着变。比如 `ssh-dss` 在
> OpenSSH 10 已经彻底移除,在 9.x 上还在。

## 8. 已知限制

* **`reuseSession`（ControlMaster）在 Windows 不可用** —— 微软的 OpenSSH 移植版没实现
  连接复用。插件会打 warning 并忽略。想要复用连接就用 WezTerm 自己的 mux domain
  （`config.ssh_domains` + `profile.domain`）。
* **`launch_menu` 里的条目不跑登录脚本** —— WezTerm 没有 "pane 创建" 事件，
  从启动器里开的 pane 拿不到句柄。走 SSH Manager / 命令面板 / `sshmgr.connect()` 就都正常。
* **`format-tab-title` 和 `augment-command-palette` 只会调用第一个注册的 handler**
  （wezterm 的 `emit_sync_callback` 语义）。所以：
  * tab 上色默认**关闭**。要么 `ui.color_tabs = true` 让插件注册，
    要么在你自己的 handler 里调 `sshmgr.decorate_tab(tab)`，返回 nil 时走你原来的逻辑。
  * 命令面板默认开启；如果你自己也有 `augment-command-palette`，
    把 `ui.command_palette` 设成 `false`，然后在你的 handler 里拼上 `sshmgr.palette_entries()`。
* 每个 pane 不能有独立配色方案（WezTerm 的 `colors` 是窗口级的）。
  profile 的 `color` 只影响 TUI 列表和 tab 标题。

---

## 9. 全部选项

```lua
sshmgr.apply_to_config(config, {
  profiles = {},                  -- 见 §2
  groups = {},                    -- { ['prod'] = { options = {...}, color = '#f00' } } 组默认值
  defaults = {},                  -- 应用到所有 profile 的默认值
  profile_files = {},
  import_ssh_config = false,
  ssh_config_group = 'ssh_config',
  import_tabby = false,

  ssh_binary = nil,               -- 默认 Windows 'ssh.exe' / 其它 'ssh'
  default_where = 'tab',          -- 'tab' | 'window' | 'split_right' | 'split_down'
  default_ssh_options = { ServerAliveInterval = '30', ServerAliveCountMax = '3' },
  host_key_policy = 'ask',        -- 'ask' | 'accept-new' | 'yes'
  filter_algorithms = true,       -- 见 §7.6
  proxy_command_template = nil,   -- 默认 'ncat --proxy %{proxy_host}:%{proxy_port} --proxy-type %{proxy_type} %h %p'

  automation = { ... },           -- 见 §4.3
  password_provider = nil,        -- 见 §5
  password_cmd_shell = nil,

  ui = {
    fuzzy = true,                   -- 旧 InputSelector 遗留项，TUI 不用
    title = 'SSH  ·  connections',
    default_icon = wezterm.nerdfonts.md_server_network,
    dim_color = '#6c7086',
    set_tab_title = true,
    tab_title_format = '{icon} {name}',
    color_tabs = false,
    command_palette = true,
    launch_menu = false,
    notify = true,
    tui = {
      backend = 'auto',            -- Rust 优先，失败回退 OpenTUI / Textual
      command = nil,               -- 可选自定义 TUI argv；不会经过 shell
      cwd = nil,                   -- 自定义 command 的工作目录
      bun = nil,                   -- Bun 路径；默认 'bun'
      python = nil,                -- Textual 回退的 Python 路径
      tab_title = 'SSH Manager',
    },
  },

  keys = {
    picker            = { key = 's', mods = 'CTRL|SHIFT' },     -- TUI
    selector          = { key = 'e', mods = 'CTRL|SHIFT' },     -- 下拉模糊搜索
    picker_new_window = { key = 'S', mods = 'CTRL|SHIFT|ALT' }, -- 下拉，新窗口
    panel             = { key = 'e', mods = 'CTRL|SHIFT|ALT' }, -- TUI
    quick_connect     = { key = 'p', mods = 'CTRL|SHIFT|ALT' },
    reconnect_tab     = false,
  },

  on_spawn = function(profile, pane, window) end,   -- pane 刚建好
  on_ready = function(profile, pane, window) end,   -- 登录脚本跑完
})
```

## 10. 可以自己调用的 API

```lua
sshmgr.profiles()                       -- 规范化后的 profile 列表
sshmgr.get 'prod/web-1'                 -- 按 id 或 name 查
sshmgr.connect(window, pane, 'web-1', 'split_right')
sshmgr.connect_action('web-1', 'tab')   -- 可以直接塞进 config.keys
sshmgr.picker()                         -- 下拉模糊搜索
sshmgr.panel()                          -- SSH Manager TUI
sshmgr.quick_connect()                  -- 输入 user@host
sshmgr.split_action 'Right'             -- SSH tab 分屏再连同一台
sshmgr.reconnect_action()               -- 重连当前 tab 的 profile
sshmgr.palette_entries()                -- 给你自己的 augment-command-palette 用
sshmgr.decorate_tab(tab)                -- 给你自己的 format-tab-title 用
sshmgr.tab_info(tab_id)                 -- 这个 tab 是哪个 profile
sshmgr.command_for 'web-1'              -- 调试：看看到底生成了什么命令行
sshmgr.reload()                         -- 重新读 profile 文件
sshmgr.export_tabby { to = '...' }      -- 把 Tabby 的 config.yaml 转成 profile 文件（§7.2）
require('sshmgr.caps').supported('ssh.exe', 'cipher')  -- 调试：本机 ssh 支持哪些算法
```

调试的时候按 `Ctrl+Shift+L` 打开 debug overlay，然后：

```lua
> wezterm.plugin.require('file:///C:/dev/wezterm-ssh-manager').command_for 'prod/web-1'
```

日志在 `%USERPROFILE%\.local\share\wezterm\wezterm-gui-log-*.txt`（Windows）——
注意不是 `%APPDATA%`，那里放的是插件 checkout。

## 11. License

本项目采用 [MIT License](LICENSE)。
