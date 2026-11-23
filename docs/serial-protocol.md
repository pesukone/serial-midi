# Korg "PC I/F" serial MIDI wire protocol

What actually goes over the RS-232 cable between a PC and the Korg "TO HOST"
port (NS5R and siblings), as reverse engineered from
`95thruNT/KORGSMDN.DRV` (the Windows NT/2000 MME driver, "KORG PC I/F NT
Driver 1.31"). See [`korg-pc-if-driver.md`](korg-pc-if-driver.md) for the
driver internals this was read out of.

Evidence tags used below:

- **[confirmed]** — observed directly in the driver disassembly.
- **[hypothesis]** — a reasonable inference, not proven.
- **[open]** — unresolved question.

## Physical layer

| Setting | Value | Evidence |
|---|---|---|
| Baud rate | 38400 | [confirmed] `BuildCommDCBW("baud=38400 parity=N data=8 stop=1")` in `OpenSerialShared` @0x69f033e0. [confirmed] NS5R manual p.16: Global `[041] BPS Select` must be `38.4` when connecting to an IBM PC. |
| Format | 8 data bits, no parity, 1 stop bit | [confirmed] same DCB string |
| Flow control | not configured by the driver (whatever `GetCommState` returned is kept; no `SetCommMask`/handshake calls observed) | [confirmed] |
| Cable | Korg AG-001 / AG-001B (COM port ↔ instrument "TO HOST" connector); AG-004 for 9-pin → 25-pin serial | NS5R manual, Preparations p.2/p.16 |
| Instrument side | "TO HOST" DIN connector on the NS5R; the same port expects `31.25` kBaud (Global [041]) for the Macintosh cable (AG-002B) | NS5R manual p.17 |

The Macintosh number (31.25 kbaud ≈ 2× the MIDI rate, i.e. the plain-MIDI-at-double-speed
"TO HOST" mode) hints the port is a plain UART whose framing expectations change with the
BPS setting; the Windows driver side is what we can pin down exactly.

## Device → host direction (MIDI IN)

[confirmed] **Plain, unframed MIDI byte stream** — standard status/data bytes,
running status supported, system real-time messages (`≥0xF8`) inserted at
arbitrary points. The driver's receive path (`ParseInByte` @0x69f02d80) is a
textbook MIDI parser (see driver doc § MIDI IN path); it contains **no**
recognition of any `F5`/`FF` framing or handshake in this direction, and never
sends ACKs.

## Host → device direction (MIDI OUT)

Every write to the serial port is one of two things:

### 1. Device select — `F5 <dev>` (2 bytes, sent raw)

The driver keeps a module-global "last selected device" word @0x69f05010,
initialised to `0xFFFF` — so the very first write after load always emits the
select. Before writing payload bytes, if the target logical port differs from
the last one written to, it writes exactly:

```
F5 <dev>
```

`<dev>` is the driver's device index:

| dev | Port name (Independent mode off) | Port name (Independent mode on) | Payload framing |
|---|---|---|---|
| 0 | `KORG PC I/F MIDI Port` (same display name as the IN port) | `KORG PC I/F Default MIDI` | `FF`-framed |
| 1 | — (not exposed) | `KORG PC I/F MIDI Out` | `FF`-framed |
| 2 | — (not exposed) | `KORG PC I/F Synth-A` | raw |
| 3 | — (not exposed) | `KORG PC I/F Synth-B` | raw |

[confirmed] The `F5` write and the dev-index routing logic are in
`SelectActiveDevice` @0x69f020a0 + `SerialWriteFramed` @0x69f01f80
(devices 2/3 → `SerialWriteRaw`, devices 0/1 → `FF` chunking).
[confirmed] The name mapping: `FillDeviceCaps` picks the szPname table index as
`dev + (Independent ? 1 : 0)`, so with the flag off the sole dev 0 shows
name[0], and with it on dev 0–3 show name[1..4]. The dev→framing split is fixed
in the write path regardless of the flag.

### 2. Payload

- **Devices 0/1:** the buffer is chopped into chunks of **1–4 MIDI bytes**, each
  chunk prefixed with a single `FF` byte:

  ```
  FF b0 b1 b2 b3        FF b4 b5            FF ...
  ```

  [confirmed] The chunking loop in `SerialWriteFramed` @0x69f01f80
  (`mov byte [ebp-0xc],0xff`, chunk size capped at 4, one `WriteFile` per
  chunk). Chunks are packed purely by length — message boundaries are **not**
  respected, a single MIDI message can straddle two `FF` chunks.
- **Devices 2/3 (Synth-A/B):** raw MIDI bytes, no `FF`, no chunking.
  [confirmed] Same function, dev 2/3 branch calls `SerialWriteRaw` @0x69f01f00.

### Example

Assuming nothing has been selected yet, sending a Program Change `C0 05` and a
Bank Select CC `B0 00 00` (5 MIDI bytes) to device 0 produces on the wire:

```
F5 00  FF C0 05 B0 00  FF 00
       └─────┬─────┘    └─┬─┘
   chunk 1 (4 payload bytes)  chunk 2 (1 byte)
```

(A second `F5 <dev>` is only emitted when a *different* device is written to.
The device-select cache is per-driver-module, not per-port-handle, and is never
reset on close — a Linux reimplementation should keep it process-global to
match.)

Note also that the driver opens the COM port with `dwShareMode = 0`
(exclusive): nothing else can hold the port while a Korg MIDI port is open.

## Semantics / open questions

- [open] **Device-side meaning of `F5`/`FF`.** The NS5R manual only says: connect
  the AG-001B cable, set Global `[041] BPS Select` to `38.4`, install the Korg
  driver. It does not document any framing. The `F5 nn` + `FF`-chunked framing is
  however strongly reminiscent of the "MIDI box"-style 38.4 kbaud TO HOST
  protocols used by Korg/Yamaha instruments of that era (where `F5`-style header
  and `FF`-prefixed chunks encode MIDI bytes over a baud rate that is not an
  exact MIDI multiple). A Linux reimplementation should reproduce the driver's
  byte stream exactly, then verify against real hardware.
- [open] Whether the device ever sends anything *other* than plain MIDI on the
  host baud rate (e.g. at power-up) — the driver's parser would treat anything
  non-MIDI as errors/orphan bytes, so presumably not.
- [hypothesis] `FF`-framing exists because 38400 baud carries 3.072 bytes per
  MIDI-byte-time; framing re-synchronizes/re-buffers the receiving tone module's
  MIDI input pipeline, and `F5 <dev>` routes between the NS5R's internal tone
  generator and its physical MIDI OUT (matching the driver's Synth vs MIDI Out
  ports and the dialog's "Independent Synth / MIDI Out" option). Not verified.

## Contrast: ttymidi framing

For comparison, `ttymidi/README` uses a fixed 3-byte framing (status+2 data,
padded) with no running status and no device select — a far simpler,
mutually-incompatible convention. The Korg link keeps full MIDI semantics
(running status intact) in the RX direction and adds only light TX framing.
