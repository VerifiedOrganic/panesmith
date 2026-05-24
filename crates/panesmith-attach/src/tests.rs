use std::cell::Cell;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use std::time::Instant;

use super::{
    test_harness::{FakePtyEndpoint, FakeSurfaceSink, FakeTerminalControl, InMemoryTerminal},
    AttachBridge, AttachGuard, AttachOptions, AttachOutputPolicy, AttachPtyEndpoint,
    AttachResizePolicy, AttachScreenPolicy, AttachState, BlockingAttachError, DetachConfig,
    HostTerminalControl, RestorePolicy, TerminalRestoreToken,
};
use panesmith_core::{InputKind, IoOperation, PaneError, PaneEventKind, PaneId, PtyFrame, Size};

// ---------- AttachOptions defaults ----------

#[test]
fn attach_options_default_uses_ctrl_bracket_detach() {
    let opts = AttachOptions::default();
    assert_eq!(opts.detach.chord, vec![0x1d]);
    assert_eq!(
        opts.detach.partial_timeout,
        std::time::Duration::from_millis(500)
    );
}

#[test]
fn attach_options_default_policies_match_spec() {
    let opts = AttachOptions::default();
    assert_eq!(opts.screen, AttachScreenPolicy::ReuseHostAlternateScreen);
    assert_eq!(opts.resize, AttachResizePolicy::UseRealTerminalSize);
    assert_eq!(opts.output, AttachOutputPolicy::FanoutToSurfaceAndStdout);
    assert_eq!(opts.restore, RestorePolicy::Full);
}

// ---------- DetachConfig ----------

#[test]
fn detach_config_default_is_ctrl_bracket() {
    let cfg = DetachConfig::default();
    assert_eq!(cfg.chord, vec![0x1d]);
    assert_eq!(cfg.partial_timeout, std::time::Duration::from_millis(500));
}

#[test]
fn detach_config_can_set_multi_byte_chord() {
    let mut cfg = DetachConfig::default();
    cfg.chord = vec![0x01, 0x64]; // Ctrl-A d
    cfg.partial_timeout = std::time::Duration::from_secs(1);
    assert_eq!(cfg.chord, vec![0x01, 0x64]);
    assert_eq!(cfg.partial_timeout, std::time::Duration::from_secs(1));
}

// ---------- Policy enum defaults ----------

#[test]
fn attach_screen_policy_default_is_reuse() {
    assert_eq!(
        AttachScreenPolicy::default(),
        AttachScreenPolicy::ReuseHostAlternateScreen
    );
}

#[test]
fn attach_resize_policy_default_is_real_terminal() {
    assert_eq!(
        AttachResizePolicy::default(),
        AttachResizePolicy::UseRealTerminalSize
    );
}

#[test]
fn attach_output_policy_default_is_fanout() {
    assert_eq!(
        AttachOutputPolicy::default(),
        AttachOutputPolicy::FanoutToSurfaceAndStdout
    );
}

#[test]
fn restore_policy_default_is_full() {
    assert_eq!(RestorePolicy::default(), RestorePolicy::Full);
}

// ---------- AttachBridge basics ----------

#[test]
fn bridge_keeps_its_pane_id_and_options() {
    let mut opts = AttachOptions::default();
    let mut detach = DetachConfig::default();
    detach.chord = vec![0x01, 0x64];
    detach.partial_timeout = std::time::Duration::from_millis(250);
    opts.detach = detach;
    let bridge = AttachBridge::with_options(PaneId::new(9), opts);

    assert_eq!(bridge.pane_id(), PaneId::new(9));
    assert_eq!(bridge.options().detach.chord, vec![0x01, 0x64]);
}

#[test]
fn bridge_default_state_is_embedded() {
    let bridge = AttachBridge::new(PaneId::new(1));
    assert_eq!(bridge.state(), AttachState::Embedded);
}

// ---------- State transitions ----------

#[test]
fn state_transition_embedded_to_attaching_to_attached() {
    let mut bridge = AttachBridge::new(PaneId::new(1));
    bridge.begin_attach();
    assert_eq!(bridge.state(), AttachState::Attaching);
    bridge.confirm_attached();
    assert_eq!(bridge.state(), AttachState::Attached);
}

#[test]
fn state_transition_attached_to_detaching_to_embedded() {
    let mut bridge = AttachBridge::new(PaneId::new(1));
    bridge.begin_attach();
    bridge.confirm_attached();
    bridge.begin_detach();
    assert_eq!(bridge.state(), AttachState::Detaching);
    bridge.confirm_detached();
    assert_eq!(bridge.state(), AttachState::Embedded);
}

#[test]
#[should_panic(expected = "can only begin attach from Embedded state")]
fn begin_attach_from_attached_panics() {
    let mut bridge = AttachBridge::new(PaneId::new(1));
    bridge.begin_attach();
    bridge.confirm_attached();
    bridge.begin_attach(); // panic
}

#[test]
#[should_panic(expected = "can only confirm attached from Attaching state")]
fn confirm_attached_from_embedded_panics() {
    let mut bridge = AttachBridge::new(PaneId::new(1));
    bridge.confirm_attached(); // panic
}

#[test]
#[should_panic(expected = "can only begin detach from Attached state")]
fn begin_detach_from_embedded_panics() {
    let mut bridge = AttachBridge::new(PaneId::new(1));
    bridge.begin_detach(); // panic
}

#[test]
#[should_panic(expected = "can only confirm detached from Detaching state")]
fn confirm_detached_from_attached_panics() {
    let mut bridge = AttachBridge::new(PaneId::new(1));
    bridge.begin_attach();
    bridge.confirm_attached();
    bridge.confirm_detached(); // panic
}

#[test]
fn attach_state_helpers_are_accurate() {
    assert!(!AttachState::Embedded.is_transitional());
    assert!(!AttachState::Embedded.is_active());

    assert!(AttachState::Attaching.is_transitional());
    assert!(AttachState::Attaching.is_active());

    assert!(!AttachState::Attached.is_transitional());
    assert!(AttachState::Attached.is_active());

    assert!(AttachState::Detaching.is_transitional());
    assert!(AttachState::Detaching.is_active());
}

// ---------- TerminalRestoreToken ----------

#[test]
fn restore_token_tracks_screen_policy() {
    let token = TerminalRestoreToken::new(AttachScreenPolicy::EnterFreshAlternateScreen);
    assert_eq!(
        token.screen_policy(),
        AttachScreenPolicy::EnterFreshAlternateScreen
    );
    assert!(!token.is_consumed());
}

#[test]
fn restore_token_can_be_consumed() {
    let mut token = TerminalRestoreToken::new(AttachScreenPolicy::LeaveAlternateScreen);
    token.consume();
    assert!(token.is_consumed());
}

// ---------- HostTerminalControl + AttachGuard ----------

#[derive(Debug)]
struct MockControl {
    suspended: bool,
    restored: bool,
    fail_suspend: bool,
    fail_restore_count: u32,
}

impl MockControl {
    fn new() -> Self {
        Self {
            suspended: false,
            restored: false,
            fail_suspend: false,
            fail_restore_count: 0,
        }
    }
}

impl HostTerminalControl for MockControl {
    type Error = &'static str;

    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> std::result::Result<TerminalRestoreToken, Self::Error> {
        if self.fail_suspend {
            return Err("suspend failed");
        }
        self.suspended = true;
        Ok(TerminalRestoreToken::new(policy))
    }

    fn restore_after_attach(
        &mut self,
        token: &mut TerminalRestoreToken,
    ) -> std::result::Result<(), Self::Error> {
        if self.fail_restore_count > 0 {
            self.fail_restore_count -= 1;
            return Err("restore failed");
        }
        token.consume();
        self.restored = true;
        Ok(())
    }
}

#[test]
fn host_terminal_control_suspend_returns_token() {
    let mut ctrl = MockControl::new();
    let token = ctrl
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .unwrap();
    assert!(ctrl.suspended);
    assert!(!token.is_consumed());
}

#[test]
fn host_terminal_control_restore_consumes_token() {
    let mut ctrl = MockControl::new();
    let mut token = ctrl
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .unwrap();
    ctrl.restore_after_attach(&mut token).unwrap();
    assert!(ctrl.restored);
    assert!(token.is_consumed());
}

#[test]
fn attach_guard_restores_on_drop() {
    let mut ctrl = MockControl::new();
    let token = ctrl
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .unwrap();
    let guard = AttachGuard::new(&mut ctrl, token);
    assert!(!guard.is_detached());
    drop(guard);
    assert!(ctrl.restored);
}

#[test]
fn attach_guard_explicit_detach_restores_and_avoids_double_restore() {
    let mut ctrl = MockControl::new();
    let token = ctrl
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .unwrap();
    let mut guard = AttachGuard::new(&mut ctrl, token);
    assert!(!guard.is_detached());
    guard.detach().unwrap();
    assert!(guard.is_detached());
    drop(guard);
    assert!(ctrl.restored);
    // Drop after explicit detach should not call restore again.
}

#[test]
fn attach_guard_detach_error_keeps_token_and_allows_retry() {
    let mut ctrl = MockControl::new();
    ctrl.fail_restore_count = 1;
    let token = ctrl
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .unwrap();
    let mut guard = AttachGuard::new(&mut ctrl, token);
    // First detach fails; mock decrements fail_restore_count.
    assert!(guard.detach().is_err());
    assert!(!guard.is_detached());
    // Second detach succeeds because the failure count reached zero.
    guard.detach().unwrap();
    assert!(guard.is_detached());
    drop(guard);
    assert!(ctrl.restored);
}

#[test]
fn attach_guard_drop_handles_restore_error_gracefully() {
    let mut ctrl = MockControl::new();
    ctrl.fail_restore_count = 1;
    let token = ctrl
        .suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen)
        .unwrap();
    let _guard = AttachGuard::new(&mut ctrl, token);
    // restore will be attempted on drop but fail; no panic should occur
}

#[test]
fn host_terminal_control_suspend_can_fail() {
    let mut ctrl = MockControl::new();
    ctrl.fail_suspend = true;
    let result = ctrl.suspend_for_attach(AttachScreenPolicy::ReuseHostAlternateScreen);
    assert!(result.is_err());
}

#[test]
fn blocking_attach_suspend_failure_resets_state_and_allows_retry() {
    let mut control = FakeTerminalControl::new();
    control.fail_suspend();
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(77),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(std::time::Duration::from_millis(1));

    let mut failing_terminal = InMemoryTerminal::new(Size::new(40, 120));
    failing_terminal.queue_stdin(Instant::now(), vec![0x1d]);
    let mut failing_pty = FakePtyEndpoint::new();
    let mut failing_surface = FakeSurfaceSink::new();

    let error = session
        .run(
            &mut failing_terminal,
            &mut failing_pty,
            &mut failing_surface,
            &mut control,
        )
        .expect_err("injected suspend failure should bubble up");
    assert!(matches!(error, BlockingAttachError::Suspend { .. }));
    assert_eq!(session.state(), AttachState::Embedded);
    let mut events = Vec::new();
    session.drain_events(&mut events);
    assert_eq!(
        events.len(),
        1,
        "unexpected suspend-failure events: {events:?}"
    );
    assert!(matches!(
        &events[0].kind,
        PaneEventKind::Error(error)
            if error.error
                == PaneError::Attach {
                    message: "attach suspend failed: injected suspend failure".into(),
                }
    ));
    assert!(!events.iter().any(|event| matches!(
        event.kind,
        PaneEventKind::AttachStarted(_) | PaneEventKind::AttachEnded(_)
    )));

    let mut retry_terminal = InMemoryTerminal::new(Size::new(40, 120));
    retry_terminal.queue_stdin(Instant::now(), vec![0x1d]);
    let mut retry_pty = FakePtyEndpoint::new();
    let mut retry_surface = FakeSurfaceSink::new();

    let outcome = session
        .run(
            &mut retry_terminal,
            &mut retry_pty,
            &mut retry_surface,
            &mut control,
        )
        .expect("session should remain reusable after suspend failure");

    assert_eq!(outcome.reason, panesmith_core::DetachReason::UserChord);
    assert_eq!(session.state(), AttachState::Embedded);
    assert_eq!(
        control.suspended_policies(),
        &[AttachScreenPolicy::ReuseHostAlternateScreen]
    );
    assert_eq!(
        control.restored_policies(),
        &[AttachScreenPolicy::ReuseHostAlternateScreen]
    );
}

struct TrackingTerminal {
    inner: InMemoryTerminal,
    stdin_polled: Arc<AtomicBool>,
}

impl TrackingTerminal {
    fn new(inner: InMemoryTerminal, stdin_polled: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            stdin_polled,
        }
    }
}

impl super::AttachTerminal for TrackingTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> std::result::Result<Option<super::AttachInputChunk>, Self::Error> {
        self.stdin_polled.store(true, Ordering::SeqCst);
        <InMemoryTerminal as super::AttachTerminal>::read_stdin(&mut self.inner)
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> std::result::Result<(), Self::Error> {
        <InMemoryTerminal as super::AttachTerminal>::write_stdout(&mut self.inner, bytes)
    }

    fn size(&self) -> std::result::Result<Size, Self::Error> {
        <InMemoryTerminal as super::AttachTerminal>::size(&self.inner)
    }
}

struct ScriptedResizeTerminal {
    inner: InMemoryTerminal,
    initial_size: Size,
    resized_size: Size,
    /// Switches to `resized_size` on the Nth call to `size()` (0-indexed).
    switch_on_size_call: usize,
    size_calls: Cell<usize>,
}

impl ScriptedResizeTerminal {
    fn new(initial_size: Size, resized_size: Size, switch_on_size_call: usize) -> Self {
        Self {
            inner: InMemoryTerminal::new(initial_size),
            initial_size,
            resized_size,
            switch_on_size_call,
            size_calls: Cell::new(0),
        }
    }

    fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.inner.queue_stdin(at, bytes);
    }
}

impl super::AttachTerminal for ScriptedResizeTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> std::result::Result<Option<super::AttachInputChunk>, Self::Error> {
        <InMemoryTerminal as super::AttachTerminal>::read_stdin(&mut self.inner)
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> std::result::Result<(), Self::Error> {
        <InMemoryTerminal as super::AttachTerminal>::write_stdout(&mut self.inner, bytes)
    }

    fn size(&self) -> std::result::Result<Size, Self::Error> {
        let call = self.size_calls.get();
        self.size_calls.set(call + 1);
        if call >= self.switch_on_size_call {
            Ok(self.resized_size)
        } else {
            Ok(self.initial_size)
        }
    }
}

struct FailingSizeTerminal {
    inner: InMemoryTerminal,
    fail_on_size_call: usize,
    size_calls: Cell<usize>,
}

impl FailingSizeTerminal {
    fn new(size: Size, fail_on_size_call: usize) -> Self {
        Self {
            inner: InMemoryTerminal::new(size),
            fail_on_size_call,
            size_calls: Cell::new(0),
        }
    }

    fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.inner.queue_stdin(at, bytes);
    }
}

impl super::AttachTerminal for FailingSizeTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> std::result::Result<Option<super::AttachInputChunk>, Self::Error> {
        <InMemoryTerminal as super::AttachTerminal>::read_stdin(&mut self.inner)
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> std::result::Result<(), Self::Error> {
        <InMemoryTerminal as super::AttachTerminal>::write_stdout(&mut self.inner, bytes)
    }

    fn size(&self) -> std::result::Result<Size, Self::Error> {
        let call = self.size_calls.get();
        self.size_calls.set(call + 1);
        if call == self.fail_on_size_call {
            Err("injected terminal size failure")
        } else {
            <InMemoryTerminal as super::AttachTerminal>::size(&self.inner)
        }
    }
}

struct ChattyPty {
    stdin_polled: Arc<AtomicBool>,
    pre_stdin_polls: AtomicUsize,
}

impl ChattyPty {
    fn new(stdin_polled: Arc<AtomicBool>) -> Self {
        Self {
            stdin_polled,
            pre_stdin_polls: AtomicUsize::new(0),
        }
    }
}

impl AttachPtyEndpoint for ChattyPty {
    type Error = &'static str;

    fn try_recv(&mut self) -> std::result::Result<Option<PtyFrame>, Self::Error> {
        if self.stdin_polled.load(Ordering::SeqCst) {
            return Ok(None);
        }

        let polls = self.pre_stdin_polls.fetch_add(1, Ordering::SeqCst);
        assert!(
            polls < 64,
            "stdin should be polled before chatty PTY output can starve detach"
        );

        Ok(Some(PtyFrame::Output {
            seq: polls as u64,
            bytes: b"spam".to_vec(),
            at: Instant::now(),
        }))
    }

    fn write_input(&mut self, _bytes: &[u8]) -> std::result::Result<(), Self::Error> {
        Ok(())
    }

    fn resize(&mut self, _size: Size) -> std::result::Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn blocking_attach_polls_stdin_before_chatty_output_can_starve_detach() {
    let stdin_polled = Arc::new(AtomicBool::new(false));
    let mut terminal = TrackingTerminal::new(
        {
            let mut terminal = InMemoryTerminal::new(Size::new(40, 120));
            terminal.queue_stdin(Instant::now(), vec![0x1d]);
            terminal
        },
        Arc::clone(&stdin_polled),
    );
    let mut pty = ChattyPty::new(Arc::clone(&stdin_polled));
    let mut surface = FakeSurfaceSink::new();
    let mut control = FakeTerminalControl::new();
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(99),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(std::time::Duration::from_millis(1));

    let outcome = session
        .run(&mut terminal, &mut pty, &mut surface, &mut control)
        .expect("chatty PTY should still detach cleanly");

    assert_eq!(outcome.reason, panesmith_core::DetachReason::UserChord);
    assert!(stdin_polled.load(Ordering::SeqCst));
}

#[test]
fn blocking_attach_emits_ordered_attach_resize_and_redacted_input_events() {
    let mut terminal = ScriptedResizeTerminal::new(Size::new(40, 120), Size::new(50, 160), 3);
    let start = Instant::now();
    terminal.queue_stdin(start, b"abc".to_vec());
    terminal.queue_stdin(start + Duration::from_millis(1), vec![0x1d]);

    let mut pty = FakePtyEndpoint::new();
    let mut surface = FakeSurfaceSink::new();
    let mut control = FakeTerminalControl::new();
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(120),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(Duration::from_millis(1));

    let outcome = session
        .run(&mut terminal, &mut pty, &mut surface, &mut control)
        .expect("attach session should detach cleanly");
    assert_eq!(outcome.reason, panesmith_core::DetachReason::UserChord);

    let mut events = Vec::new();
    session.drain_events(&mut events);

    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=events.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(events.len(), 6, "unexpected event stream: {events:?}");
    assert!(matches!(
        &events[0].kind,
        PaneEventKind::AttachStarted(started)
            if started.terminal_size == Size::new(40, 120)
                && started.embedded_size == Size::new(24, 80)
                && started.screen_policy == AttachScreenPolicy::ReuseHostAlternateScreen
    ));
    assert!(matches!(
        &events[1].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[2].kind,
        PaneEventKind::InputSent(input)
            if input.input_kind == InputKind::Bytes
                && input.bytes_len == 3
                && !input.recorded
    ));
    assert!(matches!(
        &events[3].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(50, 160)
    ));
    assert!(matches!(
        &events[4].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(24, 80)
    ));
    assert!(matches!(
        &events[5].kind,
        PaneEventKind::AttachEnded(ended)
            if ended.reason == panesmith_core::DetachReason::UserChord
                && ended.restored_size == Size::new(24, 80)
    ));
}

#[test]
fn blocking_attach_fresh_sessions_can_continue_pane_event_sequence_numbers() {
    let pane_id = PaneId::new(130);
    let options = AttachOptions::default();
    let embedded_size = Size::new(24, 80);

    let mut first_terminal = InMemoryTerminal::new(Size::new(40, 120));
    first_terminal.queue_stdin(Instant::now(), vec![0x1d]);
    let mut first_pty = FakePtyEndpoint::new();
    let mut first_surface = FakeSurfaceSink::new();
    let mut first_control = FakeTerminalControl::new();
    let mut first_session =
        super::BlockingAttachSession::new(pane_id, options.clone(), embedded_size, 0)
            .with_poll_interval(Duration::from_millis(1));

    first_session
        .run(
            &mut first_terminal,
            &mut first_pty,
            &mut first_surface,
            &mut first_control,
        )
        .expect("first attach session should detach cleanly");
    let mut first_events = Vec::new();
    first_session.drain_events(&mut first_events);
    let first_last_seq = first_session.last_event_seq();
    assert_eq!(
        first_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(first_last_seq, 4);

    let mut second_terminal = InMemoryTerminal::new(Size::new(41, 121));
    second_terminal.queue_stdin(Instant::now(), vec![0x1d]);
    let mut second_pty = FakePtyEndpoint::new();
    let mut second_surface = FakeSurfaceSink::new();
    let mut second_control = FakeTerminalControl::new();
    let mut second_session =
        super::BlockingAttachSession::new(pane_id, options, embedded_size, first_last_seq)
            .with_poll_interval(Duration::from_millis(1));

    second_session
        .run(
            &mut second_terminal,
            &mut second_pty,
            &mut second_surface,
            &mut second_control,
        )
        .expect("second attach session should continue sequence numbers");
    let mut second_events = Vec::new();
    second_session.drain_events(&mut second_events);

    assert_eq!(
        second_events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![5, 6, 7, 8]
    );
    assert_eq!(second_session.last_event_seq(), 8);
}

#[test]
fn blocking_attach_terminal_size_failure_after_suspend_still_emits_ended() {
    let mut terminal = FailingSizeTerminal::new(Size::new(40, 120), 1);
    terminal.queue_stdin(Instant::now(), vec![0x1d]);

    let mut pty = FakePtyEndpoint::new();
    let mut surface = FakeSurfaceSink::new();
    let mut control = FakeTerminalControl::new();
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(131),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(Duration::from_millis(1));

    let error = session
        .run(&mut terminal, &mut pty, &mut surface, &mut control)
        .expect_err("terminal-size failure after suspend should bubble up");
    assert!(matches!(error, BlockingAttachError::TerminalSize { .. }));
    assert_eq!(session.state(), AttachState::Embedded);

    let mut events = Vec::new();
    session.drain_events(&mut events);

    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=events.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(events.len(), 5, "unexpected event stream: {events:?}");
    assert!(matches!(
        &events[0].kind,
        PaneEventKind::AttachStarted(started)
            if started.terminal_size == Size::new(40, 120)
                && started.embedded_size == Size::new(24, 80)
    ));
    assert!(matches!(
        &events[1].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[2].kind,
        PaneEventKind::Error(error)
            if error.error
                == PaneError::Attach {
                    message: "attach terminal size failed: injected terminal size failure"
                        .into(),
                }
    ));
    assert!(matches!(
        &events[3].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(24, 80)
    ));
    assert!(matches!(
        &events[4].kind,
        PaneEventKind::AttachEnded(ended)
            if ended.reason == panesmith_core::DetachReason::Error
                && ended.restored_size == Size::new(24, 80)
    ));
}

#[test]
fn blocking_attach_emits_error_and_cleanup_events_when_stdout_write_fails() {
    let mut terminal = InMemoryTerminal::new(Size::new(40, 120));
    terminal.fail_next_stdout_write();

    let mut pty = FakePtyEndpoint::new();
    pty.queue_output(b"boom".to_vec());
    let mut surface = FakeSurfaceSink::new();
    let mut control = FakeTerminalControl::new();
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(121),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(Duration::from_millis(1));

    let error = session
        .run(&mut terminal, &mut pty, &mut surface, &mut control)
        .expect_err("stdout failure should bubble up");
    assert!(matches!(error, BlockingAttachError::TerminalOutput { .. }));
    assert_eq!(control.restore_call_count(), 1);
    assert_eq!(session.state(), AttachState::Embedded);

    let mut events = Vec::new();
    session.drain_events(&mut events);

    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=events.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(events.len(), 5, "unexpected event stream: {events:?}");
    assert!(matches!(
        &events[0].kind,
        PaneEventKind::AttachStarted(started)
            if started.terminal_size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[1].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[2].kind,
        PaneEventKind::Error(error)
            if error.error
                == PaneError::Io {
                    operation: IoOperation::Write,
                    message: "attach stdout write failed: injected stdout failure".into(),
                }
    ));
    assert!(matches!(
        &events[3].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(24, 80)
    ));
    assert!(matches!(
        &events[4].kind,
        PaneEventKind::AttachEnded(ended)
            if ended.reason == panesmith_core::DetachReason::Error
                && ended.restored_size == Size::new(24, 80)
    ));
}

#[test]
fn blocking_attach_persistent_restore_failure_still_emits_attach_ended_once() {
    let mut terminal = InMemoryTerminal::new(Size::new(40, 120));
    terminal.queue_stdin(Instant::now(), vec![0x1d]);

    let mut pty = FakePtyEndpoint::new();
    let mut surface = FakeSurfaceSink::new();
    let mut control = FakeTerminalControl::new();
    control.fail_restore_times(2);
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(123),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(Duration::from_millis(1));

    let error = session
        .run(&mut terminal, &mut pty, &mut surface, &mut control)
        .expect_err("persistent restore failure should bubble up");
    assert!(matches!(error, BlockingAttachError::Restore { .. }));
    assert_eq!(control.restore_attempt_count(), 3);
    assert_eq!(control.restore_call_count(), 1);
    assert_eq!(session.state(), AttachState::Embedded);

    let mut events = Vec::new();
    session.drain_events(&mut events);

    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=events.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(events.len(), 6, "unexpected event stream: {events:?}");
    assert!(matches!(
        &events[0].kind,
        PaneEventKind::AttachStarted(started)
            if started.terminal_size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[1].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[2].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(24, 80)
    ));
    assert!(matches!(
        &events[3].kind,
        PaneEventKind::Error(error)
            if error.error
                == PaneError::Attach {
                    message: "attach restore failed: injected restore failure".into(),
                }
    ));
    assert!(matches!(
        &events[4].kind,
        PaneEventKind::Error(error)
            if error.error
                == PaneError::Attach {
                    message: "attach restore failed: injected restore failure".into(),
                }
    ));
    assert!(matches!(
        &events[5].kind,
        PaneEventKind::AttachEnded(ended)
            if ended.reason == panesmith_core::DetachReason::UserChord
                && ended.restored_size == Size::new(24, 80)
    ));
}

#[test]
fn blocking_attach_emits_restore_error_then_error_ended_cleanup_event() {
    let mut terminal = InMemoryTerminal::new(Size::new(40, 120));
    terminal.queue_stdin(Instant::now(), vec![0x1d]);

    let mut pty = FakePtyEndpoint::new();
    let mut surface = FakeSurfaceSink::new();
    let mut control = FakeTerminalControl::new();
    control.fail_restore_times(1);
    let mut session = super::BlockingAttachSession::new(
        PaneId::new(122),
        AttachOptions::default(),
        Size::new(24, 80),
        0,
    )
    .with_poll_interval(Duration::from_millis(1));

    let error = session
        .run(&mut terminal, &mut pty, &mut surface, &mut control)
        .expect_err("restore failure should bubble up");
    assert!(matches!(error, BlockingAttachError::Restore { .. }));
    assert_eq!(control.restore_call_count(), 1);
    assert_eq!(session.state(), AttachState::Embedded);

    let mut events = Vec::new();
    session.drain_events(&mut events);

    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=events.len() as u64).collect::<Vec<_>>()
    );
    assert_eq!(events.len(), 5, "unexpected event stream: {events:?}");
    assert!(matches!(
        &events[0].kind,
        PaneEventKind::AttachStarted(started)
            if started.terminal_size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[1].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(40, 120)
    ));
    assert!(matches!(
        &events[2].kind,
        PaneEventKind::Resized(resized) if resized.size == Size::new(24, 80)
    ));
    assert!(matches!(
        &events[3].kind,
        PaneEventKind::Error(error)
            if error.error
                == PaneError::Attach {
                    message: "attach restore failed: injected restore failure".into(),
                }
    ));
    assert!(matches!(
        &events[4].kind,
        PaneEventKind::AttachEnded(ended)
            if ended.reason == panesmith_core::DetachReason::UserChord
                && ended.restored_size == Size::new(24, 80)
    ));
}

// ---------- Optional crossterm helpers ----------

#[cfg(feature = "crossterm")]
#[test]
fn optional_crossterm_helpers_compile() {
    assert_eq!(
        AttachBridge::keycode_name(crossterm::event::KeyCode::Esc),
        "esc"
    );
}
