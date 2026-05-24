//! Manager types for orchestrating PTY-backed panes.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::ops::{Bound, RangeBounds};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::default_surface::DefaultSurfaceBackend;
use crate::detach_encoding::{format_attach_bytes_for_trace, normalize_attach_detach_input};
use crate::encoder::XtermEncoder;
use crate::mode_overlay::TerminalModeOverlay as ModeOverlay;
use crate::repro::{
    redact_spawn_config, ReproDump, ReproDumpOptions, ReproRawTranscript, ReproSizeEvent,
};
use crate::transcript::Transcript;
use crate::PaneSnapshot;
use crate::{
    AttachEndedEvent, AttachOptions, AttachOutputPolicy, AttachResizePolicy, AttachStartedEvent,
    DetachReason, DirtyRows, ErrorEvent, ExitedEvent, HostInput, InputIntent, InputKind,
    InputOutcome, InputSentEvent, InputTransaction, InputTransactionError, InputVerification,
    IoOperation, KeyCode, KeyEventKind, KeyInput, KeyModifiers, KillReason, OutputEvent,
    OverflowEvent, OverflowQueue, PaneConfig, PaneError, PaneEvent, PaneEventKind, PaneId,
    PaneInteractionMode, PaneState, PaneStats, PortablePtyBackend, PtyBackend, PtyFrame,
    PtyProcess, ResizedEvent, Result, Size, SpawnedEvent, StateChangedEvent, SurfaceBackend,
    SurfaceChangedEvent, SurfaceRow, SurfaceUpdate, TerminalModes, TerminalViewport,
    TerminalViewportMetrics, TranscriptRotatedEvent,
};

const DEFAULT_MAX_PTY_FRAMES_PER_DRAIN: usize = 32;
const DEFAULT_MAX_PTY_FRAMES_PER_REPRO_DUMP: usize = 4096;
const DEFAULT_INPUT_TRANSACTION_POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEFAULT_ATTACH_POLL_INTERVAL: Duration = Duration::from_millis(10);
const DEFAULT_ATTACH_DETACH_DRAIN_QUIET: Duration = Duration::from_millis(200);
const DEFAULT_ATTACH_DETACH_DRAIN_MAX_WAIT: Duration = Duration::from_secs(2);
const DEFAULT_ATTACH_MAX_PTY_FRAMES_PER_TICK: usize = 32;
const ATTACH_SCREEN_RESET: &[u8] = b"\x1b[0m\x1b[2J\x1b[3J\x1b[H";

type PtySpawner = Arc<dyn Fn(&PaneConfig) -> Result<Box<dyn PtyProcess>> + Send + Sync>;
type SurfaceFactory =
    Arc<dyn Fn(PaneId, &PaneConfig) -> Result<Box<dyn SurfaceBackend + Send>> + Send + Sync>;
type EventSink = Arc<dyn Fn(&PaneEvent) + Send + Sync>;

mod attach_support;
use attach_support::{
    pane_attach_error_to_pane_error, render_attach_viewport_bytes, split_attach_viewport_controls,
    AttachOutputDrainResult, AttachStdinPollResult, AttachViewportAction, ManagerDetachMatcher,
    PaneAttachGuard,
};
pub use attach_support::{
    PaneAttachError, PaneAttachInputChunk, PaneAttachOutcome, PaneAttachTerminal,
    PaneAttachTerminalControl,
};

/// Clamps a generic slice range to `0..len`, converting inclusive bounds to
/// Rust's exclusive-end indexing.
fn slice_range<R>(range: R, len: usize) -> std::ops::Range<usize>
where
    R: RangeBounds<usize>,
{
    let start = match range.start_bound() {
        Bound::Included(&idx) => idx,
        Bound::Excluded(&idx) => idx.saturating_add(1),
        Bound::Unbounded => 0,
    }
    .min(len);

    let end = match range.end_bound() {
        Bound::Included(&idx) => idx.saturating_add(1),
        Bound::Excluded(&idx) => idx,
        Bound::Unbounded => len,
    }
    .min(len);

    start.min(end)..end
}

/// Rounds a byte index up to the next valid UTF-8 character boundary.
fn ceil_char_boundary(text: &str, idx: usize) -> usize {
    let len = text.len();
    let mut idx = idx.min(len);
    while idx < len && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Rounds a byte index down to the previous valid UTF-8 character boundary.
fn floor_char_boundary(text: &str, idx: usize) -> usize {
    let mut idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Clamps a byte range to valid UTF-8 boundaries without returning partial
/// code points. Misaligned starts round up; misaligned ends round down.
fn utf8_byte_slice_range<R>(text: &str, range: R) -> std::ops::Range<usize>
where
    R: RangeBounds<usize>,
{
    let range = slice_range(range, text.len());
    let start = ceil_char_boundary(text, range.start);
    let end = floor_char_boundary(text, range.end);
    start.min(end)..end
}

/// Configuration for the pane manager.
#[derive(Clone)]
pub struct PaneManagerConfig {
    pty_spawner: PtySpawner,
    surface_factory: SurfaceFactory,
    max_pty_frames_per_drain: usize,
    max_pty_frames_per_repro_dump: usize,
}

impl fmt::Debug for PaneManagerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneManagerConfig")
            .field("max_pty_frames_per_drain", &self.max_pty_frames_per_drain)
            .field(
                "max_pty_frames_per_repro_dump",
                &self.max_pty_frames_per_repro_dump,
            )
            .finish_non_exhaustive()
    }
}

impl Default for PaneManagerConfig {
    fn default() -> Self {
        Self {
            pty_spawner: Arc::new(|config| {
                PortablePtyBackend
                    .spawn(config)
                    .map(|process| Box::new(process) as Box<dyn PtyProcess>)
            }),
            surface_factory: Arc::new(|pane_id, config| {
                Ok(Box::new(DefaultSurfaceBackend::new(
                    pane_id,
                    config.size,
                    config.scrollback,
                )?) as Box<dyn SurfaceBackend + Send>)
            }),
            max_pty_frames_per_drain: DEFAULT_MAX_PTY_FRAMES_PER_DRAIN,
            max_pty_frames_per_repro_dump: DEFAULT_MAX_PTY_FRAMES_PER_REPRO_DUMP,
        }
    }
}

impl PaneManagerConfig {
    /// Overrides how PTY-backed child processes are spawned.
    pub fn with_pty_spawner<F>(mut self, spawner: F) -> Self
    where
        F: Fn(&PaneConfig) -> Result<Box<dyn PtyProcess>> + Send + Sync + 'static,
    {
        self.pty_spawner = Arc::new(spawner);
        self
    }

    /// Overrides the PTY backend used for new panes.
    pub fn with_pty_backend<B>(self, backend: B) -> Self
    where
        B: PtyBackend + Clone + Send + Sync + 'static,
        B::Process: PtyProcess + 'static,
    {
        let backend = Arc::new(backend);
        self.with_pty_spawner(move |config| {
            backend
                .spawn(config)
                .map(|process| Box::new(process) as Box<dyn PtyProcess>)
        })
    }

    /// Overrides how surface backends are created for new panes.
    pub fn with_surface_factory<F>(mut self, factory: F) -> Self
    where
        F: Fn(PaneId, &PaneConfig) -> Result<Box<dyn SurfaceBackend + Send>>
            + Send
            + Sync
            + 'static,
    {
        self.surface_factory = Arc::new(factory);
        self
    }

    /// Sets the maximum number of PTY frames drained per pane on each tick.
    pub fn with_max_pty_frames_per_drain(mut self, limit: usize) -> Self {
        self.max_pty_frames_per_drain = limit.max(1);
        self
    }

    /// Sets the maximum number of PTY frames `dump_repro()` may drain before
    /// requiring the pane to quiesce.
    pub fn with_max_pty_frames_per_repro_dump(mut self, limit: usize) -> Self {
        self.max_pty_frames_per_repro_dump = limit.max(1);
        self
    }
}

/// Range-aware access to a pane's retained scrollback.
#[derive(Debug)]
pub struct ScrollbackReader<'a> {
    snapshot: crate::ScrollbackSnapshot<'a>,
}

impl<'a> ScrollbackReader<'a> {
    fn new(snapshot: crate::ScrollbackSnapshot<'a>) -> Self {
        Self { snapshot }
    }

    /// Returns retained scrollback lines for the requested line range.
    pub fn lines<R>(&self, range: R) -> &[crate::ScrollbackLine<'a>]
    where
        R: RangeBounds<usize>,
    {
        let range = slice_range(range, self.snapshot.lines.len());
        &self.snapshot.lines[range]
    }

    /// Returns the full retained scrollback snapshot.
    pub fn snapshot(&self) -> &crate::ScrollbackSnapshot<'a> {
        &self.snapshot
    }

    /// Clones the retained scrollback into an owned snapshot.
    pub fn to_owned_snapshot(&self) -> crate::OwnedScrollbackSnapshot {
        self.snapshot.to_owned_snapshot()
    }
}

/// Range-aware access to a pane's retained transcript buffers.
#[derive(Debug, Clone)]
pub struct TranscriptReader<'a> {
    raw_start_offset: u64,
    plain_start_offset: u64,
    raw: &'a [u8],
    plain: &'a str,
}

impl<'a> TranscriptReader<'a> {
    fn new(raw_start_offset: u64, plain_start_offset: u64, raw: &'a [u8], plain: &'a str) -> Self {
        Self {
            raw_start_offset,
            plain_start_offset,
            raw,
            plain,
        }
    }

    /// Returns the absolute raw-stream offset of the first retained byte.
    pub const fn retained_raw_start_offset(&self) -> u64 {
        self.raw_start_offset
    }

    /// Returns the absolute plain-text-stream offset of the first retained
    /// plain-text byte.
    pub const fn retained_plain_start_offset(&self) -> u64 {
        self.plain_start_offset
    }

    /// Returns retained raw transcript bytes for the requested byte range.
    ///
    /// The range is relative to the currently retained raw transcript slice.
    /// To translate an absolute `OutputEvent::transcript_offset` in raw-byte
    /// or both mode, subtract [`TranscriptReader::retained_raw_start_offset`]
    /// first.
    pub fn ansi_bytes<R>(&self, range: R) -> &[u8]
    where
        R: RangeBounds<usize>,
    {
        let range = slice_range(range, self.raw.len());
        &self.raw[range]
    }

    /// Returns retained plain-text transcript content for the requested
    /// byte range.
    ///
    /// The range is relative to the currently retained plain-text transcript
    /// slice, not a character index. To translate an absolute
    /// `OutputEvent::transcript_offset` in plain-text mode, subtract
    /// [`TranscriptReader::retained_plain_start_offset`] first.
    pub fn plain_text<R>(&self, range: R) -> &str
    where
        R: RangeBounds<usize>,
    {
        let range = utf8_byte_slice_range(self.plain, range);
        &self.plain[range]
    }
}

/// Outcome of a pane's removal from the manager.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaneExit {
    /// The child process exited normally.
    Exited {
        /// The process exit code, if available.
        code: Option<i32>,
    },
    /// The pane was killed.
    Killed {
        /// The reason the pane was killed.
        reason: KillReason,
    },
    /// The pane failed.
    Failed {
        /// The error that caused the failure.
        error: PaneError,
    },
}

/// A lightweight cloneable handle for sending commands to a single pane.
///
/// This is useful when host applications need to send input from background tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneHandle {
    /// The pane this handle controls.
    pub id: PaneId,
}

impl PaneHandle {
    /// Creates a new handle for the given pane.
    pub const fn new(id: PaneId) -> Self {
        Self { id }
    }

    /// Writes raw bytes to the pane's PTY.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn write_bytes(&self, _bytes: Vec<u8>) -> Result<()> {
        Err(PaneError::NotFound { pane_id: self.id })
    }

    /// Resizes the pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn resize(&self, _size: Size) -> Result<()> {
        Err(PaneError::NotFound { pane_id: self.id })
    }

    /// Kills the pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn kill(&self, _reason: KillReason) -> Result<()> {
        Err(PaneError::NotFound { pane_id: self.id })
    }
}

/// Manager that owns panes and polls their PTY/runtime state.
pub struct PaneManager {
    next_pane_id: u64,
    panes: BTreeMap<PaneId, PaneRuntimeState>,
    events: VecDeque<PaneEvent>,
    subscribers: Vec<Sender<PaneEvent>>,
    event_sink: Option<EventSink>,
    config: PaneManagerConfig,
}

impl fmt::Debug for PaneManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneManager")
            .field("next_pane_id", &self.next_pane_id)
            .field("panes", &self.panes)
            .field("queued_events", &self.events.len())
            .field("subscriber_count", &self.subscribers.len())
            .field("has_event_sink", &self.event_sink.is_some())
            .field("config", &self.config)
            .finish()
    }
}

struct PaneRuntimeState {
    title: Option<String>,
    spawn_config: PaneConfig,
    state: PaneState,
    interaction_mode: PaneInteractionMode,
    input_config: crate::InputConfig,
    stats: PaneStats,
    surface_generation: u64,
    next_seq: u64,
    size_history: Vec<ReproSizeEvent>,
    event_log: Vec<PaneEvent>,
    surface: Box<dyn SurfaceBackend + Send>,
    process: Option<Box<dyn PtyProcess>>,
    exit: Option<PaneExit>,
    exit_event_observed: bool,
    transcript: Transcript,
    mode_overlay: ModeOverlay,
}

impl fmt::Debug for PaneRuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneRuntimeState")
            .field("title", &self.title)
            .field("state", &self.state)
            .field("interaction_mode", &self.interaction_mode)
            .field("stats", &self.stats)
            .field("surface_generation", &self.surface_generation)
            .field("next_seq", &self.next_seq)
            .field("size_history_len", &self.size_history.len())
            .field("event_log_len", &self.event_log.len())
            .field("surface", &self.surface)
            .field("process", &self.process.as_ref().map(|_| "<pty-process>"))
            .field("exit", &self.exit)
            .field("exit_event_observed", &self.exit_event_observed)
            .finish()
    }
}

impl PaneRuntimeState {
    fn merged_surface_snapshot(&self) -> crate::SurfaceSnapshot<'_> {
        let cursor = self.surface.cursor();
        let modes = self.mode_overlay.merge_with(self.surface.modes());
        let mut surface = self.surface.snapshot();
        surface.cursor = cursor;
        surface.modes = modes;
        surface
    }

    fn snapshot(&self, pane_id: PaneId) -> PaneSnapshot<'_> {
        let surface = self.merged_surface_snapshot();

        PaneSnapshot {
            id: pane_id,
            title: self.title.clone(),
            state: self.state.clone(),
            interaction_mode: self.interaction_mode,
            size: surface.size,
            cursor: surface.cursor,
            modes: surface.modes,
            stats: self.stats,
            surface,
        }
    }

    fn remove_exit(self) -> Option<PaneExit> {
        self.exit.or(match self.state {
            PaneState::Exited { code } => Some(PaneExit::Exited { code }),
            PaneState::Killed { reason } => Some(PaneExit::Killed { reason }),
            PaneState::Failed { error } => Some(PaneExit::Failed { error }),
            PaneState::Starting | PaneState::Running => None,
        })
    }
}

struct AttachOutputDrain<'a, Terminal> {
    terminal: &'a mut Terminal,
    options: &'a AttachOptions,
    replay_buffer: &'a mut Vec<u8>,
    child_exit_code: &'a mut Option<i32>,
    max_frames: usize,
    forward_stdout: bool,
}

#[derive(Debug, Clone)]
struct InputWriteStep {
    bytes: Vec<u8>,
    input_kind: InputKind,
    submits: bool,
}

impl Default for PaneManager {
    fn default() -> Self {
        Self::new(PaneManagerConfig::default())
    }
}

impl PaneManager {
    /// Creates a manager with the given configuration.
    pub fn new(config: PaneManagerConfig) -> Self {
        Self {
            next_pane_id: 0,
            panes: BTreeMap::new(),
            events: VecDeque::new(),
            subscribers: Vec::new(),
            event_sink: None,
            config,
        }
    }

    /// Allocates a predictable placeholder pane identifier for smoke tests.
    #[doc(hidden)]
    pub fn alloc_placeholder_id(&mut self) -> PaneId {
        self.next_pane_id += 1;
        PaneId::new(self.next_pane_id)
    }

    /// Spawns a new pane with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane cannot be spawned.
    pub fn spawn(&mut self, config: PaneConfig) -> Result<PaneId> {
        config.validate()?;

        let pane_id = config.id.unwrap_or_else(|| {
            self.next_pane_id += 1;
            PaneId::new(self.next_pane_id)
        });
        self.next_pane_id = self.next_pane_id.max(pane_id.get());

        if self.panes.contains_key(&pane_id) {
            return Err(PaneError::Spawn {
                message: format!("pane {} already exists", pane_id.get()),
            });
        }

        let surface = (self.config.surface_factory)(pane_id, &config)?;
        let process = (self.config.pty_spawner)(&config)?;
        let program = config.program().to_string();
        let mut spawn_config = config;
        spawn_config.id = Some(pane_id);
        let initial_size = spawn_config.size;
        let input_config = spawn_config.input;
        let transcript_config = spawn_config.transcript;

        self.panes.insert(
            pane_id,
            PaneRuntimeState {
                title: spawn_config.title.clone(),
                spawn_config,
                state: PaneState::Starting,
                interaction_mode: PaneInteractionMode::Embedded,
                input_config,
                stats: PaneStats,
                surface_generation: 0,
                next_seq: 0,
                size_history: vec![ReproSizeEvent::new(0, initial_size)],
                event_log: Vec::new(),
                surface,
                process: Some(process),
                exit: None,
                exit_event_observed: false,
                transcript: Transcript::new(transcript_config),
                mode_overlay: ModeOverlay::default(),
            },
        );

        self.push_event(pane_id, PaneEventKind::Spawned(SpawnedEvent { program }));
        self.transition_state(pane_id, PaneState::Running);
        Ok(pane_id)
    }

    /// Removes a terminal pane from the manager, returning its exit status if
    /// known.
    ///
    /// Removal is the ownership cleanup step: it drops the manager-owned pane
    /// runtime state, including the PTY process handle, surface backend,
    /// transcript buffers, and retained event log. Live panes must be killed,
    /// failed, or observed as exited before they can be removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn remove(&mut self, pane_id: PaneId) -> Result<Option<PaneExit>> {
        // Drain frames first so exit observations can transition state before
        // we validate. Do NOT flush pending transcript yet -- a rejected remove()
        // on a live pane must not corrupt held-back escape sequences.
        self.drain_all_pane_frames(pane_id);

        {
            let pane = self
                .panes
                .get(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            if !matches!(
                pane.state,
                PaneState::Exited { .. } | PaneState::Killed { .. } | PaneState::Failed { .. }
            ) {
                return Err(PaneError::InvalidState {
                    expected: "Exited, Failed, or Killed".into(),
                    actual: state_label(&pane.state),
                });
            }
        }

        // Pane is confirmed terminal -- now safe to flush pending transcript
        // and emit any synthetic exit events before actual removal.
        self.flush_terminal_pane_before_remove(pane_id);

        let pane = self
            .panes
            .remove(&pane_id)
            .expect("pane should still exist when removing a terminal pane");
        Ok(pane.remove_exit())
    }

    /// Kills a pane.
    ///
    /// This requests child termination and transitions the pane to
    /// [`PaneState::Killed`], but it does not remove the manager-owned pane
    /// state. The killed pane remains available for snapshots, transcripts,
    /// scrollback, repro dumps, and event inspection until
    /// [`PaneManager::remove`] is called. Use
    /// [`PaneManager::kill_and_remove`] when no post-kill inspection is
    /// needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn kill(&mut self, pane_id: PaneId, reason: KillReason) -> Result<()> {
        let process = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            if pane.process.is_none() {
                return Err(PaneError::InvalidState {
                    expected: "pty-backed pane".into(),
                    actual: "surface-only pane".into(),
                });
            }
            ensure_running_state(pane)?;
            pane.process
                .as_mut()
                .expect("pty-backed pane should retain its process")
        };

        process.kill()?;
        self.set_exit_if_not_killed_or_failed(pane_id, PaneExit::Killed { reason });
        self.transition_state(pane_id, PaneState::Killed { reason });
        Ok(())
    }

    /// Kills a pane and immediately removes its terminal state from the
    /// manager.
    ///
    /// This is a convenience for reset/restart paths that do not need to
    /// inspect the killed pane after termination. It first delegates to
    /// [`PaneManager::kill`], then delegates to [`PaneManager::remove`], so it
    /// has the same termination behavior as an explicit `kill(); remove()`
    /// sequence while ensuring manager-owned PTY, process, surface,
    /// transcript, and event-log resources are dropped before returning.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found, if the pane cannot be
    /// killed, or if removal fails.
    pub fn kill_and_remove(
        &mut self,
        pane_id: PaneId,
        reason: KillReason,
    ) -> Result<Option<PaneExit>> {
        self.kill(pane_id, reason)?;
        self.remove(pane_id)
    }

    /// Resizes a pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn resize(&mut self, pane_id: PaneId, size: Size) -> Result<()> {
        Size::try_new(size.rows, size.cols)?;

        {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            pane.surface.validate_resize(size)?;
            if matches!(pane.state, PaneState::Running) {
                if let Some(process) = pane.process.as_mut() {
                    process.resize(size)?;
                }
            }
            pane.surface.resize(size)?;
        }

        self.record_size_history(pane_id, size);
        self.push_event(pane_id, PaneEventKind::Resized(ResizedEvent { size }));
        Ok(())
    }

    /// Attaches an existing PTY-backed pane to the real terminal until detach.
    ///
    /// The manager owns the handoff: PTY output still flows through the
    /// transcript, surface, and pane event stream, attach lifecycle events are
    /// emitted with the pane's normal sequence numbers, and the embedded size
    /// is restored when attach ends if requested by [`AttachOptions`].
    ///
    /// The caller still owns the host terminal profile. Embedded TUIs should
    /// pass a [`PaneAttachTerminalControl`] implementation that restores the
    /// exact dashboard profile they need after detach; a generic helper can be
    /// appropriate for simple hosts but is not a substitute for an embedding
    /// application's own terminal-mode stack.
    pub fn attach_blocking<Terminal, Control>(
        &mut self,
        pane_id: PaneId,
        options: AttachOptions,
        terminal: &mut Terminal,
        control: &mut Control,
    ) -> std::result::Result<PaneAttachOutcome, PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
        Control: PaneAttachTerminalControl,
    {
        if !self.panes.contains_key(&pane_id) {
            return Err(PaneAttachError::Pane {
                error: PaneError::NotFound { pane_id },
            });
        }
        self.poll_pane_frames(pane_id);
        let embedded_size = self.prepare_attach(pane_id)?;
        let terminal_size = match terminal.size() {
            Ok(size) => size,
            Err(error) => {
                let error = PaneAttachError::TerminalSize {
                    message: format!("{error:?}"),
                };
                self.push_attach_error_event(pane_id, &error);
                return Err(error);
            }
        };

        self.set_interaction_mode_result(pane_id, PaneInteractionMode::Attaching)?;
        let token = match control.suspend_for_attach(options.screen) {
            Ok(token) => token,
            Err(error) => {
                let error = PaneAttachError::Suspend {
                    message: format!("{error:?}"),
                };
                self.set_interaction_mode_if_present(pane_id, PaneInteractionMode::Embedded);
                self.push_attach_error_event(pane_id, &error);
                return Err(error);
            }
        };

        let mut guard = PaneAttachGuard::new(control, token);
        let started_at = Instant::now();
        let mut restored_embedded_size = false;
        self.push_event(
            pane_id,
            PaneEventKind::AttachStarted(AttachStartedEvent {
                terminal_size,
                embedded_size,
                screen_policy: options.screen,
            }),
        );

        let result = self.run_attached_pane(
            pane_id,
            &options,
            terminal,
            &mut guard,
            embedded_size,
            terminal_size,
            started_at,
            &mut restored_embedded_size,
        );
        if let Err(error) = &result {
            self.push_attach_error_event(pane_id, error);
            self.best_effort_abort_attach(
                pane_id,
                &options,
                &mut guard,
                embedded_size,
                started_at,
                restored_embedded_size,
            );
        }
        result
    }

    fn prepare_attach(&mut self, pane_id: PaneId) -> std::result::Result<Size, PaneAttachError> {
        let pane = self.panes.get(&pane_id).ok_or(PaneAttachError::Pane {
            error: PaneError::NotFound { pane_id },
        })?;
        if pane.process.is_none() {
            return Err(PaneAttachError::Pane {
                error: PaneError::InvalidState {
                    expected: "pty-backed pane".into(),
                    actual: "surface-only pane".into(),
                },
            });
        }
        ensure_running_state(pane).map_err(|error| PaneAttachError::Pane { error })?;
        if pane.interaction_mode != PaneInteractionMode::Embedded {
            return Err(PaneAttachError::Pane {
                error: PaneError::InvalidState {
                    expected: "Embedded interaction mode".into(),
                    actual: format!("{:?}", pane.interaction_mode),
                },
            });
        }
        Ok(pane.surface.size())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_attached_pane<Terminal, Control>(
        &mut self,
        pane_id: PaneId,
        options: &AttachOptions,
        terminal: &mut Terminal,
        guard: &mut PaneAttachGuard<'_, Control>,
        embedded_size: Size,
        mut terminal_size: Size,
        started_at: Instant,
        restored_embedded_size: &mut bool,
    ) -> std::result::Result<PaneAttachOutcome, PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
        Control: PaneAttachTerminalControl,
    {
        let mut matcher = ManagerDetachMatcher::new(&options.detach);
        let mut viewport = TerminalViewport::default();
        let mut replay_buffer = Vec::new();
        let mut child_exit_code = None;

        if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize) {
            self.resize_attached_pane(pane_id, terminal_size, true)?;
        }
        self.set_interaction_mode_result(pane_id, PaneInteractionMode::Attached)?;
        self.render_attach_viewport(pane_id, terminal, viewport, terminal_size)?;

        loop {
            let mut made_progress = false;

            let current_size = terminal
                .size()
                .map_err(|error| PaneAttachError::TerminalSize {
                    message: format!("{error:?}"),
                })?;
            if current_size != terminal_size {
                terminal_size = current_size;
                if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize) {
                    self.resize_attached_pane(pane_id, terminal_size, true)?;
                }
                self.render_attach_viewport(pane_id, terminal, viewport, terminal_size)?;
                made_progress = true;
            }

            match self.poll_attach_stdin(
                pane_id,
                terminal,
                &mut matcher,
                &mut viewport,
                terminal_size,
            )? {
                AttachStdinPollResult::Idle => {}
                AttachStdinPollResult::Forwarded => made_progress = true,
                AttachStdinPollResult::ViewportChanged => {
                    self.render_attach_viewport(pane_id, terminal, viewport, terminal_size)?;
                    made_progress = true;
                }
                AttachStdinPollResult::Detached { remaining_input } => {
                    return self.finish_detach(
                        pane_id,
                        terminal,
                        options,
                        &mut replay_buffer,
                        guard,
                        viewport,
                        child_exit_code,
                        terminal_size,
                        DetachReason::UserChord,
                        remaining_input,
                        started_at,
                        embedded_size,
                        restored_embedded_size,
                    );
                }
            }

            let metrics_before_output =
                self.attach_viewport_metrics(pane_id, viewport, terminal_size);
            let forward_stdout = metrics_before_output.is_at_tail();
            let mut drain = AttachOutputDrain {
                terminal,
                options,
                replay_buffer: &mut replay_buffer,
                child_exit_code: &mut child_exit_code,
                max_frames: DEFAULT_ATTACH_MAX_PTY_FRAMES_PER_TICK,
                forward_stdout,
            };
            let drained = self.drain_attach_output(pane_id, &mut drain)?;
            made_progress |= drained.made_progress;
            if !forward_stdout && drained.made_progress {
                let metrics_after_output =
                    self.attach_viewport_metrics(pane_id, viewport, terminal_size);
                viewport = stabilize_attach_viewport_after_output(
                    viewport,
                    metrics_before_output,
                    metrics_after_output,
                );
            }
            if drained.child_exited {
                return self.finish_detach(
                    pane_id,
                    terminal,
                    options,
                    &mut replay_buffer,
                    guard,
                    viewport,
                    child_exit_code,
                    terminal_size,
                    DetachReason::ChildExited,
                    Vec::new(),
                    started_at,
                    embedded_size,
                    restored_embedded_size,
                );
            }

            if !made_progress {
                thread::sleep(DEFAULT_ATTACH_POLL_INTERVAL);
            }
        }
    }

    fn poll_attach_stdin<Terminal>(
        &mut self,
        pane_id: PaneId,
        terminal: &mut Terminal,
        matcher: &mut ManagerDetachMatcher,
        viewport: &mut TerminalViewport,
        terminal_size: Size,
    ) -> std::result::Result<AttachStdinPollResult, PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
    {
        if let Some(stdin) =
            terminal
                .read_stdin()
                .map_err(|error| PaneAttachError::TerminalInput {
                    message: format!("{error:?}"),
                })?
        {
            if log::log_enabled!(log::Level::Trace) {
                let normalized = normalize_attach_detach_input(&stdin.bytes, &matcher.chord);
                log::trace!(
                    target: "panesmith::attach",
                    "attach stdin chunk pane_id={} raw_len={} raw_hex={} normalized_hex={}",
                    pane_id.get(),
                    stdin.bytes.len(),
                    format_attach_bytes_for_trace(&stdin.bytes),
                    format_attach_bytes_for_trace(&normalized)
                );
            }
            let result = matcher.feed_bytes(&stdin.bytes, stdin.at);
            if result.detached {
                log::debug!(
                    target: "panesmith::attach",
                    "attach detach chord matched pane_id={} forward_len={} remaining_len={}",
                    pane_id.get(),
                    result.forward.len(),
                    result.remaining.len()
                );
            }
            let (forward, actions) = split_attach_viewport_controls(&result.forward);
            let viewport_changed =
                self.apply_attach_viewport_actions(pane_id, viewport, terminal_size, &actions);
            if !forward.is_empty() {
                self.write_attach_input(pane_id, &forward)?;
            }
            if result.detached {
                return Ok(AttachStdinPollResult::Detached {
                    remaining_input: result.remaining.to_vec(),
                });
            }
            if viewport_changed {
                return Ok(AttachStdinPollResult::ViewportChanged);
            }
            return Ok(AttachStdinPollResult::Forwarded);
        }

        if let Some(bytes) = matcher.check_timeout(Instant::now()) {
            let (forward, actions) = split_attach_viewport_controls(&bytes);
            let viewport_changed =
                self.apply_attach_viewport_actions(pane_id, viewport, terminal_size, &actions);
            if !forward.is_empty() {
                self.write_attach_input(pane_id, &forward)?;
            }
            if viewport_changed {
                return Ok(AttachStdinPollResult::ViewportChanged);
            }
            return Ok(AttachStdinPollResult::Forwarded);
        }

        Ok(AttachStdinPollResult::Idle)
    }

    fn apply_attach_viewport_actions(
        &self,
        pane_id: PaneId,
        viewport: &mut TerminalViewport,
        terminal_size: Size,
        actions: &[AttachViewportAction],
    ) -> bool {
        if actions.is_empty() {
            return false;
        }

        let mut next = *viewport;
        for action in actions {
            let metrics = self.attach_viewport_metrics(pane_id, next, terminal_size);
            next = match *action {
                AttachViewportAction::ScrollUp(rows) => next.scroll_up(rows, metrics),
                AttachViewportAction::ScrollDown(rows) => next.scroll_down(rows, metrics),
                AttachViewportAction::PageUp => next.page_up(metrics),
                AttachViewportAction::PageDown => next.page_down(metrics),
                AttachViewportAction::Home => TerminalViewport::scrolled(metrics.max_scroll_offset),
                AttachViewportAction::End => next.follow_tail(),
            };
        }

        let changed = next != *viewport;
        *viewport = next;
        changed
    }

    fn render_attach_viewport<Terminal>(
        &self,
        pane_id: PaneId,
        terminal: &mut Terminal,
        viewport: TerminalViewport,
        terminal_size: Size,
    ) -> std::result::Result<(), PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
    {
        let bytes = self.render_attach_viewport_bytes(pane_id, viewport, terminal_size)?;
        terminal
            .write_stdout(&bytes)
            .map_err(|error| PaneAttachError::TerminalOutput {
                message: format!("{error:?}"),
            })
    }

    fn render_attach_viewport_bytes(
        &self,
        pane_id: PaneId,
        viewport: TerminalViewport,
        terminal_size: Size,
    ) -> std::result::Result<Vec<u8>, PaneAttachError> {
        let pane = self.panes.get(&pane_id).ok_or(PaneAttachError::Pane {
            error: PaneError::NotFound { pane_id },
        })?;
        Ok(render_attach_viewport_bytes(
            &pane.snapshot(pane_id),
            &pane.surface.scrollback(),
            viewport,
            terminal_size,
        ))
    }

    fn attach_viewport_metrics(
        &self,
        pane_id: PaneId,
        viewport: TerminalViewport,
        terminal_size: Size,
    ) -> TerminalViewportMetrics {
        let Some(pane) = self.panes.get(&pane_id) else {
            return viewport.metrics_from_counts(0, 0, usize::from(terminal_size.rows));
        };
        viewport.metrics_from_counts(
            usize::from(pane.surface.size().rows),
            pane.surface.scrollback().lines.len(),
            usize::from(terminal_size.rows),
        )
    }

    fn write_attach_input(
        &mut self,
        pane_id: PaneId,
        bytes: &[u8],
    ) -> std::result::Result<(), PaneAttachError> {
        let write_result = {
            let pane = self.panes.get_mut(&pane_id).ok_or(PaneAttachError::Pane {
                error: PaneError::NotFound { pane_id },
            })?;
            let Some(process) = pane.process.as_ref() else {
                return Err(PaneAttachError::Pane {
                    error: PaneError::InvalidState {
                        expected: "pty-backed pane".into(),
                        actual: "surface-only pane".into(),
                    },
                });
            };
            process.writer().write_bytes(bytes)
        };

        match write_result {
            Ok(()) => {
                self.push_event(
                    pane_id,
                    PaneEventKind::InputSent(InputSentEvent {
                        input_kind: InputKind::Bytes,
                        bytes_len: bytes.len(),
                        recorded: false,
                    }),
                );
                Ok(())
            }
            Err(error) => Err(PaneAttachError::PtyWrite {
                message: format!("{error:?}"),
            }),
        }
    }

    fn drain_attach_output<Terminal>(
        &mut self,
        pane_id: PaneId,
        drain: &mut AttachOutputDrain<'_, Terminal>,
    ) -> std::result::Result<AttachOutputDrainResult, PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
    {
        let mut result = AttachOutputDrainResult {
            made_progress: false,
            child_exited: false,
        };

        for _ in 0..drain.max_frames {
            let Some(frame) = self.next_attach_pty_frame(pane_id)? else {
                return Ok(result);
            };

            result.made_progress = true;
            match frame {
                PtyFrame::Output { bytes, .. } => {
                    if drain.forward_stdout {
                        drain.terminal.write_stdout(&bytes).map_err(|error| {
                            PaneAttachError::TerminalOutput {
                                message: format!("{error:?}"),
                            }
                        })?;
                    }
                    match drain.options.output {
                        AttachOutputPolicy::FanoutToSurfaceAndStdout => {
                            self.record_attach_output_frame(pane_id, bytes)?;
                        }
                        AttachOutputPolicy::StdoutOnlyThenReplay => {
                            drain.replay_buffer.extend_from_slice(&bytes);
                        }
                    }
                }
                PtyFrame::Exited { code, .. } => {
                    if drain.child_exit_code.is_none() {
                        *drain.child_exit_code = code;
                    }
                    self.handle_exit_frame(pane_id, code);
                    result.child_exited = true;
                }
                PtyFrame::Error { message, .. } => {
                    self.fail_pane(
                        pane_id,
                        PaneError::Io {
                            operation: IoOperation::Read,
                            message: message.clone(),
                        },
                    );
                    return Err(PaneAttachError::PtyRuntime { message });
                }
                PtyFrame::Overflow {
                    dropped_frames,
                    dropped_bytes,
                    ..
                } => self.push_event(
                    pane_id,
                    PaneEventKind::Overflow(OverflowEvent {
                        dropped_frames,
                        dropped_bytes,
                        queue: OverflowQueue::PtyOutputFrames,
                    }),
                ),
                PtyFrame::CursorPositionRequest { .. } => {}
            }
        }

        Ok(result)
    }

    fn next_attach_pty_frame(
        &mut self,
        pane_id: PaneId,
    ) -> std::result::Result<Option<PtyFrame>, PaneAttachError> {
        let pane = self.panes.get_mut(&pane_id).ok_or(PaneAttachError::Pane {
            error: PaneError::NotFound { pane_id },
        })?;
        let Some(process) = pane.process.as_mut() else {
            return Err(PaneAttachError::Pane {
                error: PaneError::InvalidState {
                    expected: "pty-backed pane".into(),
                    actual: "surface-only pane".into(),
                },
            });
        };
        Ok(process.try_recv())
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_detach<Terminal, Control>(
        &mut self,
        pane_id: PaneId,
        terminal: &mut Terminal,
        options: &AttachOptions,
        replay_buffer: &mut Vec<u8>,
        guard: &mut PaneAttachGuard<'_, Control>,
        viewport: TerminalViewport,
        mut child_exit_code: Option<i32>,
        terminal_size: Size,
        reason: DetachReason,
        remaining_input: Vec<u8>,
        started_at: Instant,
        embedded_size: Size,
        restored_embedded_size: &mut bool,
    ) -> std::result::Result<PaneAttachOutcome, PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
        Control: PaneAttachTerminalControl,
    {
        self.set_interaction_mode_result(pane_id, PaneInteractionMode::Detaching)?;
        let forward_stdout = self
            .attach_viewport_metrics(pane_id, viewport, terminal_size)
            .is_at_tail();
        let mut drain = AttachOutputDrain {
            terminal,
            options,
            replay_buffer,
            child_exit_code: &mut child_exit_code,
            max_frames: DEFAULT_ATTACH_MAX_PTY_FRAMES_PER_TICK,
            forward_stdout,
        };
        self.drain_detaching_output(pane_id, &mut drain)?;

        if matches!(options.output, AttachOutputPolicy::StdoutOnlyThenReplay)
            && !replay_buffer.is_empty()
        {
            let replay = std::mem::take(replay_buffer);
            self.record_attach_output_frame(pane_id, replay)?;
        }

        if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize) {
            self.resize_attached_pane(pane_id, embedded_size, true)?;
            *restored_embedded_size = true;
        }

        guard.detach().map_err(|error| PaneAttachError::Restore {
            message: format!("{error:?}"),
        })?;
        self.set_interaction_mode_result(pane_id, PaneInteractionMode::Embedded)?;
        self.push_event(
            pane_id,
            PaneEventKind::AttachEnded(AttachEndedEvent {
                reason,
                restored_size: embedded_size,
                duration: started_at.elapsed(),
            }),
        );

        Ok(PaneAttachOutcome {
            reason,
            child_exit_code,
            terminal_size,
            restored_size: embedded_size,
            remaining_input,
        })
    }

    fn drain_detaching_output<Terminal>(
        &mut self,
        pane_id: PaneId,
        drain: &mut AttachOutputDrain<'_, Terminal>,
    ) -> std::result::Result<(), PaneAttachError>
    where
        Terminal: PaneAttachTerminal,
    {
        let started = Instant::now();
        let mut last_activity = started;

        loop {
            let saw_output = self.drain_attach_output(pane_id, drain)?.made_progress;
            if saw_output {
                last_activity = Instant::now();
            }

            let now = Instant::now();
            if now.duration_since(last_activity) >= DEFAULT_ATTACH_DETACH_DRAIN_QUIET
                || now.duration_since(started) >= DEFAULT_ATTACH_DETACH_DRAIN_MAX_WAIT
            {
                return Ok(());
            }

            thread::sleep(DEFAULT_ATTACH_POLL_INTERVAL);
        }
    }

    fn record_attach_output_frame(
        &mut self,
        pane_id: PaneId,
        bytes: Vec<u8>,
    ) -> std::result::Result<(), PaneAttachError> {
        match self.record_output_frame(pane_id, bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = surface_error(error);
                self.fail_pane(pane_id, error.clone());
                Err(PaneAttachError::SurfaceFeed {
                    message: error.to_string(),
                })
            }
        }
    }

    fn resize_attached_pane(
        &mut self,
        pane_id: PaneId,
        size: Size,
        emit_event: bool,
    ) -> std::result::Result<(), PaneAttachError> {
        Size::try_new(size.rows, size.cols).map_err(|error| PaneAttachError::Pane { error })?;

        {
            let pane = self.panes.get_mut(&pane_id).ok_or(PaneAttachError::Pane {
                error: PaneError::NotFound { pane_id },
            })?;
            pane.surface
                .validate_resize(size)
                .map_err(|error| PaneAttachError::SurfaceResize {
                    message: format!("{error:?}"),
                })?;
            if matches!(pane.state, PaneState::Starting | PaneState::Running) {
                if let Some(process) = pane.process.as_mut() {
                    process
                        .resize(size)
                        .map_err(|error| PaneAttachError::PtyResize {
                            message: format!("{error:?}"),
                        })?;
                }
            }
            pane.surface
                .resize(size)
                .map_err(|error| PaneAttachError::SurfaceResize {
                    message: format!("{error:?}"),
                })?;
        }

        self.record_size_history(pane_id, size);
        if emit_event {
            self.push_event(pane_id, PaneEventKind::Resized(ResizedEvent { size }));
        }
        Ok(())
    }

    fn best_effort_abort_attach<Control>(
        &mut self,
        pane_id: PaneId,
        options: &AttachOptions,
        guard: &mut PaneAttachGuard<'_, Control>,
        embedded_size: Size,
        started_at: Instant,
        restored_embedded_size: bool,
    ) where
        Control: PaneAttachTerminalControl,
    {
        if matches!(
            self.panes.get(&pane_id).map(|pane| pane.interaction_mode),
            Some(PaneInteractionMode::Attached)
        ) {
            self.set_interaction_mode_if_present(pane_id, PaneInteractionMode::Detaching);
        }
        if matches!(options.resize, AttachResizePolicy::UseRealTerminalSize)
            && !restored_embedded_size
        {
            if let Err(error) = self.resize_attached_pane(pane_id, embedded_size, true) {
                self.push_attach_error_event(pane_id, &error);
            }
        }
        if !guard.is_detached() {
            if let Err(error) = guard.detach() {
                let error = PaneAttachError::Restore {
                    message: format!("{error:?}"),
                };
                self.push_attach_error_event(pane_id, &error);
                if let Err(error) = guard.detach() {
                    let error = PaneAttachError::Restore {
                        message: format!("{error:?}"),
                    };
                    self.push_attach_error_event(pane_id, &error);
                    guard.disarm();
                }
            }
        }
        self.set_interaction_mode_if_present(pane_id, PaneInteractionMode::Embedded);
        self.push_event(
            pane_id,
            PaneEventKind::AttachEnded(AttachEndedEvent {
                reason: DetachReason::Error,
                restored_size: embedded_size,
                duration: started_at.elapsed(),
            }),
        );
    }

    fn set_interaction_mode_result(
        &mut self,
        pane_id: PaneId,
        mode: PaneInteractionMode,
    ) -> std::result::Result<(), PaneAttachError> {
        let pane = self.panes.get_mut(&pane_id).ok_or(PaneAttachError::Pane {
            error: PaneError::NotFound { pane_id },
        })?;
        pane.interaction_mode = mode;
        Ok(())
    }

    fn set_interaction_mode_if_present(&mut self, pane_id: PaneId, mode: PaneInteractionMode) {
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.interaction_mode = mode;
        }
    }

    fn push_attach_error_event(&mut self, pane_id: PaneId, error: &PaneAttachError) {
        if self.panes.contains_key(&pane_id) {
            self.push_event(
                pane_id,
                PaneEventKind::Error(ErrorEvent {
                    error: pane_attach_error_to_pane_error(error),
                }),
            );
        }
    }

    fn push_event(&mut self, pane_id: PaneId, kind: PaneEventKind) {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .expect("pane must exist before pushing events");
        pane.next_seq += 1;
        let event = PaneEvent {
            pane_id,
            seq: pane.next_seq,
            at: SystemTime::now(),
            kind,
        };
        pane.event_log.push(event.clone());
        self.events.push_back(event);
        let event = self
            .events
            .back()
            .expect("event queue should contain the just-pushed event");
        if let Some(sink) = self.event_sink.as_ref() {
            sink(event);
        }
        self.subscribers
            .retain(|subscriber| subscriber.send(event.clone()).is_ok());
    }

    fn transition_state(&mut self, pane_id: PaneId, new: PaneState) {
        let old = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .expect("pane must exist before transitioning state");
            if pane.state == new {
                return;
            }
            let old = pane.state.clone();
            pane.state = new.clone();
            old
        };

        self.push_event(
            pane_id,
            PaneEventKind::StateChanged(Box::new(StateChangedEvent { old, new })),
        );
    }

    #[cfg(test)]
    pub(crate) fn insert_surface_for_testing(
        &mut self,
        pane_id: PaneId,
        title: Option<String>,
        surface: Box<dyn SurfaceBackend + Send>,
    ) {
        let surface_size = surface.size();
        self.next_pane_id = self.next_pane_id.max(pane_id.get());
        self.panes.insert(
            pane_id,
            PaneRuntimeState {
                title,
                spawn_config: PaneConfig::command("registered-surface")
                    .with_id(pane_id)
                    .with_size(surface_size),
                state: PaneState::Running,
                interaction_mode: PaneInteractionMode::Embedded,
                input_config: crate::InputConfig::default(),
                stats: PaneStats,
                surface_generation: 0,
                next_seq: 0,
                size_history: vec![ReproSizeEvent::new(0, surface_size)],
                event_log: Vec::new(),
                surface,
                process: None,
                exit: None,
                exit_event_observed: false,
                transcript: Transcript::new(crate::TranscriptConfig::default()),
                mode_overlay: ModeOverlay::default(),
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn feed_surface_for_testing(
        &mut self,
        pane_id: PaneId,
        bytes: &[u8],
    ) -> Result<SurfaceUpdate> {
        let (update, generation) = self.feed_surface_bytes(pane_id, bytes)?;
        if should_emit_surface_event(update) {
            self.push_event(
                pane_id,
                PaneEventKind::SurfaceChanged(SurfaceChangedEvent {
                    generation,
                    dirty_rows: update.dirty_rows,
                    cursor_changed: update.cursor_changed,
                    title_changed: update.title_changed,
                    modes_changed: update.modes_changed,
                    scrollback_changed: update.scrollback_changed,
                }),
            );
        }
        Ok(update)
    }

    #[cfg(test)]
    pub(crate) fn feed_output_for_testing(&mut self, pane_id: PaneId, bytes: Vec<u8>) {
        self.handle_output_frame(pane_id, bytes);
    }

    #[cfg(test)]
    pub(crate) fn scrollback_for_testing(
        &self,
        pane_id: PaneId,
    ) -> Result<crate::ScrollbackSnapshot<'_>> {
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        Ok(pane.surface.scrollback())
    }

    /// Writes raw bytes to a pane's PTY.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found, the pane is surface-only,
    /// the pane has already terminated, or the PTY write fails.
    pub fn write_bytes(&mut self, pane_id: PaneId, bytes: &[u8]) -> Result<()> {
        self.send_raw_bytes(pane_id, bytes, InputKind::Bytes)
    }

    /// Sends encoded input to a pane.
    ///
    /// Uses the [`XtermEncoder`] to translate high-level [`HostInput`] events
    /// (keys, mouse, paste, focus, resize) into the appropriate byte sequences,
    /// respecting the pane's [`InputConfig`](crate::InputConfig) and the
    /// current [`TerminalModes`](crate::TerminalModes) reported by the surface.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found, the pane is surface-only,
    /// the pane has already terminated, encoding fails, or the PTY write
    /// fails.
    pub fn send_input(&mut self, pane_id: PaneId, input: HostInput) -> Result<()> {
        let (bytes, input_kind) = {
            let pane = self
                .panes
                .get(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            let encoder = XtermEncoder::new(pane.input_config);
            let modes = pane.mode_overlay.merge_with(pane.surface.modes());
            let bytes = encoder.encode(&input, &modes)?;
            (bytes, classify_input_kind(&input))
        };

        if bytes.is_empty() {
            return Ok(());
        }

        self.send_raw_bytes(pane_id, &bytes, input_kind)
    }

    /// Sends a manager-owned input transaction to a pane.
    ///
    /// The manager translates operator intent into PTY bytes, including
    /// bracketed paste for multiline text when the child has enabled it,
    /// chunked raw typing fallback otherwise, submit sequencing, transient
    /// write retries, and optional echo verification against the owned surface
    /// and retained scrollback.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found, the pane is surface-only, or
    /// input encoding fails before any transaction bytes are sent. PTY write,
    /// child-exit, and verification failures are reported in the returned
    /// [`InputOutcome`].
    pub fn send_input_transaction(
        &mut self,
        pane_id: PaneId,
        transaction: InputTransaction,
    ) -> Result<InputOutcome> {
        if !self.panes.contains_key(&pane_id) {
            return Err(PaneError::NotFound { pane_id });
        }

        self.poll_pane_frames(pane_id);

        let mut outcome = InputOutcome::default();
        if self.transaction_child_exited(pane_id)? {
            outcome.child_exited = true;
            outcome.errors.push(InputTransactionError::ChildExited);
            return Ok(outcome);
        }

        let steps = self.plan_input_transaction(pane_id, &transaction)?;
        let should_verify_before_submit =
            transaction.verification.timeout().is_some() && steps.iter().any(|step| step.submits);
        let mut verified = false;

        for (index, step) in steps.iter().enumerate() {
            if self.transaction_child_exited(pane_id)? {
                outcome.child_exited = true;
                outcome.errors.push(InputTransactionError::ChildExited);
                return Ok(outcome);
            }

            if step.submits && should_verify_before_submit && !verified {
                self.verify_input_transaction_echo(
                    pane_id,
                    &transaction.verification,
                    &mut outcome,
                )?;
                verified = true;
                if !outcome.echoed {
                    return Ok(outcome);
                }
            }

            if !self.write_input_transaction_step(
                pane_id,
                step,
                transaction.retry.max_transient_retries,
                transaction.retry.retry_delay,
                &mut outcome,
            )? {
                return Ok(outcome);
            }

            self.poll_pane_frames(pane_id);
            if self.transaction_child_exited(pane_id)? {
                outcome.child_exited = true;
                if index + 1 < steps.len() {
                    outcome.errors.push(InputTransactionError::ChildExited);
                    return Ok(outcome);
                }
            }
        }

        if !verified {
            self.verify_input_transaction_echo(pane_id, &transaction.verification, &mut outcome)?;
        }
        Ok(outcome)
    }

    fn plan_input_transaction(
        &self,
        pane_id: PaneId,
        transaction: &InputTransaction,
    ) -> Result<Vec<InputWriteStep>> {
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        if pane.process.is_none() {
            return Err(PaneError::InvalidState {
                expected: "pty-backed pane".into(),
                actual: "surface-only pane".into(),
            });
        }

        let encoder = XtermEncoder::new(pane.input_config);
        let modes = pane.mode_overlay.merge_with(pane.surface.modes());
        match &transaction.intent {
            InputIntent::InsertText(text) => Ok(text_input_steps(
                text,
                false,
                transaction.chunk_size,
                encoder,
                modes,
            )?),
            InputIntent::SubmitText(text) => Ok(text_input_steps(
                text,
                true,
                transaction.chunk_size,
                encoder,
                modes,
            )?),
            InputIntent::KeyChord(key) => {
                let bytes = encoder.encode(&HostInput::Key(key.clone()), &modes)?;
                Ok(vec![InputWriteStep {
                    bytes,
                    input_kind: InputKind::Key,
                    submits: false,
                }])
            }
            InputIntent::Interrupt => {
                let key = control_key('c');
                let bytes = encoder.encode(&HostInput::Key(key), &modes)?;
                Ok(vec![InputWriteStep {
                    bytes,
                    input_kind: InputKind::Key,
                    submits: false,
                }])
            }
            InputIntent::ClearInput => {
                let key = control_key('u');
                let bytes = encoder.encode(&HostInput::Key(key), &modes)?;
                Ok(vec![InputWriteStep {
                    bytes,
                    input_kind: InputKind::Key,
                    submits: false,
                }])
            }
            InputIntent::RawBytes(bytes) => Ok(vec![InputWriteStep {
                bytes: bytes.clone(),
                input_kind: InputKind::Bytes,
                submits: false,
            }]),
        }
    }

    fn write_input_transaction_step(
        &mut self,
        pane_id: PaneId,
        step: &InputWriteStep,
        max_transient_retries: usize,
        retry_delay: Duration,
        outcome: &mut InputOutcome,
    ) -> Result<bool> {
        if step.bytes.is_empty() {
            return Ok(true);
        }

        let write_result = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            let Some(process) = pane.process.as_ref() else {
                return Err(PaneError::InvalidState {
                    expected: "pty-backed pane".into(),
                    actual: "surface-only pane".into(),
                });
            };
            process
                .writer()
                .write_bytes_retrying(&step.bytes, max_transient_retries, retry_delay)
        };

        match write_result {
            Ok(bytes_written) => {
                outcome.bytes_sent += bytes_written;
                outcome.submitted |= step.submits;
                self.push_event(
                    pane_id,
                    PaneEventKind::InputSent(InputSentEvent {
                        input_kind: step.input_kind,
                        bytes_len: bytes_written,
                        recorded: false,
                    }),
                );
                Ok(true)
            }
            Err(failure) => {
                outcome.bytes_sent += failure.bytes_written;
                let (operation, message) = transaction_error_parts(&failure.error);
                outcome.errors.push(InputTransactionError::Write {
                    operation,
                    bytes_attempted: step.bytes.len(),
                    bytes_written: failure.bytes_written,
                    message,
                });
                self.push_event(
                    pane_id,
                    PaneEventKind::Error(ErrorEvent {
                        error: failure.error,
                    }),
                );
                Ok(false)
            }
        }
    }

    fn verify_input_transaction_echo(
        &mut self,
        pane_id: PaneId,
        verification: &InputVerification,
        outcome: &mut InputOutcome,
    ) -> Result<()> {
        let Some(timeout) = verification.timeout() else {
            return Ok(());
        };

        let started = Instant::now();
        loop {
            self.poll_pane_frames(pane_id);

            let text = self.transaction_surface_text(pane_id)?;
            if verification.matches_text(&text) {
                outcome.echoed = true;
                return Ok(());
            }

            if self.transaction_child_exited(pane_id)? {
                outcome.child_exited = true;
                outcome
                    .errors
                    .push(InputTransactionError::VerificationFailed {
                        message: "child exited before echo verification succeeded".into(),
                    });
                return Ok(());
            }

            let elapsed = started.elapsed();
            if elapsed >= timeout {
                outcome.timed_out = true;
                outcome
                    .errors
                    .push(InputTransactionError::VerificationFailed {
                        message: "echo verification timed out".into(),
                    });
                return Ok(());
            }

            let remaining = timeout.saturating_sub(elapsed);
            thread::sleep(DEFAULT_INPUT_TRANSACTION_POLL_INTERVAL.min(remaining));
        }
    }

    fn transaction_child_exited(&self, pane_id: PaneId) -> Result<bool> {
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        Ok(matches!(
            pane.state,
            PaneState::Exited { .. } | PaneState::Killed { .. } | PaneState::Failed { .. }
        ))
    }

    fn transaction_surface_text(&self, pane_id: PaneId) -> Result<String> {
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        Ok(pane_verification_text(pane))
    }

    fn send_raw_bytes(
        &mut self,
        pane_id: PaneId,
        bytes: &[u8],
        input_kind: InputKind,
    ) -> Result<()> {
        let write_result = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            let Some(process) = pane.process.as_ref() else {
                return Err(PaneError::InvalidState {
                    expected: "pty-backed pane".into(),
                    actual: "surface-only pane".into(),
                });
            };
            if matches!(
                pane.state,
                PaneState::Exited { .. } | PaneState::Killed { .. } | PaneState::Failed { .. }
            ) {
                Err(PaneError::Io {
                    operation: IoOperation::Write,
                    message: "pane has already terminated; cannot write input".into(),
                })
            } else {
                process.writer().write_bytes(bytes)
            }
        };

        match write_result {
            Ok(()) => {
                self.push_event(
                    pane_id,
                    PaneEventKind::InputSent(InputSentEvent {
                        input_kind,
                        bytes_len: bytes.len(),
                        recorded: false,
                    }),
                );
                Ok(())
            }
            Err(error) => {
                self.push_event(
                    pane_id,
                    PaneEventKind::Error(ErrorEvent {
                        error: error.clone(),
                    }),
                );
                Err(error)
            }
        }
    }

    /// Returns a read-only snapshot of pane state.
    ///
    /// Polls the pane's PTY for pending frames before returning so the caller
    /// always observes the most up-to-date state.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn snapshot(&mut self, pane_id: PaneId) -> Result<PaneSnapshot<'_>> {
        if !self.panes.contains_key(&pane_id) {
            return Err(PaneError::NotFound { pane_id });
        }
        self.poll_pane_frames(pane_id);
        let pane = self
            .panes
            .get(&pane_id)
            .expect("pane should still exist after polling");
        Ok(pane.snapshot(pane_id))
    }

    /// Returns a range-aware scrollback reader for a pane.
    ///
    /// Polls the pane's PTY for pending frames before returning so the caller
    /// always observes the most up-to-date retained scrollback.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn scrollback(&mut self, pane_id: PaneId) -> Result<ScrollbackReader<'_>> {
        if !self.panes.contains_key(&pane_id) {
            return Err(PaneError::NotFound { pane_id });
        }
        self.poll_pane_frames(pane_id);
        let pane = self
            .panes
            .get(&pane_id)
            .expect("pane should still exist after polling");
        Ok(ScrollbackReader::new(pane.surface.scrollback()))
    }

    /// Returns a range-aware transcript reader for a pane.
    ///
    /// Polls the pane's PTY for pending frames before returning so the caller
    /// always observes the most up-to-date retained transcript buffers.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn transcript(&mut self, pane_id: PaneId) -> Result<TranscriptReader<'_>> {
        if !self.panes.contains_key(&pane_id) {
            return Err(PaneError::NotFound { pane_id });
        }
        self.poll_pane_frames(pane_id);
        let pane = self
            .panes
            .get(&pane_id)
            .expect("pane should still exist after polling");
        Ok(TranscriptReader::new(
            pane.transcript.retained_raw_start_offset(),
            pane.transcript.retained_plain_start_offset(),
            pane.transcript.raw_bytes(),
            pane.transcript.plain_text(),
        ))
    }

    /// Returns the latest pane-scoped event sequence number emitted so far.
    ///
    /// Polls the pane's PTY for pending frames before returning so the caller
    /// can seed attach sessions from the current sequence.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn last_seq(&mut self, pane_id: PaneId) -> Result<u64> {
        if !self.panes.contains_key(&pane_id) {
            return Err(PaneError::NotFound { pane_id });
        }
        self.poll_pane_frames(pane_id);
        let pane = self
            .panes
            .get(&pane_id)
            .expect("pane should still exist after polling");
        Ok(pane.next_seq)
    }

    /// Returns the raw byte transcript for a pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn raw_transcript(&self, pane_id: PaneId) -> Result<&[u8]> {
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        Ok(pane.transcript.raw_bytes())
    }

    /// Returns the plain text transcript for a pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found.
    pub fn plain_transcript(&self, pane_id: PaneId) -> Result<&str> {
        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        Ok(pane.transcript.plain_text())
    }

    /// Subscribes to future pane events through a channel.
    ///
    /// Each call creates a fresh receiver that only observes events emitted
    /// after the subscription is created. Polling via
    /// [`PaneManager::drain_events`] remains available alongside
    /// subscriptions, and callers must keep draining the manager queue if they
    /// want to avoid unbounded growth in the retained pending-event buffer.
    ///
    /// The returned channel is unbounded. A slow or idle subscriber can
    /// therefore accumulate events in memory until it catches up or is
    /// dropped.
    pub fn subscribe(&mut self) -> Receiver<PaneEvent> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        rx
    }

    /// Installs a callback sink that receives every newly emitted pane event.
    ///
    /// The callback mirrors events in addition to the manager's internal
    /// pending-event queue. Callers must keep draining
    /// [`PaneManager::drain_events`] if they want to avoid unbounded growth in
    /// that queue.
    ///
    /// Calling this method again replaces the previously installed sink.
    /// The callback runs synchronously on the event emission path and must not
    /// re-enter the same [`PaneManager`].
    pub fn set_event_sink<F>(&mut self, sink: F)
    where
        F: Fn(&PaneEvent) + Send + Sync + 'static,
    {
        self.event_sink = Some(Arc::new(sink));
    }

    /// Clears any installed callback sink.
    pub fn clear_event_sink(&mut self) {
        self.event_sink = None;
    }

    /// Drains pending events into the provided vector.
    pub fn drain_events(&mut self, out: &mut Vec<PaneEvent>) {
        self.poll_panes();
        out.extend(self.events.drain(..));
    }

    /// Captures a redacted repro dump for the specified pane.
    ///
    /// The dump drains queued PTY frames up to a bounded limit so the
    /// transcript, event log, and final surface reflect the latest observable
    /// state without risking an infinite wait on panes that are still
    /// producing output.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane was not found or if the pane continues to
    /// produce output beyond the configured repro-drain frame limit.
    pub fn dump_repro(&mut self, pane_id: PaneId, options: ReproDumpOptions) -> Result<ReproDump> {
        self.drain_pane_frames_for_repro(pane_id)?;

        let pane = self
            .panes
            .get(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        let spawn_config = redact_spawn_config(&pane.spawn_config, options);

        let raw_transcript = pane
            .transcript
            .records_raw_bytes()
            .then(|| ReproRawTranscript {
                start_offset: pane.transcript.retained_raw_start_offset(),
                bytes: pane.transcript.raw_bytes().to_vec(),
            });

        Ok(ReproDump {
            pane_id,
            spawn_config: spawn_config.clone(),
            backend: pane.surface.metadata(),
            size_history: pane.size_history.clone(),
            events: redact_repro_events(&pane.event_log, &spawn_config.command.program),
            raw_transcript,
            final_surface: pane.merged_surface_snapshot().to_owned_snapshot(),
        })
    }

    fn poll_panes(&mut self) {
        let pane_ids = self.panes.keys().copied().collect::<Vec<_>>();
        for pane_id in pane_ids {
            self.poll_pane_frames(pane_id);
        }
    }

    fn poll_pane_frames(&mut self, pane_id: PaneId) {
        for _ in 0..self.config.max_pty_frames_per_drain {
            let frame = {
                let Some(pane) = self.panes.get_mut(&pane_id) else {
                    return;
                };
                let Some(process) = pane.process.as_mut() else {
                    return;
                };
                process.try_recv()
            };

            let Some(frame) = frame else {
                break;
            };
            self.handle_pty_frame(pane_id, frame);
        }
    }

    fn drain_all_pane_frames(&mut self, pane_id: PaneId) {
        loop {
            let frame = {
                let Some(pane) = self.panes.get_mut(&pane_id) else {
                    return;
                };
                let Some(process) = pane.process.as_mut() else {
                    return;
                };
                process.try_recv()
            };

            let Some(frame) = frame else {
                break;
            };
            self.handle_pty_frame(pane_id, frame);
        }
    }

    fn drain_pane_frames_for_repro(&mut self, pane_id: PaneId) -> Result<()> {
        let limit = self.config.max_pty_frames_per_repro_dump;
        for _ in 0..limit {
            let frame = {
                let pane = self
                    .panes
                    .get_mut(&pane_id)
                    .ok_or(PaneError::NotFound { pane_id })?;
                let Some(process) = pane.process.as_mut() else {
                    return Ok(());
                };
                process.try_recv()
            };

            let Some(frame) = frame else {
                return Ok(());
            };
            self.handle_pty_frame(pane_id, frame);
        }

        let extra_frame = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .ok_or(PaneError::NotFound { pane_id })?;
            let Some(process) = pane.process.as_mut() else {
                return Ok(());
            };
            process.try_recv()
        };

        let Some(frame) = extra_frame else {
            return Ok(());
        };
        self.handle_pty_frame(pane_id, frame);
        Err(PaneError::Surface {
            message: format!(
                "repro dump exceeded the PTY frame drain limit ({limit}) while the pane still had queued output"
            ),
        })
    }

    fn flush_terminal_pane_before_remove(&mut self, pane_id: PaneId) {
        self.drain_all_pane_frames(pane_id);

        // Flush any pending incomplete escape/UTF-8 sequences so they don't
        // disappear from the final plain transcript.
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            if let Some(rotated) = pane.transcript.flush_pending() {
                self.push_event(
                    pane_id,
                    PaneEventKind::TranscriptRotated(TranscriptRotatedEvent {
                        chunks_dropped: rotated.chunks_dropped,
                        bytes_dropped: rotated.bytes_dropped,
                        raw_bytes_dropped: rotated.raw_bytes_dropped,
                        plain_bytes_dropped: rotated.plain_bytes_dropped,
                    }),
                );
            }
        }

        let needs_synthetic_exit = self
            .panes
            .get(&pane_id)
            .map(|pane| {
                matches!(
                    pane.state,
                    PaneState::Killed { .. } | PaneState::Failed { .. }
                ) && !pane.exit_event_observed
            })
            .unwrap_or(false);

        if needs_synthetic_exit {
            self.record_exit_event_observed(pane_id);
            self.push_event(pane_id, PaneEventKind::Exited(ExitedEvent { code: None }));
        }
    }

    fn handle_pty_frame(&mut self, pane_id: PaneId, frame: PtyFrame) {
        match frame {
            PtyFrame::Output { bytes, .. } => self.handle_output_frame(pane_id, bytes),
            PtyFrame::Overflow {
                dropped_frames,
                dropped_bytes,
                ..
            } => self.push_event(
                pane_id,
                PaneEventKind::Overflow(OverflowEvent {
                    dropped_frames,
                    dropped_bytes,
                    queue: OverflowQueue::PtyOutputFrames,
                }),
            ),
            PtyFrame::Error { message, .. } => self.fail_pane(
                pane_id,
                PaneError::Io {
                    operation: IoOperation::Read,
                    message,
                },
            ),
            PtyFrame::Exited { code, .. } => self.handle_exit_frame(pane_id, code),
            PtyFrame::CursorPositionRequest { .. } => {
                // TODO: Surface cursor state is available; synthesize or expose
                // DSR/CPR requests instead of silently dropping them.
            }
        }
    }

    fn handle_output_frame(&mut self, pane_id: PaneId, bytes: Vec<u8>) {
        if let Err(error) = self.record_output_frame(pane_id, bytes) {
            self.fail_pane(pane_id, surface_error(error));
        }
    }

    fn record_output_frame(&mut self, pane_id: PaneId, bytes: Vec<u8>) -> Result<()> {
        // Record the transcript BEFORE feeding the surface so raw bytes are
        // captured regardless of whether the surface accepts the frame.
        let (transcript_offset, rotated) = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .expect("pane should exist while handling output");
            // Update the mode overlay from raw output bytes so modes not tracked
            // by the surface backend (e.g. focus events) are still visible.
            pane.mode_overlay.update_from_output(&bytes);
            let record = pane.transcript.record(&bytes);
            let offset = if pane.transcript.mode() == crate::TranscriptMode::Disabled {
                None
            } else {
                Some(record.offset)
            };
            (offset, record.rotated)
        };

        if let Some(rotated) = rotated {
            self.push_event(
                pane_id,
                PaneEventKind::TranscriptRotated(TranscriptRotatedEvent {
                    chunks_dropped: rotated.chunks_dropped,
                    bytes_dropped: rotated.bytes_dropped,
                    raw_bytes_dropped: rotated.raw_bytes_dropped,
                    plain_bytes_dropped: rotated.plain_bytes_dropped,
                }),
            );
        }

        let contains_escape = bytes.contains(&0x1b);
        match self.feed_surface_bytes(pane_id, &bytes) {
            Ok((update, generation)) => {
                self.push_event(
                    pane_id,
                    PaneEventKind::Output(OutputEvent {
                        bytes_len: bytes.len(),
                        transcript_offset,
                        contains_escape,
                    }),
                );

                if should_emit_surface_event(update) {
                    self.push_event(
                        pane_id,
                        PaneEventKind::SurfaceChanged(SurfaceChangedEvent {
                            generation,
                            dirty_rows: update.dirty_rows,
                            cursor_changed: update.cursor_changed,
                            title_changed: update.title_changed,
                            modes_changed: update.modes_changed,
                            scrollback_changed: update.scrollback_changed,
                        }),
                    );
                }
                Ok(())
            }
            Err(error) => {
                // Still emit an OutputEvent so the event stream has a record
                // of the frame even though the surface rejected it.
                self.push_event(
                    pane_id,
                    PaneEventKind::Output(OutputEvent {
                        bytes_len: bytes.len(),
                        transcript_offset,
                        contains_escape,
                    }),
                );
                Err(error)
            }
        }
    }

    fn handle_exit_frame(&mut self, pane_id: PaneId, code: Option<i32>) {
        self.record_exit_event_observed(pane_id);

        // Flush any pending incomplete escape/UTF-8 sequences so trailing
        // bytes are not lost from the final plain transcript.
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            if let Some(rotated) = pane.transcript.flush_pending() {
                self.push_event(
                    pane_id,
                    PaneEventKind::TranscriptRotated(TranscriptRotatedEvent {
                        chunks_dropped: rotated.chunks_dropped,
                        bytes_dropped: rotated.bytes_dropped,
                        raw_bytes_dropped: rotated.raw_bytes_dropped,
                        plain_bytes_dropped: rotated.plain_bytes_dropped,
                    }),
                );
            }
        }

        self.push_event(pane_id, PaneEventKind::Exited(ExitedEvent { code }));

        let state = self
            .panes
            .get(&pane_id)
            .map(|pane| pane.state.clone())
            .expect("pane should exist while handling exit frames");

        match state {
            PaneState::Starting | PaneState::Running => {
                self.set_exit_if_not_killed_or_failed(pane_id, PaneExit::Exited { code });
                self.transition_state(pane_id, PaneState::Exited { code });
            }
            PaneState::Exited { .. } | PaneState::Killed { .. } | PaneState::Failed { .. } => {}
        }
    }

    fn fail_pane(&mut self, pane_id: PaneId, error: PaneError) {
        // Flush any pending incomplete escape/UTF-8 sequences before pushing
        // the terminal ErrorEvent so transcript-finalization events precede
        // the failure notification in the event stream.
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            if let Some(rotated) = pane.transcript.flush_pending() {
                self.push_event(
                    pane_id,
                    PaneEventKind::TranscriptRotated(TranscriptRotatedEvent {
                        chunks_dropped: rotated.chunks_dropped,
                        bytes_dropped: rotated.bytes_dropped,
                        raw_bytes_dropped: rotated.raw_bytes_dropped,
                        plain_bytes_dropped: rotated.plain_bytes_dropped,
                    }),
                );
            }
        }

        self.push_event(
            pane_id,
            PaneEventKind::Error(ErrorEvent {
                error: error.clone(),
            }),
        );

        let current = self
            .panes
            .get(&pane_id)
            .map(|pane| pane.state.clone())
            .expect("pane should exist while reporting failure");

        if matches!(current, PaneState::Starting | PaneState::Running) {
            self.set_exit_if_not_killed_or_failed(
                pane_id,
                PaneExit::Failed {
                    error: error.clone(),
                },
            );
            self.transition_state(pane_id, PaneState::Failed { error });
        }
    }

    fn record_exit_event_observed(&mut self, pane_id: PaneId) {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .expect("pane should exist while recording exit observation");
        pane.exit_event_observed = true;
    }

    fn record_size_history(&mut self, pane_id: PaneId, size: Size) {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .expect("pane should exist while recording size history");
        let transcript_offset = pane.transcript.next_raw_offset();
        pane.size_history
            .push(ReproSizeEvent::new(transcript_offset, size));
    }

    fn set_exit_if_not_killed_or_failed(&mut self, pane_id: PaneId, exit: PaneExit) {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .expect("pane should exist while recording exit status");
        if matches!(
            pane.exit,
            Some(PaneExit::Killed { .. }) | Some(PaneExit::Failed { .. })
        ) {
            return;
        }
        pane.exit = Some(exit);
    }

    fn feed_surface_bytes(
        &mut self,
        pane_id: PaneId,
        bytes: &[u8],
    ) -> Result<(SurfaceUpdate, u64)> {
        let pane = self
            .panes
            .get_mut(&pane_id)
            .ok_or(PaneError::NotFound { pane_id })?;
        let update = pane.surface.feed(bytes)?;
        pane.surface_generation += 1;
        Ok((update, pane.surface_generation))
    }
}

fn classify_input_kind(input: &HostInput) -> InputKind {
    match input {
        HostInput::Key(_) => InputKind::Key,
        HostInput::Paste(_) => InputKind::Paste,
        HostInput::Mouse(_)
        | HostInput::FocusGained
        | HostInput::FocusLost
        | HostInput::Resize(_)
        | HostInput::Raw(_) => InputKind::Bytes,
    }
}

fn text_input_steps(
    text: &str,
    submit: bool,
    chunk_size: usize,
    encoder: XtermEncoder,
    modes: TerminalModes,
) -> Result<Vec<InputWriteStep>> {
    let mut steps = Vec::new();
    let multiline = text.contains('\n') || text.contains('\r');

    if multiline && modes.bracketed_paste {
        steps.push(InputWriteStep {
            bytes: encoder.encode(&HostInput::Paste(text.to_string()), &modes)?,
            input_kind: InputKind::Paste,
            submits: false,
        });
    } else if !text.is_empty() {
        let mut raw_typing_modes = modes;
        raw_typing_modes.bracketed_paste = false;
        let bytes = encoder.encode(&HostInput::Paste(text.to_string()), &raw_typing_modes)?;
        for chunk in bytes.chunks(chunk_size.max(1)) {
            steps.push(InputWriteStep {
                bytes: chunk.to_vec(),
                input_kind: InputKind::Bytes,
                submits: false,
            });
        }
    }

    if submit {
        steps.push(InputWriteStep {
            bytes: encoder.encode(&HostInput::Key(enter_key()), &modes)?,
            input_kind: InputKind::Key,
            submits: true,
        });
    }

    Ok(steps)
}

fn enter_key() -> KeyInput {
    KeyInput::new(KeyCode::Enter, KeyModifiers::default(), KeyEventKind::Press)
}

fn control_key(c: char) -> KeyInput {
    KeyInput::new(
        KeyCode::Char(c),
        KeyModifiers {
            control: true,
            ..KeyModifiers::default()
        },
        KeyEventKind::Press,
    )
}

fn transaction_error_parts(error: &PaneError) -> (IoOperation, String) {
    match error {
        PaneError::Io { operation, message } => (*operation, message.clone()),
        error => (IoOperation::Write, error.to_string()),
    }
}

fn pane_verification_text(pane: &PaneRuntimeState) -> String {
    let mut text = String::new();

    for line in pane.surface.scrollback().lines {
        text.push_str(line.text.as_ref());
        text.push('\n');
    }

    let snapshot = pane.merged_surface_snapshot();
    for row in &snapshot.rows {
        push_surface_row_text(&mut text, row);
        if !row.wrapped {
            text.push('\n');
        }
    }

    text
}

fn push_surface_row_text(out: &mut String, row: &SurfaceRow<'_>) {
    for cell in &row.cells {
        out.push_str(cell.text.as_ref());
    }
}

fn ensure_running_state(pane: &PaneRuntimeState) -> Result<()> {
    match pane.state {
        PaneState::Running => Ok(()),
        _ => Err(PaneError::InvalidState {
            expected: "Running".into(),
            actual: state_label(&pane.state),
        }),
    }
}

fn state_label(state: &PaneState) -> String {
    match state {
        PaneState::Starting => "Starting",
        PaneState::Running => "Running",
        PaneState::Exited { .. } => "Exited",
        PaneState::Failed { .. } => "Failed",
        PaneState::Killed { .. } => "Killed",
    }
    .into()
}

fn should_emit_surface_event(update: SurfaceUpdate) -> bool {
    !matches!(update.dirty_rows, DirtyRows::None)
        || update.cursor_changed
        || update.title_changed
        || update.modes_changed
        || update.scrollback_changed
}

fn redact_repro_events(events: &[PaneEvent], redacted_program: &str) -> Vec<PaneEvent> {
    events
        .iter()
        .cloned()
        .map(|mut event| {
            if let PaneEventKind::Spawned(spawned) = &mut event.kind {
                spawned.program = redacted_program.to_owned();
            }
            event
        })
        .collect()
}

fn surface_error(error: PaneError) -> PaneError {
    match error {
        PaneError::Surface { .. } => error,
        other => PaneError::Surface {
            message: other.to_string(),
        },
    }
}

fn stabilize_attach_viewport_after_output(
    viewport: TerminalViewport,
    before: TerminalViewportMetrics,
    after: TerminalViewportMetrics,
) -> TerminalViewport {
    if viewport.follow_tail || after.total_rows <= before.total_rows {
        return viewport.clamp(after);
    }

    TerminalViewport::scrolled(
        before
            .effective_scroll_offset
            .saturating_add(after.total_rows - before.total_rows),
    )
    .clamp(after)
}

#[cfg(test)]
mod tests;
