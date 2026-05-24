use std::time::{Duration, Instant};

use super::*;

fn default_harness(pane_id: u64) -> AttachHarness {
    AttachHarness::new(
        PaneId::new(pane_id),
        AttachOptions::default(),
        Size::new(24, 80),
        Size::new(40, 120),
    )
}

#[test]
fn child_output_reaches_stdout_and_surface_fanout() {
    let mut harness = AttachHarness::new(
        PaneId::new(1),
        AttachOptions::default(),
        Size::new(24, 80),
        Size::new(40, 120),
    );

    harness.queue_pty_output(b"hello world".to_vec());
    harness.queue_stdin(Instant::now(), vec![0x1d]);

    harness.run().expect("attach harness should detach cleanly");

    assert_eq!(harness.terminal().stdout_bytes(), b"hello world".to_vec());
    assert_eq!(
        harness.surface().fed_output_bytes(),
        b"hello world".to_vec()
    );
    assert_eq!(
        harness.pty().resize_calls(),
        &[Size::new(40, 120), Size::new(24, 80)]
    );
    assert_eq!(harness.bridge_state(), AttachState::Embedded);
}

#[test]
fn user_input_reaches_child_pty_and_detach_stops_bridge() {
    let mut harness = AttachHarness::new(
        PaneId::new(2),
        AttachOptions::default(),
        Size::new(24, 80),
        Size::new(30, 100),
    );

    harness.queue_stdin(Instant::now(), b"abc".to_vec());
    harness.queue_stdin(Instant::now(), vec![0x1d, b'x']);

    harness.run().expect("attach harness should detach cleanly");

    assert_eq!(harness.pty().input_bytes(), b"abc".to_vec());
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(
        harness.control().restored_policies(),
        &[AttachScreenPolicy::ReuseHostAlternateScreen]
    );
    assert_eq!(harness.bridge_state(), AttachState::Embedded);
}

#[test]
fn resize_changes_pty_and_surface_targets() {
    let mut harness = AttachHarness::new(
        PaneId::new(3),
        AttachOptions::default(),
        Size::new(24, 80),
        Size::new(35, 110),
    );

    harness.queue_resize(Size::new(50, 160));
    harness.queue_stdin(Instant::now(), vec![0x1d]);

    harness.run().expect("attach harness should detach cleanly");

    assert_eq!(
        harness.pty().resize_calls(),
        &[Size::new(35, 110), Size::new(50, 160), Size::new(24, 80),]
    );
    assert_eq!(
        harness.surface().resize_calls(),
        &[Size::new(35, 110), Size::new(50, 160), Size::new(24, 80),]
    );
}

#[test]
fn errors_still_restore_terminal() {
    let mut harness = AttachHarness::new(
        PaneId::new(4),
        AttachOptions::default(),
        Size::new(24, 80),
        Size::new(40, 120),
    );
    harness.terminal_mut().fail_next_stdout_write();
    harness.queue_pty_output(b"boom".to_vec());

    let error = harness.run().expect_err("stdout failure should bubble up");

    assert_eq!(error, AttachHarnessError::Stdout("injected stdout failure"));
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(
        harness.control().restored_policies(),
        &[AttachScreenPolicy::ReuseHostAlternateScreen]
    );
    assert_eq!(harness.bridge_state(), AttachState::Attached);
}

#[test]
fn partial_detach_timeout_forwards_held_bytes() {
    let mut options = AttachOptions::default();
    options.detach.chord = vec![0x01, b'd'];
    options.detach.partial_timeout = Duration::from_millis(500);

    let mut harness = AttachHarness::new(
        PaneId::new(5),
        options,
        Size::new(24, 80),
        Size::new(40, 120),
    );
    let start = Instant::now();
    harness.queue_stdin(start, vec![0x01]);
    harness.queue_tick(start + Duration::from_millis(600));
    harness.queue_stdin(start + Duration::from_millis(601), vec![0x01, b'd']);

    harness.run().expect("attach harness should detach cleanly");

    assert_eq!(harness.pty().input_bytes(), vec![0x01]);
}

#[test]
fn stdout_only_then_replay_catches_surface_up_on_detach() {
    let mut options = AttachOptions::default();
    options.output = AttachOutputPolicy::StdoutOnlyThenReplay;

    let mut harness = AttachHarness::new(
        PaneId::new(6),
        options,
        Size::new(24, 80),
        Size::new(40, 120),
    );

    harness.queue_pty_output(b"hello".to_vec());
    harness.queue_stdin(Instant::now(), vec![0x1d]);
    harness.queue_pty_output(b" world".to_vec());

    harness.run().expect("attach harness should detach cleanly");

    assert_eq!(
        harness.terminal().stdout_chunks(),
        &[b"hello".to_vec(), b" world".to_vec()]
    );
    assert_eq!(
        harness.surface().fed_output_chunks(),
        &[b"hello".to_vec(), b" world".to_vec()]
    );
}

#[test]
fn keep_embedded_size_skips_attach_and_detach_resizes() {
    let mut options = AttachOptions::default();
    options.resize = AttachResizePolicy::KeepEmbeddedSize;

    let mut harness = AttachHarness::new(
        PaneId::new(7),
        options,
        Size::new(24, 80),
        Size::new(40, 120),
    );

    harness.queue_resize(Size::new(50, 160));
    harness.queue_stdin(Instant::now(), vec![0x1d]);

    harness.run().expect("attach harness should detach cleanly");

    assert!(harness.pty().resize_calls().is_empty());
    assert!(harness.surface().resize_calls().is_empty());
    assert_eq!(harness.terminal().size(), Size::new(50, 160));
}

#[test]
fn pty_write_errors_still_restore_terminal() {
    let mut harness = default_harness(8);
    harness.pty_mut().fail_next_input_write();
    harness.queue_stdin(Instant::now(), b"abc".to_vec());

    let error = harness
        .run()
        .expect_err("pty write failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::PtyWrite("injected pty write failure")
    );
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(harness.bridge_state(), AttachState::Attached);
}

#[test]
fn surface_feed_errors_still_restore_terminal() {
    let mut harness = default_harness(9);
    harness.surface_mut().fail_next_feed();
    harness.queue_pty_output(b"boom".to_vec());

    let error = harness
        .run()
        .expect_err("surface feed failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::SurfaceFeed("injected surface feed failure")
    );
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(harness.bridge_state(), AttachState::Attached);
}

#[test]
fn stdout_replay_feed_errors_still_restore_terminal() {
    let mut options = AttachOptions::default();
    options.output = AttachOutputPolicy::StdoutOnlyThenReplay;

    let mut harness = AttachHarness::new(
        PaneId::new(15),
        options,
        Size::new(24, 80),
        Size::new(40, 120),
    );
    harness.surface_mut().fail_next_feed();
    harness.queue_pty_output(b"before".to_vec());
    harness.queue_stdin(Instant::now(), vec![0x1d]);
    harness.queue_pty_output(b" after".to_vec());

    let error = harness
        .run()
        .expect_err("stdout replay feed failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::SurfaceFeed("injected surface feed failure")
    );
    assert_eq!(harness.terminal().stdout_bytes(), b"before after".to_vec());
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(harness.bridge_state(), AttachState::Attached);
}

#[test]
fn pty_resize_errors_during_attach_restore_terminal() {
    let mut harness = default_harness(10);
    harness.pty_mut().fail_next_resize();

    let error = harness
        .run()
        .expect_err("pty resize failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::PtyResize("injected pty resize failure")
    );
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(harness.bridge_state(), AttachState::Attaching);
}

#[test]
fn surface_resize_errors_during_attach_restore_terminal() {
    let mut harness = default_harness(11);
    harness.surface_mut().fail_next_resize();

    let error = harness
        .run()
        .expect_err("surface resize failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::SurfaceResize("injected surface resize failure")
    );
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(harness.bridge_state(), AttachState::Attaching);
}

#[test]
fn suspend_errors_bubble_up_without_restoring() {
    let mut harness = default_harness(12);
    harness.control_mut().fail_suspend();

    let error = harness.run().expect_err("suspend failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::Suspend("injected suspend failure")
    );
    assert_eq!(harness.control().restore_call_count(), 0);
    assert_eq!(harness.bridge_state(), AttachState::Attaching);
}

#[test]
fn restore_errors_retry_on_drop_and_leave_bridge_attached() {
    let mut harness = default_harness(13);
    harness.control_mut().fail_restore_times(1);
    harness.queue_stdin(Instant::now(), vec![0x1d]);

    let error = harness.run().expect_err("restore failure should bubble up");

    assert_eq!(
        error,
        AttachHarnessError::Restore("injected restore failure")
    );
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(
        harness.control().restored_policies(),
        &[AttachScreenPolicy::ReuseHostAlternateScreen]
    );
    assert_eq!(harness.bridge_state(), AttachState::Attached);
}

#[test]
fn script_exhaustion_without_detach_is_an_error() {
    let mut harness = default_harness(14);
    harness.queue_pty_output(b"still attached".to_vec());

    let error = harness
        .run()
        .expect_err("script exhaustion without detach should fail");

    assert_eq!(error, AttachHarnessError::ScriptExhaustedBeforeDetach);
    assert_eq!(
        harness.terminal().stdout_bytes(),
        b"still attached".to_vec()
    );
    assert_eq!(harness.control().restore_call_count(), 1);
    assert_eq!(harness.bridge_state(), AttachState::Attached);
}
