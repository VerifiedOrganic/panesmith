#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use panesmith_core::{
    HostInput, InputKind, IoOperation, KeyCode, KeyEventKind, KeyInput, KeyModifiers, PaneConfig,
    PaneError, PaneEventKind, PaneManager, PaneManagerConfig, PaneState,
};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const WAIT_TIMEOUT: Duration = Duration::from_secs(3);

#[test]
fn send_input_types_into_echo_fixture_and_records_key_metadata() {
    let output_path = unique_output_path("typed-input");
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            shell_config(r#"IFS= read -r line; printf '%s\n' "$line" > "$PANESMITH_OUT""#)
                .with_env("PANESMITH_OUT", output_path.to_string_lossy().into_owned()),
        )
        .expect("echo fixture should spawn");

    manager
        .send_input(pane_id, key_input(KeyCode::Char('h')))
        .expect("first key should write");
    manager
        .send_input(pane_id, key_input(KeyCode::Char('i')))
        .expect("second key should write");
    manager
        .send_input(pane_id, key_input(KeyCode::Enter))
        .expect("enter should write");

    let echoed = wait_for_file_bytes(&output_path, Some(3));
    assert_eq!(echoed, b"hi\n");

    let input_events = drain_input_events(&mut manager);
    assert_eq!(
        input_events
            .iter()
            .map(|event| (event.input_kind, event.bytes_len, event.recorded))
            .collect::<Vec<_>>(),
        vec![
            (InputKind::Key, 1, false),
            (InputKind::Key, 1, false),
            (InputKind::Key, 1, false)
        ]
    );

    cleanup(&mut manager, pane_id, &output_path);
}

#[test]
fn send_input_uses_combined_surface_modes_and_omits_payload_text() {
    let output_path = unique_output_path("bracketed-paste");
    let paste = "hello\nworld";
    let expected = b"\x1b[I\x1b[200~hello\nworld\x1b[201~";
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            shell_config(&format!(
                r#"printf '\033[?1004;2004h'; stty raw -echo; dd of="$PANESMITH_OUT" bs=1 count={} 2>/dev/null"#,
                expected.len()
            ))
            .with_env("PANESMITH_OUT", output_path.to_string_lossy().into_owned()),
        )
        .expect("paste fixture should spawn");

    wait_for(
        "pane should observe focus and bracketed paste modes",
        || {
            manager
                .snapshot(pane_id)
                .map(|snapshot| snapshot.modes.focus_events && snapshot.modes.bracketed_paste)
                .unwrap_or(false)
        },
    );

    manager
        .send_input(pane_id, HostInput::FocusGained)
        .expect("focus gained should encode and write");
    manager
        .send_input(pane_id, HostInput::Paste(paste.into()))
        .expect("paste input should encode and write");

    let pasted = wait_for_file_bytes(&output_path, Some(expected.len()));
    assert_eq!(pasted, expected);

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    let input_events = events
        .into_iter()
        .filter_map(|event| {
            let debug_repr = format!("{event:?}");
            match event.kind {
                PaneEventKind::InputSent(sent) => Some((sent, debug_repr)),
                _ => None,
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        input_events
            .iter()
            .map(|(event, _)| (event.input_kind, event.bytes_len, event.recorded))
            .collect::<Vec<_>>(),
        vec![
            (InputKind::Bytes, 3, false),
            (InputKind::Paste, expected.len() - 3, false)
        ]
    );
    for (_, debug_repr) in input_events {
        assert!(
            !debug_repr.contains(paste),
            "input event should record metadata only, got {debug_repr}"
        );
    }

    cleanup(&mut manager, pane_id, &output_path);
}

#[test]
fn send_input_surfaces_write_failures_as_structured_io_errors() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(shell_config("exit 0"))
        .expect("short-lived fixture should spawn");

    wait_for("fixture should exit before the write", || {
        let mut events = Vec::new();
        manager.drain_events(&mut events);
        manager
            .snapshot(pane_id)
            .map(|snapshot| matches!(snapshot.state, PaneState::Exited { .. }))
            .unwrap_or(false)
    });

    let err = manager
        .send_input(pane_id, HostInput::Raw(vec![0x61]))
        .expect_err("writes to a closed PTY should fail");
    assert!(
        matches!(
            err,
            PaneError::Io {
                operation: IoOperation::Write,
                ..
            }
        ),
        "expected structured write error, got {err:?}"
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        events
            .into_iter()
            .any(|event| matches!(event.kind, PaneEventKind::Error(_))),
        "expected a pane error event after the failed write"
    );

    let _ = manager.remove(pane_id);
}

#[test]
fn snapshot_refreshes_exit_state_without_draining_events() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(shell_config("exit 0"))
        .expect("short-lived fixture should spawn");

    wait_for("snapshot should observe exit without drain_events", || {
        manager
            .snapshot(pane_id)
            .map(|snapshot| matches!(snapshot.state, PaneState::Exited { .. }))
            .unwrap_or(false)
    });

    let _ = manager.remove(pane_id);
}

fn shell_config(script: &str) -> PaneConfig {
    PaneConfig::command_with_args("sh", ["-lc", script])
}

fn key_input(code: KeyCode) -> HostInput {
    HostInput::Key(KeyInput::new(
        code,
        KeyModifiers::default(),
        KeyEventKind::Press,
    ))
}

fn drain_input_events(manager: &mut PaneManager) -> Vec<panesmith_core::InputSentEvent> {
    let mut events = Vec::new();
    manager.drain_events(&mut events);
    events
        .into_iter()
        .filter_map(|event| match event.kind {
            PaneEventKind::InputSent(input_event) => Some(input_event),
            _ => None,
        })
        .collect()
}

fn unique_output_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "panesmith-{label}-{}-{nonce}.out",
        std::process::id()
    ))
}

fn wait_for_file_bytes(path: &Path, expected_len: Option<usize>) -> Vec<u8> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        match fs::read(path) {
            Ok(bytes) => {
                if expected_len.is_none_or(|len| bytes.len() >= len) {
                    return bytes;
                }
                if Instant::now() < deadline {
                    thread::sleep(POLL_INTERVAL);
                    continue;
                }
                panic!(
                    "timed out waiting for {} bytes in {} (have {})",
                    expected_len.unwrap_or(bytes.len()),
                    path.display(),
                    bytes.len()
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                thread::sleep(POLL_INTERVAL);
            }
            Err(err) => panic!("failed to read {}: {err}", path.display()),
        }
    }
}

fn wait_for(description: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }
    panic!("timed out waiting for {description}");
}

fn cleanup(manager: &mut PaneManager, pane_id: panesmith_core::PaneId, path: &Path) {
    let _ = manager.remove(pane_id);
    let _ = fs::remove_file(path);
}
