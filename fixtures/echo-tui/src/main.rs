//! Echo-tui fixture
//!
//! Reads lines from stdin, echoes each line back to stdout, and reports the
//! byte count of each line to stderr.
//!
//! This fixture is used by the Panesmith PTY integration test suite to verify
//! that input reaches a child process and that output can be read back.

use std::env;
use std::io::{self, BufRead, Write};

const SIZE_COMMAND: &str = "__PANESMITH_SIZE__";
const PWD_COMMAND: &str = "__PANESMITH_PWD__";
const ARGS_COMMAND: &str = "__PANESMITH_ARGS__";
const PID_COMMAND: &str = "__PANESMITH_PID__";
const EXIT_COMMAND: &str = "__PANESMITH_EXIT__";
const ANSI_COMMAND: &str = "__PANESMITH_ANSI__";
const ENV_KEYS_COMMAND: &str = "__PANESMITH_ENV_KEYS__";
const ENV_PREFIX: &str = "__PANESMITH_ENV__:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureAction {
    Echo,
    Exit,
}

fn handle_fixture_command(line: &str, mut stdout: impl Write) -> io::Result<FixtureAction> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    handle_fixture_command_with(line, &mut stdout, &args, |key| env::var(key).ok())
}

fn handle_fixture_command_with<F>(
    line: &str,
    mut stdout: impl Write,
    args: &[String],
    env_lookup: F,
) -> io::Result<FixtureAction>
where
    F: Fn(&str) -> Option<String>,
{
    if line == SIZE_COMMAND {
        let (cols, rows) = crossterm::terminal::size()?;
        writeln!(stdout, "size:{rows}x{cols}")?;
        return Ok(FixtureAction::Echo);
    }

    if line == PWD_COMMAND {
        writeln!(stdout, "cwd:{}", env::current_dir()?.display())?;
        return Ok(FixtureAction::Echo);
    }

    if line == ARGS_COMMAND {
        writeln!(stdout, "args:{}", args.join(" "))?;
        return Ok(FixtureAction::Echo);
    }

    if line == PID_COMMAND {
        writeln!(stdout, "pid:{}", std::process::id())?;
        return Ok(FixtureAction::Echo);
    }

    if let Some(key) = line.strip_prefix(ENV_PREFIX) {
        let value = env_lookup(key).unwrap_or_default();
        writeln!(stdout, "env:{key}={value}")?;
        return Ok(FixtureAction::Echo);
    }

    if line == EXIT_COMMAND {
        return Ok(FixtureAction::Exit);
    }

    if line == ANSI_COMMAND {
        writeln!(stdout, "\x1b[31mred\x1b[0m")?;
        return Ok(FixtureAction::Echo);
    }

    if line == ENV_KEYS_COMMAND {
        let mut keys = env::vars_os()
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        keys.sort();
        writeln!(stdout, "env-keys:{}", keys.join(","))?;
        return Ok(FixtureAction::Echo);
    }

    writeln!(stdout, "{line}")?;
    Ok(FixtureAction::Echo)
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();

    let mut stdout_lock = stdout.lock();
    let mut stderr_lock = stderr.lock();

    for line_result in stdin.lock().lines() {
        let line = line_result?;
        let bytes = line.as_bytes();

        let action = handle_fixture_command(&line, &mut stdout_lock)?;
        stdout_lock.flush()?;

        writeln!(stderr_lock, "bytes: {}", bytes.len())?;
        stderr_lock.flush()?;

        if action == FixtureAction::Exit {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn echo_reports_correct_byte_count() {
        let input = b"hello\n";
        let reader = Cursor::new(&input[..]);
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        for line_result in reader.lines() {
            let line = line_result.unwrap();
            let bytes = line.as_bytes();

            writeln!(stdout_buf, "{}", line).unwrap();
            writeln!(stderr_buf, "bytes: {}", bytes.len()).unwrap();
        }

        assert_eq!(String::from_utf8(stdout_buf).unwrap(), "hello\n");
        assert_eq!(String::from_utf8(stderr_buf).unwrap(), "bytes: 5\n");
    }

    #[test]
    fn echo_handles_empty_line() {
        let input = b"\n";
        let reader = Cursor::new(&input[..]);
        let mut stderr_buf = Vec::new();

        for line_result in reader.lines() {
            let line = line_result.unwrap();
            writeln!(stderr_buf, "bytes: {}", line.len()).unwrap();
        }

        assert_eq!(String::from_utf8(stderr_buf).unwrap(), "bytes: 0\n");
    }

    #[test]
    fn fixture_reports_explicit_args() {
        let mut output = Vec::new();
        let args = vec!["fixture-filter".into(), "--nocapture".into()];
        let action =
            handle_fixture_command_with(ARGS_COMMAND, &mut output, &args, |_| None).unwrap();

        assert_eq!(action, FixtureAction::Echo);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "args:fixture-filter --nocapture\n"
        );
    }

    #[test]
    fn fixture_reports_process_id() {
        let mut output = Vec::new();
        let action = handle_fixture_command_with(PID_COMMAND, &mut output, &[], |_| None).unwrap();

        assert_eq!(action, FixtureAction::Echo);
        let text = String::from_utf8(output).unwrap();
        let pid = text
            .trim()
            .strip_prefix("pid:")
            .expect("fixture should prefix the process id with pid:")
            .parse::<u32>()
            .expect("fixture should report a numeric process id");
        assert_eq!(pid, std::process::id());
    }

    #[test]
    fn fixture_reports_env_values() {
        let mut output = Vec::new();
        let action =
            handle_fixture_command_with("__PANESMITH_ENV__:TEST_VALUE", &mut output, &[], |key| {
                (key == "TEST_VALUE").then(|| "present".to_string())
            })
            .unwrap();

        assert_eq!(action, FixtureAction::Echo);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "env:TEST_VALUE=present\n"
        );
    }

    #[test]
    fn fixture_ansi_command_emits_colored_output() {
        let mut output = Vec::new();
        let action = handle_fixture_command_with(ANSI_COMMAND, &mut output, &[], |_| None).unwrap();

        assert_eq!(action, FixtureAction::Echo);
        assert_eq!(String::from_utf8(output).unwrap(), "\x1b[31mred\x1b[0m\n");
    }

    #[test]
    fn fixture_exit_command_breaks_loop() {
        let mut output = Vec::new();
        let action = handle_fixture_command(EXIT_COMMAND, &mut output).unwrap();

        assert_eq!(action, FixtureAction::Exit);
        assert!(output.is_empty());
    }
}
