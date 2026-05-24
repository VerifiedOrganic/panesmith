use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("gate");
    let extra = args.get(1..).unwrap_or(&[]);

    match cmd {
        "gate" => run_gate(extra),
        _ => {
            eprintln!("Unknown xtask command: {cmd}");
            eprintln!("Usage: cargo xtask <command>");
            eprintln!("  gate   Run the full local gate (fmt + check + tests)");
            ExitCode::from(1)
        }
    }
}

fn run_gate(extra: &[String]) -> ExitCode {
    let (test_args, ignored_args) = build_gate_args(extra);

    let code = step("fmt", || {
        run_cmd("cargo", &["fmt", "--all", "--", "--check"])
    });
    if code != ExitCode::SUCCESS {
        return code;
    }

    let code = step("check", || {
        run_cmd("cargo", &["check", "--workspace", "--all-features"])
    });
    if code != ExitCode::SUCCESS {
        return code;
    }

    let code = step("test", || run_cmd("cargo", &test_args));
    if code != ExitCode::SUCCESS {
        return code;
    }

    let code = step("ignored tests", || run_cmd("cargo", &ignored_args));
    if code != ExitCode::SUCCESS {
        return code;
    }

    println!("=== cargo gate: PASS ===");
    ExitCode::SUCCESS
}

fn step<F: FnOnce() -> ExitCode>(name: &str, f: F) -> ExitCode {
    println!("=== cargo gate: {name} ===");
    let code = f();
    if code != ExitCode::SUCCESS {
        eprintln!("=== cargo gate: FAILED at {name} ===");
    }
    code
}

fn run_cmd(program: &str, args: &[&str]) -> ExitCode {
    let status = Command::new(program)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {program}: {e}"));

    match status.code() {
        Some(code) => ExitCode::from(code as u8),
        None => {
            eprintln!("{program} terminated by signal");
            ExitCode::from(1)
        }
    }
}

fn build_gate_args(extra: &[String]) -> (Vec<&str>, Vec<&str>) {
    let mut test_args = vec!["test", "--workspace"];
    let mut ignored_args = vec!["test", "--workspace", "--", "--ignored"];

    if !extra.is_empty() {
        test_args.push("--");
        for arg in extra {
            test_args.push(arg);
            ignored_args.push(arg);
        }
    }

    (test_args, ignored_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_cmd_true_returns_success() {
        assert_eq!(run_cmd("true", &[]), ExitCode::SUCCESS);
    }

    #[test]
    fn run_cmd_false_returns_failure() {
        assert_ne!(run_cmd("false", &[]), ExitCode::SUCCESS);
    }

    #[test]
    fn step_propagates_failure() {
        let code = step("failing-step", || run_cmd("false", &[]));
        assert_ne!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn step_propagates_success() {
        let code = step("passing-step", || run_cmd("true", &[]));
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn gate_args_without_extra() {
        let (test_args, ignored_args) = build_gate_args(&[]);
        assert_eq!(test_args, vec!["test", "--workspace"]);
        assert_eq!(ignored_args, vec!["test", "--workspace", "--", "--ignored"]);
    }

    #[test]
    fn gate_args_with_extra_forwards_to_both() {
        let extra = vec![String::from("--test-threads=1")];
        let (test_args, ignored_args) = build_gate_args(&extra);
        assert_eq!(
            test_args,
            vec!["test", "--workspace", "--", "--test-threads=1"]
        );
        assert_eq!(
            ignored_args,
            vec!["test", "--workspace", "--", "--ignored", "--test-threads=1"]
        );
    }

    #[test]
    fn gate_args_with_multiple_extra() {
        let extra = vec![
            String::from("--test-threads=1"),
            String::from("--nocapture"),
        ];
        let (test_args, ignored_args) = build_gate_args(&extra);
        assert_eq!(
            test_args,
            vec![
                "test",
                "--workspace",
                "--",
                "--test-threads=1",
                "--nocapture"
            ]
        );
        assert_eq!(
            ignored_args,
            vec![
                "test",
                "--workspace",
                "--",
                "--ignored",
                "--test-threads=1",
                "--nocapture"
            ]
        );
    }
}
