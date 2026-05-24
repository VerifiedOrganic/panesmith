//! Crossterm-backed terminal control and raw stdio attach helpers.

use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{
    EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use panesmith_core::{
    AttachScreenPolicy, PaneAttachInputChunk, PaneAttachTerminal, PaneAttachTerminalControl, Size,
};

use crate::{AttachInputChunk, AttachTerminal, HostTerminalControl, TerminalRestoreToken};

const ATTACH_SAVE_MOUSE_MODES: &[u8] = b"\x1b[?1000s\x1b[?1002s\x1b[?1003s\x1b[?1015s\x1b[?1006s";
const ATTACH_RESTORE_MOUSE_MODES: &[u8] =
    b"\x1b[?1006r\x1b[?1015r\x1b[?1003r\x1b[?1002r\x1b[?1000r";
const WRITE_RETRY_SLEEP: Duration = Duration::from_millis(1);

fn retrying_write_all<W>(writer: &mut W, mut bytes: &[u8]) -> io::Result<()>
where
    W: Write,
{
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write attach bytes",
                ));
            }
            Ok(written) => bytes = &bytes[written..],
            Err(error) if is_retryable_write_error(&error) => thread::sleep(WRITE_RETRY_SLEEP),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn retrying_flush<W>(writer: &mut W) -> io::Result<()>
where
    W: Write,
{
    loop {
        match writer.flush() {
            Ok(()) => return Ok(()),
            Err(error) if is_retryable_write_error(&error) => thread::sleep(WRITE_RETRY_SLEEP),
            Err(error) => return Err(error),
        }
    }
}

fn is_retryable_write_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

/// Raw-mode operations used by [`CrosstermTerminalControl`].
pub trait RawModeOps {
    /// Returns whether raw mode is already enabled.
    fn is_raw_mode_enabled(&self) -> io::Result<bool>;

    /// Enables raw mode.
    fn enable_raw_mode(&self) -> io::Result<()>;

    /// Disables raw mode.
    fn disable_raw_mode(&self) -> io::Result<()>;
}

/// System raw-mode operations backed by `crossterm::terminal`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemRawModeOps;

impl RawModeOps for SystemRawModeOps {
    fn is_raw_mode_enabled(&self) -> io::Result<bool> {
        terminal::is_raw_mode_enabled()
    }

    fn enable_raw_mode(&self) -> io::Result<()> {
        terminal::enable_raw_mode()
    }

    fn disable_raw_mode(&self) -> io::Result<()> {
        terminal::disable_raw_mode()
    }
}

/// Crossterm-backed host terminal suspend/restore helper.
///
/// This is a convenience implementation for simple crossterm hosts. It saves
/// and restores the terminal modes it manages, but it is not a replacement for
/// an embedding TUI's own terminal-mode stack. Hosts that already own a precise
/// raw-mode, alternate-screen, mouse, bracketed-paste, keyboard-enhancement,
/// cursor, or redraw profile should implement [`HostTerminalControl`] or
/// [`PaneAttachTerminalControl`] directly and restore that profile exactly.
#[derive(Debug, Clone)]
pub struct CrosstermTerminalControl<W, M = SystemRawModeOps> {
    writer: W,
    raw_mode_ops: M,
    host_uses_alternate_screen: bool,
}

impl<W> CrosstermTerminalControl<W, SystemRawModeOps>
where
    W: Write,
{
    /// Creates a crossterm terminal control using the system raw-mode hooks.
    pub fn new(writer: W) -> Self {
        Self::with_raw_mode_ops(writer, SystemRawModeOps)
    }
}

impl<W, M> CrosstermTerminalControl<W, M>
where
    W: Write,
    M: RawModeOps,
{
    /// Creates a crossterm terminal control with explicit raw-mode hooks.
    pub fn with_raw_mode_ops(writer: W, raw_mode_ops: M) -> Self {
        Self {
            writer,
            raw_mode_ops,
            host_uses_alternate_screen: false,
        }
    }

    /// Marks whether the host UI is already running in the alternate screen.
    ///
    /// This only informs the generic helper's screen-policy decisions. It does
    /// not describe the rest of an embedded TUI's terminal profile.
    pub fn with_host_alternate_screen(mut self, enabled: bool) -> Self {
        self.host_uses_alternate_screen = enabled;
        self
    }

    /// Returns a mutable reference to the underlying terminal writer.
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }
}

impl<W, M> HostTerminalControl for CrosstermTerminalControl<W, M>
where
    W: Write,
    M: RawModeOps,
{
    type Error = io::Error;

    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> Result<TerminalRestoreToken, Self::Error> {
        retrying_flush(&mut self.writer)?;

        let raw_mode_was_enabled = self.raw_mode_ops.is_raw_mode_enabled()?;
        if !raw_mode_was_enabled {
            self.raw_mode_ops.enable_raw_mode()?;
        }

        let mut token = TerminalRestoreToken::new(policy);
        token.set_raw_mode_was_enabled(raw_mode_was_enabled);
        let suspend = (|| -> Result<(), io::Error> {
            let mut bytes = Vec::new();
            crossterm::queue!(
                bytes,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::empty())
            )?;
            token.set_keyboard_enhancement_pushed(true);
            bytes.extend_from_slice(ATTACH_SAVE_MOUSE_MODES);
            token.set_mouse_modes_saved(true);
            crossterm::queue!(bytes, EnableMouseCapture)?;

            match policy {
                AttachScreenPolicy::ReuseHostAlternateScreen => {
                    crossterm::queue!(bytes, cursor::Show)?;
                }
                AttachScreenPolicy::LeaveAlternateScreen => {
                    crossterm::queue!(bytes, cursor::Show)?;
                    if self.host_uses_alternate_screen {
                        crossterm::queue!(bytes, LeaveAlternateScreen)?;
                        token.set_left_host_alternate_screen(true);
                    }
                }
                AttachScreenPolicy::EnterFreshAlternateScreen => {
                    crossterm::queue!(bytes, cursor::Show)?;
                    if !self.host_uses_alternate_screen {
                        crossterm::queue!(bytes, EnterAlternateScreen)?;
                        token.set_entered_fresh_alternate_screen(true);
                    }
                    crossterm::queue!(bytes, Clear(ClearType::All), cursor::MoveTo(0, 0))?;
                }
            }

            retrying_write_all(&mut self.writer, &bytes)?;
            retrying_flush(&mut self.writer)?;
            Ok(())
        })();

        if let Err(error) = suspend {
            let mut cleanup = Vec::new();
            if token.mouse_modes_saved() {
                cleanup.extend_from_slice(ATTACH_RESTORE_MOUSE_MODES);
            }
            if token.keyboard_enhancement_pushed() {
                let _ = crossterm::queue!(cleanup, PopKeyboardEnhancementFlags);
            }
            if token.entered_fresh_alternate_screen() {
                let _ = crossterm::queue!(cleanup, LeaveAlternateScreen);
            }
            if token.left_host_alternate_screen() {
                let _ = crossterm::queue!(cleanup, EnterAlternateScreen);
            }
            let _ = retrying_write_all(&mut self.writer, &cleanup);
            let _ = retrying_flush(&mut self.writer);
            if !raw_mode_was_enabled {
                let _ = self.raw_mode_ops.disable_raw_mode();
            }
            return Err(error);
        }

        Ok(token)
    }

    fn restore_after_attach(
        &mut self,
        token: &mut TerminalRestoreToken,
    ) -> Result<(), Self::Error> {
        let mut bytes = Vec::new();
        if token.mouse_modes_saved() {
            bytes.extend_from_slice(ATTACH_RESTORE_MOUSE_MODES);
        }
        if token.keyboard_enhancement_pushed() {
            crossterm::queue!(bytes, PopKeyboardEnhancementFlags)?;
        }
        if token.entered_fresh_alternate_screen() {
            crossterm::queue!(bytes, LeaveAlternateScreen)?;
        }
        if token.left_host_alternate_screen() {
            crossterm::queue!(bytes, EnterAlternateScreen)?;
        }
        retrying_write_all(&mut self.writer, &bytes)?;
        retrying_flush(&mut self.writer)?;

        if !token.raw_mode_was_enabled() {
            self.raw_mode_ops.disable_raw_mode()?;
        }

        // Host redraw logic owns any post-attach cursor visibility policy.
        token.consume();
        Ok(())
    }
}

impl<W, M> PaneAttachTerminalControl for CrosstermTerminalControl<W, M>
where
    W: Write,
    M: RawModeOps,
{
    type Error = io::Error;
    type RestoreToken = TerminalRestoreToken;

    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> Result<Self::RestoreToken, Self::Error> {
        HostTerminalControl::suspend_for_attach(self, policy)
    }

    fn restore_after_attach(&mut self, token: &mut Self::RestoreToken) -> Result<(), Self::Error> {
        HostTerminalControl::restore_after_attach(self, token)
    }
}

/// Raw-stdio terminal used by blocking attach on Unix crossterm hosts.
#[cfg(unix)]
#[derive(Debug)]
pub struct StdioAttachTerminal<W>
where
    W: Write,
{
    stdout: W,
    stdin: RawStdin,
}

#[cfg(unix)]
impl<W> StdioAttachTerminal<W>
where
    W: Write,
{
    /// Creates a raw-stdio attach terminal.
    ///
    /// The caller must ensure exclusive ownership of stdin for the duration of
    /// the attach session because this temporarily sets stdin to nonblocking
    /// mode at the process level.
    pub fn new(stdout: W) -> io::Result<Self> {
        Ok(Self {
            stdout,
            stdin: RawStdin::new()?,
        })
    }

    /// Returns a mutable reference to the underlying stdout writer.
    pub fn stdout_mut(&mut self) -> &mut W {
        &mut self.stdout
    }
}

#[cfg(unix)]
impl<W> AttachTerminal for StdioAttachTerminal<W>
where
    W: Write,
{
    type Error = io::Error;

    fn read_stdin(&mut self) -> Result<Option<AttachInputChunk>, Self::Error> {
        self.stdin.read_nonblocking()
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        retrying_write_all(&mut self.stdout, bytes)?;
        retrying_flush(&mut self.stdout)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        let (cols, rows) = terminal::size()?;
        Ok(Size::new(rows, cols))
    }
}

#[cfg(unix)]
impl<W> PaneAttachTerminal for StdioAttachTerminal<W>
where
    W: Write,
{
    type Error = io::Error;

    fn read_stdin(&mut self) -> Result<Option<PaneAttachInputChunk>, Self::Error> {
        self.stdin.read_nonblocking().map(|chunk| {
            chunk.map(|chunk| PaneAttachInputChunk {
                at: chunk.at,
                bytes: chunk.bytes,
            })
        })
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        retrying_write_all(&mut self.stdout, bytes)?;
        retrying_flush(&mut self.stdout)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        let (cols, rows) = terminal::size()?;
        Ok(Size::new(rows, cols))
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct RawStdin {
    fd: std::os::fd::RawFd,
    original_flags: libc::c_int,
}

#[cfg(unix)]
impl RawStdin {
    fn new() -> io::Result<Self> {
        use std::os::fd::AsRawFd;

        let fd = io::stdin().as_raw_fd();
        // SAFETY: `fd` is borrowed from process stdin and is valid for this
        // call. `F_GETFL` does not require an extra pointer argument.
        let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if original_flags < 0 {
            return Err(io::Error::last_os_error());
        }

        let new_flags = original_flags | libc::O_NONBLOCK;
        // SAFETY: `fd` is valid and `new_flags` is the original flag set with
        // `O_NONBLOCK` added, which is the expected integer argument for
        // `F_SETFL`.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, new_flags) } < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self { fd, original_flags })
    }

    fn read_nonblocking(&mut self) -> io::Result<Option<AttachInputChunk>> {
        let mut buf = [0_u8; 4096];
        // SAFETY: `self.fd` is the stdin file descriptor captured at
        // construction. `buf` is a valid writable byte buffer for `buf.len()`
        // bytes, and libc will not retain the pointer after `read` returns.
        let read =
            unsafe { libc::read(self.fd, buf.as_mut_ptr().cast::<libc::c_void>(), buf.len()) };

        if read > 0 {
            return Ok(Some(AttachInputChunk {
                at: std::time::Instant::now(),
                bytes: buf[..read as usize].to_vec(),
            }));
        }

        if read == 0 {
            return Ok(None);
        }

        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => Ok(None),
            _ => Err(error),
        }
    }
}

#[cfg(unix)]
impl Drop for RawStdin {
    fn drop(&mut self) {
        // SAFETY: `self.fd` is the stdin descriptor captured at construction
        // and `original_flags` was returned by `F_GETFL` for the same
        // descriptor.
        let _ = unsafe { libc::fcntl(self.fd, libc::F_SETFL, self.original_flags) };
    }
}
