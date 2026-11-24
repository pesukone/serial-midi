//! Minimal ALSA raw sequencer client: FFI surface and event layout verified
//! against alsa-lib `include/seq.h`, `seq_event.h` and `seqmid.h`.
//!
//! Only the handful of functions needed to expose one read port (device →
//! system) and one or more write ports (system → device) is declared; the
//! structs mirror the C ABI exactly (see the layout tests at the bottom).

use std::ffi::CString;
use std::io;
use std::os::raw::{c_char, c_int, c_short, c_uint, c_void};

// Event types (enum snd_seq_event_type, seq_event.h).
pub const EV_NOTEON: u8 = 6;
pub const EV_NOTEOFF: u8 = 7;
pub const EV_KEYPRESS: u8 = 8;
pub const EV_CONTROLLER: u8 = 10;
pub const EV_PGMCHANGE: u8 = 11;
pub const EV_CHANPRESS: u8 = 12;
pub const EV_PITCHBEND: u8 = 13;
pub const EV_CONTROL14: u8 = 14;
pub const EV_NONREGPARAM: u8 = 15;
pub const EV_REGPARAM: u8 = 16;
pub const EV_SONGPOS: u8 = 20;
pub const EV_SONGSEL: u8 = 21;
pub const EV_QFRAME: u8 = 22;
pub const EV_START: u8 = 30;
pub const EV_CONTINUE: u8 = 31;
pub const EV_STOP: u8 = 32;
pub const EV_CLOCK: u8 = 36;
pub const EV_TUNE_REQUEST: u8 = 40;
pub const EV_RESET: u8 = 41;
pub const EV_SENSING: u8 = 42;
pub const EV_SYSEX: u8 = 130;

/// Variable-length event flag (`SND_SEQ_EVENT_LENGTH_VARIABLE`).
const FL_LENGTH_VARIABLE: u8 = 1 << 2;

const ADDR_SUBSCRIBERS: u8 = 254; // SND_SEQ_ADDRESS_SUBSCRIBERS
const QUEUE_DIRECT: u8 = 253; // SND_SEQ_QUEUE_DIRECT

// Port capabilities / types (seq.h).
const PORT_CAP_READ: c_uint = 1 << 0;
const PORT_CAP_WRITE: c_uint = 1 << 1;
const PORT_CAP_SUBS_READ: c_uint = 1 << 5;
const PORT_CAP_SUBS_WRITE: c_uint = 1 << 6;
const PORT_TYPE_MIDI_GENERIC: c_uint = 1 << 1;
const PORT_TYPE_SYNTH: c_uint = 1 << 10;

const OPEN_DUPLEX: c_int = 3;

#[repr(C)]
#[derive(Copy, Clone)]
union Timestamp {
    tick: u32,
    time: [u32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Addr {
    pub client: u8,
    pub port: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct EvNote {
    channel: u8,
    note: u8,
    velocity: u8,
    off_velocity: u8,
    duration: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct EvCtrl {
    channel: u8,
    unused: [u8; 3],
    param: u32,
    value: i32,
}

/// `snd_seq_ev_ext_t` is `__attribute__((packed))`: len + pointer at offset
/// 4, 12 bytes total.
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct EvExt {
    len: u32,
    ptr: *mut c_void,
}

impl EvExt {
    /// Packed fields must not be borrowed directly (unaligned).
    unsafe fn get(&self) -> (u32, *mut c_void) {
        unsafe {
            (
                std::ptr::read_unaligned(std::ptr::addr_of!(self.len)),
                std::ptr::read_unaligned(std::ptr::addr_of!(self.ptr)),
            )
        }
    }
}

#[repr(C)]
union EventData {
    note: EvNote,
    control: EvCtrl,
    raw8: [u8; 12],
    ext: EvExt,
}

/// `snd_seq_event_t`: type/flags/tag/queue, timestamp, source, dest, data.
#[repr(C)]
pub struct SeqEvent {
    pub kind: u8,
    pub flags: u8,
    pub tag: u8,
    pub queue: u8,
    time: Timestamp,
    pub source: Addr,
    pub dest: Addr,
    data: EventData,
}

impl SeqEvent {
    /// A zeroed event ready for direct dispatch to all subscribers.
    fn new(kind: u8) -> Self {
        // SAFETY: SeqEvent contains only integers, pointers and unions
        // thereof; an all-zero bit pattern is valid, and every constructor
        // overwrites the data union member it reads back.
        let mut ev: Self = unsafe { std::mem::zeroed() };
        ev.kind = kind;
        ev.queue = QUEUE_DIRECT;
        ev.dest = Addr {
            client: ADDR_SUBSCRIBERS,
            port: 0,
        };
        ev
    }

    fn note(kind: u8, channel: u8, note: u8, velocity: u8) -> Self {
        let mut ev = Self::new(kind);
        ev.data = EventData {
            note: EvNote {
                channel,
                note,
                velocity,
                off_velocity: 0,
                duration: 0,
            },
        };
        ev
    }

    fn ctrl(kind: u8, channel: u8, param: u32, value: i32) -> Self {
        let mut ev = Self::new(kind);
        ev.data = EventData {
            control: EvCtrl {
                channel,
                unused: [0; 3],
                param,
                value,
            },
        };
        ev
    }

    /// Variable-length sysex event. `data` must stay alive across
    /// [`Seq::send_event`] (direct dispatch copies it synchronously).
    fn sysex(data: &[u8]) -> Self {
        let mut ev = Self::new(EV_SYSEX);
        ev.flags |= FL_LENGTH_VARIABLE;
        // Write the packed ext struct in place through the union (the event
        // must not contain a reference into itself).
        let ext = std::ptr::addr_of_mut!(ev.data) as *mut EvExt;
        unsafe {
            std::ptr::write_unaligned(
                ext,
                EvExt {
                    len: data.len() as u32,
                    ptr: data.as_ptr() as *mut c_void,
                },
            );
        }
        ev
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct SndSeq {
    _opaque: [u8; 0],
}

#[link(name = "asound")]
unsafe extern "C" {
    fn snd_seq_open(
        handle: *mut *mut SndSeq,
        name: *const c_char,
        streams: c_int,
        mode: c_int,
    ) -> c_int;
    fn snd_seq_close(handle: *mut SndSeq) -> c_int;
    fn snd_seq_set_client_name(handle: *mut SndSeq, name: *const c_char) -> c_int;
    fn snd_seq_client_id(handle: *mut SndSeq) -> c_int;
    fn snd_seq_create_simple_port(
        handle: *mut SndSeq,
        name: *const c_char,
        caps: c_uint,
        type_: c_uint,
    ) -> c_int;
    fn snd_seq_poll_descriptors_count(handle: *mut SndSeq, events: c_short) -> c_int;
    fn snd_seq_poll_descriptors(
        handle: *mut SndSeq,
        pfds: *mut libc::pollfd,
        space: c_uint,
        events: c_short,
    ) -> c_int;
    fn snd_seq_event_output_direct(handle: *mut SndSeq, ev: *mut SeqEvent) -> c_int;
    fn snd_seq_event_input(handle: *mut SndSeq, ev: *mut *mut SeqEvent) -> c_int;
    fn snd_seq_event_input_pending(handle: *mut SndSeq, fetch: c_int) -> c_int;
    fn snd_seq_free_event(ev: *mut SeqEvent) -> c_int;
}

fn err(code: c_int, ctx: &str) -> io::Error {
    let e = io::Error::from_raw_os_error(-code);
    io::Error::new(e.kind(), format!("{ctx}: {e}"))
}

/// Handle to an open ALSA sequencer client. Not `Sync`; share it through a
/// `Mutex` — both directions use it (the RX thread sends events, the TX loop
/// receives them).
pub struct Seq {
    handle: *mut SndSeq,
    client_id: u8,
}

// SAFETY: all uses of the raw handle are serialized by a Mutex in main.rs.
unsafe impl Send for Seq {}

impl Seq {
    /// Open the default sequencer client and set its display name.
    pub fn open(client_name: &str) -> io::Result<Self> {
        let mut handle: *mut SndSeq = std::ptr::null_mut();
        let conf = CString::new("default").unwrap();
        let rc = unsafe { snd_seq_open(&mut handle, conf.as_ptr(), OPEN_DUPLEX, 0) };
        if rc < 0 {
            return Err(err(rc, "cannot open ALSA sequencer (is snd-seq loaded?)"));
        }
        let name =
            CString::new(client_name).map_err(|_| io::Error::other("client name contains NUL"))?;
        let rc = unsafe { snd_seq_set_client_name(handle, name.as_ptr()) };
        if rc < 0 {
            unsafe { snd_seq_close(handle) };
            return Err(err(rc, "snd_seq_set_client_name"));
        }
        let client_id = (unsafe { snd_seq_client_id(handle) }).max(0) as u8;
        Ok(Self { handle, client_id })
    }

    pub fn client_id(&self) -> u8 {
        self.client_id
    }

    /// Create a port the system can read from (delivers device → host data).
    pub fn create_read_port(&self, name: &str) -> io::Result<u8> {
        self.create_port(
            name,
            PORT_CAP_READ | PORT_CAP_SUBS_READ,
            PORT_TYPE_MIDI_GENERIC | PORT_TYPE_SYNTH,
        )
    }

    /// Create a port the system can write to (feeds host → device data).
    pub fn create_write_port(&self, name: &str) -> io::Result<u8> {
        self.create_port(
            name,
            PORT_CAP_WRITE | PORT_CAP_SUBS_WRITE,
            PORT_TYPE_MIDI_GENERIC | PORT_TYPE_SYNTH,
        )
    }

    fn create_port(&self, name: &str, caps: c_uint, type_: c_uint) -> io::Result<u8> {
        let name = CString::new(name).map_err(|_| io::Error::other("port name contains NUL"))?;
        let rc = unsafe { snd_seq_create_simple_port(self.handle, name.as_ptr(), caps, type_) };
        if rc < 0 {
            return Err(err(rc, "snd_seq_create_simple_port"));
        }
        Ok(rc as u8)
    }

    /// The file descriptor to poll for incoming events.
    pub fn poll_fd(&self) -> io::Result<libc::pollfd> {
        let count = unsafe { snd_seq_poll_descriptors_count(self.handle, libc::POLLIN) };
        if count < 1 {
            return Err(io::Error::other("snd_seq_poll_descriptors_count"));
        }
        let mut pfds = vec![unsafe { std::mem::zeroed::<libc::pollfd>() }; count as usize];
        let rc = unsafe {
            snd_seq_poll_descriptors(self.handle, pfds.as_mut_ptr(), count as c_uint, libc::POLLIN)
        };
        if rc < 1 {
            return Err(io::Error::other("snd_seq_poll_descriptors"));
        }
        Ok(pfds[0])
    }

    /// Dispatch one event directly to all subscribers of its destination.
    pub fn send_event(&self, ev: &mut SeqEvent) -> io::Result<()> {
        ev.source.client = self.client_id;
        let rc = unsafe { snd_seq_event_output_direct(self.handle, ev) };
        if rc < 0 {
            return Err(err(rc, "snd_seq_event_output_direct"));
        }
        Ok(())
    }

    /// Send a note-type event out of read port `port`.
    pub fn send_note(&self, kind: u8, port: u8, ch: u8, note: u8, vel: u8) -> io::Result<()> {
        self.send_event(&mut SeqEvent::note(kind, ch, note, vel).with_source(port))
    }

    /// Send a controller-type event out of read port `port`.
    pub fn send_ctrl(&self, kind: u8, port: u8, ch: u8, param: u32, value: i32) -> io::Result<()> {
        self.send_event(&mut SeqEvent::ctrl(kind, ch, param, value).with_source(port))
    }

    /// Send an argument-less event (real-time, tune request, reset).
    pub fn send_bare(&self, kind: u8, port: u8) -> io::Result<()> {
        self.send_event(&mut SeqEvent::new(kind).with_source(port))
    }

    /// Send a complete sysex buffer out of read port `port`.
    pub fn send_sysex(&self, port: u8, data: &[u8]) -> io::Result<()> {
        let mut ev = SeqEvent::sysex(data);
        ev.source.port = port;
        self.send_event(&mut ev)
    }

    /// Drain all pending received events, calling `f` for each. Variable
    /// length (sysex) events additionally get their payload bytes, valid for
    /// the duration of the callback only.
    pub fn receive<F>(&self, mut f: F) -> io::Result<()>
    where
        F: FnMut(&SeqEvent, Option<&[u8]>),
    {
        loop {
            // fetch=1: pull anything readable on the sequencer fd into the
            // local pool first; otherwise pending would stay 0 even when
            // poll() reported readable data.
            let pending = unsafe { snd_seq_event_input_pending(self.handle, 1) };
            if pending <= 0 {
                return Ok(());
            }
            let mut ev: *mut SeqEvent = std::ptr::null_mut();
            let rc = unsafe { snd_seq_event_input(self.handle, &mut ev) };
            if rc < 0 {
                if rc == -libc::EAGAIN {
                    return Ok(());
                }
                return Err(err(rc, "snd_seq_event_input"));
            }
            if ev.is_null() {
                return Ok(());
            }
            {
                let ev_ref = unsafe { &*ev };
                let payload = if ev_ref.flags & FL_LENGTH_VARIABLE != 0 {
                    let (len, ptr) = unsafe { ev_ref.data.ext.get() };
                    if ptr.is_null() || len == 0 {
                        None
                    } else {
                        Some(unsafe {
                            std::slice::from_raw_parts(ptr as *const u8, len as usize)
                        })
                    }
                } else {
                    None
                };
                f(ev_ref, payload);
            }
            unsafe { snd_seq_free_event(ev) };
        }
    }
}

impl Drop for Seq {
    fn drop(&mut self) {
        unsafe { snd_seq_close(self.handle) };
    }
}

trait WithSource {
    fn with_source(self, port: u8) -> Self;
}

impl WithSource for SeqEvent {
    fn with_source(mut self, port: u8) -> Self {
        self.source.port = port;
        self
    }
}

/// What an incoming sequencer event means on the wire.
pub enum Wire<'a> {
    /// MIDI bytes of a channel-voice / system message.
    Bytes(&'a [u8]),
    /// Nothing to send (queue management, user events, ...).
    Ignore,
}

/// Render an incoming event into MIDI wire bytes, using no running status
/// (exactly like the Windows driver's `MODM_DATA` path). `scratch` is reused
/// for every short message.
pub fn event_to_wire<'a>(
    ev: &'a SeqEvent,
    sysex: Option<&'a [u8]>,
    scratch: &'a mut [u8; 12],
) -> Wire<'a> {
    // SAFETY: each data union member is only read for event types whose
    // documented data type is that member (seq_event.h).
    let (note, ctrl) = unsafe { (ev.data.note, ev.data.control) };
    let ch = ctrl.channel & 0x0f;
    let nch = note.channel & 0x0f;
    match ev.kind {
        EV_NOTEON => fixed(scratch, &[0x90 | nch, note.note, note.velocity]),
        EV_NOTEOFF => fixed(scratch, &[0x80 | nch, note.note, note.velocity]),
        EV_KEYPRESS => fixed(scratch, &[0xA0 | nch, note.note, note.velocity]),
        EV_CONTROLLER => fixed(scratch, &[0xB0 | ch, ctrl.param as u8, ctrl.value as u8]),
        EV_PGMCHANGE => fixed(scratch, &[0xC0 | ch, ctrl.value as u8]),
        EV_CHANPRESS => fixed(scratch, &[0xD0 | ch, ctrl.value as u8]),
        EV_PITCHBEND => {
            let v = (ctrl.value + 8192).clamp(0, 16383) as u16;
            fixed(scratch, &[0xE0 | ch, (v & 0x7f) as u8, (v >> 7) as u8])
        }
        // 14-bit CC as an explicit MSB/LSB CC pair (LSB number = MSB + 32).
        EV_CONTROL14 => {
            let lsb_param = (ctrl.param as u8).wrapping_add(32);
            fixed(
                scratch,
                &[
                    0xB0 | ch,
                    ctrl.param as u8,
                    (ctrl.value >> 7) as u8,
                    0xB0 | ch,
                    lsb_param,
                    (ctrl.value & 0x7f) as u8,
                ],
            )
        }
        // NRPN / RPN: param pair + data entry pair, as four CC messages.
        EV_NONREGPARAM | EV_REGPARAM => {
            let (msb, lsb) = if ev.kind == EV_REGPARAM { (100, 98) } else { (101, 99) };
            fixed(
                scratch,
                &[
                    0xB0 | ch,
                    msb,
                    (ctrl.param >> 7) as u8,
                    0xB0 | ch,
                    lsb,
                    (ctrl.param & 0x7f) as u8,
                    0xB0 | ch,
                    6, // data entry MSB
                    (ctrl.value >> 7) as u8,
                    0xB0 | ch,
                    38, // data entry LSB
                    (ctrl.value & 0x7f) as u8,
                ],
            )
        }
        EV_SONGPOS => fixed(
            scratch,
            &[0xF2, (ctrl.param & 0x7f) as u8, (ctrl.param >> 7) as u8],
        ),
        EV_SONGSEL => fixed(scratch, &[0xF3, ctrl.param as u8]),
        EV_QFRAME => fixed(scratch, &[0xF1, ctrl.param as u8]),
        EV_CLOCK => fixed(scratch, &[0xF8]),
        EV_START => fixed(scratch, &[0xFA]),
        EV_CONTINUE => fixed(scratch, &[0xFB]),
        EV_STOP => fixed(scratch, &[0xFC]),
        EV_TUNE_REQUEST => fixed(scratch, &[0xF6]),
        EV_SENSING => fixed(scratch, &[0xFE]),
        EV_RESET => fixed(scratch, &[0xF0]),
        EV_SYSEX => match sysex {
            Some(b) => Wire::Bytes(b),
            None => Wire::Ignore,
        },
        _ => Wire::Ignore,
    }
}

fn fixed<'a>(scratch: &'a mut [u8; 12], msg: &[u8]) -> Wire<'a> {
    scratch[..msg.len()].copy_from_slice(msg);
    Wire::Bytes(&scratch[..msg.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_struct_matches_c_abi() {
        // offsetof-ish checks against seq_event.h; ext is packed, so the
        // data union has alignment 8 only via nothing larger than u32 →
        // align 4? No: raw8/control/queue are 4-aligned, ext is 1-aligned
        // (packed) → union align 4, size 12; event size 28.
        assert_eq!(std::mem::size_of::<SeqEvent>(), 28);
        assert_eq!(std::mem::align_of::<SeqEvent>(), 4);
        assert_eq!(std::mem::offset_of!(SeqEvent, queue), 3);
        assert_eq!(std::mem::offset_of!(SeqEvent, time), 4);
        assert_eq!(std::mem::offset_of!(SeqEvent, source), 12);
        assert_eq!(std::mem::offset_of!(SeqEvent, dest), 14);
        assert_eq!(std::mem::offset_of!(SeqEvent, data), 16);
        assert_eq!(std::mem::size_of::<EvExt>(), 12);
        assert_eq!(std::mem::offset_of!(EvExt, ptr), 4);
        assert_eq!(std::mem::size_of::<EvCtrl>(), 12);
    }
}
