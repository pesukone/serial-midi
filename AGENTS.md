# serial-midi

Rust re-implementation of the Korg NS5R rs232 MIDI protocol (and later potentially Roland SC-88 Pro) as a Linux userspace program. The protocol is reverse engineered from the reference material below.

## Development environment

- Work inside the Nix dev shell: `nix develop` (or let direnv activate it — `.envrc` runs `use flake`). The shell provides rustup, bindgen, and JDK 25 (for running Ghidra); `rust-toolchain.toml` pins the toolchain to `stable`.
- Verify with the usual `cargo build` / `cargo test` — there is no CI, lint, or formatter config in the repo.

## Project state

- Codebase is at the initial stage: `src/main.rs` is a stub and `Cargo.toml` has no dependencies yet. Rust edition 2024.

## Reverse engineering workflow

- Windows driver binaries are analyzed in Ghidra, reached from OpenCode through the `ghidra-mcp` MCP server (`import_file`, `connect_instance`, symbol/function tools). The Ghidra project lives at `ghidra/korg-driver/`.
- If `ghidra-mcp` reports no running instance, ask the user to start Ghidra (the dev shell ships JDK 25 for this) before attempting RE work.
- Primary RE target is `95thruNT/KORGSMDN.DRV` (32-bit PE32 NT driver). `95thruNT/ME/` holds 16-bit DOS drivers (harder to analyze); `serial32_wxp2k/RDSMIDNT/` holds the Roland PE32 drivers for the later SC-88 Pro target.

## Reference files (DO NOT EDIT THESE)

All of these are gitignored reference material, not part of the Rust project, and they exist only in this working copy (a fresh clone or worktree will not have them):

- Roland midi driver for Windows XP and 2k: `serial32_wxp2k/` (actual PE32 binaries in `RDSMIDNT/`)
- Korg "PC I/F" midi driver for Windows 95/ME/NT: `95thruNT/` (32-bit NT driver `KORGSMDN.DRV`, 16-bit DOS drivers in `ME/`)
- Roland SC-88 Pro manual: `SC-88PRO_OM.pdf`
- Korg NS5R manual: `NS5R_OM_E3.pdf`
- ttymidi source code: `ttymidi/` — a GPL C program bridging serial MIDI to the ALSA sequencer; its `README` documents the 3-byte, no-running-status serial framing it expects (status byte = command + channel, then two parameter bytes; 1-parameter commands padded with 0) — a useful model for the protocol work. GPL: read for reference, don't lift code into the Rust implementation
- The two PDF manuals are the source of truth for the on-device side; the agent's model can't read PDFs directly, so extract text first (e.g. `nix run nixpkgs#poppler-utils --command pdftotext NS5R_OM_E3.pdf -`)
