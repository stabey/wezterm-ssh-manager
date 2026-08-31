# Rust 1.98.0 notice origin

These files are preserved from the Rust 1.98.0 installation used to build and
validate this release candidate:

- `LICENSE-APACHE` and `LICENSE-MIT` are the Rust project's governing texts.
- `COPYRIGHT` is the Rust project's distribution notice.
- `COPYRIGHT-library.html` is Rust 1.98.0's generated copyright and license
  inventory for the subset used by the Rust Standard Library.
  Its generated trailing whitespace was normalized for repository hygiene;
  the legal text and markup are otherwise unchanged.

The release workflow pins `rustc` 1.98.0. The packager refuses a different
compiler version rather than pairing a moving standard library with stale
notices. Upstream project: <https://github.com/rust-lang/rust>.
