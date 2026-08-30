# Rebuilding and Relinking the Standalone TUI

This document applies to version 1.0.0. It records how to reproduce the
application bundle and how to rebuild the Bun runtime with modified
LGPL-covered components before embedding that runtime in the application.

## Identify the exact application source

Official release artifacts include `BUILD-METADATA.json`. Its
`sourceCommit` field is the Git commit that produced that artifact. Obtain the
matching source from:

```text
https://github.com/stabey/wezterm-ssh-manager
```

Check out that commit, not merely the moving default branch. A source archive
for a release tag can also be used if it resolves to the same commit. All
application Lua, TypeScript, TSX, Python, PowerShell, tests, build scripts, and
configuration needed for the project are in that source tree.

## Reproduce the ordinary application build

Install Bun 1.3.13 and run from the repository root:

```sh
cd tui-opentui
bun install --frozen-lockfile
bun run scripts/build.ts --target current
bun run scripts/package.ts
```

The checked-in `bun.lock` fixes the npm packages. The build script supports
`macos-arm64`, `macos-x64`, and `windows-x64`; `current` selects the host. A
normal release build uses the unmodified Bun 1.3.13 runtime downloaded by Bun
for the requested compile target.

`scripts/package.ts` refuses to package with another Bun version unless its
explicit override flag is used. It creates one staging directory per detected
executable under `dist/packages/`, copies the full notice set, and records the
source commit and tool version in `BUILD-METADATA.json`.

## Exact Bun and WebKit source

The standalone executable contains the complete Bun runtime. This release
uses:

- Bun tag: `bun-v1.3.13`
- Bun release revision: `bf2e2cecf`
- Bun source archive:
  `https://github.com/oven-sh/bun/archive/refs/tags/bun-v1.3.13.tar.gz`
- Bun Git repository: `https://github.com/oven-sh/bun.git`
- WebKit revision:
  `4d5e75ebd84a14edbc7ae264245dcd77fe597c10`
- WebKit source archive:
  `https://github.com/oven-sh/WebKit/archive/4d5e75ebd84a14edbc7ae264245dcd77fe597c10.tar.gz`
- WebKit Git repository: `https://github.com/oven-sh/WebKit.git`

The WebKit revision above is the value of `WEBKIT_VERSION` in
`scripts/build/deps/webkit.ts` at the Bun tag. Keep local copies of these exact
sources when redistributing binaries; an upstream URL is convenient but is
not a guarantee of permanent source availability.

## Rebuild Bun with a modified LGPL component

Bun 1.3.13's build documentation requires an existing release Bun and LLVM
21.1.8. Its build scripts install the matching Zig toolchain. Follow the
platform prerequisites in the tagged
[`CONTRIBUTING.md`](https://github.com/oven-sh/bun/blob/bun-v1.3.13/CONTRIBUTING.md),
then run:

```sh
git clone https://github.com/oven-sh/bun.git bun-1.3.13
cd bun-1.3.13
git checkout --detach bun-v1.3.13
git submodule update --init --recursive
bun install --frozen-lockfile

git clone https://github.com/oven-sh/WebKit vendor/WebKit
git -C vendor/WebKit checkout --detach 4d5e75ebd84a14edbc7ae264245dcd77fe597c10

# Make the desired changes in vendor/WebKit (or another LGPL component), then:
bun run build:release:local
```

The release-local profile uses the local WebKit checkout and writes under
`build/release-local`. The tagged source also provides the debug command
`bun run build:local`, which writes under `build/debug-local`. Building WebKit
requires substantial disk space (the tagged Bun documentation warns that the
WebKit tree plus build output exceeds 8 GB).

For tinycc changes, modify the tinycc source/dependency selected by the same
Bun tag and use the same release build. Consult Bun's tagged build scripts for
the exact dependency revision selected on the target platform.

## Embed the rebuilt runtime in this application

Build on each target operating system and architecture for which a modified
runtime is needed. In a working copy of this project's
`tui-opentui/scripts/build.ts`, change the `compile` configuration for the
current-platform build as follows:

1. Remove `target: target.bun` from the `compile` object.
2. Add `executablePath: "/absolute/path/to/bun-1.3.13/build/release-local/bun"`
   (use the `.exe` path on Windows).
3. Keep the existing entrypoint, Solid plugin, `external`, `minify`, `env`,
   autoload, and output options unchanged.
4. Run that edited build script with the rebuilt Bun executable and select
   only the current platform.

Bun 1.3.13 exposes `compile.executablePath` specifically to choose the Bun
executable used as the base of a standalone build. Omitting `compile.target`
is important: a named cross-compile target can select a downloaded stock
runtime instead of the locally rebuilt one.

Example, after setting `executablePath` in a local copy of the build script:

```sh
/absolute/path/to/bun-1.3.13/build/release-local/bun \
  run scripts/build.ts --target current
/absolute/path/to/bun-1.3.13/build/release-local/bun \
  run scripts/package.ts --allow-bun-version-mismatch
```

The packaging override is necessary only when the rebuilt binary reports a
development-flavoured version string instead of exactly `1.3.13`. Inspect the
resulting `BUILD-METADATA.json`; its `bunVersion` must identify the runtime
actually used. Preserve your modified source, configuration, and scripts with
the redistributed executable.

## LGPL distribution note

Bun's own `LICENSE.md` says that its statically linked JavaScriptCore/WebKit
requires recipients to be able to modify the library and relink the
application. The source and procedure above are intended to make that
possible; the complete LGPL-2.0 and LGPL-2.1 texts are included in every
package.

This repository does not mirror the multi-gigabyte Bun and WebKit source
archives or provide a long-term written source offer. Anyone distributing the
standalone binaries is responsible for choosing and maintaining a source-code
delivery method that satisfies the LGPL in every jurisdiction and for as long
as their chosen distribution method requires. This document is engineering
provenance, not legal advice.
