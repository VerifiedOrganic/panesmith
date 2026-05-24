use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

const ENABLE_MODES: &[u8] = b"\x1b[?1004;2004h";
const OUTPUT_FLAG: &str = "--out";
const COUNT_FLAG: &str = "--count";
const OUTPUT_ENV: &str = "PANESMITH_OUT";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    output: PathBuf,
    count: Option<usize>,
}

fn parse_args() -> Result<Options, String> {
    parse_args_from(
        env::args().skip(1),
        env::var_os(OUTPUT_ENV).map(PathBuf::from),
    )
}

fn parse_args_from<I, S>(args: I, output_env: Option<PathBuf>) -> Result<Options, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut output = output_env;
    let mut count = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_ref() {
            OUTPUT_FLAG => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {OUTPUT_FLAG}"))?;
                output = Some(PathBuf::from(value.as_ref()));
            }
            COUNT_FLAG => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {COUNT_FLAG}"))?;
                count =
                    Some(value.as_ref().parse::<usize>().map_err(|_| {
                        format!("invalid value for {COUNT_FLAG}: {}", value.as_ref())
                    })?);
            }
            other => return Err(format!("unsupported argument: {other}")),
        }
    }

    let output = output.ok_or_else(|| {
        format!("missing {OUTPUT_FLAG} argument or {OUTPUT_ENV} environment value")
    })?;

    Ok(Options { output, count })
}

fn read_capture(mut input: impl Read, count: Option<usize>) -> io::Result<Vec<u8>> {
    let mut capture = Vec::new();

    match count {
        Some(len) => {
            capture.resize(len, 0);
            input.read_exact(&mut capture)?;
        }
        None => {
            input.read_to_end(&mut capture)?;
        }
    }

    Ok(capture)
}

fn main() -> io::Result<()> {
    let options =
        parse_args().map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    stdout_lock.write_all(ENABLE_MODES)?;
    stdout_lock.flush()?;

    let capture = read_capture(stdin.lock(), options.count)?;
    fs::write(options.output, capture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parse_args_prefers_explicit_output_path() {
        let options = parse_args_from(["--out", "capture.bin", "--count", "12"], None).unwrap();

        assert_eq!(
            options,
            Options {
                output: PathBuf::from("capture.bin"),
                count: Some(12),
            }
        );
    }

    #[test]
    fn parse_args_uses_environment_fallback() {
        let options = parse_args_from(
            std::iter::empty::<&str>(),
            Some(PathBuf::from("capture.bin")),
        )
        .unwrap();

        assert_eq!(
            options,
            Options {
                output: PathBuf::from("capture.bin"),
                count: None,
            }
        );
    }

    #[test]
    fn read_capture_honors_exact_byte_count() {
        let capture = read_capture(Cursor::new(b"hello world"), Some(5)).unwrap();

        assert_eq!(capture, b"hello");
    }

    #[test]
    fn parse_args_rejects_missing_output_value() {
        let err = parse_args_from(["--out"], None).unwrap_err();

        assert_eq!(err, "missing value for --out");
    }

    #[test]
    fn parse_args_rejects_invalid_count() {
        let err = parse_args_from(["--out", "capture.bin", "--count", "abc"], None).unwrap_err();

        assert_eq!(err, "invalid value for --count: abc");
    }
}
