//! Blocking fullscreen attach driver and terminal/PTY/surface bridge traits.

use std::collections::VecDeque;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use panesmith_core::detach_encoding::{
    format_attach_bytes_for_trace, normalize_attach_detach_input,
};
use panesmith_core::{
    AttachEndedEvent, AttachOptions, AttachOutputPolicy, AttachResizePolicy, AttachStartedEvent,
    DetachReason, ErrorEvent, InputKind, InputSentEvent, IoOperation, PaneError, PaneEvent,
    PaneEventKind, PaneId, PtyFrame, PtyProcess, ResizedEvent, Size, SurfaceBackend,
};

use crate::{AttachBridge, AttachGuard, AttachState, DetachMatcher, HostTerminalControl};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEFAULT_DETACH_DRAIN_QUIET: Duration = Duration::from_millis(200);
const DEFAULT_DETACH_DRAIN_MAX_WAIT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_PTY_FRAMES_PER_TICK: usize = 32;

/// A chunk of raw stdin bytes captured for attach-mode forwarding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachInputChunk {
    /// Capture timestamp used for detach-chord timeout handling.
    pub at: Instant,
    /// Raw bytes read from the host terminal.
    pub bytes: Vec<u8>,
}

/// Terminal I/O contract used by the blocking attach driver.
pub trait AttachTerminal {
    /// Backend-specific terminal I/O error type.
    type Error: fmt::Debug;

    /// Attempts to read the next raw stdin chunk without blocking forever.
    fn read_stdin(&mut self) -> Result<Option<AttachInputChunk>, Self::Error>;

    /// Writes child PTY output to the host terminal stdout.
    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Returns the current real-terminal size.
    fn size(&self) -> Result<Size, Self::Error>;
}

/// PTY contract used by the blocking attach driver.
pub trait AttachPtyEndpoint {
    /// Backend-specific PTY/runtime error type.
    type Error: fmt::Debug;

    /// Attempts to receive the next PTY frame without blocking.
    fn try_recv(&mut self) -> Result<Option<PtyFrame>, Self::Error>;

    /// Writes raw bytes to the child PTY.
    fn write_input(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Resizes the child PTY.
    fn resize(&mut self, size: Size) -> Result<(), Self::Error>;
}

/// Surface contract used by the blocking attach driver.
pub trait AttachSurfaceSink {
    /// Backend-specific surface error type.
    type Error: fmt::Debug;

    /// Feeds child PTY output into the embedded surface.
    fn feed_output(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Resizes the embedded surface.
    fn resize(&mut self, size: Size) -> Result<(), Self::Error>;
}

impl<T> AttachPtyEndpoint for T
where
    T: PtyProcess + ?Sized,
{
    type Error = PaneError;

    fn try_recv(&mut self) -> Result<Option<PtyFrame>, Self::Error> {
        Ok(PtyProcess::try_recv(self))
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        PtyProcess::writer(self).write_bytes(bytes)
    }

    fn resize(&mut self, size: Size) -> Result<(), Self::Error> {
        PtyProcess::resize(self, size)
    }
}

impl<T> AttachSurfaceSink for T
where
    T: SurfaceBackend + ?Sized,
{
    type Error = PaneError;

    fn feed_output(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        SurfaceBackend::feed(self, bytes).map(|_| ())
    }

    fn resize(&mut self, size: Size) -> Result<(), Self::Error> {
        SurfaceBackend::resize(self, size)
    }
}

/// Structured failure produced by the blocking attach driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockingAttachError {
    /// Suspending the host terminal failed.
    Suspend { message: String },
    /// Restoring the host terminal failed.
    Restore { message: String },
    /// Reading host stdin failed.
    TerminalInput { message: String },
    /// Writing host stdout failed.
    TerminalOutput { message: String },
    /// Reading the real-terminal size failed.
    TerminalSize { message: String },
    /// Reading PTY frames failed.
    PtyRead { message: String },
    /// A PTY/runtime frame reported an error.
    PtyRuntime { message: String },
    /// Writing bytes to the child PTY failed.
    PtyWrite { message: String },
    /// Resizing the child PTY failed.
    PtyResize { message: String },
    /// Feeding the embedded surface failed.
    SurfaceFeed { message: String },
    /// Resizing the embedded surface failed.
    SurfaceResize { message: String },
}

impl std::fmt::Display for BlockingAttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Suspend { message } => write!(f, "attach suspend failed: {message}"),
            Self::Restore { message } => write!(f, "attach restore failed: {message}"),
            Self::TerminalInput { message } => write!(f, "attach stdin read failed: {message}"),
            Self::TerminalOutput { message } => {
                write!(f, "attach stdout write failed: {message}")
            }
            Self::TerminalSize { message } => write!(f, "attach terminal size failed: {message}"),
            Self::PtyRead { message } => write!(f, "attach PTY read failed: {message}"),
            Self::PtyRuntime { message } => write!(f, "attach PTY runtime failed: {message}"),
            Self::PtyWrite { message } => write!(f, "attach PTY write failed: {message}"),
            Self::PtyResize { message } => write!(f, "attach PTY resize failed: {message}"),
            Self::SurfaceFeed { message } => write!(f, "attach surface feed failed: {message}"),
            Self::SurfaceResize { message } => {
                write!(f, "attach surface resize failed: {message}")
            }
        }
    }
}

impl std::error::Error for BlockingAttachError {}

/// Successful result from a blocking attach session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockingAttachOutcome {
    /// Why attach ended.
    pub reason: DetachReason,
    /// Exit code observed while attached, if the child exited.
    pub child_exit_code: Option<i32>,
    /// The real-terminal size used during attach.
    pub terminal_size: Size,
    /// The embedded size restored on detach.
    pub restored_size: Size,
    /// Bytes read after the detach chord in the same stdin chunk.
    pub remaining_input: Vec<u8>,
}

/// Blocking attach runner for synchronous host applications.
#[derive(Debug, PartialEq, Eq)]
pub struct BlockingAttachSession {
    bridge: AttachBridge,
    embedded_size: Size,
    poll_interval: Duration,
    next_event_seq: u64,
    events: VecDeque<PaneEvent>,
}

enum StdinPollResult {
    Idle,
    Forwarded,
    Detached { remaining_input: Vec<u8> },
}

struct AttachRuntime<'a, Terminal, Pty, Surface> {
    terminal: &'a mut Terminal,
    pty: &'a mut Pty,
    surface: &'a mut Surface,
    options: &'a AttachOptions,
}

struct AttachLoopState<'a> {
    detach_reason: &'a mut Option<DetachReason>,
    restored_embedded_size: &'a mut bool,
    replay_buffer: Vec<u8>,
    child_exit_code: Option<i32>,
    terminal_size: Size,
    started_at: Instant,
}

struct AbortAttachState {
    attach_started_at: Option<Instant>,
    end_reason: DetachReason,
    restored_embedded_size: bool,
}

impl BlockingAttachSession {
    /// Creates a new blocking attach session for the given pane.
    pub fn new(
        pane_id: PaneId,
        options: AttachOptions,
        embedded_size: Size,
        initial_event_seq: u64,
    ) -> Self {
        Self {
            bridge: AttachBridge::with_options(pane_id, options),
            embedded_size,
            poll_interval: DEFAULT_POLL_INTERVAL,
            next_event_seq: initial_event_seq,
            events: VecDeque::new(),
        }
    }

    /// Overrides the idle poll interval used while attached.
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval.max(Duration::from_millis(1));
        self
    }

    /// Returns the pane identifier for this session.
    pub const fn pane_id(&self) -> PaneId {
        self.bridge.pane_id()
    }

    /// Returns the configured embedded size restored on detach.
    pub const fn embedded_size(&self) -> Size {
        self.embedded_size
    }

    /// Returns the current attach bridge state.
    pub const fn state(&self) -> crate::AttachState {
        self.bridge.state()
    }

    /// Returns the current event sequence counter.
    ///
    /// Callers creating a new session for the same pane should pass this value
    /// back into [`BlockingAttachSession::new`] so attach telemetry stays
    /// monotonic within the pane-scoped event stream.
    pub const fn last_event_seq(&self) -> u64 {
        self.next_event_seq
    }

    /// Drains any queued attach telemetry events into `out` in emission order.
    pub fn drain_events(&mut self, out: &mut Vec<PaneEvent>) {
        out.extend(self.events.drain(..));
    }

    /// Runs a blocking attach session until the detach chord is pressed.
    pub fn run<Terminal, Pty, Surface, Control>(
        &mut self,
        terminal: &mut Terminal,
        pty: &mut Pty,
        surface: &mut Surface,
        control: &mut Control,
    ) -> Result<BlockingAttachOutcome, BlockingAttachError>
    where
        Terminal: AttachTerminal,
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
        Control: HostTerminalControl,
    {
        let initial_terminal_size = match terminal.size() {
            Ok(size) => size,
            Err(error) => {
                let error = BlockingAttachError::TerminalSize {
                    message: format!("{error:?}"),
                };
                self.push_error_event(&error);
                return Err(error);
            }
        };
        let options = self.bridge.options().clone();
        let mut detach_reason = None;
        let mut restored_embedded_size = false;

        self.bridge.begin_attach();
        let token = match control.suspend_for_attach(options.screen) {
            Ok(token) => token,
            Err(error) => {
                let error = BlockingAttachError::Suspend {
                    message: format!("{error:?}"),
                };
                self.push_error_event(&error);
                self.reset_bridge(&options);
                return Err(error);
            }
        };
        let mut guard = AttachGuard::new(control, token);
        let started_at = Instant::now();
        let attach_started_at = Some(started_at);
        self.push_event(PaneEventKind::AttachStarted(AttachStartedEvent {
            terminal_size: initial_terminal_size,
            embedded_size: self.embedded_size,
            screen_policy: options.screen,
        }));

        let mut runtime = AttachRuntime {
            terminal,
            pty,
            surface,
            options: &options,
        };
        let result = {
            let mut state = AttachLoopState {
                detach_reason: &mut detach_reason,
                restored_embedded_size: &mut restored_embedded_size,
                replay_buffer: Vec::new(),
                child_exit_code: None,
                terminal_size: initial_terminal_size,
                started_at,
            };
            self.run_attached(&mut runtime, &mut guard, &mut state)
        };
        if result.is_err() {
            if let Err(error) = &result {
                self.push_error_event(error);
            }
            self.best_effort_abort(
                &mut runtime,
                &mut guard,
                AbortAttachState {
                    attach_started_at,
                    end_reason: detach_reason.unwrap_or(DetachReason::Error),
                    restored_embedded_size,
                },
            );
        }
        result
    }

    fn run_attached<Terminal, Pty, Surface, Control>(
        &mut self,
        runtime: &mut AttachRuntime<'_, Terminal, Pty, Surface>,
        guard: &mut AttachGuard<'_, Control>,
        state: &mut AttachLoopState<'_>,
    ) -> Result<BlockingAttachOutcome, BlockingAttachError>
    where
        Terminal: AttachTerminal,
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
        Control: HostTerminalControl,
    {
        let mut matcher = DetachMatcher::new(&runtime.options.detach);

        if matches!(
            runtime.options.resize,
            AttachResizePolicy::UseRealTerminalSize
        ) {
            self.resize_attached_target(runtime.pty, runtime.surface, state.terminal_size, true)?;
        }
        self.bridge.confirm_attached();
        runtime
            .terminal
            .size()
            .map_err(|error| BlockingAttachError::TerminalSize {
                message: format!("{error:?}"),
            })?;

        loop {
            let mut made_progress = false;

            let current_size =
                runtime
                    .terminal
                    .size()
                    .map_err(|error| BlockingAttachError::TerminalSize {
                        message: format!("{error:?}"),
                    })?;
            if current_size != state.terminal_size {
                state.terminal_size = current_size;
                if matches!(
                    runtime.options.resize,
                    AttachResizePolicy::UseRealTerminalSize
                ) {
                    self.resize_attached_target(
                        runtime.pty,
                        runtime.surface,
                        state.terminal_size,
                        true,
                    )?;
                }
                made_progress = true;
            }

            match self.poll_stdin(runtime.terminal, runtime.pty, &mut matcher)? {
                StdinPollResult::Idle => {}
                StdinPollResult::Forwarded => made_progress = true,
                StdinPollResult::Detached { remaining_input } => {
                    *state.detach_reason = Some(DetachReason::UserChord);
                    return self.finish_detach(
                        runtime,
                        guard,
                        state,
                        DetachReason::UserChord,
                        remaining_input,
                    );
                }
            }

            made_progress |= self.drain_output(runtime, state, DEFAULT_MAX_PTY_FRAMES_PER_TICK)?;

            if !made_progress {
                thread::sleep(self.poll_interval);
            }
        }
    }

    fn poll_stdin<Terminal, Pty>(
        &mut self,
        terminal: &mut Terminal,
        pty: &mut Pty,
        matcher: &mut DetachMatcher,
    ) -> Result<StdinPollResult, BlockingAttachError>
    where
        Terminal: AttachTerminal,
        Pty: AttachPtyEndpoint,
    {
        if let Some(stdin) =
            terminal
                .read_stdin()
                .map_err(|error| BlockingAttachError::TerminalInput {
                    message: format!("{error:?}"),
                })?
        {
            if log::log_enabled!(log::Level::Trace) {
                let normalized = normalize_attach_detach_input(
                    &stdin.bytes,
                    &self.bridge.options().detach.chord,
                );
                log::trace!(
                    target: "panesmith::attach",
                    "blocking attach stdin chunk pane_id={} raw_len={} raw_hex={} normalized_hex={}",
                    self.bridge.pane_id().get(),
                    stdin.bytes.len(),
                    format_attach_bytes_for_trace(&stdin.bytes),
                    format_attach_bytes_for_trace(&normalized)
                );
            }
            let result = matcher.feed_bytes(&stdin.bytes, stdin.at);
            if result.detached {
                log::debug!(
                    target: "panesmith::attach",
                    "blocking attach detach chord matched pane_id={} forward_len={} remaining_len={}",
                    self.bridge.pane_id().get(),
                    result.forward.len(),
                    result.remaining.len()
                );
            }
            if !result.forward.is_empty() {
                pty.write_input(&result.forward).map_err(|error| {
                    BlockingAttachError::PtyWrite {
                        message: format!("{error:?}"),
                    }
                })?;
                self.push_event(PaneEventKind::InputSent(InputSentEvent {
                    input_kind: InputKind::Bytes,
                    bytes_len: result.forward.len(),
                    recorded: false,
                }));
            }
            if result.detached {
                return Ok(StdinPollResult::Detached {
                    remaining_input: result.remaining.to_vec(),
                });
            }
            return Ok(StdinPollResult::Forwarded);
        }

        if let Some(bytes) = matcher.check_timeout(Instant::now()) {
            pty.write_input(&bytes)
                .map_err(|error| BlockingAttachError::PtyWrite {
                    message: format!("{error:?}"),
                })?;
            self.push_event(PaneEventKind::InputSent(InputSentEvent {
                input_kind: InputKind::Bytes,
                bytes_len: bytes.len(),
                recorded: false,
            }));
            return Ok(StdinPollResult::Forwarded);
        }

        Ok(StdinPollResult::Idle)
    }

    fn drain_output<Terminal, Pty, Surface>(
        &mut self,
        runtime: &mut AttachRuntime<'_, Terminal, Pty, Surface>,
        state: &mut AttachLoopState<'_>,
        max_frames: usize,
    ) -> Result<bool, BlockingAttachError>
    where
        Terminal: AttachTerminal,
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
    {
        let mut saw_output = false;

        for _ in 0..max_frames {
            let frame = runtime
                .pty
                .try_recv()
                .map_err(|error| BlockingAttachError::PtyRead {
                    message: format!("{error:?}"),
                })?;

            let Some(frame) = frame else {
                return Ok(saw_output);
            };

            saw_output = true;
            match frame {
                PtyFrame::Output { bytes, .. } => {
                    runtime.terminal.write_stdout(&bytes).map_err(|error| {
                        BlockingAttachError::TerminalOutput {
                            message: format!("{error:?}"),
                        }
                    })?;
                    match runtime.options.output {
                        AttachOutputPolicy::FanoutToSurfaceAndStdout => runtime
                            .surface
                            .feed_output(&bytes)
                            .map_err(|error| BlockingAttachError::SurfaceFeed {
                                message: format!("{error:?}"),
                            })?,
                        AttachOutputPolicy::StdoutOnlyThenReplay => {
                            state.replay_buffer.extend_from_slice(&bytes);
                        }
                    }
                }
                PtyFrame::Exited { code, .. } => {
                    if state.child_exit_code.is_none() {
                        state.child_exit_code = code;
                    }
                }
                PtyFrame::Error { message, .. } => {
                    return Err(BlockingAttachError::PtyRuntime { message });
                }
                PtyFrame::Overflow { .. } | PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        Ok(saw_output)
    }

    fn finish_detach<Terminal, Pty, Surface, Control>(
        &mut self,
        runtime: &mut AttachRuntime<'_, Terminal, Pty, Surface>,
        guard: &mut AttachGuard<'_, Control>,
        state: &mut AttachLoopState<'_>,
        reason: DetachReason,
        remaining_input: Vec<u8>,
    ) -> Result<BlockingAttachOutcome, BlockingAttachError>
    where
        Terminal: AttachTerminal,
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
        Control: HostTerminalControl,
    {
        self.bridge.begin_detach();

        self.drain_detaching_output(runtime, state, DEFAULT_MAX_PTY_FRAMES_PER_TICK)?;

        if matches!(
            runtime.options.output,
            AttachOutputPolicy::StdoutOnlyThenReplay
        ) && !state.replay_buffer.is_empty()
        {
            runtime
                .surface
                .feed_output(&state.replay_buffer)
                .map_err(|error| BlockingAttachError::SurfaceFeed {
                    message: format!("{error:?}"),
                })?;
            state.replay_buffer.clear();
        }

        if matches!(
            runtime.options.resize,
            AttachResizePolicy::UseRealTerminalSize
        ) {
            self.resize_attached_target(runtime.pty, runtime.surface, self.embedded_size, true)?;
            *state.restored_embedded_size = true;
        }

        guard
            .detach()
            .map_err(|error| BlockingAttachError::Restore {
                message: format!("{error:?}"),
            })?;
        self.bridge.confirm_detached();
        self.push_event(PaneEventKind::AttachEnded(AttachEndedEvent {
            reason,
            restored_size: self.embedded_size,
            duration: state.started_at.elapsed(),
        }));

        Ok(BlockingAttachOutcome {
            reason,
            child_exit_code: state.child_exit_code,
            terminal_size: state.terminal_size,
            restored_size: self.embedded_size,
            remaining_input,
        })
    }

    fn drain_detaching_output<Terminal, Pty, Surface>(
        &mut self,
        runtime: &mut AttachRuntime<'_, Terminal, Pty, Surface>,
        state: &mut AttachLoopState<'_>,
        max_frames: usize,
    ) -> Result<(), BlockingAttachError>
    where
        Terminal: AttachTerminal,
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
    {
        let started = Instant::now();
        let mut last_activity = started;

        loop {
            let saw_output = self.drain_output(runtime, state, max_frames)?;
            if saw_output {
                last_activity = Instant::now();
            }

            let now = Instant::now();
            if now.duration_since(last_activity) >= DEFAULT_DETACH_DRAIN_QUIET
                || now.duration_since(started) >= DEFAULT_DETACH_DRAIN_MAX_WAIT
            {
                return Ok(());
            }

            thread::sleep(self.poll_interval);
        }
    }

    fn best_effort_abort<Pty, Surface, Control>(
        &mut self,
        runtime: &mut AttachRuntime<'_, impl AttachTerminal, Pty, Surface>,
        guard: &mut AttachGuard<'_, Control>,
        abort: AbortAttachState,
    ) where
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
        Control: HostTerminalControl,
    {
        if self.bridge.state() == AttachState::Attached {
            self.bridge.begin_detach();
        }
        if self.bridge.state().is_active()
            && matches!(
                runtime.options.resize,
                AttachResizePolicy::UseRealTerminalSize
            )
            && !abort.restored_embedded_size
        {
            if let Err(error) = self.resize_attached_target(
                runtime.pty,
                runtime.surface,
                self.embedded_size,
                abort.attach_started_at.is_some(),
            ) {
                self.push_error_event(&error);
            }
        }
        if !guard.is_detached() {
            if let Err(error) = guard.detach() {
                let error = BlockingAttachError::Restore {
                    message: format!("{error:?}"),
                };
                self.push_error_event(&error);
                if let Err(error) = guard.detach() {
                    let error = BlockingAttachError::Restore {
                        message: format!("{error:?}"),
                    };
                    self.push_error_event(&error);
                    guard.disarm();
                }
            }
        }
        if let Some(started_at) = abort.attach_started_at {
            self.push_event(PaneEventKind::AttachEnded(AttachEndedEvent {
                reason: abort.end_reason,
                restored_size: self.embedded_size,
                duration: started_at.elapsed(),
            }));
        }
        self.reset_bridge(runtime.options);
    }

    fn reset_bridge(&mut self, options: &AttachOptions) {
        self.bridge = AttachBridge::with_options(self.bridge.pane_id(), options.clone());
    }

    fn resize_attached_target<Pty, Surface>(
        &mut self,
        pty: &mut Pty,
        surface: &mut Surface,
        size: Size,
        emit_event: bool,
    ) -> Result<(), BlockingAttachError>
    where
        Pty: AttachPtyEndpoint,
        Surface: AttachSurfaceSink,
    {
        pty.resize(size)
            .map_err(|error| BlockingAttachError::PtyResize {
                message: format!("{error:?}"),
            })?;
        surface
            .resize(size)
            .map_err(|error| BlockingAttachError::SurfaceResize {
                message: format!("{error:?}"),
            })?;
        if emit_event {
            self.push_event(PaneEventKind::Resized(ResizedEvent { size }));
        }
        Ok(())
    }

    fn push_event(&mut self, kind: PaneEventKind) {
        self.next_event_seq += 1;
        self.events.push_back(PaneEvent {
            pane_id: self.bridge.pane_id(),
            seq: self.next_event_seq,
            at: SystemTime::now(),
            kind,
        });
    }

    fn push_error_event(&mut self, error: &BlockingAttachError) {
        self.push_event(PaneEventKind::Error(ErrorEvent {
            error: blocking_attach_error_to_pane_error(error),
        }));
    }
}

fn blocking_attach_error_to_pane_error(error: &BlockingAttachError) -> PaneError {
    match error {
        BlockingAttachError::Suspend { message } => PaneError::Attach {
            message: format!(
                "attach suspend failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::Restore { message } => PaneError::Attach {
            message: format!(
                "attach restore failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::TerminalSize { message } => PaneError::Attach {
            message: format!(
                "attach terminal size failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::PtyRuntime { message } => PaneError::Attach {
            message: format!("attach PTY runtime failed: {message}"),
        },
        BlockingAttachError::TerminalInput { message } => PaneError::Io {
            operation: IoOperation::Read,
            message: format!(
                "attach stdin read failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::PtyRead { message } => PaneError::Io {
            operation: IoOperation::Read,
            message: format!(
                "attach PTY read failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::TerminalOutput { message } => PaneError::Io {
            operation: IoOperation::Write,
            message: format!(
                "attach stdout write failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::PtyWrite { message } => PaneError::Io {
            operation: IoOperation::Write,
            message: format!(
                "attach PTY write failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::PtyResize { message } => PaneError::Io {
            operation: IoOperation::Resize,
            message: format!(
                "attach PTY resize failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::SurfaceFeed { message } => PaneError::Surface {
            message: format!(
                "attach surface feed failed: {}",
                normalize_debug_message(message)
            ),
        },
        BlockingAttachError::SurfaceResize { message } => PaneError::Surface {
            message: format!(
                "attach surface resize failed: {}",
                normalize_debug_message(message)
            ),
        },
    }
}

fn normalize_debug_message(message: &str) -> String {
    // Safe: Debug formatting for &str/String always emits ASCII double-quotes.
    if message.len() >= 2 && message.starts_with('"') && message.ends_with('"') {
        message[1..message.len() - 1].to_string()
    } else {
        message.to_string()
    }
}
