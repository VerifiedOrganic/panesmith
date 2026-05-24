use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use panesmith_core::{PaneConfig, PortablePtyBackend, PtyBackend, PtyFrame, PtyProcess};

pub const WAIT_TIMEOUT: Duration = Duration::from_secs(5);
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

pub fn fixture_path(name: &str) -> PathBuf {
    let hyphenated = format!("CARGO_BIN_EXE_{name}");
    let underscored = format!("CARGO_BIN_EXE_{}", name.replace('-', "_"));

    env::var(&hyphenated)
        .or_else(|_| env::var(&underscored))
        .map(PathBuf::from)
        .unwrap_or_else(|_| panic!("{name} fixture path should be available to integration tests"))
}

pub fn spawn_fixture(config: PaneConfig) -> impl PtyProcess {
    PortablePtyBackend
        .spawn(&config)
        .expect("fixture should spawn through portable PTY backend")
}

pub fn normalize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace('\r', "")
}

pub fn wait_for_exit(process: &mut impl PtyProcess, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;
    let mut _residual_output = Vec::new();
    let mut _residual_cpr_count = 0u64;

    while Instant::now() < deadline {
        while let Some(frame) = process.try_recv() {
            match frame {
                PtyFrame::Exited { code, .. } => return code,
                PtyFrame::Error { message, .. } => {
                    panic!("unexpected PTY error while waiting for exit: {message}");
                }
                PtyFrame::Output { bytes, .. } => {
                    _residual_output.extend_from_slice(&bytes);
                }
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => {
                    panic!(
                        "unexpected PTY overflow while waiting for exit: \
                         dropped {dropped_frames} frame(s) / {dropped_bytes} byte(s)"
                    );
                }
                PtyFrame::CursorPositionRequest { .. } => {
                    _residual_cpr_count += 1;
                }
            }
        }

        thread::sleep(POLL_INTERVAL);
    }

    panic!("timed out waiting for process exit");
}
