//! Serial port setup: raw 8N1 at the hardcoded link speed, via termios.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};

/// A raw RS-232 link (no flow control, no echo), exclusive to this process.
pub struct SerialPort {
    file: File,
}

impl SerialPort {
    /// Open `path` (e.g. `/dev/ttyUSB0`) as raw 8N1 at `baud`.
    ///
    /// Reads are non-blocking-with-timeout: `read` returns `Ok(0)` after
    /// ~100 ms without data, so both pump loops stay responsive.
    pub fn open_baud(path: &str, baud: u32) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            // O_NOCTTY: never make the modem line our controlling terminal.
            .custom_flags(libc::O_NOCTTY)
            .open(path)?;
        configure(file.as_raw_fd(), baud)?;
        Ok(Self { file })
    }

    /// Duplicate the descriptor (independent userspace handle, same link).
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
        })
    }
}

fn configure(fd: RawFd, baud: u32) -> io::Result<()> {
    let speed = match baud {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported baud rate {baud}"),
            ));
        }
    };
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd is a valid terminal descriptor; termios is the right struct.
    unsafe {
        if libc::tcgetattr(fd, &mut t) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut t);
        // 8N1, receiver enabled, ignore modem status lines.
        t.c_cflag &= !(libc::CSIZE | libc::PARENB | libc::CSTOPB | libc::CRTSCTS);
        t.c_cflag |= libc::CS8 | libc::CLOCAL | libc::CREAD;
        if libc::cfsetispeed(&mut t, speed) != 0 || libc::cfsetospeed(&mut t, speed) != 0 {
            return Err(io::Error::last_os_error());
        }
        // read() returns 0 after 100 ms with no data.
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 1;
        if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
            return Err(io::Error::last_os_error());
        }
        // Drop anything the driver left in the buffers.
        libc::tcflush(fd, libc::TCIOFLUSH);
    }
    Ok(())
}

impl io::Read for SerialPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.file, buf)
    }
}

impl io::Write for SerialPort {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut self.file, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        // No-op: the kernel TTY line discipline buffers the bytes and the
        // UART drains at wire speed. Draining per frame (tcdrain) would
        // stall the pump at ~1 message per frame time.
        Ok(())
    }
}
