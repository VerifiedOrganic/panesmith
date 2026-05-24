#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod support;

use panesmith_core::{
    OverflowStats, PaneConfig, PortablePtyBackend, PtyBackend, PtyFrame, PtyProcess, Size,
};
use support::{fixture_path, normalize, spawn_fixture, wait_for_exit, POLL_INTERVAL, WAIT_TIMEOUT};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

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

        thread::sleep(POLL_INTERVAL);
    }

    panic!("timed out waiting for {needle:?}; saw output {seen:?}");
}

fn assert_no_output(process: &mut impl PtyProcess, timeout: Duration) {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Output { bytes, .. } => {
                    panic!(
                        "expected no output before resize signal, saw {:?}",
                        normalize(&bytes)
                    );
                }
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => {
                    panic!(
                        "unexpected PTY overflow while asserting no pre-resize output: dropped {dropped_frames} frame(s) / {dropped_bytes} byte(s)"
                    );
                }
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error while asserting no pre-resize output: {message}");
                }
                PtyFrame::Exited { code, .. } => {
                    panic!("process exited early with code {code:?} before resize");
                }
                PtyFrame::CursorPositionRequest { .. } => {
                    panic!("unexpected CPR request from resize-reporter fixture");
                }
            }
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn unique_output_path(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let suffix = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);

    env::temp_dir().join(format!(
        "panesmith-{label}-{}-{timestamp}-{suffix}.out",
        std::process::id()
    ))
}

fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }

    panic!("timed out waiting for {}", path.display());
}

#[test]
fn alt_screen_fixture_emits_enter_draw_and_leave_sequences() {
    let output = Command::new(fixture_path("alt-screen"))
        .arg("--auto-exit")
        .output()
        .expect("alt-screen fixture should run");

    assert!(output.status.success());
    assert!(output.stdout.starts_with(b"\x1b[?1049h\x1b[2J\x1b[H"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("alt-screen ready"));
    assert!(output.stdout.ends_with(b"\x1b[?1049l"));
}

#[test]
fn bracketed_paste_fixture_enables_modes_and_captures_raw_bytes() {
    let capture_path = unique_output_path("bracketed-paste");
    let expected = b"\x1b[I\x1b[200~hello\nworld\x1b[201~";
    let mut child = Command::new(fixture_path("bracketed-paste"))
        .arg("--out")
        .arg(&capture_path)
        .arg("--count")
        .arg(expected.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("bracketed-paste fixture should spawn");
    let mut stdout = child
        .stdout
        .take()
        .expect("fixture stdout should be available");
    let mut stdin = child
        .stdin
        .take()
        .expect("fixture stdin should be available");

    let mut enable_modes = vec![0_u8; b"\x1b[?1004;2004h".len()];
    stdout
        .read_exact(&mut enable_modes)
        .expect("fixture should emit focus and bracketed paste enable sequences");
    assert_eq!(enable_modes.as_slice(), b"\x1b[?1004;2004h");

    stdin
        .write_all(expected)
        .expect("fixture should accept raw paste bytes");
    drop(stdin);

    let status = child.wait().expect("fixture should exit cleanly");
    assert!(status.success());
    let captured = fs::read(&capture_path).expect("captured bytes should be written");
    assert_eq!(captured, expected);
    fs::remove_file(&capture_path).expect("cleanup should succeed");
}

#[test]
fn cpr_query_fixture_requests_cursor_position_and_reports_the_reply() {
    let mut child = Command::new(fixture_path("cpr-query"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("cpr-query fixture should spawn");
    let mut stdout = child
        .stdout
        .take()
        .expect("fixture stdout should be available");
    let mut stdin = child
        .stdin
        .take()
        .expect("fixture stdin should be available");

    let mut query = [0_u8; 4];
    stdout
        .read_exact(&mut query)
        .expect("fixture should emit a CPR query");
    assert_eq!(&query, b"\x1b[6n");

    stdin
        .write_all(b"\x1b[12;34R")
        .expect("fixture should accept a CPR reply");
    drop(stdin);

    let mut remainder = String::new();
    stdout
        .read_to_string(&mut remainder)
        .expect("fixture should report the parsed CPR reply");
    let status = child.wait().expect("fixture should exit cleanly");
    assert!(status.success());
    assert_eq!(remainder, "cpr:12;34\n");
}

#[test]
fn exit_codes_fixture_reports_the_configured_exit_status() {
    let fixture_program = fixture_path("exit-codes").to_string_lossy().into_owned();
    let mut process = spawn_fixture(PaneConfig::command_with_args(
        fixture_program,
        ["--code", "42"],
    ));

    assert_eq!(wait_for_exit(&mut process, WAIT_TIMEOUT), Some(42));
}

#[test]
fn frame_storm_fixture_overflows_tiny_queues_and_output_recovers() {
    let fixture_program = fixture_path("frame-storm").to_string_lossy().into_owned();
    let mut process = PortablePtyBackend
        .spawn(
            &PaneConfig::command_with_args(
                fixture_program,
                ["--frames", "4000", "--trailer-sleep-ms", "50"],
            )
            .with_output_queue_capacity(2),
        )
        .expect("frame-storm fixture should spawn");

    let mut saw_overflow = false;
    let mut saw_output_after_overflow = false;
    let mut total_dropped_frames = 0_u64;
    let mut total_dropped_bytes = 0_u64;
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
                }
                PtyFrame::Output { .. } => {
                    if saw_overflow {
                        saw_output_after_overflow = true;
                    }
                }
                PtyFrame::Exited { code, .. } => {
                    assert_eq!(code, Some(0));
                    assert!(saw_overflow, "expected a queue overflow before exit");
                    assert!(
                        saw_output_after_overflow,
                        "expected output to continue after the overflow summary"
                    );
                    assert_eq!(
                        process.overflow_stats(),
                        OverflowStats {
                            dropped_frames: total_dropped_frames,
                            dropped_bytes: total_dropped_bytes,
                        }
                    );
                    return;
                }
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error: {message}");
                }
                PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        thread::sleep(POLL_INTERVAL);
    }

    panic!("timed out waiting for frame-storm fixture to exit");
}

#[test]
fn resize_reporter_fixture_observes_terminal_size_changes() {
    let fixture_program = fixture_path("resize-reporter")
        .to_string_lossy()
        .into_owned();
    let ready_path = unique_output_path("resize-reporter-ready");
    let mut process = spawn_fixture(
        PaneConfig::command(fixture_program)
            .with_size(Size::new(12, 34))
            .with_env(
                "PANESMITH_READY_FILE",
                ready_path.to_string_lossy().into_owned(),
            ),
    );
    let writer = process.writer();

    wait_for_path(&ready_path, WAIT_TIMEOUT);
    assert_no_output(&mut process, Duration::from_millis(200));
    process
        .resize(Size::new(20, 70))
        .expect("portable PTY backend should resize the child");
    wait_for_output(&mut process, "size:20x70\n", WAIT_TIMEOUT);
    writer
        .write_bytes(b"__PANESMITH_EXIT__\n")
        .expect("fixture should accept the exit command");
    assert_eq!(wait_for_exit(&mut process, WAIT_TIMEOUT), Some(0));
    fs::remove_file(&ready_path).expect("cleanup should succeed");
}

#[test]
fn slow_output_fixture_emits_delayed_chunks() {
    let delay_ms = 40_u64;
    let start = Instant::now();
    let mut child = Command::new(fixture_path("slow-output"))
        .arg("--delay-ms")
        .arg(delay_ms.to_string())
        .stdout(Stdio::piped())
        .spawn()
        .expect("slow-output fixture should spawn");
    let mut stdout = child
        .stdout
        .take()
        .expect("fixture stdout should be available");

    let mut first = [0_u8; 4];
    stdout
        .read_exact(&mut first)
        .expect("first chunk should arrive");
    assert_eq!(&first, b"slow");

    let mut second = [0_u8; 7];
    stdout
        .read_exact(&mut second)
        .expect("second chunk should arrive");
    assert_eq!(&second, b"-output");
    let after_second = start.elapsed();

    let mut newline = [0_u8; 1];
    stdout
        .read_exact(&mut newline)
        .expect("final newline should arrive");
    assert_eq!(&newline, b"\n");

    let status = child.wait().expect("fixture should exit cleanly");
    assert!(status.success());
    assert!(
        after_second >= Duration::from_millis(delay_ms),
        "second chunk should arrive after a visible delay"
    );
    assert!(
        start.elapsed() >= Duration::from_millis(delay_ms * 2),
        "full output should take at least two delay intervals"
    );
}
