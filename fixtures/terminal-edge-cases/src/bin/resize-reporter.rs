use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use signal_hook::consts::signal::SIGWINCH;
use signal_hook::iterator::Signals;

const EXIT_COMMAND: &str = "__PANESMITH_EXIT__";
const READY_FILE_ENV: &str = "PANESMITH_READY_FILE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureEvent {
    Exit,
    Sigwinch,
}

fn current_size_line() -> io::Result<String> {
    let (cols, rows) = crossterm::terminal::size()?;
    Ok(format!("size:{rows}x{cols}"))
}

fn ready_file_path() -> Option<PathBuf> {
    env::var_os(READY_FILE_ENV).map(PathBuf::from)
}

fn write_ready_file(path: &PathBuf) -> io::Result<()> {
    fs::write(path, b"ready")
}

fn spawn_stdin_thread(tx: Sender<FixtureEvent>) {
    thread::spawn(move || {
        let stdin = io::stdin();
        for line_result in stdin.lock().lines() {
            match line_result {
                Ok(line) => {
                    if line == EXIT_COMMAND {
                        let _ = tx.send(FixtureEvent::Exit);
                        return;
                    }
                }
                Err(_) => return,
            }
        }

        let _ = tx.send(FixtureEvent::Exit);
    });
}

fn spawn_sigwinch_thread(tx: Sender<FixtureEvent>) -> io::Result<()> {
    let mut signals = Signals::new([SIGWINCH])?;

    thread::spawn(move || {
        for signal in signals.forever() {
            if signal == SIGWINCH && tx.send(FixtureEvent::Sigwinch).is_err() {
                return;
            }
        }
    });

    Ok(())
}

fn main() -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();
    let (tx, rx): (Sender<FixtureEvent>, Receiver<FixtureEvent>) = mpsc::channel();
    spawn_sigwinch_thread(tx.clone())?;
    spawn_stdin_thread(tx);
    if let Some(path) = ready_file_path() {
        write_ready_file(&path)?;
    }

    while let Ok(event) = rx.recv() {
        match event {
            FixtureEvent::Exit => break,
            FixtureEvent::Sigwinch => {
                writeln!(stdout_lock, "{}", current_size_line()?)?;
                stdout_lock.flush()?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn write_ready_file_creates_marker() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should move forward")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "panesmith-resize-reporter-ready-{}-{nonce}.marker",
            std::process::id()
        ));

        write_ready_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"ready");
        fs::remove_file(path).unwrap();
    }
}
