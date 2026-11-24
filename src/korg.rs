//! Korg "PC I/F" serial wire protocol (NS5R "TO HOST" port at 38400 8N1).
//!
//! Reverse engineered from the Windows driver `KORGSMDN.DRV`; see
//! `docs/serial-protocol.md` for the evidence. Host→device traffic consists of
//! a device-select escape (`F5 <dev>`, emitted whenever the target logical
//! port changes) followed by the payload, either chopped into `FF` + ≤4-byte
//! chunks (devices 0/1) or written raw (devices 2/3). Device→host traffic is
//! a plain, unframed MIDI byte stream, parsed by [`MidiParser`].

use std::io::{self, Write};

/// A complete MIDI message received from the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// A channel-voice or system-common/real-time message, delivered as
    /// finished bytes (1–3, running status already resolved).
    Short { bytes: [u8; 3], len: u8 },
    /// A complete system-exclusive message, including the leading `F0` and
    /// trailing `F7`.
    Sysex(Vec<u8>),
}

impl Message {
    /// The message bytes (for `Sysex`, borrow the payload instead).
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Message::Short { bytes, len } => &bytes[..*len as usize],
            Message::Sysex(v) => v,
        }
    }
}

/// Longest sysex we accept; a longer one is silently discarded (the device
/// should never produce anything near this).
const MAX_SYSEX: usize = 64 * 1024;

/// Streaming MIDI byte-stream → [`Message`] parser, matching the behaviour of
/// the Windows driver's `ParseInByte` (`docs/korg-pc-if-driver.md` § MIDI IN
/// receive path): running status supported, real-time messages (`≥ F8`) are
/// emitted immediately even mid-message or mid-sysex.
#[derive(Default)]
pub struct MidiParser {
    /// Current running status byte (0 = none).
    running: u8,
    /// Data bytes a fresh message under `running` would expect.
    run_expect: u8,
    /// Data bytes still expected for the message being collected.
    expect: u8,
    /// Collected bytes: `[status, data...]`.
    buf: [u8; 3],
    /// Number of bytes collected in `buf`.
    got: u8,
    in_sysex: bool,
    sysex: Vec<u8>,
}

impl MidiParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one received byte; returns the message completed by it, if any.
    pub fn push(&mut self, b: u8) -> Option<Message> {
        // System real-time: delivered immediately, never disturbs state.
        if b >= 0xF8 {
            if self.in_sysex && self.sysex.len() < MAX_SYSEX {
                self.sysex.push(b);
            }
            return Some(Message::Short {
                bytes: [b, 0, 0],
                len: 1,
            });
        }

        if b >= 0x80 {
            // A status byte arriving mid-sysex terminates the sysex (the
            // driver emits an error there; we simply discard it).
            let was_in_sysex = std::mem::take(&mut self.in_sysex);
            let pending_sysex = std::mem::take(&mut self.sysex);

            // Stand-alone system messages.
            if b == 0xF0 {
                self.in_sysex = true;
                self.sysex.push(0xF0);
                self.expect = 0;
                self.got = 0;
                return None;
            }
            if b == 0xF7 {
                self.expect = 0;
                self.got = 0;
                if was_in_sysex {
                    let mut msg = pending_sysex;
                    msg.push(0xF7);
                    return Some(Message::Sysex(msg));
                }
                return None; // unmatched F7: discard
            }
            if matches!(b, 0xF4 | 0xF5 | 0xF6) {
                // F4/F5 undefined, F6 tune request: no data bytes.
                self.expect = 0;
                self.got = 0;
                return Some(Message::Short {
                    bytes: [b, 0, 0],
                    len: 1,
                });
            }

            let expect = match b {
                0x80..=0xBF | 0xE0..=0xEF => 2,
                0xC0..=0xDF => 1,
                0xF1 | 0xF3 => 1, // MTC quarter frame, song select
                0xF2 => 2,        // song position
                _ => 0,
            };
            if b < 0xF0 {
                self.running = b;
                self.run_expect = expect;
            }
            self.buf = [b, 0, 0];
            self.got = 1;
            self.expect = expect;
            if expect == 0 {
                self.got = 0;
                return Some(Message::Short {
                    bytes: self.buf,
                    len: 1,
                });
            }
            return None;
        }

        // Data byte.
        if self.in_sysex {
            if self.sysex.len() >= MAX_SYSEX {
                self.in_sysex = false;
                self.sysex.clear();
            } else {
                self.sysex.push(b);
            }
            return None;
        }
        if self.expect == 0 || self.got == 0 {
            return None; // orphan data byte
        }
        self.buf[self.got as usize] = b;
        self.got += 1;
        self.expect -= 1;
        if self.expect == 0 {
            let len = self.got;
            let bytes = self.buf;
            if self.running < 0xF0 {
                // Running status: collection restarts with the status byte.
                self.buf = [self.running, 0, 0];
                self.got = 1;
                self.expect = self.run_expect;
            } else {
                self.got = 0;
                self.expect = 0;
            }
            Some(Message::Short { bytes, len })
        } else {
            None
        }
    }
}

/// Host→device framing writer, reproducing `SerialWriteFramed` +
/// `SelectActiveDevice` from the driver: a process-global cache of the last
/// device written to (like the driver's `0x69f05010` word, initially unset so
/// the first write always selects), `F5 <dev>` on every change, and `FF` +
/// ≤4-byte payload chunks for devices 0/1.
pub struct KorgLink<W: Write> {
    w: W,
    last_dev: Option<u8>,
}

impl<W: Write> KorgLink<W> {
    pub fn new(w: W) -> Self {
        Self { w, last_dev: None }
    }

    /// Write `payload` MIDI bytes to logical device `dev`
    /// (0 = Default MIDI, 1 = MIDI Out, 2 = Synth-A, 3 = Synth-B).
    pub fn send(&mut self, dev: u8, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        if self.last_dev != Some(dev) {
            self.w.write_all(&[0xF5, dev])?;
            self.w.flush()?;
            self.last_dev = Some(dev);
        }
        if dev <= 1 {
            // Chunked payload: one FF byte + 1..4 payload bytes per chunk,
            // packed by length only — message boundaries are not respected
            // (this matches the driver exactly).
            let n = payload.len() + payload.len().div_ceil(4);
            let mut frame = Vec::with_capacity(n);
            for chunk in payload.chunks(4) {
                frame.push(0xFF);
                frame.extend_from_slice(chunk);
            }
            self.w.write_all(&frame)?;
        } else {
            self.w.write_all(payload)?;
        }
        self.w.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_write_selects_device() {
        let mut out = Vec::new();
        {
            let mut link = KorgLink::new(&mut out);
            link.send(0, &[0xC0, 0x05]).unwrap();
        }
        assert_eq!(out, [0xF5, 0x00, 0xFF, 0xC0, 0x05]);
    }

    #[test]
    fn example_from_protocol_doc() {
        // C0 05 B0 00 00 (5 bytes) to device 0, nothing selected yet:
        // F5 00 FF C0 05 B0 00 FF 00
        let mut out = Vec::new();
        {
            let mut link = KorgLink::new(&mut out);
            link.send(0, &[0xC0, 0x05, 0xB0, 0x00, 0x00]).unwrap();
        }
        assert_eq!(out, [0xF5, 0x00, 0xFF, 0xC0, 0x05, 0xB0, 0x00, 0xFF, 0x00]);
    }

    #[test]
    fn reselect_only_on_change() {
        let mut out = Vec::new();
        {
            let mut link = KorgLink::new(&mut out);
            link.send(0, &[0x90, 0x3C, 0x7F]).unwrap();
            link.send(0, &[0x80, 0x3C, 0x00]).unwrap();
            link.send(2, &[0xB0, 0x07, 0x64]).unwrap(); // Synth-A: raw
            link.send(2, &[0x90, 0x40, 0x7F]).unwrap(); // same device: no F5
            link.send(0, &[0xF8]).unwrap(); // back to dev 0
        }
        assert_eq!(
            out,
            [
                0xF5, 0x00, 0xFF, 0x90, 0x3C, 0x7F, // select 0, one chunk
                0xFF, 0x80, 0x3C, 0x00, // still dev 0
                0xF5, 0x02, 0xB0, 0x07, 0x64, // select 2, raw
                0x90, 0x40, 0x7F, // still dev 2, raw
                0xF5, 0x00, 0xFF, 0xF8, // select 0 again, realtime chunked
            ]
        );
    }

    #[test]
    fn chunks_are_4_bytes_max_across_messages() {
        let mut out = Vec::new();
        {
            let mut link = KorgLink::new(&mut out);
            link.send(1, &[0x90, 0x3C, 0x7F, 0x90, 0x3E, 0x7F]).unwrap();
        }
        assert_eq!(
            out,
            [0xF5, 0x01, 0xFF, 0x90, 0x3C, 0x7F, 0x90, 0xFF, 0x3E, 0x7F]
        );
    }

    fn parse(bytes: &[u8]) -> Vec<Message> {
        let mut p = MidiParser::new();
        bytes
            .iter()
            .filter_map(|&b| p.push(b))
            .collect::<Vec<_>>()
    }

    #[test]
    fn short_messages_and_running_status() {
        let msgs = parse(&[
            0x90, 0x3C, 0x7F, // note on ch0
            0x3E, 0x40, // running status note on
            0x80, 0x3C, 0x00, // note off
            0xC2, 0x05, // program change ch2
            0x07, // running status program change
            0xE1, 0x00, 0x40, // pitch bend ch1
        ]);
        assert_eq!(
            msgs,
            vec![
                Message::Short {
                    bytes: [0x90, 0x3C, 0x7F],
                    len: 3
                },
                Message::Short {
                    bytes: [0x90, 0x3E, 0x40],
                    len: 3
                },
                Message::Short {
                    bytes: [0x80, 0x3C, 0x00],
                    len: 3
                },
                Message::Short {
                    bytes: [0xC2, 0x05, 0],
                    len: 2
                },
                Message::Short {
                    bytes: [0xC2, 0x07, 0],
                    len: 2
                },
                Message::Short {
                    bytes: [0xE1, 0x00, 0x40],
                    len: 3
                },
            ]
        );
    }

    #[test]
    fn realtime_interleaves_anywhere() {
        let msgs = parse(&[0x90, 0xF8, 0x3C, 0xFE, 0x7F, 0xFA]);
        assert_eq!(
            msgs,
            vec![
                Message::Short {
                    bytes: [0xF8, 0, 0],
                    len: 1
                },
                Message::Short {
                    bytes: [0xFE, 0, 0],
                    len: 1
                },
                Message::Short {
                    bytes: [0x90, 0x3C, 0x7F],
                    len: 3
                },
                Message::Short {
                    bytes: [0xFA, 0, 0],
                    len: 1
                },
            ]
        );
    }

    #[test]
    fn sysex_and_interruptions() {
        let msgs = parse(&[0xF0, 0x42, 0x30, 0x01, 0xF7]);
        assert_eq!(msgs, vec![Message::Sysex(vec![0xF0, 0x42, 0x30, 0x01, 0xF7])]);

        // Sysex interrupted by a status byte: sysex discarded, status used.
        let msgs = parse(&[0xF0, 0x42, 0x90, 0x3C, 0x7F]);
        assert_eq!(
            msgs,
            vec![Message::Short {
                bytes: [0x90, 0x3C, 0x7F],
                len: 3
            }]
        );

        // Real-time inside sysex: delivered, sysex continues.
        let msgs = parse(&[0xF0, 0xF8, 0x42, 0xF7]);
        assert_eq!(
            msgs,
            vec![
                Message::Short {
                    bytes: [0xF8, 0, 0],
                    len: 1
                },
                Message::Sysex(vec![0xF0, 0xF8, 0x42, 0xF7]),
            ]
        );

        // Orphan data bytes and unmatched F7 are dropped.
        assert!(parse(&[0x42, 0xF7, 0x90, 0x01, 0x02]).eq(&[Message::Short {
            bytes: [0x90, 0x01, 0x02],
            len: 3
        }]));
    }

    #[test]
    fn system_common_with_data() {
        let msgs = parse(&[0xF2, 0x00, 0x20, 0xF1, 0x03, 0xF3, 0x09, 0xF6]);
        assert_eq!(
            msgs,
            vec![
                Message::Short {
                    bytes: [0xF2, 0x00, 0x20],
                    len: 3
                },
                Message::Short {
                    bytes: [0xF1, 0x03, 0],
                    len: 2
                },
                Message::Short {
                    bytes: [0xF3, 0x09, 0],
                    len: 2
                },
                Message::Short {
                    bytes: [0xF6, 0, 0],
                    len: 1
                },
            ]
        );
    }
}

