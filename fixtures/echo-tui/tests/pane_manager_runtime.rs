use std::env;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use panesmith_core::{
    KillConfig, KillReason, PaneConfig, PaneError, PaneEvent, PaneEventKind, PaneExit, PaneId,
    PaneManager, PaneManagerConfig, PaneState, ReproDumpOptions, Size, SurfaceRow,
    TranscriptConfig, TranscriptMode,
};

fn fixture_path() -> PathBuf {
    env::var("CARGO_BIN_EXE_echo-tui")
        .or_else(|_| env::var("CARGO_BIN_EXE_echo_tui"))
        .map(PathBuf::from)
        .expect("echo-tui fixture path should be available to integration tests")
}

fn fixture_env_key() -> &'static str {
    if env::var_os("CARGO_BIN_EXE_echo-tui").is_some() {
        "CARGO_BIN_EXE_echo-tui"
    } else {
        "CARGO_BIN_EXE_echo_tui"
    }
}

fn row_text(row: &SurfaceRow<'_>) -> String {
    row.cells.iter().map(|cell| cell.text.as_ref()).collect()
}

fn surface_text(manager: &mut PaneManager, pane_id: PaneId) -> String {
    let snapshot = manager
        .snapshot(pane_id)
        .expect("pane should still exist while waiting for output")
        .to_owned_snapshot();
    snapshot
        .surface
        .rows
        .iter()
        .map(row_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn drain_now(manager: &mut PaneManager) -> Vec<PaneEvent> {
    let mut events = Vec::new();
    manager.drain_events(&mut events);
    events
}

fn wait_for_snapshot_contains(
    manager: &mut PaneManager,
    pane_id: PaneId,
    needle: &str,
    timeout: Duration,
) -> Vec<PaneEvent> {
    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();

    while Instant::now() < deadline {
        collected.extend(drain_now(manager));
        if surface_text(manager, pane_id).contains(needle) {
            collected.extend(drain_now(manager));
            return collected;
        }
        thread::sleep(Duration::from_millis(10));
    }

    panic!(
        "timed out waiting for snapshot to contain {needle:?}; saw surface:\n{}\ncollected events: {collected:?}",
        surface_text(manager, pane_id)
    );
}

fn wait_for_plain_transcript_occurrences(
    manager: &mut PaneManager,
    pane_id: PaneId,
    needle: &str,
    expected_count: usize,
    timeout: Duration,
) -> Vec<PaneEvent> {
    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();

    while Instant::now() < deadline {
        collected.extend(drain_now(manager));
        let transcript = manager
            .plain_transcript(pane_id)
            .expect("pane should still exist while waiting for transcript");
        if transcript.matches(needle).count() >= expected_count {
            collected.extend(drain_now(manager));
            return collected;
        }
        thread::sleep(Duration::from_millis(10));
    }

    panic!(
        "timed out waiting for transcript to contain {needle:?} at least {expected_count} time(s); saw transcript:\n{}\ncollected events: {collected:?}",
        manager
            .plain_transcript(pane_id)
            .expect("pane should still exist for timeout diagnostics")
    );
}

fn wait_for(
    manager: &mut PaneManager,
    pane_id: PaneId,
    timeout: Duration,
    predicate: impl Fn(&PaneState, &[PaneEvent]) -> bool,
) -> Vec<PaneEvent> {
    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();

    while Instant::now() < deadline {
        collected.extend(drain_now(manager));
        let state = manager
            .snapshot(pane_id)
            .expect("pane should still exist while waiting for state")
            .state;
        if predicate(&state, &collected) {
            collected.extend(drain_now(manager));
            return collected;
        }
        thread::sleep(Duration::from_millis(10));
    }

    panic!(
        "timed out waiting for pane state/event predicate; state={:?}, surface=\n{}\ncollected events: {collected:?}",
        manager
            .snapshot(pane_id)
            .expect("pane should still exist for timeout diagnostics")
            .state,
        surface_text(manager, pane_id)
    );
}

fn assert_spawn_sequence(events: &[PaneEvent]) {
    assert!(matches!(
        events,
        [
            PaneEvent {
                kind: PaneEventKind::Spawned(_),
                seq: 1,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::StateChanged(change),
                seq: 2,
                ..
            },
            ..
        ] if matches!(change.as_ref(), panesmith_core::StateChangedEvent {
            old: PaneState::Starting,
            new: PaneState::Running,
        })
    ));
}

fn assert_has_input_output_and_surface(events: &[PaneEvent]) {
    assert!(
        matches!(
            events.first(),
            Some(PaneEvent {
                kind: PaneEventKind::InputSent(_),
                ..
            })
        ),
        "expected the first post-write event to be InputSent, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Output(_))),
        "expected output metadata event, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::SurfaceChanged(_))),
        "expected surface-changed event, got {events:?}"
    );
}

fn assert_output_events_include_transcript_offsets(events: &[PaneEvent]) {
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            PaneEventKind::Output(panesmith_core::OutputEvent {
                transcript_offset: Some(_),
                ..
            })
        )),
        "expected output metadata to carry transcript offsets, got {events:?}"
    );
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

#[test]
fn pane_manager_runtime_supports_spawn_write_snapshot_resize_and_remove() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 80))
                .with_transcript(TranscriptConfig::new(TranscriptMode::Both))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");

    let spawn_events = drain_now(&mut manager);
    assert_spawn_sequence(&spawn_events);
    assert_eq!(
        manager.snapshot(pane_id).expect("pane should exist").state,
        PaneState::Running
    );

    manager
        .write_bytes(pane_id, b"__PANESMITH_PID__\n")
        .expect("pid probe should reach the child process");
    let pid_events =
        wait_for_snapshot_contains(&mut manager, pane_id, "pid:", Duration::from_secs(3));
    assert_has_input_output_and_surface(&pid_events);
    assert_output_events_include_transcript_offsets(&pid_events);
    #[cfg(unix)]
    let child_pid = manager
        .plain_transcript(pane_id)
        .expect("plain transcript should be available")
        .lines()
        .find_map(|line| line.strip_prefix("pid:"))
        .expect("pid probe should be captured in the transcript")
        .parse::<u32>()
        .expect("fixture should report a numeric pid");

    manager
        .write_bytes(pane_id, b"hello panesmith\n")
        .expect("write_bytes should reach the child process");
    let write_events = wait_for_snapshot_contains(
        &mut manager,
        pane_id,
        "hello panesmith",
        Duration::from_secs(3),
    );
    assert_has_input_output_and_surface(&write_events);
    assert_output_events_include_transcript_offsets(&write_events);

    manager
        .write_bytes(pane_id, b"__PANESMITH_SIZE__\n")
        .expect("size probe should reach the child process");
    let size_events =
        wait_for_snapshot_contains(&mut manager, pane_id, "size:12x80", Duration::from_secs(3));
    assert_has_input_output_and_surface(&size_events);
    assert_output_events_include_transcript_offsets(&size_events);

    manager
        .resize(pane_id, Size::new(20, 70))
        .expect("resize should reach the PTY and the surface");
    manager
        .write_bytes(pane_id, b"__PANESMITH_SIZE__\n")
        .expect("post-resize size probe should reach the child process");
    let resize_events =
        wait_for_snapshot_contains(&mut manager, pane_id, "size:20x70", Duration::from_secs(3));
    assert!(
        matches!(
            resize_events.first(),
            Some(PaneEvent {
                kind: PaneEventKind::Resized(_),
                ..
            })
        ),
        "expected resize to emit a Resized event first, got {resize_events:?}"
    );
    assert_output_events_include_transcript_offsets(&resize_events);
    assert!(
        resize_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::InputSent(_))),
        "expected the size probe write to emit InputSent, got {resize_events:?}"
    );
    assert!(
        resize_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Output(_))),
        "expected post-resize output, got {resize_events:?}"
    );
    let plain_transcript = manager
        .plain_transcript(pane_id)
        .expect("plain transcript should be available");
    assert!(
        plain_transcript.contains("hello panesmith"),
        "expected echoed input in the plain transcript, got {plain_transcript:?}"
    );
    assert!(
        plain_transcript.contains("size:12x80"),
        "expected the initial size probe in the plain transcript, got {plain_transcript:?}"
    );
    assert!(
        plain_transcript.contains("size:20x70"),
        "expected the resized size probe in the plain transcript, got {plain_transcript:?}"
    );

    let raw_transcript = String::from_utf8_lossy(
        manager
            .raw_transcript(pane_id)
            .expect("raw transcript should be available"),
    );
    assert!(
        raw_transcript.contains("hello panesmith"),
        "expected echoed output in the raw transcript, got {raw_transcript:?}"
    );
    assert!(
        raw_transcript.contains("size:12x80"),
        "expected the initial size probe in the raw transcript, got {raw_transcript:?}"
    );
    assert!(
        raw_transcript.contains("size:20x70"),
        "expected the resized size probe in the raw transcript, got {raw_transcript:?}"
    );

    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\n")
        .expect("exit probe should reach the child process");
    let exit_events = wait_for(
        &mut manager,
        pane_id,
        Duration::from_secs(3),
        |state, events| {
            matches!(state, PaneState::Exited { code: Some(0) })
                && events
                    .iter()
                    .any(|event| matches!(event.kind, PaneEventKind::Exited(_)))
        },
    );
    assert!(
        exit_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Exited(_))),
        "expected a structured Exited event, got {exit_events:?}"
    );
    assert_eq!(
        manager.remove(pane_id).expect("remove should succeed"),
        Some(PaneExit::Exited { code: Some(0) })
    );
    #[cfg(unix)]
    wait_for_pid_absent(child_pid, Duration::from_secs(3));
}

#[test]
fn pane_manager_spawn_uses_default_backend_environment_policy() {
    let fixture_key = fixture_env_key();
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_clear_env()
                .with_env("PANESMITH_MANAGER_ENV", "present")
                .with_transcript(TranscriptConfig::new(TranscriptMode::PlainText))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");
    let _spawn_events = drain_now(&mut manager);

    manager
        .write_bytes(pane_id, b"__PANESMITH_ENV__:PANESMITH_MANAGER_ENV\n")
        .expect("explicit env probe should reach the child process");
    wait_for_snapshot_contains(
        &mut manager,
        pane_id,
        "env:PANESMITH_MANAGER_ENV=present",
        Duration::from_secs(3),
    );
    let transcript = manager
        .plain_transcript(pane_id)
        .expect("plain transcript should be retained");
    assert!(
        transcript
            .lines()
            .any(|line| line == "env:PANESMITH_MANAGER_ENV=present"),
        "explicit environment value should be visible in transcript: {transcript:?}"
    );

    manager
        .write_bytes(
            pane_id,
            format!("__PANESMITH_ENV__:{fixture_key}\n").as_bytes(),
        )
        .expect("disallowed parent env probe should reach the child process");
    wait_for_snapshot_contains(
        &mut manager,
        pane_id,
        &format!("env:{fixture_key}="),
        Duration::from_secs(3),
    );
    let expected_absent_line = format!("env:{fixture_key}=");
    let transcript = manager
        .plain_transcript(pane_id)
        .expect("plain transcript should be retained");
    assert!(
        transcript.lines().any(|line| line == expected_absent_line),
        "disallowed parent environment value should not be visible in transcript: {transcript:?}"
    );

    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\n")
        .expect("fixture exit request should write");
    wait_for(&mut manager, pane_id, Duration::from_secs(3), |state, _| {
        matches!(state, PaneState::Exited { code: Some(0) })
    });
    manager
        .remove(pane_id)
        .expect("exited pane should be removable");
}

#[test]
fn pane_manager_runtime_does_not_render_ansi_sequences_as_literal_text() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 80))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");

    let _spawn_events = drain_now(&mut manager);
    manager
        .write_bytes(pane_id, b"__PANESMITH_ANSI__\n")
        .expect("ansi probe should reach the child process");
    let events = wait_for_snapshot_contains(&mut manager, pane_id, "red", Duration::from_secs(3));
    assert_has_input_output_and_surface(&events);

    let surface = surface_text(&mut manager, pane_id);
    assert!(
        surface.contains("red"),
        "expected visible text in snapshot, got {surface:?}"
    );
    assert!(
        !surface.contains("[31m") && !surface.contains("[0m"),
        "expected escape sequences to be consumed by the default surface backend, got {surface:?}"
    );

    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\n")
        .expect("exit probe should reach the child process");
    let _ = wait_for(
        &mut manager,
        pane_id,
        Duration::from_secs(3),
        |state, events| {
            matches!(state, PaneState::Exited { code: Some(0) })
                && events
                    .iter()
                    .any(|event| matches!(event.kind, PaneEventKind::Exited(_)))
        },
    );
    assert_eq!(
        manager.remove(pane_id).expect("remove should succeed"),
        Some(PaneExit::Exited { code: Some(0) })
    );
}

#[test]
fn pane_manager_runtime_rejects_remove_while_child_is_still_running() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 80))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");

    let _spawn_events = drain_now(&mut manager);
    let err = manager
        .remove(pane_id)
        .expect_err("remove should reject live panes");
    assert!(
        matches!(
            err,
            PaneError::InvalidState { ref expected, ref actual }
                if expected == "Exited, Failed, or Killed" && actual == "Running"
        ),
        "expected InvalidState for a live pane, got {err:?}"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist")
            .state,
        PaneState::Running
    );

    manager
        .kill(pane_id, KillReason::UserRequested)
        .expect("kill should terminate the child process");
    let _ = wait_for(
        &mut manager,
        pane_id,
        Duration::from_secs(3),
        |state, events| {
            matches!(
                state,
                PaneState::Killed {
                    reason: KillReason::UserRequested
                }
            ) && events
                .iter()
                .any(|event| matches!(event.kind, PaneEventKind::Exited(_)))
        },
    );
    assert_eq!(
        manager
            .remove(pane_id)
            .expect("remove should succeed after kill"),
        Some(PaneExit::Killed {
            reason: KillReason::UserRequested,
        })
    );
}

#[test]
fn pane_manager_runtime_rejects_single_row_default_surface() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let err = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(1, 20))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect_err("default surface should reject single-row panes");

    assert_eq!(
        err,
        PaneError::Surface {
            message: "default surface backend requires at least two rows; got rows=1 cols=20"
                .to_string(),
        }
    );
}

#[test]
fn pane_manager_runtime_rejects_single_row_resize_without_resizing_child() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(2, 20))
                .with_transcript(TranscriptConfig::new(TranscriptMode::Both))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("two-row default surface should spawn");

    let _spawn_events = drain_now(&mut manager);
    manager
        .write_bytes(pane_id, b"__PANESMITH_SIZE__\n")
        .expect("initial size probe should reach the child process");
    let initial_events = wait_for_plain_transcript_occurrences(
        &mut manager,
        pane_id,
        "size:2x20",
        1,
        Duration::from_secs(3),
    );
    assert_has_input_output_and_surface(&initial_events);
    assert_output_events_include_transcript_offsets(&initial_events);

    let err = manager
        .resize(pane_id, Size::new(1, 20))
        .expect_err("default surface should reject single-row resize");
    assert_eq!(
        err,
        PaneError::Surface {
            message: "default surface backend requires at least two rows; got rows=1 cols=20"
                .to_string(),
        }
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist after rejected resize")
            .size,
        Size::new(2, 20)
    );
    let rejected_resize_events = drain_now(&mut manager);
    assert!(
        !rejected_resize_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Resized(_))),
        "rejected resize should not emit a Resized event, got {rejected_resize_events:?}"
    );

    manager
        .write_bytes(pane_id, b"__PANESMITH_SIZE__\n")
        .expect("post-rejection size probe should reach the child process");
    let post_rejection_events = wait_for_plain_transcript_occurrences(
        &mut manager,
        pane_id,
        "size:2x20",
        2,
        Duration::from_secs(3),
    );
    assert_has_input_output_and_surface(&post_rejection_events);
    assert_output_events_include_transcript_offsets(&post_rejection_events);
    let plain_transcript = manager
        .plain_transcript(pane_id)
        .expect("plain transcript should be available");
    assert_eq!(
        plain_transcript.matches("size:2x20").count(),
        2,
        "child PTY should stay at the original size after a rejected resize"
    );

    manager
        .kill(pane_id, KillReason::UserRequested)
        .expect("kill should terminate the child process");
    let _ = wait_for(
        &mut manager,
        pane_id,
        Duration::from_secs(3),
        |state, events| {
            matches!(
                state,
                PaneState::Killed {
                    reason: KillReason::UserRequested
                }
            ) && events
                .iter()
                .any(|event| matches!(event.kind, PaneEventKind::Exited(_)))
        },
    );
    assert_eq!(
        manager
            .remove(pane_id)
            .expect("remove should succeed after kill"),
        Some(PaneExit::Killed {
            reason: KillReason::UserRequested,
        })
    );
}

#[test]
fn pane_manager_runtime_remove_after_kill_preserves_exited_event() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 80))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");

    let _spawn_events = drain_now(&mut manager);
    manager
        .kill(pane_id, KillReason::UserRequested)
        .expect("kill should terminate the child process");
    assert_eq!(
        manager
            .remove(pane_id)
            .expect("remove should succeed after kill"),
        Some(PaneExit::Killed {
            reason: KillReason::UserRequested,
        })
    );

    let post_remove_events = drain_now(&mut manager);
    assert!(
        post_remove_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Exited(_))),
        "expected remove() to preserve an Exited event after kill, got {post_remove_events:?}"
    );
}

#[test]
fn pane_manager_runtime_remove_succeeds_after_natural_exit_without_prior_drain() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 80))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");

    let _spawn_events = drain_now(&mut manager);
    manager
        .write_bytes(pane_id, b"__PANESMITH_EXIT__\n")
        .expect("exit probe should reach the child process");

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match manager.remove(pane_id) {
            Ok(Some(PaneExit::Exited { code: Some(0) })) => break,
            Err(PaneError::InvalidState { .. }) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            other => panic!(
                "expected remove() to succeed once the queued exit frame was available without a prior drain_events() call, got {other:?}"
            ),
        }
    }

    let post_remove_events = drain_now(&mut manager);
    assert!(
        post_remove_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Exited(_))),
        "expected remove() to preserve the queued Exited event, got {post_remove_events:?}"
    );
}

#[test]
fn pane_manager_runtime_supports_kill_and_reports_structured_events() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 80))
                .with_kill(KillConfig::new(Duration::from_millis(50), true)),
        )
        .expect("fixture should spawn through PaneManager");

    let spawn_events = drain_now(&mut manager);
    assert_spawn_sequence(&spawn_events);

    manager
        .kill(pane_id, KillReason::UserRequested)
        .expect("kill should terminate the child process");
    let kill_events = wait_for(
        &mut manager,
        pane_id,
        Duration::from_secs(3),
        |state, events| {
            matches!(
                state,
                PaneState::Killed {
                    reason: KillReason::UserRequested
                }
            ) && events
                .iter()
                .any(|event| matches!(event.kind, PaneEventKind::Exited(_)))
        },
    );

    assert!(
        matches!(
            kill_events.first(),
            Some(PaneEvent {
                kind: PaneEventKind::StateChanged(change),
                ..
            }) if matches!(change.as_ref(), panesmith_core::StateChangedEvent {
                old: PaneState::Running,
                new: PaneState::Killed { reason: KillReason::UserRequested },
            })
        ),
        "expected the kill transition to be recorded before the exit frame, got {kill_events:?}"
    );
    assert!(
        kill_events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Exited(_))),
        "expected a structured Exited event after kill, got {kill_events:?}"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist")
            .state,
        PaneState::Killed {
            reason: KillReason::UserRequested,
        }
    );
    assert_eq!(
        manager.remove(pane_id).expect("remove should succeed"),
        Some(PaneExit::Killed {
            reason: KillReason::UserRequested,
        })
    );
}

#[test]
fn pane_manager_runtime_dump_repro_replays_fixture_surface() {
    let fixture_program = fixture_path().to_string_lossy().into_owned();
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(
            PaneConfig::command(fixture_program)
                .with_size(Size::new(12, 40))
                .with_env("SECRET_TOKEN", "shh")
                .with_transcript(TranscriptConfig::new(TranscriptMode::Both)),
        )
        .expect("fixture should spawn through PaneManager");

    let spawn_events = drain_now(&mut manager);
    assert_spawn_sequence(&spawn_events);

    manager
        .write_bytes(pane_id, b"__PANESMITH_ANSI__\n")
        .expect("ansi probe should reach the child process");
    let _ansi_events =
        wait_for_snapshot_contains(&mut manager, pane_id, "red", Duration::from_secs(3));

    manager
        .resize(pane_id, Size::new(20, 50))
        .expect("resize should reach the PTY and surface");
    manager
        .write_bytes(pane_id, b"__PANESMITH_SIZE__\n")
        .expect("size probe should reach the child process");
    let _size_events =
        wait_for_snapshot_contains(&mut manager, pane_id, "size:20x50", Duration::from_secs(3));

    let dump = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect("dump_repro should succeed");
    let replay = dump.replay().expect("repro dump should be replayable");
    let replayed_surface = replay.final_surface().expect("replay should succeed");

    assert_eq!(
        replayed_surface, dump.final_surface,
        "replayed surface should match the recorded final surface"
    );
    assert!(
        dump.events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Resized(_))),
        "repro dump should retain resize events"
    );
    assert_eq!(
        dump.spawn_config
            .env
            .get("SECRET_TOKEN")
            .map(String::as_str),
        Some("<redacted>")
    );
    let raw = dump
        .raw_transcript
        .as_ref()
        .expect("both-mode transcript should include raw bytes");
    assert!(
        !raw.bytes.is_empty(),
        "raw transcript should retain fixture output for replay"
    );
    assert_eq!(dump.final_surface.size, Size::new(20, 50));
}
