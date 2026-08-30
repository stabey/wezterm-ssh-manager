# Contributing

Thanks for helping improve wezterm-ssh-manager.

## Before opening an issue

- Search existing issues first.
- Remove passwords, tokens, private keys, public IP addresses, internal host
  names, user names, profile contents, and terminal escape payloads from logs.
- Use the private vulnerability-reporting flow described in `SECURITY.md` for
  security-sensitive reports.

## Development checks

For the OpenTUI helper:

```bash
cd tui-opentui
bun install --frozen-lockfile
bun run typecheck
bun test
bun run build
```

For the Python Textual fallback, install `tui/requirements.txt` plus `pytest`,
then run:

```bash
python -m pytest -q tui/tests
```

Keep pull requests focused. New dependencies must use an OSI-approved license
compatible with this project's MIT license, and distributed binary changes
must update `THIRD_PARTY_NOTICES.md` and `tui-opentui/licenses/`.
