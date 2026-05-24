//! In-memory attach bridge harness for deterministic unit tests.
//!
//! This module provides fake terminal, PTY, surface, and terminal-control
//! endpoints plus a small scripted driver that exercises attach bridging
//! without a real terminal.

use std::collections::VecDeque;
use std::time::Instant;

use panesmith_core::{
    AttachOptions, AttachOutputPolicy, AttachResizePolicy, AttachScreenPolicy, PaneId, Size,
};

use crate::{
    AttachBridge, AttachGuard, AttachInputChunk, AttachPtyEndpoint, AttachState, AttachSurfaceSink,
    AttachTerminal, DetachMatcher, HostTerminalControl, TerminalRestoreToken,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueuedStdin {
    at: Instant,
    bytes: Vec<u8>,
}

/// Scripted event processed by [`AttachHarness::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachHarnessStep {
    /// Read the next queued stdin chunk from the in-memory terminal.
    ReadStdin,
    /// Drain the next queued PTY output chunk.
    DrainPtyOutput,
    /// Apply a real-terminal resize during attach.
    ResizeTerminal(Size),
    /// Advance time so the detach matcher can flush a partial chord.
    Tick(Instant),
}

/// Error returned by the in-memory attach harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachHarnessError {
    /// Suspending the host terminal failed before attach started.
    Suspend(&'static str),
    /// The scripted steps ended before the harness observed a detach.
    ScriptExhaustedBeforeDetach,
    /// Restoring the host terminal failed during detach.
    Restore(&'static str),
    /// Writing bridged output to stdout failed.
    Stdout(&'static str),
    /// Writing bridged input to the PTY failed.
    PtyWrite(&'static str),
    /// Resizing the PTY failed.
    PtyResize(&'static str),
    /// Feeding mirrored output into the surface sink failed.
    SurfaceFeed(&'static str),
    /// Resizing the surface sink failed.
    SurfaceResize(&'static str),
}

/// Fake real terminal with queued stdin, captured stdout, and mutable size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InMemoryTerminal {
    stdin: VecDeque<QueuedStdin>,
    stdout: Vec<Vec<u8>>,
    size: Size,
    fail_next_stdout_write: bool,
}

impl InMemoryTerminal {
    /// Creates a new terminal with the given initial real-terminal size.
    pub fn new(size: Size) -> Self {
        Self {
            stdin: VecDeque::new(),
            stdout: Vec::new(),
            size,
            fail_next_stdout_write: false,
        }
    }

    /// Queues a raw stdin chunk to be read by the harness.
    pub fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.stdin.push_back(QueuedStdin {
            at,
            bytes: bytes.into(),
        });
    }

    /// Returns the current terminal size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Updates the current terminal size.
    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    /// Injects a single stdout write failure.
    pub fn fail_next_stdout_write(&mut self) {
        self.fail_next_stdout_write = true;
    }

    /// Returns the captured stdout chunks.
    pub fn stdout_chunks(&self) -> &[Vec<u8>] {
        &self.stdout
    }

    /// Returns the captured stdout as one flattened byte buffer.
    pub fn stdout_bytes(&self) -> Vec<u8> {
        self.stdout.concat()
    }

    fn read_stdin(&mut self) -> Option<QueuedStdin> {
        self.stdin.pop_front()
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.fail_next_stdout_write {
            self.fail_next_stdout_write = false;
            return Err("injected stdout failure");
        }
        self.stdout.push(bytes.to_vec());
        Ok(())
    }
}

impl AttachTerminal for InMemoryTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> Result<Option<AttachInputChunk>, Self::Error> {
        Ok(
            InMemoryTerminal::read_stdin(self).map(|stdin| AttachInputChunk {
                at: stdin.at,
                bytes: stdin.bytes,
            }),
        )
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        InMemoryTerminal::write_stdout(self, bytes)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(InMemoryTerminal::size(self))
    }
}

/// Fake PTY reader/writer pair used by attach bridge tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePtyEndpoint {
    output: VecDeque<Vec<u8>>,
    input_writes: Vec<Vec<u8>>,
    resize_calls: Vec<Size>,
    fail_next_input_write: bool,
    fail_next_resize: bool,
}

impl FakePtyEndpoint {
    /// Creates an empty fake PTY endpoint.
    pub fn new() -> Self {
        Self {
            output: VecDeque::new(),
            input_writes: Vec::new(),
            resize_calls: Vec::new(),
            fail_next_input_write: false,
            fail_next_resize: false,
        }
    }

    /// Queues a child-output chunk to be drained by the harness.
    pub fn queue_output(&mut self, bytes: impl Into<Vec<u8>>) {
        self.output.push_back(bytes.into());
    }

    /// Injects a single PTY input-write failure.
    pub fn fail_next_input_write(&mut self) {
        self.fail_next_input_write = true;
    }

    /// Injects a single PTY resize failure.
    pub fn fail_next_resize(&mut self) {
        self.fail_next_resize = true;
    }

    /// Returns the captured PTY input-write chunks.
    pub fn input_write_chunks(&self) -> &[Vec<u8>] {
        &self.input_writes
    }

    /// Returns the captured PTY input as one flattened byte buffer.
    pub fn input_bytes(&self) -> Vec<u8> {
        self.input_writes.concat()
    }

    /// Returns the PTY resize calls observed during the test run.
    pub fn resize_calls(&self) -> &[Size] {
        &self.resize_calls
    }

    fn try_read_output(&mut self) -> Option<Vec<u8>> {
        self.output.pop_front()
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.fail_next_input_write {
            self.fail_next_input_write = false;
            return Err("injected pty write failure");
        }
        self.input_writes.push(bytes.to_vec());
        Ok(())
    }

    fn resize(&mut self, size: Size) -> Result<(), &'static str> {
        if self.fail_next_resize {
            self.fail_next_resize = false;
            return Err("injected pty resize failure");
        }
        self.resize_calls.push(size);
        Ok(())
    }
}

impl Default for FakePtyEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachPtyEndpoint for FakePtyEndpoint {
    type Error = &'static str;

    fn try_recv(&mut self) -> Result<Option<panesmith_core::PtyFrame>, Self::Error> {
        Ok(self
            .try_read_output()
            .map(|bytes| panesmith_core::PtyFrame::Output {
                seq: 0,
                bytes,
                at: Instant::now(),
            }))
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        FakePtyEndpoint::write_input(self, bytes)
    }

    fn resize(&mut self, size: Size) -> Result<(), Self::Error> {
        FakePtyEndpoint::resize(self, size)
    }
}

/// Fake surface sink that captures mirrored output and resize calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeSurfaceSink {
    fed_output: Vec<Vec<u8>>,
    resize_calls: Vec<Size>,
    fail_next_feed: bool,
    fail_next_resize: bool,
}

impl FakeSurfaceSink {
    /// Creates an empty fake surface sink.
    pub fn new() -> Self {
        Self {
            fed_output: Vec::new(),
            resize_calls: Vec::new(),
            fail_next_feed: false,
            fail_next_resize: false,
        }
    }

    /// Injects a single surface-feed failure.
    pub fn fail_next_feed(&mut self) {
        self.fail_next_feed = true;
    }

    /// Injects a single surface-resize failure.
    pub fn fail_next_resize(&mut self) {
        self.fail_next_resize = true;
    }

    /// Returns the mirrored output chunks.
    pub fn fed_output_chunks(&self) -> &[Vec<u8>] {
        &self.fed_output
    }

    /// Returns the mirrored output as one flattened byte buffer.
    pub fn fed_output_bytes(&self) -> Vec<u8> {
        self.fed_output.concat()
    }

    /// Returns the surface resize calls observed during the test run.
    pub fn resize_calls(&self) -> &[Size] {
        &self.resize_calls
    }

    fn feed_output(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if self.fail_next_feed {
            self.fail_next_feed = false;
            return Err("injected surface feed failure");
        }
        self.fed_output.push(bytes.to_vec());
        Ok(())
    }

    fn resize(&mut self, size: Size) -> Result<(), &'static str> {
        if self.fail_next_resize {
            self.fail_next_resize = false;
            return Err("injected surface resize failure");
        }
        self.resize_calls.push(size);
        Ok(())
    }
}

impl Default for FakeSurfaceSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AttachSurfaceSink for FakeSurfaceSink {
    type Error = &'static str;

    fn feed_output(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        FakeSurfaceSink::feed_output(self, bytes)
    }

    fn resize(&mut self, size: Size) -> Result<(), Self::Error> {
        FakeSurfaceSink::resize(self, size)
    }
}

/// Fake host terminal control that records suspend/restore calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeTerminalControl {
    suspended_policies: Vec<AttachScreenPolicy>,
    restored_policies: Vec<AttachScreenPolicy>,
    fail_suspend: bool,
    fail_restore_count: u32,
    restore_attempts: usize,
}

impl FakeTerminalControl {
    /// Creates a new fake terminal control with no injected failures.
    pub fn new() -> Self {
        Self {
            suspended_policies: Vec::new(),
            restored_policies: Vec::new(),
            fail_suspend: false,
            fail_restore_count: 0,
            restore_attempts: 0,
        }
    }

    /// Injects a suspend failure.
    pub fn fail_suspend(&mut self) {
        self.fail_suspend = true;
    }

    /// Causes the next `count` restore attempts to fail.
    pub fn fail_restore_times(&mut self, count: u32) {
        self.fail_restore_count = count;
    }

    /// Returns the screen policies passed to suspend.
    pub fn suspended_policies(&self) -> &[AttachScreenPolicy] {
        &self.suspended_policies
    }

    /// Returns the screen policies restored so far.
    pub fn restored_policies(&self) -> &[AttachScreenPolicy] {
        &self.restored_policies
    }

    /// Returns the number of successful restore calls.
    ///
    /// Failed restore attempts are not counted.
    pub fn restore_call_count(&self) -> usize {
        self.restored_policies.len()
    }

    /// Returns the number of restore attempts, including failures.
    pub fn restore_attempt_count(&self) -> usize {
        self.restore_attempts
    }
}

impl HostTerminalControl for FakeTerminalControl {
    type Error = &'static str;

    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> std::result::Result<TerminalRestoreToken, Self::Error> {
        if self.fail_suspend {
            self.fail_suspend = false;
            return Err("injected suspend failure");
        }
        self.suspended_policies.push(policy);
        Ok(TerminalRestoreToken::new(policy))
    }

    fn restore_after_attach(
        &mut self,
        token: &mut TerminalRestoreToken,
    ) -> std::result::Result<(), Self::Error> {
        self.restore_attempts += 1;
        if self.fail_restore_count > 0 {
            self.fail_restore_count -= 1;
            return Err("injected restore failure");
        }
        self.restored_policies.push(token.screen_policy());
        token.consume();
        Ok(())
    }
}

impl Default for FakeTerminalControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Scripted in-memory attach bridge runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachHarness {
    bridge: AttachBridge,
    terminal: InMemoryTerminal,
    pty: FakePtyEndpoint,
    surface: FakeSurfaceSink,
    control: FakeTerminalControl,
    embedded_size: Size,
    script: VecDeque<AttachHarnessStep>,
}

impl AttachHarness {
    /// Creates a new attach harness with fake endpoints.
    pub fn new(
        pane_id: PaneId,
        options: AttachOptions,
        embedded_size: Size,
        real_terminal_size: Size,
    ) -> Self {
        Self {
            bridge: AttachBridge::with_options(pane_id, options),
            terminal: InMemoryTerminal::new(real_terminal_size),
            pty: FakePtyEndpoint::new(),
            surface: FakeSurfaceSink::new(),
            control: FakeTerminalControl::new(),
            embedded_size,
            script: VecDeque::new(),
        }
    }

    /// Returns the current attach bridge state.
    pub fn bridge_state(&self) -> AttachState {
        self.bridge.state()
    }

    /// Returns the fake terminal.
    pub fn terminal(&self) -> &InMemoryTerminal {
        &self.terminal
    }

    /// Returns the fake terminal for mutation.
    pub fn terminal_mut(&mut self) -> &mut InMemoryTerminal {
        &mut self.terminal
    }

    /// Returns the fake PTY endpoint.
    pub fn pty(&self) -> &FakePtyEndpoint {
        &self.pty
    }

    /// Returns the fake PTY endpoint for mutation.
    pub fn pty_mut(&mut self) -> &mut FakePtyEndpoint {
        &mut self.pty
    }

    /// Returns the fake surface sink.
    pub fn surface(&self) -> &FakeSurfaceSink {
        &self.surface
    }

    /// Returns the fake surface sink for mutation.
    pub fn surface_mut(&mut self) -> &mut FakeSurfaceSink {
        &mut self.surface
    }

    /// Returns the fake terminal control.
    pub fn control(&self) -> &FakeTerminalControl {
        &self.control
    }

    /// Returns the fake terminal control for mutation.
    pub fn control_mut(&mut self) -> &mut FakeTerminalControl {
        &mut self.control
    }

    /// Queues a stdin read event for the given bytes and timestamp.
    pub fn queue_stdin(&mut self, at: Instant, bytes: impl Into<Vec<u8>>) {
        self.terminal.queue_stdin(at, bytes);
        self.script.push_back(AttachHarnessStep::ReadStdin);
    }

    /// Queues a PTY-output drain event for the given bytes.
    pub fn queue_pty_output(&mut self, bytes: impl Into<Vec<u8>>) {
        self.pty.queue_output(bytes);
        self.script.push_back(AttachHarnessStep::DrainPtyOutput);
    }

    /// Queues a real-terminal resize event.
    pub fn queue_resize(&mut self, size: Size) {
        self.script
            .push_back(AttachHarnessStep::ResizeTerminal(size));
    }

    /// Queues a clock tick for detach-matcher timeout checks.
    pub fn queue_tick(&mut self, at: Instant) {
        self.script.push_back(AttachHarnessStep::Tick(at));
    }

    /// Runs the scripted attach session.
    ///
    /// Successful runs always model a complete attach/detach cycle. If the
    /// script ends before a detach sequence is observed, this returns
    /// [`AttachHarnessError::ScriptExhaustedBeforeDetach`].
    ///
    /// After a detach match, trailing scripted [`AttachHarnessStep::DrainPtyOutput`]
    /// steps are still consumed before cleanup so the harness can model
    /// detach-time PTY draining for [`AttachOutputPolicy::StdoutOnlyThenReplay`].
    ///
    /// On errors after attach has been confirmed, the bridge remains in
    /// [`AttachState::Attached`] even though [`AttachGuard::drop`] still
    /// restores the host terminal during cleanup.
    pub fn run(&mut self) -> Result<(), AttachHarnessError> {
        let Self {
            bridge,
            terminal,
            pty,
            surface,
            control,
            embedded_size,
            script,
        } = self;
        let options = bridge.options().clone();

        bridge.begin_attach();
        let token = control
            .suspend_for_attach(options.screen)
            .map_err(AttachHarnessError::Suspend)?;
        let mut guard = AttachGuard::new(control, token);
        let mut matcher = DetachMatcher::new(&options.detach);
        let mut deferred_surface_output = Vec::new();

        if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize) {
            Self::resize_attached_target(pty, surface, terminal.size())?;
        }
        bridge.confirm_attached();

        while let Some(step) = script.pop_front() {
            match step {
                AttachHarnessStep::ReadStdin => {
                    if let Some(stdin) = terminal.read_stdin() {
                        let result = matcher.feed_bytes(&stdin.bytes, stdin.at);
                        if !result.forward.is_empty() {
                            pty.write_input(&result.forward)
                                .map_err(AttachHarnessError::PtyWrite)?;
                        }
                        if result.detached {
                            Self::drain_trailing_pty_output(
                                script,
                                terminal,
                                pty,
                                surface,
                                &options,
                                &mut deferred_surface_output,
                            )?;
                            Self::finish_detach(
                                bridge,
                                &options,
                                pty,
                                surface,
                                *embedded_size,
                                &deferred_surface_output,
                                &mut guard,
                            )?;
                            return Ok(());
                        }
                    }
                }
                AttachHarnessStep::DrainPtyOutput => {
                    Self::drain_pty_output(
                        terminal,
                        pty,
                        surface,
                        &options,
                        &mut deferred_surface_output,
                    )?;
                }
                AttachHarnessStep::ResizeTerminal(size) => {
                    terminal.set_size(size);
                    if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize) {
                        Self::resize_attached_target(pty, surface, size)?;
                    }
                }
                AttachHarnessStep::Tick(at) => {
                    if let Some(bytes) = matcher.check_timeout(at) {
                        pty.write_input(&bytes)
                            .map_err(AttachHarnessError::PtyWrite)?;
                    }
                }
            }
        }

        Err(AttachHarnessError::ScriptExhaustedBeforeDetach)
    }

    fn drain_pty_output(
        terminal: &mut InMemoryTerminal,
        pty: &mut FakePtyEndpoint,
        surface: &mut FakeSurfaceSink,
        options: &AttachOptions,
        deferred_surface_output: &mut Vec<Vec<u8>>,
    ) -> Result<(), AttachHarnessError> {
        if let Some(bytes) = pty.try_read_output() {
            terminal
                .write_stdout(&bytes)
                .map_err(AttachHarnessError::Stdout)?;
            match options.output {
                AttachOutputPolicy::FanoutToSurfaceAndStdout => {
                    surface
                        .feed_output(&bytes)
                        .map_err(AttachHarnessError::SurfaceFeed)?;
                }
                AttachOutputPolicy::StdoutOnlyThenReplay => {
                    deferred_surface_output.push(bytes);
                }
            }
        }
        Ok(())
    }

    fn drain_trailing_pty_output(
        script: &mut VecDeque<AttachHarnessStep>,
        terminal: &mut InMemoryTerminal,
        pty: &mut FakePtyEndpoint,
        surface: &mut FakeSurfaceSink,
        options: &AttachOptions,
        deferred_surface_output: &mut Vec<Vec<u8>>,
    ) -> Result<(), AttachHarnessError> {
        while matches!(script.front(), Some(AttachHarnessStep::DrainPtyOutput)) {
            script.pop_front();
            Self::drain_pty_output(terminal, pty, surface, options, deferred_surface_output)?;
        }
        Ok(())
    }

    fn resize_attached_target(
        pty: &mut FakePtyEndpoint,
        surface: &mut FakeSurfaceSink,
        size: Size,
    ) -> Result<(), AttachHarnessError> {
        pty.resize(size).map_err(AttachHarnessError::PtyResize)?;
        surface
            .resize(size)
            .map_err(AttachHarnessError::SurfaceResize)?;
        Ok(())
    }

    fn finish_detach(
        bridge: &mut AttachBridge,
        options: &AttachOptions,
        pty: &mut FakePtyEndpoint,
        surface: &mut FakeSurfaceSink,
        embedded_size: Size,
        deferred_surface_output: &[Vec<u8>],
        guard: &mut AttachGuard<'_, FakeTerminalControl>,
    ) -> Result<(), AttachHarnessError> {
        if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize) {
            Self::resize_attached_target(pty, surface, embedded_size)?;
        }
        if matches!(options.output, AttachOutputPolicy::StdoutOnlyThenReplay) {
            for bytes in deferred_surface_output {
                surface
                    .feed_output(bytes)
                    .map_err(AttachHarnessError::SurfaceFeed)?;
            }
        }
        guard.detach().map_err(AttachHarnessError::Restore)?;
        bridge.begin_detach();
        bridge.confirm_detached();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
