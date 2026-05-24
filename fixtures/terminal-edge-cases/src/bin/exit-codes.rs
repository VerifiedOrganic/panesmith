use std::env;

const CODE_FLAG: &str = "--code";
const MAX_EXIT_CODE: i32 = 255;

fn parse_code() -> Result<i32, String> {
    parse_code_from(env::args().skip(1))
}

fn parse_code_from<I, S>(args: I) -> Result<i32, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut code = 0_i32;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_ref() {
            CODE_FLAG => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {CODE_FLAG}"))?;
                let parsed = value
                    .as_ref()
                    .parse::<i32>()
                    .map_err(|_| format!("invalid value for {CODE_FLAG}: {}", value.as_ref()))?;
                if !(0..=MAX_EXIT_CODE).contains(&parsed) {
                    return Err(format!(
                        "invalid value for {CODE_FLAG}: {} (expected 0..={MAX_EXIT_CODE})",
                        value.as_ref()
                    ));
                }
                code = parsed;
            }
            other => return Err(format!("unsupported argument: {other}")),
        }
    }

    Ok(code)
}

fn main() {
    let code = parse_code().unwrap_or_else(|message| {
        eprintln!("{message}");
        std::process::exit(2);
    });

    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_code_defaults_to_zero() {
        assert_eq!(parse_code_from(std::iter::empty::<&str>()).unwrap(), 0);
    }

    #[test]
    fn parse_code_reads_explicit_value() {
        assert_eq!(parse_code_from(["--code", "42"]).unwrap(), 42);
    }

    #[test]
    fn parse_code_rejects_invalid_string_value() {
        let err = parse_code_from(["--code", "abc"]).unwrap_err();

        assert_eq!(err, "invalid value for --code: abc");
    }

    #[test]
    fn parse_code_rejects_out_of_range_values() {
        let err = parse_code_from(["--code", "256"]).unwrap_err();

        assert_eq!(err, "invalid value for --code: 256 (expected 0..=255)");
    }
}
