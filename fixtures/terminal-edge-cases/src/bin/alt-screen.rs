use std::env;
use std::io::{self, BufRead, Write};

const AUTO_EXIT_FLAG: &str = "--auto-exit";
const EXIT_COMMAND: &str = "__PANESMITH_EXIT__";
const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";
const CLEAR_AND_HOME: &str = "\x1b[2J\x1b[H";
const READY_FRAME: &str = "alt-screen ready\r\n";

fn render_frame(mut stdout: impl Write) -> io::Result<()> {
    stdout.write_all(ENTER_ALT_SCREEN.as_bytes())?;
    stdout.write_all(CLEAR_AND_HOME.as_bytes())?;
    stdout.write_all(READY_FRAME.as_bytes())
}

fn leave_alt_screen(mut stdout: impl Write) -> io::Result<()> {
    stdout.write_all(LEAVE_ALT_SCREEN.as_bytes())
}

fn main() -> io::Result<()> {
    let auto_exit = env::args().skip(1).any(|arg| arg == AUTO_EXIT_FLAG);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    render_frame(&mut stdout_lock)?;
    stdout_lock.flush()?;

    if !auto_exit {
        for line_result in stdin.lock().lines() {
            if line_result? == EXIT_COMMAND {
                break;
            }
        }
    }

    leave_alt_screen(&mut stdout_lock)?;
    stdout_lock.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_frame_emits_alt_screen_intro_and_payload() {
        let mut bytes = Vec::new();
        render_frame(&mut bytes).unwrap();

        assert!(bytes.starts_with(ENTER_ALT_SCREEN.as_bytes()));
        assert!(bytes
            .windows(CLEAR_AND_HOME.len())
            .any(|w| w == CLEAR_AND_HOME.as_bytes()));
        assert!(bytes.ends_with(READY_FRAME.as_bytes()));
    }

    #[test]
    fn leave_alt_screen_emits_restore_sequence() {
        let mut bytes = Vec::new();
        leave_alt_screen(&mut bytes).unwrap();

        assert_eq!(bytes, LEAVE_ALT_SCREEN.as_bytes());
    }
}
