use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use panesmith_core::{
    KillConfig, OverflowStats, PaneConfig, PortablePtyBackend, PtyBackend, PtyFrame, PtyProcess,
    Size,
};

static NEXT_TEMP_DIR_ID: AtomicU64 = AtomicU64::new(1);

fn fixture_path() -> PathBuf {
    env::var("CARGO_BIN_EXE_echo-tui")
        .or_else(|_| env::var("CARGO_BIN_EXE_echo_tui"))
        .map(PathBuf::from)
        .expect("echo-tui fixture path should be available to integration tests")
}

fn spawn_fixture(config: PaneConfig) -> impl PtyProcess {
    PortablePtyBackend
        .spawn(&config)
        .expect("fixture should spawn through portable PTY backend")
}

fn normalize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\r', "")
}

fn wait_for_output(process: &mut impl PtyProcess, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();

    while Instant::now() < deadline {
        while let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Output { bytes, .. } => {
                    seen.push_str(&normalize(&bytes));
                    if seen.contains(needle) {
                        return seen;
                    }
                }
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => {
                    panic!(
                        "unexpected PTY overflow while waiting for {needle:?}: dropped {dropped_frames} frame(s) / {dropped_bytes} byte(s)"
                    );
                }
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error while waiting for {needle:?}: {message}");
                }
                PtyFrame::Exited { code, .. } => {
                    panic!("process exited early with code {code:?} while waiting for {needle:?}");
                }
                PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    panic!("timed out waiting for {needle:?}; saw output {seen:?}");
}

fn wait_for_exit(process: &mut impl PtyProcess, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        while let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Exited { code, .. } => return code,
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error while waiting for exit: {message}");
                }
                PtyFrame::Output { .. }
                | PtyFrame::Overflow { .. }
                | PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    panic!("timed out waiting for process exit");
}

fn unique_temp_dir() -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let suffix = NEXT_TEMP_DIR_ID.fetch_add(1, Ordering::Relaxed);
    let dir = env::temp_dir().join(format!(
        "panesmith-echo-tui-{}-{}-{suffix}",
        std::process::id(),
        timestamp
    ));
    fs::create_dir_all(&dir).expect("temporary fixture directory should be created");
    dir
}

#[cfg(unix)]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn wait_for_pid_absent(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps should run for process verification");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if state.is_empty() {
            return;
        }

        if Instant::now() >= deadline {
            panic!("pid {pid} was not reaped before timeout; ps state={state:?}");
        }

        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn wait_for_pid_exited_or_absent(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps should run for process verification");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if state.is_empty() || state.starts_with('Z') {
            return;
        }

        if Instant::now() >= deadline {
            panic!("pid {pid} did not exit before timeout; ps state={state:?}");
        }

        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn wait_for_child_pid(path: &Path, timeout: Duration) -> u32 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(contents) = fs::read_to_string(path) {
            return contents
                .trim()
                .parse::<u32>()
                .expect("child pid file should contain a numeric pid");
        }

        if Instant::now() >= deadline {
            panic!("timed out waiting for child pid file at {}", path.display());
        }

        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn spawn_applies_args_cwd_and_env() {
    let cwd = unique_temp_dir();
    let fixture = fixture_path();
    let fixture_program = fixture.to_string_lossy().into_owned();
    let expected_cwd = cwd.canonicalize().expect("fixture cwd should canonicalize");

    let config = PaneConfig::command_with_args(fixture_program, ["--fixture-arg"])
        .with_cwd(&expected_cwd)
        .with_env("PANESMITH_TEST_ENV", "present");
    let mut process = spawn_fixture(config);
    let writer = process.writer();

    writer
        .write_bytes(b"__PANESMITH_ARGS__\n")
        .expect("fixture arg probe should write");
    wait_for_output(&mut process, "args:--fixture-arg\n", Duration::from_secs(3));

    writer
        .write_bytes(b"__PANESMITH_PWD__\n")
        .expect("fixture cwd probe should write");
    let cwd_output = wait_for_output(&mut process, "cwd:", Duration::from_secs(3));
    let actual_cwd = cwd_output
        .lines()
        .find_map(|line| line.strip_prefix("cwd:"))
        .map(PathBuf::from)
        .expect("fixture cwd probe should report a cwd line")
        .canonicalize()
        .expect("reported fixture cwd should canonicalize");
    assert_eq!(actual_cwd, expected_cwd);

    writer
        .write_bytes(b"__PANESMITH_ENV__:PANESMITH_TEST_ENV\n")
        .expect("fixture env probe should write");
    wait_for_output(
        &mut process,
        "env:PANESMITH_TEST_ENV=present\n",
        Duration::from_secs(3),
    );

    writer
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
    fs::remove_dir_all(cwd).expect("temporary fixture directory should be removed");
}

#[test]
fn writer_and_resize_reach_the_child_process() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let config = PaneConfig::command(fixture_program).with_size(Size::new(12, 34));
    let mut process = spawn_fixture(config);
    let writer = process.writer();

    writer
        .write_bytes(b"__PANESMITH_SIZE__\n")
        .expect("fixture size probe should write");
    wait_for_output(&mut process, "size:12x34\n", Duration::from_secs(3));

    writer
        .write_bytes(b"hello panesmith\n")
        .expect("fixture echo probe should write");
    wait_for_output(&mut process, "hello panesmith\n", Duration::from_secs(3));

    process
        .resize(Size::new(20, 70))
        .expect("portable PTY backend should resize the child");
    writer
        .write_bytes(b"__PANESMITH_SIZE__\n")
        .expect("fixture resized probe should write");
    wait_for_output(&mut process, "size:20x70\n", Duration::from_secs(3));

    writer
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    assert_eq!(wait_for_exit(&mut process, Duration::from_secs(3)), Some(0));
}

#[test]
fn kill_terminates_the_child_and_reports_one_exit() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command(fixture_program));

    process
        .kill()
        .expect("portable PTY backend should send a kill signal");
    let _first_exit = wait_for_exit(&mut process, Duration::from_secs(3));

    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if let Some(frame) = process.try_recv() {
            if let PtyFrame::Exited { code, .. } = frame {
                panic!("observed duplicate exit frame with code {code:?}");
            }
        } else {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
#[test]
fn dropping_process_reaps_a_busy_child() {
    let process = spawn_fixture(PaneConfig::command_with_args(
        "sh",
        ["-c", "while :; do printf x; done"],
    ));
    let pid = process
        .id()
        .parse::<u32>()
        .expect("portable PTY backend should expose a numeric child pid on unix");

    thread::sleep(Duration::from_millis(50));
    drop(process);

    wait_for_pid_absent(pid, Duration::from_secs(3));
}

#[cfg(unix)]
#[test]
fn kill_escalates_past_ignored_hup_and_reaps_descendants() {
    let temp_dir = unique_temp_dir();
    let child_pid_path = temp_dir.join("child.pid");
    let script = format!(
        "trap '' HUP TERM; sh -c 'trap \"\" HUP TERM; while :; do sleep 1; done' & echo $! > {}; while :; do sleep 1; done",
        shell_single_quote(&child_pid_path.to_string_lossy())
    );
    let config = PaneConfig::command_with_args("sh", ["-c", script.as_str()])
        .with_kill(KillConfig::new(Duration::from_millis(50), true));
    let mut process = spawn_fixture(config);
    let parent_pid = process
        .id()
        .parse::<u32>()
        .expect("portable PTY backend should expose a numeric child pid on unix");
    let child_pid = wait_for_child_pid(&child_pid_path, Duration::from_secs(3));

    process
        .kill()
        .expect("portable PTY backend should escalate termination past ignored HUP/TERM");
    let _ = wait_for_exit(&mut process, Duration::from_secs(3));
    wait_for_pid_absent(parent_pid, Duration::from_secs(3));
    wait_for_pid_absent(child_pid, Duration::from_secs(3));
    fs::remove_dir_all(temp_dir).expect("temporary fixture directory should be removed");
}

#[cfg(unix)]
#[test]
fn dropping_after_parent_exit_still_reaps_hup_ignoring_descendants() {
    let temp_dir = unique_temp_dir();
    let child_pid_path = temp_dir.join("child.pid");
    let script = format!(
        "sh -c 'trap \"\" HUP TERM; while :; do sleep 1; done' & echo $! > {}; exit 0",
        shell_single_quote(&child_pid_path.to_string_lossy())
    );
    let process = spawn_fixture(
        PaneConfig::command_with_args("sh", ["-c", script.as_str()])
            .with_kill(KillConfig::new(Duration::from_millis(50), true)),
    );
    let parent_pid = process
        .id()
        .parse::<u32>()
        .expect("portable PTY backend should expose a numeric child pid on unix");

    let child_pid = wait_for_child_pid(&child_pid_path, Duration::from_secs(3));
    wait_for_pid_exited_or_absent(parent_pid, Duration::from_secs(3));

    drop(process);
    wait_for_pid_absent(child_pid, Duration::from_secs(3));
    fs::remove_dir_all(temp_dir).expect("temporary fixture directory should be removed");
}

#[test]
fn high_output_with_tiny_queue_produces_overflow_and_recoveres() {
    let mut process = PortablePtyBackend
        .spawn(
            &PaneConfig::command_with_args(
                "sh",
                [
                    "-c",
                    "for i in $(seq 1 10000); do echo line $i; done; read _; echo trailer",
                ],
            )
            .with_output_queue_capacity(2),
        )
        .expect("high-output fixture should spawn");
    let writer = process.writer();

    let mut saw_overflow = false;
    let mut saw_output_after_overflow = false;
    let mut released_trailer = false;
    let mut total_dropped_frames: u64 = 0;
    let mut total_dropped_bytes: u64 = 0;

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        while let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => {
                    saw_overflow = true;
                    total_dropped_frames += dropped_frames;
                    total_dropped_bytes += dropped_bytes;
                    if !released_trailer {
                        writer
                            .write_bytes(b"release trailer\n")
                            .expect("trailer release should write to the child");
                        released_trailer = true;
                    }
                }
                PtyFrame::Output { .. } => {
                    if saw_overflow {
                        saw_output_after_overflow = true;
                    }
                }
                PtyFrame::Exited { .. } => {
                    assert!(saw_overflow, "should have observed overflow before exit");
                    assert!(
                        saw_output_after_overflow,
                        "output should resume after overflow before exit"
                    );

                    let stats = process.overflow_stats();
                    assert_eq!(
                        stats,
                        OverflowStats {
                            dropped_frames: total_dropped_frames,
                            dropped_bytes: total_dropped_bytes,
                        },
                        "cumulative stats should match the sum of observed overflow frames"
                    );
                    return;
                }
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error: {message}");
                }
                PtyFrame::CursorPositionRequest { .. } => {}
            }
        }
        thread::sleep(Duration::from_millis(10));
    }

    panic!("timed out waiting for high-output fixture to exit");
}
