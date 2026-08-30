# OpenTUI SSH Manager

这是 `wezterm-ssh-manager` 的 OpenTUI + Solid 前端。它保留 Python TUI 的
snapshot/request/OSC 协议语义，并提供 SSH Manager 与真实 SSH2/SFTP 双栏界面。

## 开发

需要 Bun 1.3 或更新版本：

```sh
bun install
bun run typecheck
bun test
```

正常运行时必须由 WezTerm 插件提供私有 runtime 目录、`snapshot.json` 和 `WEZTERM_SSHMGR_SESSION_TOKEN`：

```sh
bun run src/index.tsx --snapshot /path/to/wezterm-sshmgr-xxx/snapshot.json
```

CLI 还提供 Lua 启动器可调用的无界面 helper：

```text
--create-runtime
--cleanup-runtime <runtime-dir>
--replace-file <source> <destination>
```

## 键位

- `1` 主机管理，`2`/`s` SFTP
- `Tab` 切换区域，`↑↓`/`jk` 选择
- `Enter` 连接，`Ctrl+Enter` 新窗口连接
- `n` 新建，`e` 编辑，`d` 删除，`/` 过滤，`p` 快捷连接
- `r` 刷新，`q`/`Esc` 返回
- 编辑表单用 `Tab` 切字段、`Ctrl+S` 保存

SFTP 页面左栏使用当前系统的本地路径规则，右栏固定使用 POSIX 路径；支持目录浏览、
创建、改名、递归删除，以及普通文件上传/下载、覆盖确认、字节进度和取消；目录和符号链接
需先打包。认证 adapter 支持密码、`password_env`、多把未加密私钥、Agent 和单跳 jump host。
snapshot 不携带保存的明文密码，
所以密码 profile 会在每次新 SFTP 会话时重新询问。

当前未加载 `known_hosts`，自定义 algorithms/任意 OpenSSH options 只会 warning 后忽略；
加密私钥没有 passphrase 输入框，OpenTUI 0.5.9 的密码输入也不会遮罩。它目前按个人工具边界实现。

## 构建

构建当前 macOS 或 Windows 平台：

```sh
bun run build
```

构建全部声明的平台（macOS arm64/x64、Windows x64）：

```sh
bun install --os='*' --cpu='*'
bun run build:all
```

OpenTUI 使用平台原生包；交叉编译机器需要先确保目标平台对应的 `@opentui/core-*` 可选依赖已经安装。CI 会分别在 macOS 与 Windows 运行本平台构建和 smoke test，并上传 macOS `tar.gz` / Windows `zip`；Windows 的真实 WezTerm/ConPTY 交互仍需实机验收。
版本 tag 会把三个目标的合规归档和 `SHA256SUMS` 发布到 GitHub Release；根 README 说明了如何把预编译程序放进插件 checkout。

`ssh2` 的 `cpu-features` 只是可选加速器，单文件构建会把它保持为 external；找不到时
`ssh2` 自动使用纯 JavaScript 路径，不影响 SFTP 功能。
