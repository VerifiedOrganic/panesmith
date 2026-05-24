use super::*;
#[cfg(unix)]
use std::process::Command;
use std::sync::Arc;

fn output_frame(seq: u64, bytes: &[u8]) -> PtyFrame {
    PtyFrame::Output {
        seq,
        bytes: bytes.to_vec(),
        at: Instant::now(),
    }
}

#[derive(Clone, Default)]
struct CleanupFlags {
    killed: Arc<Mutex<bool>>,
    waited: Arc<Mutex<bool>>,
}

#[derive(Default)]
struct FakeCleanupChild {
    flags: CleanupFlags,
}

impl ReaderThreadSpawnCleanup for FakeCleanupChild {
    fn cleanup_after_reader_thread_spawn_failure(&mut self) -> Vec<String> {
        *self.flags.killed.lock().expect("kill flag lock") = true;
        *self.flags.waited.lock().expect("wait flag lock") = true;
        Vec::new()
    }
}

struct FakeReaderThreadState {
    child: FakeCleanupChild,
}

#[test]
fn reader_thread_spawn_failures_cleanup_the_already_started_child() {
    let flags = CleanupFlags::default();
    let err = spawn_named_thread_with_state(
        "panesmith-test".into(),
        FakeReaderThreadState {
            child: FakeCleanupChild {
                flags: flags.clone(),
            },
        },
        |_state| panic!("task should not run when the spawner fails"),
        |state, error| reader_thread_spawn_error(error, &mut state.child),
        |_builder, _task| Err(std::io::Error::other("thread quota reached")),
    )
    .expect_err("thread spawn should fail");

    assert!(
        matches!(&err, PaneError::Spawn { message }
                if message.contains("failed to spawn PTY reader thread")
                    && message.contains("thread quota reached")),
        "expected mapped spawn error, got {err:?}"
    );
    assert!(
        *flags.killed.lock().expect("kill flag lock"),
        "spawn failure should kill the started child process"
    );
    assert!(
        *flags.waited.lock().expect("wait flag lock"),
        "spawn failure should reap the started child process"
    );
}

#[test]
fn bounded_queue_summarizes_output_overflow() {
    let queue = PtyFrameQueue::new(2);
    queue.push(output_frame(1, b"one"));
    queue.push(output_frame(2, b"two"));
    queue.push(output_frame(3, b"three"));
    queue.push(output_frame(4, b"four"));

    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 1, bytes, .. }) if bytes == b"one"
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 2, bytes, .. }) if bytes == b"two"
    ));

    match queue.try_recv() {
        Some(PtyFrame::Overflow {
            seq,
            dropped_frames,
            dropped_bytes,
            ..
        }) => {
            assert_eq!(seq, 3);
            assert_eq!(dropped_frames, 2);
            assert_eq!(dropped_bytes, (b"three".len() + b"four".len()) as u64);
        }
        other => panic!("expected overflow summary frame, got {other:?}"),
    }

    assert!(queue.try_recv().is_none());
}

#[test]
fn bounded_queue_keeps_terminal_frames_after_output_overflow() {
    let queue = PtyFrameQueue::new(1);
    let exited_at = Instant::now();
    queue.push(output_frame(1, b"first"));
    queue.push(output_frame(2, b"second"));
    queue.push(PtyFrame::Exited {
        seq: 3,
        code: Some(0),
        at: exited_at,
    });

    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 1, bytes, .. }) if bytes == b"first"
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Overflow {
            seq: 2,
            dropped_frames: 1,
            dropped_bytes,
            ..
        }) if dropped_bytes == b"second".len() as u64
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Exited {
            seq: 3,
            code: Some(0),
            ..
        })
    ));
    assert!(queue.try_recv().is_none());
}

#[test]
fn output_resumes_after_overflow_summary_is_consumed() {
    let queue = PtyFrameQueue::new(1);
    queue.push(output_frame(1, b"first"));
    queue.push(output_frame(2, b"second"));

    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 1, bytes, .. }) if bytes == b"first"
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Overflow {
            seq: 2,
            dropped_frames: 1,
            ..
        })
    ));

    // After the overflow summary is consumed, new output should be queued again.
    queue.push(output_frame(3, b"third"));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 3, bytes, .. }) if bytes == b"third"
    ));
    assert!(queue.try_recv().is_none());
}

#[test]
fn cumulative_overflow_stats_track_total_loss() {
    let queue = PtyFrameQueue::new(1);
    assert_eq!(
        queue.overflow_stats(),
        OverflowStats {
            dropped_frames: 0,
            dropped_bytes: 0,
        }
    );

    queue.push(output_frame(1, b"a"));
    queue.push(output_frame(2, b"bb"));
    queue.push(output_frame(3, b"ccc"));

    // Consume the single allowed output frame.
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 1, .. })
    ));

    // Consume the coalesced overflow summary.
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Overflow {
            seq: 2,
            dropped_frames: 2,
            dropped_bytes: 5,
            ..
        })
    ));

    // Cumulative stats should reflect all dropped frames/bytes.
    assert_eq!(
        queue.overflow_stats(),
        OverflowStats {
            dropped_frames: 2,
            dropped_bytes: 5,
        }
    );

    // Trigger another round of overflow.
    queue.push(output_frame(4, b"dddd"));
    queue.push(output_frame(5, b"eeeee"));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Output { seq: 4, .. })
    ));
    assert!(matches!(
        queue.try_recv(),
        Some(PtyFrame::Overflow {
            seq: 5,
            dropped_frames: 1,
            dropped_bytes: 5,
            ..
        })
    ));

    assert_eq!(
        queue.overflow_stats(),
        OverflowStats {
            dropped_frames: 3,
            dropped_bytes: 10,
        }
    );
}

#[cfg(unix)]
#[test]
fn process_target_sigkill_is_skipped_once_child_exit_is_observed() {
    let child_exited = AtomicBool::new(true);

    assert!(
        should_skip_unix_sigkill(
            // SAFETY: `getpid` has no preconditions and only returns the
            // current process id for a liveness-probe unit test.
            UnixKillTarget::Process(unsafe { libc::getpid() }),
            &child_exited,
        )
        .expect("exit flag check should succeed"),
        "process-target escalation should stop once the child exit flag is observed",
    );
}

#[cfg(unix)]
#[test]
fn process_target_sigkill_is_not_skipped_when_child_is_still_running() {
    let child_exited = AtomicBool::new(false);

    assert!(
        !should_skip_unix_sigkill(
            // SAFETY: `getpid` has no preconditions and only returns the
            // current process id for a liveness-probe unit test.
            UnixKillTarget::Process(unsafe { libc::getpid() }),
            &child_exited,
        )
        .expect("liveness probe should succeed"),
        "process-target escalation should proceed when child has not exited",
    );
}

#[cfg(unix)]
#[test]
fn descendant_kill_target_falls_back_to_the_child_when_group_is_not_ready() {
    let child_exited = AtomicBool::new(false);
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("helper test should spawn a child process");
    let child_pid = libc::pid_t::try_from(child.id()).expect("child pid should fit pid_t");

    let target = resolve_initial_unix_kill_target(child_pid, true, &child_exited)
        .expect("target resolution should succeed");

    assert_eq!(target, Some(UnixKillTarget::Process(child_pid)));

    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn process_group_sigkill_is_not_skipped_only_because_the_child_exited() {
    let child_exited = AtomicBool::new(true);

    assert!(
        !should_skip_unix_sigkill(
            // SAFETY: `getpgrp` has no preconditions and returns the current
            // process group id for a liveness-probe unit test.
            UnixKillTarget::ProcessGroup(unsafe { libc::getpgrp() }),
            &child_exited,
        )
        .expect("group liveness probe should succeed"),
        "process-group escalation should still be allowed while the group remains alive",
    );
}
