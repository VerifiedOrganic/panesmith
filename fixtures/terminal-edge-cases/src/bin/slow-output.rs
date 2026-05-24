use std::env;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

const DELAY_FLAG: &str = "--delay-ms";
const CHUNKS: [&str; 3] = ["slow", "-output", "\n"];

fn parse_delay_ms() -> Result<u64, String> {
    parse_delay_ms_from(env::args().skip(1))
}

fn parse_delay_ms_from<I, S>(args: I) -> Result<u64, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut delay_ms = 75_u64;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_ref() {
            DELAY_FLAG => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {DELAY_FLAG}"))?;
                delay_ms = value
                    .as_ref()
                    .parse::<u64>()
                    .map_err(|_| format!("invalid value for {DELAY_FLAG}: {}", value.as_ref()))?;
            }
            other => return Err(format!("unsupported argument: {other}")),
        }
    }

    Ok(delay_ms)
}

fn main() -> io::Result<()> {
    let delay_ms =
        parse_delay_ms().map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();
    let delay = Duration::from_millis(delay_ms);

    for (index, chunk) in CHUNKS.iter().enumerate() {
        stdout_lock.write_all(chunk.as_bytes())?;
        stdout_lock.flush()?;

        if index + 1 != CHUNKS.len() {
            thread::sleep(delay);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_delay_ms_defaults_to_seventy_five() {
        assert_eq!(parse_delay_ms_from(std::iter::empty::<&str>()).unwrap(), 75);
    }

    #[test]
    fn parse_delay_ms_reads_explicit_value() {
        assert_eq!(parse_delay_ms_from(["--delay-ms", "9"]).unwrap(), 9);
    }

    #[test]
    fn parse_delay_ms_rejects_missing_value() {
        let err = parse_delay_ms_from(["--delay-ms"]).unwrap_err();

        assert_eq!(err, "missing value for --delay-ms");
    }

    #[test]
    fn parse_delay_ms_rejects_invalid_value() {
        let err = parse_delay_ms_from(["--delay-ms", "abc"]).unwrap_err();

        assert_eq!(err, "invalid value for --delay-ms: abc");
    }
}
