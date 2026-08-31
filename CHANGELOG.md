# Changelog

All notable changes to this project are documented here.

## 1.1.0 - Unreleased

- Add a native Rust and Ratatui SSH Manager while retaining the OpenTUI and
  Python Textual implementations as fallback backends.
- Add native two-pane SFTP browsing and transfer support to the Rust TUI.
- Add locked Cargo validation, release builds, helper-protocol smoke tests,
  and artifact generation for macOS arm64, macOS x64, and Windows x64 in CI.

## 1.0.0

- Initial public source release from a clean Git history.
- WezTerm SSH profile management, connection picker, login automation, Tabby
  import/export, OpenTUI manager, and integrated two-pane SFTP.
- Native OpenTUI build validation for macOS and Windows through GitHub Actions.
- Public-release documentation, CI hardening, dependency-source cleanup, and
  third-party license packaging.
