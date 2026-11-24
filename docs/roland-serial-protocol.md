# Roland serial MIDI wire protocol (SC-8850 / SC-8820 / SK-500 / SC-88 Pro)

What goes over the RS-232 cable between a PC and the Roland "Serial MIDI"
port, as reverse engineered from `serial32_wxp2k/RDSMIDNT/` (the Windows
2000/XP driver set: `RDSMIDNT.DLL` + `RDSMIDNT.SYS` + `RDSMIDNT.EXE`,
"Roland Serial MIDI Driver Ver.3.2", (C) 1996–2000). Driver internals are in
[`roland-rdsmidnt-driver.md`](roland-rdsmidnt-driver.md).

Evidence tags used below:

- **[confirmed]** — observed directly in the driver disassembly/decompilation.
- **[hypothesis]** — a reasonable inference, not proven.
- **[open]** — unresolved question.

Addresses cited: `RDSMIDNT.SYS` at its native image base (`0x10000`, so
e.g. `RsMidiRxDataWrite` @0x10f96), `RDSMIDNT.DLL` at `0x6bf00000`,
`RDSMIDNT.EXE` at `0x400000`.

## Summary

The link is **plain MIDI bytes in both directions**, with exactly one
protocol addition: a 2-byte **`F5 <device>` port-select message** that the
host emits before writing to a different logical port, and that the driver
consumes (never shows to applications) in the device→host direction to tell
apart data coming from the instrument's internal groups vs. its physical
MIDI IN jack. There is **no chunking, no escaping, no ACK, and no
checksum** — byte-level transparency apart from the `F5` handling. This is
the same idea as the Korg NS5R's `F5 <dev>` device select
([`serial-protocol.md`](serial-protocol.md)), but without Korg's `FF`
chunking layer.

## Physical layer

| Setting | Value | Evidence |
|---|---|---|
| Baud rate | 38400 | [confirmed] `RDSMIDNT.EXE` serial init @0x401000: `DCB.BaudRate = 0x9600`. The driver refuses any UART that does not report `BAUD_38400`. |
| Format | 8 data bits, none parity, 1 stop bit | [confirmed] same init: `ByteSize=8`, `Parity=0`, `StopBits=0` |
| DTR | never asserted (`DTR_CONTROL_DISABLE` in every config) | [confirmed] same DCB setup |
| Flow control, default (`Handshaking=0`) | `RTS_CONTROL_ENABLE` (RTS held active), CTS **not** honoured | [confirmed] same DCB setup |
| Flow control, `Handshaking=1` | `fOutxCtsFlow` + `RTS_CONTROL_HANDSHAKE` (hardware RTS/CTS both directions) | [confirmed] same; the UART must also report `PC_DSR_SENSITIVITY`-safe modem lines, else error |
| XON/XOFF | never enabled | [confirmed] |
| Read timeouts | all zero; RX thread blocks on `ReadFile` one byte at a time | [confirmed] EXE @0x401e80 |
| UART requirements | `GetCommProperties` must report `PROV_SUBTYPE_8250`, `BAUD_38400`, 8 data bits, `PC_HARDWARE` and `PC_VC` | [confirmed] EXE @0x401000, each check has its own error code (0x500–0x510) |
| Host port choice | driver opens `COM<value+1>` from the stored zero-based `COM Port` value (see driver doc for the COM4 dialog bug) | [confirmed] EXE `wsprintfW("COM%d", v+1)` |
| Instrument side (SC-88 Pro) | `COMPUTER` switch: **PC-2** = serial port at 38 400 bps raw MIDI; PC-1 = 31 250 bps plain MIDI | SC-88 Pro manual (System Settings → `COMPUTER`) |
| Instrument side (SC-8850/8820/SK-500) | dedicated serial "Serial MIDI" connector driven at 38 400 bps by the multi-instrument link | README.TXT + manual; PC driver only supports these models' serial port at 38400 [hypothesis re: exact jack naming per model] |

`README.TXT` warns that cables without CTS/RTS wiring need the
"handshaking" option turned **off** (the default), consistent with the DCB
modes above. [confirmed]

## Host → device direction (MIDI OUT)

### Device select — `F5 <n>`

The kernel driver keeps a cached "current device number" byte per device
context (`RsMidiDevice+0xd09c`). Each MME output port has its own helper
(`MMEMidiOutA`..`F`, `RDSMIDNT.SYS` @0x10770–0x10e46 in the analysis dump —
names per the module's own export table). When the app writes to a port
whose device number differs from the cached one, the driver first pushes
`F5 <n>` and then the payload bytes into the TX ring; otherwise the payload
goes out raw:

```
F5 <n>  <raw MIDI bytes ...>
```

[confirmed] `MMEMidiOutX` helpers: compare `+0xd09c`, on mismatch write the
2-byte prefix `0xF5, n`, store the new value, then copy the MIDIHDR bytes.

Device numbers per port letter:

| Port letter | `F5 <n>` | Notes |
|---|---|---|
| A | 1 | In **PortMode 1** (SC-88/SC-88 Pro single-port) port A emits **no** `F5` at all — raw MIDI only [confirmed] |
| B | 2 | |
| C | 3 | **5** instead when PortMode is 200 (SC-8820) or 300 (SK-500) [confirmed] |
| D | 4 | stays 4 in every PortMode [confirmed] |
| E | 5 | not exposed as an MME port in any supported PortMode (legacy/reserved) [confirmed] |
| F | 6 | same, reserved [confirmed] |

The cache is per device object and is **not** reset when applications close
ports, so the first write after a fresh driver load always emits the select
(the field's zero value never matches a valid port number). [confirmed +
hypothesis about intent]

### Payload

Everything after the (optional) prefix is the application's MIDIHDR bytes
verbatim: status bytes, data bytes, `F0 ... F7` dumps, running status, and
real-time messages all pass through untouched. The driver does **not**
expand running status, does not split buffers, and computes nothing over
the payload. [confirmed] `SoundIoctlMidiPlay` @0x11d88 → per-port helper →
4 KB TX ring; `RDSMIDNT.EXE` copies the ring to the UART with
`WriteFileEx`.

## Device → host direction (MIDI IN)

The EXE's RX thread (`RDSMIDNT.EXE` @0x401e80) reads the UART **one byte at
a time** and applies a small filter before forwarding each surviving byte,
one per `DeviceIoControl` call, to the kernel (control-device code
`0x1dc008`):

1. `0xF4` → **suppress**: drop the `F4` byte and every following data byte
   (`<0x80`) until the next status byte re-arms the filter. The `F4`'s own
   "parameters" are discarded too — a `F4 xx` message is never delivered.
2. Any other status byte (`≥0x80`, including `0xF5`, `0xF7`, `0xF9`, `0xFD`,
   `≥0xF8` real-time) → clear suppression, deliver normally.
3. Data bytes outside suppression → deliver normally.

[confirmed] the filter code in the EXE RX thread; [open] the meaning of
`0xF4` — see Open questions.

The kernel's `RsMidiRxDataWrite` @0x10f96 then:

- `F5` → enter "expect parameter" state; the byte is **not** delivered to
  any client.
- next byte while in that state → store as the current device number
  (`RsMidiDevice+0xd0a4`), also not delivered.
- any other byte → deliver into one of the two per-port receive rings:
  - **ring B** (the "second input", surfaced as the physical-MIDI-IN MME
    port) iff `PortMode ∈ {100, 200, 300}` (SC-8850/SC-8820/SK-500) **and**
    the last stored `F5` parameter is `≥ 5`;
  - **ring A** (port "A", the internal part-groups) otherwise.

[confirmed] `RsMidiRxDataWrite`; routing constant comparison to 5.

So on SC-8850/SC-8820/SK-500 the device is expected to tag its outgoing
stream with `F5 <n>` just like the host does, with `n ≥ 5` meaning "this
came from the physical MIDI IN jack / external keyboard" and `n < 5`
meaning "internal group data". [hypothesis: the exact device-side meaning
of each parameter — e.g. 5 = MIDI IN jack, 6 = keyboard — since only the
`≥5` split is visible in the driver]

On SC-88/SC-88 Pro (PortMode 1/2) everything goes to ring A regardless of
the parameter; a device that still sends `F5 <n>` messages has them
swallowed, the payload delivered to a single input. [confirmed]

### Quirks

- A real-time byte (`≥0xF8`) arriving **between** `F5` and its parameter:
  the decompiled state machine delivers it but the "expect parameter" flag
  handling around that branch leaves the mode flag set afterwards, so the
  *next* data byte can be swallowed instead of delivered. [confirmed code
  path; hypothesis about the observable effect — a rare 1-byte drop]
- Bytes `≥0xF8` bypass suppression (filter rule 2) so `F8`–`FF` inside a
  suppressed `F4` window still flow. Real-time insertion mid-message is
  therefore preserved. [confirmed]

## Per-model MME port map

How the wire maps to what applications see (`PortMode` registry value;
strings from `RDSMIDNT.DLL`, caps built by `SoundMidiOut/InGetCapsX` in
`RDSMIDNT.SYS`):

| PortMode | Model | Output ports | Input ports | product word (caps, OUT/IN) |
|---|---|---|---|---|
| 0 (default, legacy) | "Roland Serial MIDI" | `Roland Serial MIDI Out A`…`E` | `Roland Serial MIDI Input` | `0x9543` / `0x9538` |
| 1 | SC-88 / SC-88 Pro, single port | `Roland Serial MIDI` (1×) | `Roland Serial MIDI` (1×) | `0x94df` / `0x94d4` |
| 2 | SC-88 / SC-88 Pro, multiple ports | `Roland Serial MIDI Out A`…`D` | `Roland Serial MIDI Out A` (1×) | `0x9543` / `0x9538` |
| 100 | SC-8850 | `Roland SC-8850` | `Roland SC-8850 MIDI IN 1` | `0x95a6` / `0x959c` |
| 200 | SC-8820 | `Roland SC-8820` | `Roland SC-8820 MIDI IN` | `0x960a` / `0x9600` |
| 300 | SK-500 | `Roland SK-500 (S)` | `Roland SK-500 Keyboard (S)` | `0x966e` / `0x9664` |

(Names [confirmed] as DLL string resources; assigning them to PortModes is
[confirmed] for the three dedicated model pairs, [hypothesis] for the
mode 0/1/2 cells. Product words and the fixed `0x0018`/`0x0079` ID pair
and driver-version word `800` (=3.20) per PortMode: [confirmed] from
`SoundMidiOutGetCapsA` @0x13110 / `SoundMidiInGetCapsA` @0x13562.)

The caps blob also carries an 800 count (voices/notes) and a formats word
`0x18`; the caps name field is copied from a device-name pointer at
device-context `+0x13678` that no static code path writes — the display
name strings exist only in `RDSMIDNT.DLL`, so the name is almost certainly
handed to the kernel via the user-mode config write [hypothesis].

## Sequences

Host plays a C-major chord on port B (SC-8850), first transmission:

```
host→dev:  F5 02  90 3C 40  91 44 40  92 4B 40        (select B, then raw)
host→dev:  B0 07 64                                 (same port: no F5)
host→dev:  F5 01  90 3C 40                          (switch to port A: new F5)
```

Device reports keys pressed on its physical MIDI IN jack:

```
dev→host:  F5 05  90 3C 64  80 3C 00                (kernel: F5 05 → ring B)
dev→host:  F0 41 ...  F7                             (exclusive: delivered byte-exact)
```

Device signalling a buffer problem (hypothesised):

```
dev→host:  F4 01              → dropped entirely by the EXE filter, then
dev→host:  90 3C 64           → normal data resumes (delivered)
```

## Comparison with the other targets

- **Korg NS5R** ([`serial-protocol.md`](serial-protocol.md)): same 38400
  8N1 and same `F5 <dev>` select concept, but Korg additionally wraps
  "Default MIDI"/"MIDI Out" payloads in `FF`-prefixed 1–4-byte chunks.
  Roland has no such layer — a ttymidi-style 3-byte-fixed framing or the
  Korg chunking would both be wrong for Roland.
- **ttymidi**: expects fixed 3-byte frames with padded single-parameter
  messages; Roland's stream is plain MIDI, so an NS5R/ttymidi bridge cannot
  be reused for Roland without dropping that framing.

## Open questions

- **[open]** What is `0xF4`? The driver only ever *suppresses* it, on both
  sides (the DLL also strips app-emitted `F5`+data, and `modMIDIlength`
  gives `F4` a length of 1 parameter). No device-side documentation was
  found. [hypothesis] It is an out-of-band status/error notification from
  the serial firmware — plausibly the "MIDI Buff. Full"/"Check Sum Error"
  conditions `README.TXT` mentions for the SC-8850-series displays. The
  driver never logs or surfaces it.
- **[open]** `0xF9` and `0xFD` are undefined in the MIDI spec but are not
  filtered (they merely re-arm after `F4`). Whether the device ever emits
  them [unverified].
- **[open]** Whether a device ever sends `F5 06` (port F / device number 6,
  which the host allocates but never uses itself).
- **[open]** The device-side meaning of `F5` parameters 1–4 per model
  (which part groups route to which number on SC-8850 vs SC-8820/SK-500,
  where port C is 5 and port D is 4 — the C/D numbering swap must match the
  instruments' expectations).
- **[confirmed]** No checksum exists anywhere in the host stack: every TX
  and RX path was checked; nothing computes a running sum. README's
  "Check Sum Error" messages are the instruments' own SmartMedia/display
  errors, unrelated to the serial host link. [hypothesis] the SCCI-era
  checksum idea simply doesn't apply to this protocol.
