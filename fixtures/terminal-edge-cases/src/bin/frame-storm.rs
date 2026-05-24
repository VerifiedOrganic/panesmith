use std::env;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

const FRAMES_FLAG: &str = "--frames";
const TRAILER_SLEEP_FLAG: &str = "--trailer-sleep-ms";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    frames: usize,
    trailer_sleep_ms: u64,
}

fn parse_args() -> Result<Options, String> {
    parse_args_from(env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Options, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut options = Options {
        frames: 2048,
        trailer_sleep_ms: 50,
    };
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_ref() {
            FRAMES_FLAG => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {FRAMES_FLAG}"))?;
                options.frames = value
                    .as_ref()
                    .parse::<usize>()
                    .map_err(|_| format!("invalid value for {FRAMES_FLAG}: {}", value.as_ref()))?;
            }
            TRAILER_SLEEP_FLAG => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {TRAILER_SLEEP_FLAG}"))?;
                options.trailer_sleep_ms = value.as_ref().parse::<u64>().map_err(|_| {
                    format!("invalid value for {TRAILER_SLEEP_FLAG}: {}", value.as_ref())
                })?;
            }
            other => return Err(format!("unsupported argument: {other}")),
        }
    }

    Ok(options)
}

fn emit_frames(mut stdout: impl Write, frames: usize) -> io::Result<()> {
    for index in 0..frames {
        write!(stdout, "\x1b[Hframe:{index:04}\r\n")?;
        stdout.flush()?;
    }

    Ok(())
}

fn main() -> io::Result<()> {
    let options =
        parse_args().map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    emit_frames(&mut stdout_lock, options.frames)?;
    thread::sleep(Duration::from_millis(options.trailer_sleep_ms));
    writeln!(stdout_lock, "frame-storm complete")?;
    stdout_lock.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_reads_frame_count_and_sleep() {
        let options = parse_args_from(["--frames", "12", "--trailer-sleep-ms", "7"]).unwrap();

        assert_eq!(
            options,
            Options {
                frames: 12,
                trailer_sleep_ms: 7,
            }
        );
    }

    #[test]
    fn emit_frames_writes_expected_marker() {
        let mut bytes = Vec::new();
        emit_frames(&mut bytes, 2).unwrap();

        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("frame:0000"));
        assert!(text.contains("frame:0001"));
    }

    #[test]
    fn parse_args_rejects_invalid_frame_count() {
        let err = parse_args_from(["--frames", "abc"]).unwrap_err();

        assert_eq!(err, "invalid value for --frames: abc");
    }

    #[test]
    fn parse_args_rejects_invalid_trailer_sleep() {
        let err = parse_args_from(["--trailer-sleep-ms", "abc"]).unwrap_err();

        assert_eq!(err, "invalid value for --trailer-sleep-ms: abc");
    }
}
