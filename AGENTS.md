# serial-midi

Rust re-implementation of the Korg NS5R rs232 MIDI protocol (and later potentially Roland SC-88 Pro) as a Linux userspace program. The protocol is reverse engineered from the reference material below.

## Development environment

- Work inside the Nix dev shell: `nix develop` (or let direnv activate it — `.envrc` runs `use flake`). The shell provides rustup, bindgen, and Ghidra; `rust-toolchain.toml` pins the toolchain to `stable`.
- Verify with the usual `cargo build` / `cargo test` — there is no CI, lint, or formatter config in the repo.

## Project state

- Codebase is at the initial stage: `src/main.rs` is a stub and `Cargo.toml` has no dependencies yet.

## Reference files (DO NOT EDIT THESE)

All of these are gitignored reference material, not part of the Rust project:

- Roland midi driver for Windows XP and 2k: `serial32_wxp2k/`
- Korg midi driver for Windows 95 to NT: `95thruNT/`
- Roland SC-88 Pro manual: `SC-88PRO_OM.pdf`
- Korg NS5R manual: `NS5R_OM_E3.pdf`
- ttymidi source code: `ttymidi/` — a GPL C program bridging serial MIDI to the ALSA sequencer; its `README` documents the 3-byte, no-running-status serial framing it expects, a useful model for the protocol work
