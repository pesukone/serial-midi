# KORG PC I/F Windows driver (KORGSMDN.DRV) — reverse engineering notes

Analysis of `95thruNT/KORGSMDN.DRV` (identical copy ships as
`95thruNT/NT/KORGSMDN.DRV`): the Windows NT/2000 MME `.DRV` module Korg shipped
for the serial "PC I/F" (NS5R etc.). The on-wire protocol it implements is
extracted into [`serial-protocol.md`](serial-protocol.md); this file documents
the driver itself.

Method: static analysis only — initial pass in Ghidra (project
`ghidra/korg-driver`, program `KORGSMDN.DRV`, image base `0x69F00000`, x86
PE32), full verification pass against the raw binary with radare2. All function
names below are analysis names applied in that project (addresses are the
ground truth). Notification/message constants were cross-checked against
mingw-w64 `mmsyscom.h`/`mmeapi.h`.

## File identity & installation

- PE32 (i386) DLL, image base `0x69F00000`, ~33 KB, 76 functions.
- Exports: `DriverProc` @0x69f01970, `midMessage` @0x69f02690,
  `modMessage` @0x69f02840, `entry` (DllMain) @0x69f01a30.
- DllMain (`entry`) calls `InitDriver` @0x69f012d0 on attach.
- Registered via `OEMSETUP.INF`: `korgsmdn = 1:korgsmdn.drv, "midi",
  "KORG PC I/F MIDI Driver for Windows NT"` — i.e. a multimedia driver in the
  `midi` .drv category (loaded by WINMM/mmdrv the classic MME way).
- Setup dialog title (screenshot `95thruNT/IMAGES/IMAGE7.GIF`):
  **"KORG PC I/F NT Driver 1.31"**.
- 16-bit Win 3.1 / DOS equivalents live in `95thruNT/ME/`
  (`KORGSMD1.DRV`, `KORGSMV1.386`) — not yet analyzed; they are the likely next
  RE target (they additionally document the protocol for Windows 3.1 and may
  contain the framing logic in plain 16-bit code).

## Registry state

HKCU, key `Software\KORG\KORG PC I/F MIDI Driver` (created if absent), both
values DWORD:

| Value | Cached in | Notes |
|---|---|---|
| `COM` | word @0x69f05000 | default `0xFFFF`; forced to 1 if not in 1–4; written back to the registry on load |
| `Messages` | word @0x69f0501c | static default `0x7E7F`; filter bits + flag `0x100` = "Independent Synth / MIDI Out" (see § MIDI OUT transmit path) |

Read at `InitDriver` time and whenever the setup dialog saves; both are also
written back by `InitDriver` on load.

## Device model

Device-name pointer table @0x69f04000:

| idx | Name |
|---|---|
| 0 | `KORG PC I/F MIDI Port` |
| 1 | `KORG PC I/F Default MIDI` |
| 2 | `KORG PC I/F MIDI Out` |
| 3 | `KORG PC I/F Synth-A` |
| 4 | `KORG PC I/F Synth-B` |

- **MIDI IN: exactly 1 device** (`GetDeviceCount` @0x69f03640; always name[0]).
- **MIDI OUT: 1 device** normally — dev 0 with szPname = name[0], i.e. it
  carries the same display name as the IN port ("KORG PC I/F MIDI Port") — or
  **4 devices** when the `Messages` word has bit `0x100` ("Independent Synth /
  MIDI Out"): the szPname index becomes `dev + 1`, so dev 0..3 are named
  "Default MIDI", "MIDI Out", "Synth-A", "Synth-B". [confirmed] (The dev→framing
  split is fixed in the write path regardless of the flag: dev 0/1 `FF`-framed,
  dev 2/3 raw — see [`serial-protocol.md`](serial-protocol.md).)
- Device caps (`FillDeviceCaps` @0x69f01aa0): both IN and OUT write a
  template with `cbStruct = 0x37`, `dw4 = 0x11F`; the only difference is the
  word at offset +2: `2` for MIDI IN, `1` for MIDI OUT. Names are taken from
  the table above.

## Module globals

| Addr | Meaning |
|---|---|
| 0x69f04040 | critical section (init/deleted with the module heap) |
| 0x69f04058 | module instance (`HINSTANCE` from `DllMain`) |
| 0x69f0405c | module heap (`HeapCreate(0, 0x320, 0)`) |
| 0x69f05000 | COM port number (1–4) |
| 0x69f05004 | byte bitmask: COM1–COM4 present (probed via `CreateFileW("COM%d", 0)` in `InitDriver`) |
| 0x69f05008 | Shared serial handle (`HANDLE`; −1 = not open) |
| 0x69f0500c | Refcount (word) for the shared serial handle |
| 0x69f05010 | Last device index written to the wire (`F5 <dev>` cache; `0xFFFF` initially, so the first write always selects) |
| 0x69f0501c | `Messages` filter word (bit 0x100 = Independent) |
| 0x69f0501f[type+dev] | byte per (type, device) pair, 1 = currently open (shared handle re-attach without refcount bump if already set) |
| 0x69f05028 | Heap that owns the shared-serial bookkeeping |

## Driver entry points & dispatch

### DriverProc @0x69f01970

`switch (msg - 1) { case 0..8 }` — module-level `DRV_*` plumbing. Every case
just `return 1` except the case for message 7, which opens the setup dialog
(`ShowSetupDialog` @0x69f01950 → `DialogBoxParamW` on dialog template resource
#100 with `SetupDialogProc` @0x69f014e0). Unknown messages fall through to
`DefDriverProc`. [open] The case→official-`DRV_*`-name mapping doesn't line up
cleanly with the published `DRV_*` values; message 7 behaves like a
"configure" hook.

### modMessage @0x69f02840 (MIDI OUT driver messages)

Dispatches `msg` 1..9 via jump table @0x69f029c0 — the classic `MODM_*`
numbering:

| msg | Name | Behaviour |
|---|---|---|
| 1 | `MODM_GETNUMDEVS` | 1 or 4 (Independent flag) |
| 2 | `MODM_GETDEVCAPS` | `FillDeviceCaps` |
| 3 | `MODM_OPEN` | `OpenInstance` (size 0x6C), open shared serial; notifies `MOM_OPEN` |
| 4 | `MODM_CLOSE` | sends `MOM_CLOSE` (0x3C8), frees instance, decrements refcount |
| 5 | `MODM_PREPARE` | returns 8 (`MMSYSERR_NOTSUPPORTED`) |
| 6 | `MODM_UNPREPARE` | returns 8 |
| 7 | `MODM_DATA` | unpack `dwParam1` bytes → `GetShortMsgTxLength` filter → `SendBytes` |
| 8 | `MODM_LONGDATA` | require `MHDR_PREPARED` (else 0x40 `MIDIERR_UNPREPARED`); gate on `IsMessageEnabled(dev, 6)` (exclusive); `SendBytes` of the whole buffer; updates `dwFlags` and sends `MOM_DONE` (0x3C9) when the write succeeded or was gated off |
| 9 | `MODM_RESET` | calls the receive-pump helper (`RunReceiveMachine`, a no-op on OUT instances), returns 0 |

### midMessage @0x69f02690 (MIDI IN driver messages)

Dispatches `msg` 0x35..0x3E via jump table @0x69f0280a — the classic `MIDM_*`
numbering:

| msg | Name | Behaviour |
|---|---|---|
| 0x35 | `MIDM_GETNUMDEVS` | 1 |
| 0x36 | `MIDM_GETDEVCAPS` | `FillDeviceCaps` |
| 0x37 | `MIDM_OPEN` | `OpenInstance` (size 0x2D0), open shared serial, create receive task; notifies `MIM_OPEN` |
| 0x38 | `MIDM_CLOSE` | asks the task to close; if bytes are still pending in the current header, refuses (0x41 `MIDIERR_STILLPLAYING`); otherwise waits for the task (`inst+0x34` mmTask handle), notifies `MIM_CLOSE` (0x3C2), frees instance |
| 0x39 | `MIDM_PREPARE` | returns 8 |
| 0x3A | `MIDM_UNPREPARE` | returns 8 |
| 0x3B | `MIDM_ADDBUFFER` | require `MHDR_PREPARED` (else 0x40 `MIDIERR_UNPREPARED`), refuse if `MHDR_INQUEUE` already set (0x41); `QueueInBuffer` @0x69f021c0 (clears PREPARED, sets INQUEUE, signals task) |
| 0x3C | `MIDM_START` | sets instance state to 3 and signals the task; the task arms one-byte `ReadFileEx` on all 8 slots (no client notification) |
| 0x3D | `MIDM_STOP` | sets state to 1 and signals the task; task completes/aborts the current header |
| 0x3E | `MIDM_RESET` | sets state to 4 and signals the task; task calls `AbortCurrentHeader` / `FlushInputHeaders` |

## Serial port management

`OpenSerialShared` @0x69f033e0:

1. Device validation (`OpenSerialShared` receives type+dev): dev ≥ 1 on IN, or
   dev ≥ 4 on OUT with the Independent flag set, or dev ≥ 1 on OUT without it,
   returns 2 (`MMSYSERR_BADDEVICEID`).
2. `wsprintfW("COM%d")` from the global, `CreateFileW(...,
   GENERIC_READ|GENERIC_WRITE, dwShareMode = 0 /* exclusive */,
   OPEN_EXISTING, FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT)` → global
   handle at 0x69f05008 (one handle shared by all open instances; an
   already-open handle is re-attached without a refcount bump when the
   (type, dev) open byte is already set; a second caller from a different
   owning heap gets error 4).
3. `GetCommState` → `BuildCommDCBW("baud=38400 parity=N data=8 stop=1")` →
   `SetCommState` (failure → 8). No timeouts, no flow control configured;
   `PurgeComm`/`SetCommMask` are never used.
4. `timeBeginPeriod(1)` on the fresh open; `timeEndPeriod(1)` when the
   refcount drops to 0 in `FreeInstance` (which also `CloseHandle`s the serial
   port and resets the globals).

`SerialWriteRaw` @0x69f01f00: `WriteFile` with the instance's `OVERLAPPED` at
+0x54 and event at +0x64; if it returns FALSE with `ERROR_IO_PENDING` (997),
waits `INFINITE` on the event and treats it as success. Returns 0 on success,
0xB (`MMSYSERR_INVALPARAM` — odd choice) when the shared handle is not open or
the write failed. `SerialWriteFramed` @0x69f01f80 layers the wire protocol on
top (see protocol doc): raw passthrough for dev 2/3, `FF`+≤4-byte chunks for
dev 0/1, stopping the chunk loop on any failed write.
`CommErrorToMmsyserr` @0x69f035a0 maps `ClearCommError` flags to MMSYSERR
codes.

## MIDI OUT transmit path

`modMessage(MODM_DATA)` → `GetShortMsgTxLength` @0x69f02a30 decides how many
bytes of the packed message go out (a select-case byte-table maps the status
category to a handler; jump table @0x69f02bfd, category table @0x69f02c1d):

| Status | Length | Gate |
|---|---|---|
| 0x80 / 0x90 / 0xE0 | 3 | **none — note-off, note-on and pitch-bend are always sent** |
| 0xA0 (Poly Key Pressure) | 3 | `IsMessageEnabled(dev, 5)` |
| 0xB0 (Control Change) | 3 | `IsMessageEnabled(dev, 2)` **and** (`IsMessageEnabled(dev, 3)` **or** `(cc & ~0x20) != 0`) — i.e. category 2 is the master CC enable; bank-select CC (#0/#32) additionally needs code 3, other CCs don't |
| 0xC0 (Program Change) | 2 | `IsMessageEnabled(dev, 1)` |
| 0xD0 (Channel Pressure) | 2 | `IsMessageEnabled(dev, 4)` |
| 0xF0 / 0xF1 / 0xF2 | 1 / 2 / 3 | — |
| 0xF5 / 0xF6 / 0xF7 | 1 | — |
| ≥0xF8 | 1 | — |
| 0xF3 / 0xF4, other | undefined | fall into a stale-value handler (compiler artifact; never reached by well-formed messages) |

[confirmed] all rows (jump table @0x69f02bfd + handler bodies). The earlier
"notes are dropped" reading from a previous analysis pass was **wrong** — the
8/9/E categories share the length-3 handler; the six filter categories
(1=Program Change, 2=Control Change, 3=Bank Select, 4=Channel Pressure,
5=Poly Key Pressure, 6=Exclusive) line up exactly with the six checkbox labels
in the setup dialog, and there is no note category because notes are never
filtered.

`IsMessageEnabled(dev, code)` = `(MessagesWord >> (dev + (code ∈ {2,3} ? 8 : 0))) & 1`
@0x69f029f0. [confirmed] as an expression — note the first argument is the
**device index**, so categories 1/4/5/6 share one bit per device (bits 0–3)
and categories 2/3 share one bit per device (bits 8–11); the categories within
a group are *not* independently switchable. Which checkbox label drives which
bit is therefore genuinely murky — see § Setup dialog. [open]

`MODM_LONGDATA` also marks the header (sets bit 0, clears bit 2 of `dwFlags`)
and sends `MOM_DONE` when the write succeeded or the message was gated off
(result 0); on a failed write it returns the error without `MOM_DONE`.

Then `SendBytes` @0x69f020f0 → (if length ≠ 0) `SelectActiveDevice` @0x69f020a0
(emit `F5 <dev>` when the global last-device cache @0x69f05010 differs) →
`SerialWriteFramed` @0x69f01f80 (`FF` chunking for dev 0/1, raw for dev 2/3).

## MIDI IN receive path

- `MIDM_OPEN` allocates a 0x2D0-byte instance; the receive task entry
  (`MidiInTask` @0x69f02380) sets its own priority to `TIME_CRITICAL` (15) and
  waits alertably on the task event, re-running the receive machine on
  `WAIT_IO_COMPLETION`.
- The IN instance's slot-0 region doubles as parser state (reachable via the
  ring pointer at `inst+0x48`, ring base = `inst+0x6C`): current-header pointer
  (`inst+0x1C`), packed message dword (`ring+0x4`), running status (`ring+0xC`),
  sysex-active flag (`ring+0xD`), expected-data count (`ring+0xF`),
  bytes-collected count (`ring+0x10`), buffer-handle mirror (`ring+0x18`).
  Eight 0x48-byte slots hang off `inst+0x6C`; each read is a `ReadFileEx`
  record at `slot+0x20` with driver extensions: bytes transferred at +0x14,
  link at +0x18, done flag at +0x1C, and the 1-byte staging buffer at +0x30
  (it overlaps the next slot — staging buffers live inside the ring).
- `MIDM_START` makes the task arm `ReadFileEx` for **one byte** at a time on
  all 8 slots (`ArmSerialRead` @0x69f02120, completion routine
  `ReadByteCompleteApc` @0x69f02260 sets the done flag + byte count).
  `RunReceiveMachine` @0x69f02290 walks completed nodes and feeds every byte
  to `ParseInByte` @0x69f02d80, re-arming reads as slots drain; `timeGetTime()`
  is kept in `inst+0x50` while the queue empties.
- `ParseInByte` is a standard MIDI parser (jump table @0x69f03248 + category
  table @0x69f03268), with these verified behaviours:
  - **No queued input header → received bytes are silently discarded.**
  - Running status: stored at `ring+0xC`; when a data byte completes a message,
    collection immediately restarts with the stored status.
  - Expected-data counts: 0x80–0xBF & 0xE0 → 2, 0xC0/0xD0 → 1.
  - Real-time bytes ≥0xF8 → immediate `MIM_DATA` (p1 = the byte alone,
    p2 = the ADDBUFFER handle).
  - Completed short messages → `MIM_DATA` with p1 = bytes packed little-endian
    (status in the low byte, running status injected if needed),
    p2 = buffer handle. **Not** a buffer offset.
  - `F0` starts SysEx into the current header (`EmitSysexByte` @0x69f02c80).
    SysEx bytes are *streamed*: each byte written to the header triggers a
    `MIM_LONGDATA` notification (p1 = header, p2 = buffer handle), so does a
    buffer-full rollover to the next queued header; `F7` closes it.
  - SysEx interrupted by any non-`F7` status byte is terminated with a
    synthetic **`0xFF`**, which emits `MIM_LONGERROR` (not `F7`). The
    interrupting `F0–F7` byte is then swallowed; channel/system `≥0x80`
    statuses are processed normally, real-time is delivered.
  - Status arriving with a half-built short message → `MIM_ERROR` (p1 = the
    stale packed dword); orphan data bytes with no running status →
    `MIM_ERROR`; unmatched `F7` → `MIM_ERROR`; `F3`/`F4` ignored; `F1`/`F2`
    accumulate like short messages.
- On stop/reset, `AbortCurrentHeader` @0x69f03360 completes the active header
  with `MIM_LONGERROR` (only if bytes were recorded), and
  `FlushInputHeaders` @0x69f032d0 drains the queue.

No framing of any kind is applied to received bytes — device→host is plain MIDI.

## Client notifications

`NotifyClient` @0x69f02640 wraps `DriverCallback`:
`cb = inst+0xC`, `dwFlags = HIWORD(inst+0x44)`, `hInst = inst+0x14`,
`dwUser = inst+0x10`. Codes used, matching `mmsyscom.h` exactly:

| Code | Constant |
|---|---|
| 0x3C1 | `MIM_OPEN` |
| 0x3C2 | `MIM_CLOSE` |
| 0x3C3 | `MIM_DATA` |
| 0x3C4 | `MIM_LONGDATA` |
| 0x3C5 | `MIM_ERROR` |
| 0x3C6 | `MIM_LONGERROR` |
| 0x3C7 | `MOM_OPEN` |
| 0x3C8 | `MOM_CLOSE` (observed on `MODM_CLOSE`) |
| 0x3C9 | `MOM_DONE` |

## Instance layout (allocated by `OpenInstance` @0x69f01b80)

OUT instance 0x6C bytes; IN instance 0x2D0 bytes.

| Off | Meaning |
|---|---|
| +0x04 | device index (0–3) |
| +0x08 | instance type (1 = MIDI IN, 2 = MIDI OUT) |
| +0x0C / +0x10 / +0x14 | callback, dwUser, hInst (for `DriverCallback`) |
| +0x18 | serial handle |
| +0x1C | current input header (IN) |
| +0x20, +0x38, +0x3C | event handles (task wake / RX ack) |
| +0x24 | task command word (`SignalInTask` @0x69f02220 writes it, sets event, then blocks on the ack event; returns `inst+0x40`) |
| +0x28 | instance state (1=stopped, 3=started, 4=reset) — `QueueInBuffer` also parks the just-queued header here for the task |
| +0x34 | receive-task handle (`mmTaskCreate`; waited on by `MIDM_CLOSE`, closed there) |
| +0x40 | task result |
| +0x44 | open flags (HIWORD = DriverCallback flags) |
| +0x48 | pointer to 8-slot input-header ring (IN only; ring base = `inst+0x6C`) |
| +0x4C | scratch dword used by `GetShortMsgTxLength` |
| +0x50 | last receive activity (`timeGetTime`) |
| +0x54 | `OVERLAPPED` for TX writes |
| +0x64 | TX completion event |
| +0x68 | owning heap block |

`FreeInstance` @0x69f01df0 releases everything and clears the (type, device)
bit in the global open bitmap.

## Setup dialog

`SetupDialogProc` @0x69f014e0, template resource #100, invoked from
DriverProc msg 7 and at install. Verified control wiring:

- IDs 0x65–0x68: COM1–COM4 radio buttons (`CheckRadioButton` group 101–104);
  radios for COM ports not found by the `InitDriver` presence probe
  (bitmask @0x69f05004) are disabled.
- IDs 0x71–0x76: six checkboxes ↔ `Messages` **bits 1–6**.
- IDs 0x69–0x6F: one anchor control (0x69) plus six checkboxes (0x6A–0x6F) ↔
  `Messages` **bits 9–14**. This whole group is disabled while the Independent
  checkbox is unchecked.
- ID 0x77: "&Independent Synth / MIDI Out" ↔ bit 8 (`0x100`).
- OK (ID 1) writes `COM` and the freshly-assembled `Messages` DWORD to the
  registry and updates the globals; Cancel (ID 2) just closes.
- Bit 0 (and 7/15) are **not** reachable from the dialog; the static default
  word is `0x7E7F` (bits 0–6 + 9–14 set, bit 8 clear).

The screenshot (`95thruNT/IMAGES/IMAGE7.GIF`) shows two identical label sets —
"Synth Out Messages" and "MIDI Out Messages" — each: *Program Change, Control
Change, Bank Select, Channel Pressure, Poly Key Pressure, Exclusive* — plus
the "Synth Out Messages" group greyed when the Independent box is off.

[open] **The category labels don't line up with the filter's bit semantics.**
The filter indexes bits by *device* (bits 0–3 = devices 0–3 for Program
Change/Channel Pressure/Poly AT/Exclusive; bits 8–11 = devices 0–3 for CC/Bank
Select), while the dialog has six category-labelled checkboxes per group writing
bits 1–6 / 9–14. Consistent reading: the checkboxes actually move the
**devices 1–3** filter bits (bit 8/0 are taken by the Independent flag /
unreachable), which explains why the 0x69–0x6F group is greyed when
Independent is off — devices 1–3 (the extra ports) don't exist then — and the
six category labels are effectively cosmetic. [open] Which visual group is
the 0x69–0x6F one (label order), and whether the label↔bit mismatch is a
driver bug, remains unproven from static code alone.

## Error codes used

| Code | Constant | Where |
|---|---|---|
| 2 | `MMSYSERR_BADDEVICEID` | `OpenSerialShared` device validation |
| 4 | `MMSYSERR_ALLOCATED` | shared serial owned by a different heap |
| 7 | `MMSYSERR_NOMEM` | `OpenInstance` alloc/event/task failures |
| 8 | `MMSYSERR_NOTSUPPORTED` | PREPARE/UNPREPARE on both sides |
| 0xB | `MMSYSERR_INVALPARAM` (serial not open check returns this — odd choice) | SerialWrite paths when handle == −1 |
| 0x40 | `MIDIERR_UNPREPARED` | LONGDATA / ADDBUFFER on unprepared header |
| 0x41 | `MIDIERR_STILLPLAYING` | MIDM_CLOSE with data pending; ADDBUFFER on an already-queued header |

## Function map

| Address | Analysis name | Role |
|---|---|---|
| 0x69f012d0 | `InitDriver` | load registry, init globals |
| 0x69f014e0 | `SetupDialogProc` | dialog window proc |
| 0x69f01950 | `ShowSetupDialog` | DialogBoxParamW wrapper |
| 0x69f01970 | `DriverProc` (export) | module-level dispatch |
| 0x69f01a30 | `entry` (export) | DllMain → InitDriver |
| 0x69f01a90 | `NoOp` | returns 0 |
| 0x69f01aa0 | `FillDeviceCaps` | caps template + name table |
| 0x69f01b80 | `OpenInstance` | alloc/init instance, serial share |
| 0x69f01df0 | `FreeInstance` | teardown |
| 0x69f01f00 | `SerialWriteRaw` | overlapped WriteFile, event wait |
| 0x69f01f80 | `SerialWriteFramed` | F5/FF wire protocol, dev routing |
| 0x69f020a0 | `SelectActiveDevice` | emit `F5 <dev>` on change |
| 0x69f020f0 | `SendBytes` | select + framed write |
| 0x69f02120 | `ArmSerialRead` | ReadFileEx 1-byte |
| 0x69f021c0 | `QueueInBuffer` | MIDM_ADDBUFFER |
| 0x69f02200 | `PumpReceive` | one pump pass |
| 0x69f02220 | `SignalInTask` | set cmd word + SetEvent |
| 0x69f02260 | `ReadByteCompleteApc` | ReadFileEx completion |
| 0x69f02290 | `RunReceiveMachine` | drain completed bytes |
| 0x69f02380 | `MidiInTask` | mmTask body, alertable wait |
| 0x69f02640 | `NotifyClient` | DriverCallback wrapper |
| 0x69f02690 | `midMessage` (export) | MIDI IN dispatch |
| 0x69f0280a | (jump table) | midMessage cases 0x35–0x3E |
| 0x69f02840 | `modMessage` (export) | MIDI OUT dispatch |
| 0x69f029c0 | (jump table) | modMessage cases 1–9 |
| 0x69f029f0 | `IsMessageEnabled` | filter-bit test |
| 0x69f02a30 | `GetShortMsgTxLength` | TX filter |
| 0x69f02c80 | `EmitSysexByte` | sysex byte into current header |
| 0x69f02d80 | `ParseInByte` | MIDI RX parser |
| 0x69f032d0 | `FlushInputHeaders` | complete/return headers |
| 0x69f03360 | `AbortCurrentHeader` | long-error flush |
| 0x69f033e0 | `OpenSerialShared` | CreateFileW + DCB + refcount |
| 0x69f035a0 | `CommErrorToMmsyserr` | CE_* → MMSYSERR |
| 0x69f03640 | `GetDeviceCount` | respects Independent flag |
| 0x69f03680 | `KorgMemcpy` | inline memcpy helper |

## Notes for the Rust port

- The Linux equivalent maps cleanly: one ALSA sequencer port per logical
  device; shared `termios` handle on the real UART at 38400 8N1 raw;
  `F5 <dev>` cache and `FF`-chunked framing are trivial to reproduce exactly.
- Reproduce the *observed* behaviour first (including the per-device message
  filter bits), then decide which parts to make configurable — they are clearly
  policy, not protocol.
- RX side needs no framing logic at all: standard byte-stream MIDI parsing,
  optionally note that real-time messages must bypass any chunk buffering.

## Reproducing the analysis

- Project: `ghidra/korg-driver.gpr` (auto-analyzed). Open it via the MCP
  bridge, or use `analyzeHeadless` when the GUI does not hold the project lock.
- The verification pass (2026-11-23) was done entirely with radare2, which
  needs no project at all:
  `nix shell nixpkgs#radare2 -c r2 -q -e scr.color=0 -e asm.bytes=false -c "aa; s <addr>; af; pdf" 95thruNT/KORGSMDN.DRV`
  (jump tables: `pf 9d @<table_addr>`; strings: `iz`, `psw @<addr>`).
- The Ghidra MCP `force_decompile` currently fails in this environment
  (`SleighLanguageDescription` error); work directly with
  `disassemble_function` + `read_memory` + manual jump-table decoding.
- Analysis names are applied by `ghidra/scripts/RenameKorg.java`:
  `analyzeHeadless ghidra korg-driver -process KORGSMDN.DRV -noanalysis
  -scriptPath ghidra/scripts -postScript RenameKorg.java` (requires the
  project not be open in the Ghidra GUI, which holds the lock).
- NS5R manual (`NS5R_OM_E3.pdf`) is a scan with no text layer — render pages
  with `pdftoppm` and OCR with `tesseract` (both via `nix shell`). Relevant
  pages: 12–24 (Preparations / driver setup), Global [041] BPS Select p.68.
