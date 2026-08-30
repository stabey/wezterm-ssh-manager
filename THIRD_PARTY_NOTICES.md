# Third-Party Notices

This file applies to the version 1.0.0 source distribution and to the
`sshmgr-tui` standalone executables built from it. The project itself is
licensed under the MIT License; see `LICENSE`.

The standalone executable embeds the application bundle, platform-native
OpenTUI code, and the Bun runtime. The original license texts described below
are shipped in `tui-opentui/licenses/` in source distributions and in
`licenses/` beside every packaged executable.

## JavaScript packages in the application bundle

The following packages were observed in the production bundle generated from
`src/index.tsx`. Packages marked "prebundled by OpenTUI" are already folded
into the JavaScript files published by `@opentui/core`; their exact versions
come from the source maps shipped in `@opentui/core` 0.5.9.

| Component | Version | License | License file |
| --- | ---: | --- | --- |
| `@opentui/core` | 0.5.9 | MIT | `npm/opentui-core-0.5.9-LICENSE` |
| `@opentui/solid` | 0.5.9 | MIT | `npm/opentui-solid-0.5.9-LICENSE` |
| `solid-js` | 1.9.12 | MIT | `npm/solid-js-1.9.12-LICENSE` |
| `entities` | 7.0.1 | BSD-2-Clause | `npm/entities-7.0.1-LICENSE` |
| `ssh2` | 1.17.0 | MIT-style | `npm/ssh2-1.17.0-LICENSE` |
| `asn1` | 0.2.6 | MIT | `npm/asn1-0.2.6-LICENSE` |
| `bcrypt-pbkdf` | 1.0.2 | BSD-3-Clause and ISC-style terms | `npm/bcrypt-pbkdf-1.0.2-LICENSE` |
| `safer-buffer` | 2.1.2 | MIT | `npm/safer-buffer-2.1.2-LICENSE` |
| `tweetnacl` | 0.14.5 | Unlicense | `npm/tweetnacl-0.14.5-LICENSE` |
| `web-tree-sitter` | 0.25.10 | MIT | `npm/web-tree-sitter-0.25.10-LICENSE` |
| `ansi-regex` (prebundled by OpenTUI) | 6.2.2 | MIT | `npm/ansi-regex-6.2.2-LICENSE` |
| `strip-ansi` (prebundled by OpenTUI) | 7.1.2 | MIT | `npm/strip-ansi-7.1.2-LICENSE` |
| `get-east-asian-width` (prebundled by OpenTUI) | 1.4.0 | MIT | `npm/get-east-asian-width-1.4.0-LICENSE` |
| `emoji-regex` (prebundled by OpenTUI) | 10.6.0 | MIT | `npm/emoji-regex-10.6.0-LICENSE` |
| `string-width` (prebundled by OpenTUI) | 7.2.0 | MIT | `npm/string-width-7.2.0-LICENSE` |
| `bun-ffi-structs` (prebundled by OpenTUI) | 0.3.1 | MIT | `npm/bun-ffi-structs-0.3.1-LICENSE` |
| `diff` (prebundled by OpenTUI) | 9.0.0 | BSD-3-Clause | `npm/diff-9.0.0-LICENSE` |
| `marked` (prebundled by OpenTUI) | 17.0.1 | MIT | `npm/marked-17.0.1-LICENSE` |

`cpu-features` is deliberately externalized by the production build and is
not included in the standalone executable. Build-only packages are likewise
not included in this runtime table.

## Tree-sitter parser assets bundled by OpenTUI

OpenTUI 0.5.9 embeds the `web-tree-sitter` runtime above together with five
language-parser WASM assets and their highlight/injection queries. The npm
registry metadata for `@opentui/core` 0.5.9 records Git commit
`df2fc1594bb7a1274fc490155305e3d9f61f1b01`; the checked-in `bun.lock` records
the package integrity for the exact published bytes.

| Embedded asset | Upstream version | License | License file |
| --- | ---: | --- | --- |
| JavaScript grammar | `tree-sitter-javascript` v0.25.0 | MIT | `tree-sitter/tree-sitter-javascript-0.25.0-LICENSE` |
| TypeScript grammar | `tree-sitter-typescript` v0.23.2 | MIT | `tree-sitter/tree-sitter-typescript-0.23.2-LICENSE` |
| Markdown and Markdown-inline grammars | `tree-sitter-markdown` v0.5.1 | MIT | `tree-sitter/tree-sitter-markdown-0.5.1-LICENSE` |
| Zig grammar | `tree-sitter-zig` v1.1.2 | MIT | `tree-sitter/tree-sitter-zig-1.1.2-LICENSE` |

The bundled query files identify
[`nvim-treesitter`](https://github.com/nvim-treesitter/nvim-treesitter),
MDeiml's/tree-sitter-grammars' Markdown grammar, and
[`Helix`](https://github.com/helix-editor/helix) as sources. Their Apache-2.0,
MIT, and MPL-2.0 terms are preserved in `tree-sitter/nvim-treesitter-LICENSE`,
the Markdown grammar license above, and `tree-sitter/helix-MPL-2.0.txt`.
OpenTUI's generation config referenced moving upstream branches for several
queries, so this inventory describes the exact copies published inside
`@opentui/core` 0.5.9 rather than claiming an unavailable upstream revision.

## OpenTUI native code

The platform packages published for `@opentui/core` 0.5.9 contain native code
and ship the following notice set. The files were byte-identical across the
installed macOS, Windows, Linux, x64, and arm64 packages and are preserved
under `opentui-native-0.5.9/` with their original names:

- `LICENSE`
- `LICENSE-GHOSTTY`
- `LICENSE-WUFFS`
- `LICENSE-STB`
- `LICENSE-LIBWEBP`
- `PATENTS-LIBWEBP`
- `AUTHORS-LIBWEBP`
- `LICENSE-LCMS2`

Those originals cover OpenTUI itself and native components including Ghostty
code, Wuffs, stb, libwebp, and Little CMS. The complete text, copyright
notices, attribution, and patent terms in those files control.

## Bun standalone runtime

The executables are produced with Bun 1.3.13 (`bun-v1.3.13`, release revision
`bf2e2cecf`). Bun's official license inventory is preserved verbatim as
`bun-1.3.13/LICENSE.md`.

Bun states that it statically links JavaScriptCore and WebKit under LGPL-2.0
and tinycc under LGPL-2.1. The corresponding full license texts are included
as `bun-1.3.13/LGPL-2.0.txt` and `bun-1.3.13/LGPL-2.1.txt`. Bun's license file
also identifies the other libraries and polyfills embedded in its runtime and
links to their governing terms.

The WebKit revision pinned by Bun 1.3.13 is
`4d5e75ebd84a14edbc7ae264245dcd77fe597c10`. Exact source locations and
instructions for rebuilding Bun, replacing the LGPL-covered code, and
regenerating this standalone executable are in `REBUILDING.md`.

## No endorsement

Third-party names are used only for attribution and identification. They do
not imply endorsement of this project. All third-party software is provided
under its own terms and without additional warranty from this project.
