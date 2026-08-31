# Rust TUI

`sshmgr-tui` 是 `wezterm-ssh-manager` 默认的原生 SSH Manager / 双栏 SFTP
前端，使用 Rust、Ratatui 与 Crossterm。它继续使用 Lua 插件已有的
snapshot/request/OSC 协议，所以必须由插件启动，不能脱离 WezTerm 单独配置主机。

## 本地构建

需要 Rust 1.88 或更新版本：

```sh
cargo test --locked
cargo build --locked --release
```

Lua 会自动查找 `target/release/sshmgr-tui[.exe]`。也可以从当前 main 分支的
[GitHub Actions](https://github.com/stabey/wezterm-ssh-manager/actions/workflows/opentui.yml)
或后续 GitHub Release 下载对应平台归档，把其中的可执行文件解压到本目录的 `dist/`：

```text
dist/sshmgr-tui-macos-arm64
dist/sshmgr-tui-macos-x64
dist/sshmgr-tui-windows-x64.exe
```

## Lua 启动器协议

正常启动形式为：

```text
sshmgr-tui --snapshot <runtime-dir>/snapshot.json
```

二进制还必须保留以下无界面 helper；Lua 启动器与 CI 都依赖它们：

```text
--create-runtime
--cleanup-runtime <runtime-dir>
--replace-file <source> <destination>
```

运行正常界面时，Lua 会设置一次性的 `WEZTERM_SSHMGR_SESSION_TOKEN`。不要在
命令行中手工复用旧 snapshot 或 token。

## 发布产物

`.github/workflows/opentui.yml`（为保留原工作流路径而沿用文件名）在三个本机
runner 上构建并测试：

- `aarch64-apple-darwin` → `sshmgr-tui-macos-arm64.tar.gz`
- `x86_64-apple-darwin` → `sshmgr-tui-macos-x64.tar.gz`
- `x86_64-pc-windows-msvc` → `sshmgr-tui-windows-x64.zip`

每个归档都包含本平台二进制、项目许可证、第三方声明、重建说明、`Cargo.lock`、
构建元数据和由依赖锁文件生成的逐 crate 上游许可证。`main` 推送会把归档保留为
Actions artifact；`v*` tag 会把三个归档和覆盖它们的 `SHA256SUMS` 上传到 GitHub Release。
