# serial-midi

Rust re-implementation of the Korg NS5R rs232 MIDI protocol (and later potentially Roland SC-88 Pro) as a Linux userspace program. The protocol is reverse engineered from the reference material below.

## Development environment

- Work inside the Nix dev shell: `nix develop` (or let direnv activate it — `.envrc` runs `use flake`). The shell provides rustup, bindgen, and JDK 25 (for running Ghidra); `rust-toolchain.toml` pins the toolchain to `stable`.
- Verify with the usual `cargo build` / `cargo test` — there is no CI, lint, or formatter config in the repo.

## Project state

- Codebase is at the initial stage: `src/main.rs` is a stub and `Cargo.toml` has no dependencies yet. Rust edition 2024.
- RE findings for the NT driver are written up in `docs/korg-pc-if-driver.md` (driver internals, dispatch tables, function map) and `docs/serial-protocol.md` (wire protocol: 38400 8N1, `F5 <dev>` device select, `FF`+≤4-byte chunk framing on host→device for the "Default MIDI"/"MIDI Out" ports, raw for Synth-A/B, plain MIDI device→host).

## Reverse engineering workflow

- Windows driver binaries are analyzed in Ghidra (project `ghidra/korg-driver.gpr`, created with `analyzeHeadless`, auto-analyzed; contains `KORGSMDN.DRV` with ~30 functions renamed from the analysis) or directly with **radare2**, which needs no Ghidra at all: `nix shell nixpkgs#radare2 -c r2 -q -e scr.color=0 -e asm.bytes=false -c "aa; s <addr>; af; pdf" 95thruNT/KORGSMDN.DRV`.
- `ghidra-mcp` setup: Ghidra GUI extension "GhidraMCP" 7.0.0 (`Tools > GhidraMCP > Start MCP Server`, HTTP on 8089, socket under `/run/user/1000/ghidra-mcp/`) + Python bridge `ghidra-mcp-bridge` 7.0.0 (`uv tool install` of the release wheel; executable `bridge-mcp-ghidra`, configured as MCP server in opencode.json).
- If `ghidra-mcp` reports no running instance, ask the user to start Ghidra (the dev shell ships JDK 25 for this) before attempting RE work.
- Known ghidra-mcp quirks in this environment:
  - `rename_function` takes `function_address` + `new_name`. Renames apply and `save_program` **persists** when the program was opened properly via `open_project` + `open_program` (the earlier "not checked out" failure came from opening the program outside the project). The 32 analysis names are saved in the project as of 2026-11-23; `ghidra/scripts/RenameKorg.java` remains as a headless fallback.
  - `get_functions({})` errors and `force_decompile` fails with a `NoClassDefFoundError SleighLanguageDescription` — **known plugin/Ghidra version mismatch**: extension 7.0.0 is built for Ghidra 12.1.4, but nixpkgs ships Ghidra 12.2, which moved `SleighLanguageDescription` (upstream fix PR #624 merged 2026-10-28, not in any release yet). `disassemble_function`/`read_memory`/`get_metadata` are unaffected. Until a 7.0.1 release lands (or Ghidra is pinned to 12.1.x), use radare2 for decompiler-grade work.
  - The bridge is **lazy about tool groups**: only `listing,function,program` load by default. `get_xrefs_to`/call-graph live in `xref`, project/file ops in `project` — call `load_tool_group` (or add `--default-groups listing,function,program,xref` to the bridge command) before concluding a tool is missing. Connecting resets loaded groups to the defaults.
  - Opening a program requires the project: `open_project` (path to `ghidra/korg-driver.gpr`) then `open_program`. **After a Ghidra restart the bridge keeps dialing the old instance's socket** (`Could not connect ... No such file or directory`) — fix without restarting anything by calling `connect_instance({project:"korg-driver"})`; the GUI-side `Tools > GhidraMCP > Start MCP Server` toggle must be on for the socket/port 8089 to answer at all.
- Primary RE target `95thruNT/KORGSMDN.DRV` (32-bit PE32 NT driver) is analyzed; follow-up targets are `95thruNT/ME/` (16-bit DOS drivers `KORGSMD1.DRV`/`KORGSMV1.386`, import with `analyzeHeadless ghidra korg-driver -import <file>`) and `serial32_wxp2k/RDSMIDNT/` (Roland PE32 drivers for the later SC-88 Pro target).
- The NS5R manual PDF is a **scan with no text layer** (`pdftotext` yields ~nothing). Render pages with `nix shell nixpkgs#poppler-utils -c pdftoppm -r 150 -f N -l M -png NS5R_OM_E3.pdf out` and OCR with `nix shell nixpkgs#tesseract -c tesseract page.png - -l eng` (`nix run nixpkgs#poppler-utils` fails in this setup — use `nix shell`).

## Reference files (DO NOT EDIT THESE)

All of these are gitignored reference material, not part of the Rust project, and they exist only in this working copy (a fresh clone or worktree will not have them):

- Roland midi driver for Windows XP and 2k: `serial32_wxp2k/` (actual PE32 binaries in `RDSMIDNT/`)
- Korg "PC I/F" midi driver for Windows 95/ME/NT: `95thruNT/` (32-bit NT driver `KORGSMDN.DRV`, 16-bit DOS drivers in `ME/`)
- Roland SC-88 Pro manual: `SC-88PRO_OM.pdf`
- Korg NS5R manual: `NS5R_OM_E3.pdf`
- ttymidi source code: `ttymidi/` — a GPL C program bridging serial MIDI to the ALSA sequencer; its `README` documents the 3-byte, no-running-status serial framing it expects (status byte = command + channel, then two parameter bytes; 1-parameter commands padded with 0) — a useful model for the protocol work. GPL: read for reference, don't lift code into the Rust implementation
- The two PDF manuals are the source of truth for the on-device side; the agent's model can't read PDFs directly, so extract text first — `pdftotext` works for the Roland manual, the NS5R one needs the render+OCR pipeline described above.
