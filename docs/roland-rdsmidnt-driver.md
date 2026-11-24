# Roland "Serial MIDI" driver (`RDSMIDNT`) architecture

Architecture notes for `serial32_wxp2k/RDSMIDNT/` — Roland's Windows
2000/XP driver that bridges MME MIDI to the serial "Serial MIDI" port of the
SC-8850 / SC-8820 / SK-500 / SC-88(Pro) sound generators
(`RDSMIDNT.DLL` + `RDSMIDNT.SYS` + `RDSMIDNT.EXE`, `DriverVer 07/31/2000,
3.2.0.0`). The wire protocol these move is in
[`roland-serial-protocol.md`](roland-serial-protocol.md).

Evidence tags: **[confirmed]** read directly from the binaries,
**[hypothesis]** inferred, **[open]** unresolved.

Address bases: `RDSMIDNT.SYS` @0x10000 (native), `RDSMIDNT.DLL` @0x6bf00000,
`RDSMIDNT.EXE` @0x400000. Function names come from each PE's own export
table where available; Ghidra dump names are used otherwise. Caveat: the
SYS export table's ordinal→name pairing disagrees slightly with the
address-ordered function dump (e.g. `MMEMidiOutA` is listed @0x109f0 but
the dump places the F5-port-A helper at 0x10770); the dump's names are
behaviour-based and internally consistent, cited below.

## Why three components

```
app (MME) ── winmm ──► RDSMIDNT.DLL      (per-process MME driver, 0x6bf00000)
                           │ IOCTLs \\.\RsMidi*
                           ▼
                       RDSMIDNT.SYS      (kernel, Services\rdsmidnt, 0x10000)
                           │ pending IRPs: EXE reads TX ring, writes RX ring
                           ▼
                       RDSMIDNT.EXE      (user-mode serial engine, 0x400000)
                           │ ReadFile/WriteFileEx
                           ▼
                       \\.\COM<n>  ──►  instrument (38400 8N1)
```

The kernel driver never touches the COM port; the EXE never talks to apps.
The DLL spawns the EXE on the first port open and kills it on the last
close. [confirmed]

Install (`RDSMIDNT.INF`, class MEDIA): `rdsmidnt.dll`+`rdsmidnt.exe` →
`%SystemRoot%\System32`, `rdsmidnt.sys` → `System32\drivers`; registered as
MME driver `Drivers\midi\rdsmidnt.dll`. The kernel *service* is **not**
installed by the INF (`AddService=,0x2` with no section) — the DLL creates
and configures `Services\rdsmidnt` itself (`_DrvCreateServicesNode`
@0x6bf03598: `CreateServiceW`, LoadOrderGroup `"Synthesizer Drivers"`,
driver path `\SystemRoot\System32\drivers\`+`rdsmidnt.sys`). [confirmed]

## RDSMIDNT.DLL — MME driver

Exports: `DriverProc`, `midMessage`, `modMessage` (+ unnamed ordinal copies
and the config-dialog plumbing: `_ConfigDlgProc`, `_SetupComPortItem`,
`_SetupPortModeItem`, `_SetupHandshakingItem`, `_GetComPortUseing`, …).

- `midMessage`/`modMessage` implement MOM/WOM_OPEN / _CLOSE / _GETNUMDEVS /
  _GETDEVCAPS / _WRITE / _WRITE_LONG; open/close reference-count driver
  instances; the app's callback receives `0x3c8` on close and
  `0x3c9` (`MOM_DONE`) on long-data completion. [confirmed]
- Caps (driver caps path): device name strings from the DLL's
  string resources (`Roland Serial MIDI Input`, `Roland Serial MIDI Out
  A`…`E`, `Roland SC-8850`, `Roland SC-8850 MIDI IN 1`, `Roland SC-8820`,
  `Roland SC-8820 MIDI IN`, `Roland SK-500 (S)`, `Roland SK-500 Keyboard
  (S)`, default `Roland Serial MIDI`) — table in the protocol doc.
- Open path (in `midMessage` @0x6bf029b4 / `modMessage` @0x6bf02bde): reads
  `HKLM\SYSTEM\CurrentControlSet\Services\rdsmidnt\Parameters`
  (`COM Port`, `PortMode`, `Handshaking`), opens
  `\\.\RsMidiOutControl` / `\\.\RsMidi{In,Out}<N>` via `sndOpenDev`
  @0x6bf03ad4, then
  `FindWindowExW(NULL, NULL, "Roland Serial MIDI Driver(Windows NT4.0)
  Control Program")`; if absent, `CreateProcess("rdsmidnt.exe")` and pings
  the control window with `SendMessageW(hwnd, 0x2400, 0x1e, 0x89875000)`
  until it answers. [confirmed]
- Last close: `FindWindowExW` again → `Sleep(500)` →
  `SendMessageW(hwnd, WM_CLOSE)` to terminate the EXE. [confirmed]
- Data: `WOM_WRITE` → `IRP` on `\\.\RsMidiOut<N>` with
  `IOCTL 0x1d821b` (MIDIHDR buffer, user buffer length ≤ 0x10000).
  `WOM_WRITE_LONG` runs a filter first: app-emitted `F5` plus its following
  data bytes are stripped (`modMIDIlength`: `F5`→len 0, `F4`→len 1) so
  applications cannot forge port selects; the remainder goes out as a MIDIHDR.
  [confirmed]
- Port presence probe: `_GetComPortUseing` tries `CreateFileW("COM1".."COM4")`
  (share-exclusive); opening **successfully** *or* failing with
  `ERROR_ACCESS_DENIED` sets the port's bit — the resulting bitmap means
  "port exists" and is used both to default-select the COM radio and to
  reject unusable selections ("Cannot use the selected COM port").
  Kernel-side port-busy flags go through IOCTLs `0x1d4010`/`0x1d4014`
  (EXE marks the port while the COM handle is open; event name
  `"RsMidDriver%08x"`). [confirmed]
- Settings dialog (`modMessage 0x3d1`, "Roland Serial MIDI Driver
  Settings"): radio group `0x514–0x517` = COM1–4 → `COM Port`; checkbox
  `0x708` = handshaking (0/1); checkbox `0x5dc` "Use multiple ports"
  (off → `PortMode=1`) plus combo `0x6a4` (`SC-88, SC-88Pro`→2,
  `SC-8850`→100, `SC-8820`→200, `SK-500`→300; invalid → reset to default,
  `_GetPortModeItem` @0x6bf0182a / `_SetupPortModeItem` @0x6bf01a9e).
  Invalid COM values (>4) reset to the first free port from the in-use
  bitmap (`_SetupComPortItem` @0x6bf01a40). **Bug:** `_GetComPortItem`
  @0x6bf0174e maps the COM4 radio (`0x517`) to the value **4**, but
  `SetupComPortItem` renders value+0x514 (so 4 → nonexistent `0x518`,
  nothing shown checked) and the EXE opens `COM<value+1>` — the correct
  value for COM4 is **3**, which can only be set by editing the registry
  by hand; configuring COM4 through the dialog yields value 4 → the EXE
  opens COM5. [confirmed] `_Configure` then
  `_SetDriverConfig` (registry) + `_DrvConfigureDriver` (restart service).

## RDSMIDNT.SYS — kernel driver

DriverEntry @0x14124; reads its own `Parameters` (value-name callback
`SoundReadConfiguration` @0x1463a compares `COM Port`/`PortMode`/
`Handshaking`), creates device `\Device\RsMidiOutControl` (string
`"Serial MIDI Control"` for the description, symlink under
`\DosDevices\`) plus up to 100 instances of `\Device\RsMidiIn<N>` /
`\Device\RsMidiOut<N>` (`SoundCreateDevice` @0x14754, `RsMidiCreateDevice`
@0x141ca); each created device is recorded under the service key's
`Devices` key as a REG_DWORD value named after the device name, data = the
device type byte, 3=input / 4=output (`SoundSaveDeviceName` @0x13b9c) —
the user-facing *display* name strings live only in `RDSMIDNT.DLL`.
[confirmed]

Device context layout (observed offsets):

| Offset | Contents |
|---|---|
| `+0x94` | RX ring array base; one ring per input queue, stride `0x2000`, ring header `{read, write, len}` |
| `+0xc094 / +0xc098` | TX ring read / write pointers; ring data at `+0xc09c`, size 4 KB |
| `+0xd09c` | cached last-TX-selected device number (`F5` prefix suppression) |
| `+0xd0a0 / +0xd0a4` | RX `F5`-expect-parameter flag / last RX `F5` parameter |
| `+0x13678 / +0x13680` | saved device-name pointer / active `PortMode` (used by caps) |
| per-port tables | handler pointers `+0xec`(read)/`+0xf0`(write), stride 0x10cc per letter A–F |

IOCTLs (device type `0x1D`):

| Code | Device | Handler @ | Purpose |
|---|---|---|---|
| `0x1d4004` | control | `SoundRsMidiDispatch` 0x11c60 | get config block (0x54 bytes) |
| `0x1d400c` | control | 〃 | get `[comPort, handshaking]` (consumed by EXE) |
| `0x1d4010` | control | 〃 | get port-in-use state |
| `0x1d4014` | control | 〃 | set port-in-use (1/0) |
| `0x1dc008` | control | `RsMidiRxDataWrite` 0x10f96 | deliver one RX byte from EXE |
| `0x1d4204` | midi | `SoundMidiDispatch` 0x12556 | write config to device (forwarded) |
| `0x1d8208` | midi | 〃 | set MIDI input state; type-4 clients trigger `SoundMidiResetTable` |
| `0x1d820c` | midi | `SoundIoctlGetMidiState` 0x1387c | get MIDI state |
| `0x1d821b` | midi | `SoundIoctlMidiPlay` 0x11d88 | play: `SoundMidiTableSet` (note/CC64 tracking) then per-port `MMEMidiOutX` helper |
| `0x1d821b`-sibling | midi | `SoundIoctlMidiRecord` 0x11e0c | record (input) state |
| `0x1d8220/24` | midi | 〃 | get/set 0x100-byte driver state (DLL cases 0xc/0xd) |
| `0x1d4210/14` | midi | 〃 | get/set 4-byte driver state (DLL `sndSetData`/`sndGetData`) |

The play/rx ioctls lack `ProbeForRead/Write` guards (only an SEH frame and
a length check in some handlers), i.e. an unprivileged caller could aim
kernel copies at arbitrary addresses. **[confirmed]** presence/absence of
probes; noted as an RE observation, not a recommendation.

Data flow:

- **TX**: `SoundIoctlMidiPlay` → letter helper `MMEMidiOutA`…`F`
  (0x10770–0x10e46) pushes optional `F5 <n>` + payload into the 4 KB ring.
  The EXE keeps a standing `ReadFile` (= pending `IRP_MJ_READ`, registered
  via `SoundSetRsMidiReadData` @0x11a50) on the control device; when ≥1
  byte is queued and the EXE buffer is ≥ 0x1000, `RsMidiTxDataRead`
  @0x10f2e completes it with up to 4096 bytes. [confirmed]
- **RX**: `RsMidiRxDataWrite` (ioctl `0x1dc008`) implements the `F5` state
  machine and picks ring A vs B (see protocol doc); `SoundMidiInDeferred`
  @0x11ed4 completes pending client `ReadFile`s. The generic parser
  `MidiByteRec` @0x11f6c + `MidiRecord` @0x121b0 timestamp messages
  (`SoundGetTime` @0x12930 = KeQuerySystemTime + perf counter) and
  `SoundMidiTableSet`/`SoundMidiResetTable` (0x1229a/0x12482) track
  sounding notes so that `MidiFlushDevice` @0x12116 / client close can
  emit note-offs. `SoundExcludeRoutine` @0x11532 is the MIDI-exclusion
  filter for shared devices. [confirmed]
- Caps: `SoundMidiOutGetCapsA`…`F` @0x13110+, `SoundMidiInGetCaps*`
  @0x13562+ fill the caps blob consumed by the DLL (fixed `{0x18, 0x79}` ID
  pair, version word 800 = 3.20, per-model product word — tables in the
  protocol doc; 800-count, formats `0x18`); the name field reads device
  context `+0x13678`. [confirmed]; see protocol doc § port map for the
  name-source caveat.
- Port bring-up stubs `HwInitializePortA`…`F`/`HwInitializeRsMidi`
  @0x13f40+ (no UART access — they only reset per-port software state,
  including the `F5` cache). [confirmed + hypothesis re: intent]

## RDSMIDNT.EXE — serial engine

Registers window class `"Roland Serial MIDI Driver(Windows NT4.0) Control
Program"` (message loop handles the DLL's `0x2400` ping, replies;
`WM_CLOSE` exits). Three threads:

1. **TX pump** [confirmed @0x401000 area]: loops `ReadFile` on
   `\\.\RsMidiOutControl` (4 KB buffer); on data, fetches the config block
   via `DeviceIoControl(0x1d4004)` and opens
   `CreateFileW("COM<comPortByte+1>")` — config byte at offset `0x430`,
   `wsprintfW("COM%d", v+1)` — configures the UART,
   `WriteFileEx` the buffer verbatim, `CloseHandle` on the port per batch;
   on write failure `ClearCommError` + `PurgeComm` and retry.
2. **RX pump** @0x401e80: `ReadFile` one byte at a time, applies the
   `0xF4`-suppression filter, forwards survivors via `DeviceIoControl
   (0x1dc008)`.
3. **message loop** (main thread).

UART setup (`FUN_00401000` @0x401000) and its error ladder — failures map
to codes `0x500`–`0x510`, which the DLL shows as strings
`"Error has occurred in serial port settings(CodeNNN)"` (Code000…Code016
are exactly this ladder, plus `No Serial MIDI Driver`,
`Serial MIDI Driver not recognized`, `does not open`,
`Serial port currently in use`): `GetCommProperties` must report
`PROV_SUBTYPE_8250`/`BAUD_38400`/8 bits/`PC_HARDWARE`/`PC_VC`; then
`GetCommState`→`SetCommState` with 38400-8-N-1, `DTR_CONTROL_DISABLE`,
XON off, zero timeouts, `fAbortOnError`; `SetupComm(4096, 4096)`. A debug
string `"CTS/RTS flow control cannot be enabled"` accompanies the
`Handshaking=1` path (debug code `0xb00`). [confirmed]

## Registry summary

`HKLM\SYSTEM\CurrentControlSet\Services\rdsmidnt`:

| Value | Type | Meaning |
|---|---|---|
| `Parameters\COM Port` | DWORD | zero-based COM index, EXE opens `COM<value+1>`; the dialog emits 0/1/2/**4** (4 = broken COM4, see dialog bug above — correct COM4 value 3 is only reachable by hand-edit) |
| `Parameters\PortMode` | DWORD | 0 legacy, 1 SC-88/Pro single, 2 SC-88/Pro multi, 100 SC-8850, 200 SC-8820, 300 SK-500 |
| `Parameters\Handshaking` | DWORD | 0: RTS held, CTS ignored; 1: RTS/CTS hardware flow |
| `Devices` | key of REG_DWORDs | one value per created device: name = device name, data = device type byte (3=input, 4=output) |

[confirmed] read by both SYS (`SoundReadConfiguration`) and DLL
(`sndOpenParametersKey` @0x6bf03cea).

## Quirks / bugs worth knowing

- The `F5`-then-realtime corner in the RX state machine can swallow one
  data byte (protocol doc § Quirks). [confirmed path, inferred impact]
- Port letter → device number swap on SC-8820/SK-500: **C→5, D→4**
  (everywhere else C→3). Deliberate, must match the instruments' group
  numbering — but the numbering itself is undocumented. [open]
- Ports E/F exist throughout (device numbers 5/6, `MMEMidiOutE/F`,
  `SoundMidiOutGetCapsE/F`, legacy names `…Out E`) but no PortMode
  exposes them as MME ports. Legacy leftovers. [confirmed]
- The EXE opens/closes the COM handle for every TX batch — a port probe
  (`CreateFileW`) can briefly stall playback. [confirmed code; hypothesis
  re: observable stalls]
- `0x1dc008`/`0x1d821b` ioctl parameter probing is missing (above).
- Window class name says "Windows NT4.0" although the INF only targets
  2000/XP — the EXE is the same codebase as the NT4-era Ver 2.00
  (DLL string `Roland Serial MIDI Driver(Windows NT4.0) Ver2.00`).

## How this was reverse engineered

`analyzeHeadless` import into a scratch project (`/tmp/opencode/
ghidra-roland`) + whole-program decompile post-script
(`ghidra/scripts/DecompileDump.java`); cross-checks with
`r2`/`rabin2` (export tables, small functions; SYS native base 0x10000).
Caveats: a lingering headless JVM once re-saved the project and
regenerated the dump files mid-analysis with a different load base
(+0x10000000) — if dumps look inconsistent, delete them and re-run
`DecompileDump.java`; the current 0x10000-based dumps name functions by
the PE export table. `RDSMIDNT.SYS` exports (MMEMidiOutA..F,
RsMidiTxDataRead, SoundIoctlMidiPlay, Sound*…) are its real symbols, not
user-mode exports — it is a kernel driver whose PE happens to export its
internals.
