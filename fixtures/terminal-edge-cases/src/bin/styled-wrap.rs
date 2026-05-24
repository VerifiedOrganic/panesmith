use std::io::{self, Write};

const FRAME: &[u8] = b"\x1b[31;1mredwrap\x1b[0m plain\r\n";

fn render(mut stdout: impl Write) -> io::Result<()> {
    stdout.write_all(FRAME)?;
    stdout.flush()
}

fn main() -> io::Result<()> {
    render(io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_emits_styled_wrapping_payload() {
        let mut out = Vec::new();

        render(&mut out).unwrap();

        assert!(out.starts_with(b"\x1b[31;1mredwrap"));
        assert!(out.ends_with(b" plain\r\n"));
    }
}
