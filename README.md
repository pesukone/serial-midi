# serial-midi

Expose a Korg NS5R (or other Korg "PC I/F" instrument) hanging off a serial
port as an ALSA MIDI device — a Rust equivalent of
[ttymidi](https://github.com/bbbradshaw/ttymidi), speaking the Korg "TO HOST"
serial protocol instead of ttymidi's 3-byte framing.

The wire protocol is reproduced from the Windows driver `KORGSMDN.DRV`;
see [`docs/serial-protocol.md`](docs/serial-protocol.md) and
[`docs/korg-pc-if-driver.md`](docs/korg-pc-if-driver.md).

## Usage

```
serial-midi [OPTIONS] <SERIAL-DEVICE>
```

`<SERIAL-DEVICE>` is e.g. `/dev/ttyS0` or `/dev/ttyUSB0`. The link runs at
38400 baud 8N1 (hardcoded, like the Windows driver); on the NS5R set Global
`[041] BPS Select` to `38.4` and connect via a Korg AG-001/AG-001B cable.

options:

- `--name NAME` — ALSA client name (default `Korg NS5R`)
- `--independent` — expose all four MIDI OUT ports ("Default MIDI",
  "MIDI Out", "Synth-A", "Synth-B") as in the driver's "Independent Synth /
  MIDI Out" mode; by default a single port (device 0) is created
- `-v, --verbose` — log MIDI traffic to stderr

Example session:

```sh
serial-midi /dev/ttyUSB0 &      # creates ALSA client, prints its id
aplaymidi -p 128:1 song.mid     # or aconnect a sender to 128:1
aconnect 128:0 129:0            # route NS5R MIDI IN to some receiver
```

The program opens the serial port exclusively and creates one read port
(instrument → system) plus one or four write ports (system → instrument).
Events are dispatched direct (no ALSA queue timestamps involved, like
ttymidi).

## Behaviour notes

- Host→device emits `F5 <dev>` on every switch of the target logical device
  (process-global cache, exactly like the driver) and frames payload for
  devices 0/1 as `FF` + 1–4 byte chunks; Synth-A/B payloads go out raw.
- Device→host is plain MIDI: running status, sysex, and interleaved
  real-time bytes are all handled.
- Not yet implemented (driver features that are policy, not protocol): the
  per-device message filter bits ("Messages" registry word) and the setup
  dialog's category checkboxes.

## Building

Inside the Nix dev shell (`nix develop`, needs `alsa-lib` for the sequencer
API — declared in `flake.nix`):

```sh
cargo build
cargo test
```

The only runtime dependency beyond libc is `libasound` (linked via
`#[link(name = "asound")]`; no C headers needed at build time).

## Tests

`cargo test` covers the wire protocol (F5/FF framing against the byte
examples from the driver analysis, MIDI byte-stream parsing with running
status/sysex/real-time) and the ALSA event struct ABI layout.

For a manual end-to-end check without hardware, a PTY pair works well as a
fake instrument:

```sh
socat -d pty,raw,echo=0,link=/tmp/ttyA pty,raw,echo=0,link=/tmp/ttyB &
serial-midi /tmp/ttyA -v &
printf '\xf0\x42\x30\x01\xf7\x90\x3c\x7f\x3e\x40' > /tmp/ttyB  # fake device sends
aseqdump -p <client>:0                                          # should show sysex + 2 notes
```
