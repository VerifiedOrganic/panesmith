use std::io::{self, Read, Write};

const CPR_QUERY: &[u8] = b"\x1b[6n";

fn parse_report(bytes: &[u8]) -> Option<(u16, u16)> {
    let report = std::str::from_utf8(bytes).ok()?;
    let body = report.strip_prefix("\x1b[")?.strip_suffix('R')?;
    let (row, col) = body.split_once(';')?;

    Some((row.parse().ok()?, col.parse().ok()?))
}

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();

    stdout_lock.write_all(CPR_QUERY)?;
    stdout_lock.flush()?;

    let mut report = Vec::new();
    let mut byte = [0_u8; 1];
    while report.len() < 64 {
        let read = stdin_lock.read(&mut byte)?;
        if read == 0 {
            break;
        }

        report.push(byte[0]);
        if byte[0] == b'R' {
            break;
        }
    }

    match parse_report(&report) {
        Some((row, col)) => writeln!(stdout_lock, "cpr:{row};{col}")?,
        None => writeln!(stdout_lock, "cpr:invalid")?,
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_report_accepts_cursor_position_reply() {
        assert_eq!(parse_report(b"\x1b[12;34R"), Some((12, 34)));
    }

    #[test]
    fn parse_report_rejects_invalid_sequences() {
        assert_eq!(parse_report(b"\x1b[12;34n"), None);
        assert_eq!(parse_report(b"12;34R"), None);
    }
}
