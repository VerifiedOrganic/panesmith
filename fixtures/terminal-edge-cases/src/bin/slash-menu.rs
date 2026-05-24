use std::io::{self, Read, Write};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

const ENABLE_MODES: &[u8] = b"\x1b[?1004;2004h";
const DISABLE_MODES: &[u8] = b"\x1b[?1004;2004l";
const PROMPT: &str = "prompt> ";
const EXIT_COMMAND: &str = "__PANESMITH_EXIT__";
const MENU_FRAME: &str = "\r\nmenu:\r\n/status\r\n/review\r\n/paste\r\nprompt> ";
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Default)]
struct FixtureState {
    line: Vec<u8>,
    escape: Vec<u8>,
    paste: Option<PasteCapture>,
}

#[derive(Debug, Default)]
struct PasteCapture {
    bytes: Vec<u8>,
    suffix: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Continue,
    Exit,
}

#[derive(Debug)]
struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Err(error) = disable_raw_mode() {
            eprintln!("warning: failed to disable raw mode: {error}");
        }
    }
}

/// Guard that writes `ENABLE_MODES` on creation and best-effort writes
/// `DISABLE_MODES` on drop. This ensures focus-report and bracketed-paste
/// modes are disabled even if the main loop returns early via `?`.
struct ModesGuard<W: Write> {
    writer: W,
}

impl ModesGuard<io::Stdout> {
    fn enter_stdout() -> io::Result<Self> {
        let stdout = io::stdout();
        stdout.lock().write_all(ENABLE_MODES)?;
        Ok(Self { writer: stdout })
    }
}

impl<W: Write> ModesGuard<W> {
    #[cfg(test)]
    fn enter(mut writer: W) -> io::Result<Self> {
        writer.write_all(ENABLE_MODES)?;
        Ok(Self { writer })
    }
}

#[cfg(test)]
impl<W: Write> Write for ModesGuard<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> Drop for ModesGuard<W> {
    fn drop(&mut self) {
        if let Err(error) = self.writer.write_all(DISABLE_MODES) {
            eprintln!("warning: failed to disable modes: {error}");
        }
        if let Err(error) = self.writer.flush() {
            eprintln!("warning: failed to flush stdout: {error}");
        }
    }
}

impl FixtureState {
    fn consume_byte(&mut self, byte: u8, mut out: impl Write) -> io::Result<Step> {
        if let Some(paste) = &mut self.paste {
            let step = consume_paste_byte(paste, byte, &mut out)?;
            let finished = paste.suffix.is_empty() && paste.bytes.is_empty();
            if finished {
                self.paste = None;
            }
            return Ok(step);
        }

        if !self.escape.is_empty() || byte == 0x1b {
            return self.consume_escape_byte(byte, out);
        }

        match byte {
            b'/' if self.line.is_empty() => {
                out.write_all(MENU_FRAME.as_bytes())?;
                out.flush()?;
                Ok(Step::Continue)
            }
            b'\r' | b'\n' => {
                let command = String::from_utf8_lossy(&self.line).into_owned();
                self.line.clear();
                if command == EXIT_COMMAND {
                    Ok(Step::Exit)
                } else if command.is_empty() {
                    out.write_all(b"\r\n")?;
                    out.write_all(PROMPT.as_bytes())?;
                    out.flush()?;
                    Ok(Step::Continue)
                } else {
                    writeln!(out, "\r\nentered:{command}")?;
                    out.write_all(PROMPT.as_bytes())?;
                    out.flush()?;
                    Ok(Step::Continue)
                }
            }
            0x7f => {
                if self.line.pop().is_some() {
                    out.write_all(b"\x08 \x08")?;
                    out.flush()?;
                }
                Ok(Step::Continue)
            }
            byte => {
                self.line.push(byte);
                out.write_all(&[byte])?;
                out.flush()?;
                Ok(Step::Continue)
            }
        }
    }

    fn consume_escape_byte(&mut self, byte: u8, mut out: impl Write) -> io::Result<Step> {
        self.escape.push(byte);

        if BRACKETED_PASTE_START.starts_with(&self.escape) {
            if self.escape == BRACKETED_PASTE_START {
                self.escape.clear();
                self.paste = Some(PasteCapture::default());
            }
            return Ok(Step::Continue);
        }

        if self.escape == b"\x1b[I" || self.escape == b"\x1b[O" {
            self.escape.clear();
            return Ok(Step::Continue);
        }

        if self.escape.len() >= BRACKETED_PASTE_START.len() {
            let buffered = std::mem::take(&mut self.escape);
            for byte in buffered {
                self.line.push(byte);
                out.write_all(&[byte])?;
            }
            out.flush()?;
        }

        Ok(Step::Continue)
    }
}

/// Parses a single byte inside a bracketed-paste region.
///
/// NOTE: This fixture assumes well-formed bracketed-paste payloads that do
/// not contain embedded ESC (`0x1b`) characters. If an ESC appears inside the
/// paste payload, the fallback path that moves partial suffix bytes into
/// `paste.bytes` will treat those bytes as paste data rather than re-examining
/// them as potential escape sequences.
fn consume_paste_byte(paste: &mut PasteCapture, byte: u8, mut out: impl Write) -> io::Result<Step> {
    if !paste.suffix.is_empty() || byte == BRACKETED_PASTE_END[0] {
        paste.suffix.push(byte);

        if BRACKETED_PASTE_END.starts_with(&paste.suffix) {
            if paste.suffix == BRACKETED_PASTE_END {
                let text = String::from_utf8_lossy(&paste.bytes);
                writeln!(out, "\r\npaste:{text}")?;
                out.write_all(PROMPT.as_bytes())?;
                out.flush()?;
                paste.bytes.clear();
                paste.suffix.clear();
            }
            return Ok(Step::Continue);
        }

        paste.bytes.append(&mut paste.suffix);
        return Ok(Step::Continue);
    }

    paste.bytes.push(byte);
    Ok(Step::Continue)
}

fn run(state: &mut FixtureState, mut stdin: impl Read, mut stdout: impl Write) -> io::Result<()> {
    stdout.write_all(PROMPT.as_bytes())?;
    stdout.flush()?;

    let mut byte = [0_u8; 1];
    loop {
        let read = stdin.read(&mut byte)?;
        if read == 0 {
            break;
        }

        if state.consume_byte(byte[0], &mut stdout)? == Step::Exit {
            break;
        }
    }

    Ok(())
}

fn main() -> io::Result<()> {
    let _raw_mode = RawModeGuard::enter()?;
    let _modes_guard = ModesGuard::enter_stdout()?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    let mut state = FixtureState::default();

    run(&mut state, &mut stdin_lock, &mut stdout_lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_script(bytes: &[u8]) -> String {
        let mut state = FixtureState::default();
        let mut out = Vec::new();
        for byte in bytes {
            if state.consume_byte(*byte, &mut out).unwrap() == Step::Exit {
                break;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn slash_opens_menu_frame() {
        let rendered = run_script(b"/");

        assert!(rendered.contains("menu:"));
        assert!(rendered.contains("/status"));
        assert!(rendered.contains("/review"));
    }

    #[test]
    fn bracketed_paste_echoes_payload() {
        let rendered = run_script(b"\x1b[200~cargo test --workspace\x1b[201~");

        assert!(rendered.contains("paste:cargo test --workspace"));
        assert!(rendered.ends_with(PROMPT));
    }

    #[test]
    fn exit_command_stops_processing() {
        let mut state = FixtureState::default();
        let mut out = Vec::new();
        let mut last = Step::Continue;
        for byte in b"__PANESMITH_EXIT__\r" {
            last = state.consume_byte(*byte, &mut out).unwrap();
        }

        assert_eq!(last, Step::Exit);
    }

    #[test]
    fn disable_modes_emits_restore_sequence() {
        let mut out = Vec::new();
        {
            let mut guard = ModesGuard::enter(&mut out).unwrap();
            let mut state = FixtureState::default();
            run(&mut state, &b"__PANESMITH_EXIT__\r"[..], &mut guard).unwrap();
        }
        assert!(
            out.starts_with(ENABLE_MODES),
            "expected output to start with ENABLE_MODES"
        );
        assert!(
            out.ends_with(DISABLE_MODES),
            "expected output to end with DISABLE_MODES"
        );
    }
}
