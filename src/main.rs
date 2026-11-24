//! serial-midi — expose a Korg NS5R on its serial "TO HOST" port as an ALSA
//! MIDI device, in the spirit of ttymidi.
//!
//! Wire protocol (see `docs/serial-protocol.md`): 38400 8N1; host→device is
//! an `F5 <dev>` device select plus `FF`-chunked payload for the "Default
//! MIDI" and "MIDI Out" logical devices and raw bytes for Synth-A/B;
//! device→host is plain MIDI. The framing and the process-global
//! device-select cache reproduce the Windows driver `KORGSMDN.DRV`.

mod alsaseq;
mod korg;
mod serial;

use std::io::Read;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;

use alsaseq::{Seq, Wire};
use korg::{KorgLink, Message, MidiParser};
use serial::SerialPort;

/// Hardcoded per the driver: `baud=38400 parity=N data=8 stop=1`
/// (docs/serial-protocol.md, and NS5R Global [041] BPS Select = 38.4).
const BAUD: u32 = 38400;

/// Logical device indices as used in the `F5 <dev>` select.
const DEV_DEFAULT_MIDI: u8 = 0;
const DEV_MIDI_OUT: u8 = 1;
const DEV_SYNTH_A: u8 = 2;
const DEV_SYNTH_B: u8 = 3;

struct Args {
    serial_path: String,
    client_name: String,
    independent: bool,
    verbose: bool,
}

fn usage(program: &str) -> ! {
    eprintln!(
        "usage: {program} [OPTIONS] <SERIAL-DEVICE>

Expose a Korg NS5R (or other Korg \"PC I/F\" instrument) connected to
<SERIAL-DEVICE> (e.g. /dev/ttyS0 or /dev/ttyUSB0) as an ALSA MIDI device.
The link runs at 38400 baud, 8N1; set NS5R Global [041] BPS Select to 38.4.

options:
  --name NAME        ALSA client name (default: Korg NS5R)
  --independent      expose all four MIDI OUT ports (Default MIDI, MIDI Out,
                     Synth-A, Synth-B) like the driver's \"Independent Synth /
                     MIDI Out\" mode, instead of just \"Default MIDI\"
  -v, --verbose      log MIDI traffic to stderr
  -h, --help         show this help

connect with e.g.  aconnect <client> <this-client>:1",
        program = program,
    );
    std::process::exit(2);
}

fn parse_args() -> Args {
    let mut args = Args {
        serial_path: String::new(),
        client_name: "Korg NS5R".into(),
        independent: false,
        verbose: false,
    };
    let mut it = std::env::args();
    let program = it.next().unwrap_or_else(|| "serial-midi".into());
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => {
                args.client_name = it.next().unwrap_or_else(|| usage(&program));
            }
            "--independent" => args.independent = true,
            "-v" | "--verbose" => args.verbose = true,
            "-h" | "--help" => usage(&program),
            s if s.starts_with("--") => {
                eprintln!("unknown option: {s}");
                usage(&program);
            }
            s if args.serial_path.is_empty() => args.serial_path = s.to_string(),
            s => {
                eprintln!("unexpected argument: {s}");
                usage(&program);
            }
        }
    }
    if args.serial_path.is_empty() {
        usage(&program);
    }
    args
}

fn main() -> ExitCode {
    let args = parse_args();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("serial-midi: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> std::io::Result<()> {
    let serial = SerialPort::open_baud(&args.serial_path, BAUD)
        .map_err(|e| std::io::Error::other(format!("{}: {e}", args.serial_path)))?;

    let seq = Seq::open(&args.client_name)?;
    let rx_port = seq.create_read_port("KORG PC I/F MIDI Port")?;

    // Write ports: a single "Default MIDI" device, or all four (the driver's
    // "Independent Synth / MIDI Out" mode). Port ids map to F5 device
    // indices for framing.
    let out_ports: &[(&str, u8)] = if args.independent {
        &[
            ("Default MIDI", DEV_DEFAULT_MIDI),
            ("MIDI Out", DEV_MIDI_OUT),
            ("Synth-A", DEV_SYNTH_A),
            ("Synth-B", DEV_SYNTH_B),
        ]
    } else {
        &[("KORG PC I/F MIDI Port", DEV_DEFAULT_MIDI)]
    };
    let mut write_ports = Vec::new();
    for (name, dev) in out_ports {
        let port = seq.create_write_port(name)?;
        write_ports.push((port, *dev));
    }

    let client_id = seq.client_id();
    eprintln!(
        "{client_id}:0 \"{}\" — MIDI IN (from instrument)",
        args.client_name
    );
    for (i, (name, dev)) in out_ports.iter().enumerate() {
        eprintln!(
            "{client_id}:{} \"{name}\" — MIDI OUT to instrument (F5 device {dev})",
            i + 1
        );
    }

    let seq = Arc::new(Mutex::new(seq));

    // ---- Device → host: parse serial MIDI, emit events on the read port.
    let rx_seq = Arc::clone(&seq);
    let rx_path = args.serial_path.clone();
    let mut rx_serial = serial.try_clone()?;
    let verbose = args.verbose;
    thread::spawn(move || {
        let mut parser = MidiParser::new();
        let mut buf = [0u8; 256];
        loop {
            match rx_serial.read(&mut buf) {
                // Ok(0) is just the 100 ms read timeout; a real device
                // removal surfaces as an error (usually EIO).
                Ok(0) => continue,
                Ok(n) => {
                    let seq = rx_seq.lock().unwrap_or_else(|p| p.into_inner());
                    for &b in &buf[..n] {
                        let Some(msg) = parser.push(b) else { continue };
                        if verbose {
                            eprintln!("in   {}", hex(msg.as_bytes()));
                        }
                        if let Err(e) = emit(&seq, &msg, rx_port) {
                            eprintln!("serial-midi: {e}");
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("serial-midi: reading {rx_path}: {e}");
                    break;
                }
            }
        }
    });

    // ---- Host → device: receive events on the write ports, frame them onto
    // the wire. KorgLink owns the process-global F5 device-select cache.
    let mut link = KorgLink::new(serial);
    let mut scratch = [0u8; 12];
    let mut pfds = {
        let guard = seq.lock().unwrap_or_else(|p| p.into_inner());
        let mut p = vec![guard.poll_fd()?];
        p[0].events = libc::POLLIN;
        p
    };

    loop {
        let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, 200) };
        if rc < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if rc == 0 {
            continue;
        }
        // The seq poll fd only ever signals POLLIN; revents is not inspected.
        let guard = seq.lock().unwrap_or_else(|p| p.into_inner());
        let res = guard.receive(|ev, payload| {
            // Which of our write ports did this arrive on?
            let Some(&(_, dev)) = write_ports.iter().find(|(port, _)| *port == ev.dest.port) else {
                return;
            };
            match alsaseq::event_to_wire(ev, payload, &mut scratch) {
                Wire::Ignore => {}
                Wire::Bytes(bytes) => {
                    if verbose {
                        eprintln!("out d{dev} {}", hex(bytes));
                    }
                    if let Err(e) = link.send(dev, bytes) {
                        eprintln!("serial-midi: writing serial port: {e}");
                    }
                }
            }
        });
        drop(guard);
        res?;
    }
}

/// Deliver one parsed MIDI message as an ALSA event on read port `port`.
fn emit(seq: &Seq, msg: &Message, port: u8) -> std::io::Result<()> {
    match msg {
        Message::Sysex(v) => seq.send_sysex(port, v),
        Message::Short { bytes, len } => {
            let (status, d1, d2) = (bytes[0], bytes[1], bytes[2]);
            let ch = status & 0x0f;
            match status {
                0x80..=0xaf => seq.send_note(
                    alsaseq_ev_of_status(status),
                    port,
                    ch,
                    d1,
                    d2,
                ),
                0xb0..=0xbf => seq.send_ctrl(
                    alsaseq::EV_CONTROLLER,
                    port,
                    ch,
                    d1 as u32,
                    d2 as i32,
                ),
                0xc0..=0xdf => {
                    debug_assert_eq!(*len, 2);
                    seq.send_ctrl(alsaseq_ev_of_status(status), port, ch, 0, d1 as i32)
                }
                0xe0..=0xef => {
                    let v = ((d2 as i32) << 7 | d1 as i32) - 8192;
                    seq.send_ctrl(alsaseq::EV_PITCHBEND, port, ch, 0, v)
                }
                0xf2 => seq.send_ctrl(
                    alsaseq::EV_SONGPOS,
                    port,
                    0,
                    (d2 as u32) << 7 | d1 as u32,
                    0,
                ),
                0xf1 | 0xf3 => seq.send_ctrl(alsaseq_ev_of_status(status), port, 0, d1 as u32, 0),
                // F4/F5 are undefined; the parser never completes them as
                // shorts, but stay defensive.
                0xf4 | 0xf5 => Ok(()),
                _ => seq.send_bare(alsaseq_ev_of_status(status), port),
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Map a MIDI status byte to the ALSA sequencer event type to deliver it as.
fn alsaseq_ev_of_status(status: u8) -> u8 {
    match status {
        0x80..=0x8f => alsaseq::EV_NOTEOFF,
        0x90..=0x9f => alsaseq::EV_NOTEON,
        0xa0..=0xaf => alsaseq::EV_KEYPRESS,
        0xb0..=0xbf => alsaseq::EV_CONTROLLER,
        0xc0..=0xcf => alsaseq::EV_PGMCHANGE,
        0xd0..=0xdf => alsaseq::EV_CHANPRESS,
        0xe0..=0xef => alsaseq::EV_PITCHBEND,
        0xf1 => alsaseq::EV_QFRAME,
        0xf2 => alsaseq::EV_SONGPOS,
        0xf3 => alsaseq::EV_SONGSEL,
        0xf6 => alsaseq::EV_TUNE_REQUEST,
        0xf8 => alsaseq::EV_CLOCK,
        0xfa => alsaseq::EV_START,
        0xfb => alsaseq::EV_CONTINUE,
        0xfc => alsaseq::EV_STOP,
        0xfe => alsaseq::EV_SENSING,
        _ => alsaseq::EV_SYSEX, // F0/F7 never arrive here
    }
}
