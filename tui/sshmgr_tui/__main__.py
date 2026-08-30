from __future__ import annotations

import argparse
import os
import sys

from .app import SshManagerApp
from .protocol import cleanup_runtime, configure


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog='sshmgr_tui', description='wezterm-ssh-manager TUI')
    parser.add_argument('--snapshot', required=True, help='snapshot.json written by the Lua plugin')
    args = parser.parse_args(argv)
    path = os.path.abspath(args.snapshot)
    if not os.path.isfile(path) or os.path.basename(path) != 'snapshot.json':
        print(f'snapshot not found: {path}', file=sys.stderr)
        return 2
    token = os.environ.pop('WEZTERM_SSHMGR_SESSION_TOKEN', '')
    runtime_dir = os.path.dirname(path)
    try:
        configure(token, runtime_dir)
    except (OSError, ValueError) as exc:
        print(f'invalid TUI session: {exc}', file=sys.stderr)
        cleanup_runtime(path, runtime_dir)
        return 2
    try:
        os.chmod(path, 0o600)
    except OSError:
        if os.name != 'nt':
            cleanup_runtime(path)
            raise
    try:
        SshManagerApp(path).run()
    finally:
        cleanup_runtime(path)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
