//! PTY abstractions and the default portable-pty backend.

use std::collections::VecDeque;
use std::env;
#[cfg(unix)]
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::{ChildEnvironmentPolicy, IoOperation, KillConfig, PaneConfig, PaneError, Result, Size};

static NEXT_SYNTHETIC_PROCESS_ID: AtomicU64 = AtomicU64::new(1);
const DEFAULT_PTY_OUTPUT_FRAME_QUEUE_CAPACITY: usize = 128;
const UNIX_KILL_GRACE_POLL_INTERVAL: Duration = Duration::from_millis(10);

type ThreadTask = Box<dyn FnOnce() + Send + 'static>;

/// Runtime PTY backend abstraction.
pub trait PtyBackend {
    /// Concrete process handle returned by this backend.
    type Process: PtyProcess;

    /// Spawns a new PTY-backed child process.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration validation fails, the PTY cannot be
    /// allocated, or the child process cannot be spawned.
    fn spawn(&self, config: &PaneConfig) -> Result<Self::Process>;
}

/// Runtime PTY process abstraction.
pub trait PtyProcess: Send {
    /// Returns the backend-specific process identifier.
    fn id(&self) -> &str;

    /// Returns a synchronized writer for sending raw bytes to the child PTY.
    fn writer(&self) -> PtyWriter;

    /// Attempts to receive the next PTY frame without blocking.
    fn try_recv(&mut self) -> Option<PtyFrame>;

    /// Resizes the PTY.
    ///
    /// # Errors
    ///
    /// Returns an error if the resize operation fails.
    fn resize(&mut self, size: Size) -> Result<()>;

    /// Terminates the child process.
    ///
    /// # Errors
    ///
    /// Returns an error if the kill request fails.
    fn kill(&mut self) -> Result<()>;
}

/// Raw PTY frames emitted before any higher-level parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyFrame {
    /// Raw output bytes emitted by the child.
    Output {
        /// Monotonic frame sequence number for this process.
        seq: u64,
        /// Raw PTY bytes.
        bytes: Vec<u8>,
        /// Capture timestamp.
        at: Instant,
    },
    /// A child request for the current cursor position.
    ///
    /// Reserved for a future output-splitting pass that detects CPR queries
    /// before surface parsing.
    CursorPositionRequest {
        /// Monotonic frame sequence number for this process.
        seq: u64,
        /// Capture timestamp.
        at: Instant,
    },
    /// Child exit notification.
    Exited {
        /// Monotonic frame sequence number for this process.
        seq: u64,
        /// Exit code, if the backend can determine one.
        code: Option<i32>,
        /// Capture timestamp.
        at: Instant,
    },
    /// Queue overflow notification with structured loss metadata.
    Overflow {
        /// Monotonic frame sequence number for this process.
        seq: u64,
        /// Number of dropped output frames.
        dropped_frames: u64,
        /// Number of dropped bytes.
        dropped_bytes: u64,
        /// Capture timestamp.
        at: Instant,
    },
    /// Backend or I/O error notification.
    Error {
        /// Monotonic frame sequence number for this process.
        seq: u64,
        /// Human-readable error description.
        message: String,
        /// Capture timestamp.
        at: Instant,
    },
}

/// Cumulative overflow statistics for a PTY process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OverflowStats {
    /// Number of dropped output frames.
    pub dropped_frames: u64,
    /// Number of dropped output bytes.
    pub dropped_bytes: u64,
}

/// Cloneable synchronized writer for PTY stdin.
#[derive(Clone)]
pub struct PtyWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl std::fmt::Debug for PtyWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyWriter").finish_non_exhaustive()
    }
}

impl PtyWriter {
    pub(crate) fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(writer)),
        }
    }

    /// Writes all bytes to the child PTY.
    ///
    /// # Errors
    ///
    /// Returns an error if the writer lock is poisoned or the write fails.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.lock(IoOperation::Write)?;
        writer
            .write_all(bytes)
            .map_err(|error| io_failure(IoOperation::Write, error))?;
        writer
            .flush()
            .map_err(|error| io_failure(IoOperation::Flush, error))?;
        Ok(())
    }

    pub(crate) fn write_bytes_retrying(
        &self,
        bytes: &[u8],
        max_transient_retries: usize,
        retry_delay: Duration,
    ) -> std::result::Result<usize, PtyWriteFailure> {
        let mut writer = self
            .lock(IoOperation::Write)
            .map_err(|error| PtyWriteFailure {
                bytes_written: 0,
                error,
            })?;
        let mut bytes_written = 0;
        let mut retries = 0;

        while bytes_written < bytes.len() {
            match writer.write(&bytes[bytes_written..]) {
                Ok(0) => {
                    return Err(PtyWriteFailure {
                        bytes_written,
                        error: PaneError::Io {
                            operation: IoOperation::Write,
                            message: "write returned zero bytes".into(),
                        },
                    });
                }
                Ok(written) => {
                    bytes_written += written;
                    retries = 0;
                }
                Err(error) if is_transient_io(error.kind()) && retries < max_transient_retries => {
                    retries += 1;
                    thread::sleep(retry_delay);
                }
                Err(error) => {
                    return Err(PtyWriteFailure {
                        bytes_written,
                        error: io_failure(IoOperation::Write, error),
                    });
                }
            }
        }

        retry_flush(
            &mut **writer,
            bytes_written,
            max_transient_retries,
            retry_delay,
        )?;
        Ok(bytes_written)
    }

    /// Flushes the child PTY writer.
    ///
    /// # Errors
    ///
    /// Returns an error if the writer lock is poisoned or the flush fails.
    pub fn flush(&self) -> Result<()> {
        let mut writer = self.lock(IoOperation::Flush)?;
        writer
            .flush()
            .map_err(|error| io_failure(IoOperation::Flush, error))
    }

    fn lock(
        &self,
        operation: IoOperation,
    ) -> Result<std::sync::MutexGuard<'_, Box<dyn Write + Send>>> {
        self.inner.lock().map_err(|_| PaneError::Io {
            operation,
            message: "pty writer lock poisoned".into(),
        })
    }
}

pub(crate) struct PtyWriteFailure {
    pub(crate) bytes_written: usize,
    pub(crate) error: PaneError,
}

/// Default runtime PTY backend built on `portable-pty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PortablePtyBackend;

impl PtyBackend for PortablePtyBackend {
    type Process = PortablePtyProcess;

    fn spawn(&self, config: &PaneConfig) -> Result<Self::Process> {
        config.validate()?;

        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(to_pty_size(config.size))
            .map_err(|error| PaneError::Spawn {
                message: error.to_string(),
            })?;

        let command = build_command(config);

        let writer = pty_pair
            .master
            .take_writer()
            .map(PtyWriter::new)
            .map_err(|error| PaneError::Spawn {
                message: error.to_string(),
            })?;
        let reader = pty_pair
            .master
            .try_clone_reader()
            .map_err(|error| PaneError::Spawn {
                message: error.to_string(),
            })?;
        let child = pty_pair
            .slave
            .spawn_command(command)
            .map_err(|error| PaneError::Spawn {
                message: error.to_string(),
            })?;
        let child_pid = child.process_id();
        let killer = child.clone_killer();
        let process_id = make_process_id(child_pid);
        let child_exited = Arc::new(AtomicBool::new(false));
        let queue_capacity = if config.output_queue_capacity == 0 {
            DEFAULT_PTY_OUTPUT_FRAME_QUEUE_CAPACITY
        } else {
            config.output_queue_capacity
        };
        let frames = Arc::new(PtyFrameQueue::new(queue_capacity));
        let reader_thread = spawn_reader_thread(
            process_id.clone(),
            reader,
            child,
            Arc::clone(&frames),
            Arc::clone(&child_exited),
        )?;

        Ok(PortablePtyProcess {
            process_id,
            master: pty_pair.master,
            writer,
            killer,
            child_pid,
            kill_config: config.kill,
            child_exited,
            frames,
            reader_thread: Some(reader_thread),
        })
    }
}

fn build_command(config: &PaneConfig) -> CommandBuilder {
    #[cfg(unix)]
    if should_wrap_command_env(config) {
        return build_unix_env_wrapper_command(config);
    }

    let mut command = CommandBuilder::new(config.program());
    for arg in config.args() {
        command.arg(arg);
    }
    if let Some(cwd) = &config.cwd {
        command.cwd(cwd);
    }
    apply_child_environment(&mut command, config);
    command
}

fn apply_child_environment(command: &mut CommandBuilder, config: &PaneConfig) {
    match &config.env_policy {
        ChildEnvironmentPolicy::Inherit => {}
        ChildEnvironmentPolicy::Clear => command.env_clear(),
        ChildEnvironmentPolicy::Allowlist(keys) => {
            command.env_clear();
            for key in keys {
                if let Some(value) = env::var_os(key.as_str()) {
                    command.env(key.as_str(), value);
                }
            }
        }
    }

    if let Some(term) = &config.term_fallback {
        if command.get_env("TERM").is_none() {
            command.env("TERM", term);
        }
    }

    for (key, value) in &config.env {
        command.env(key, value);
    }
}

#[cfg(unix)]
fn should_wrap_command_env(config: &PaneConfig) -> bool {
    !matches!(config.env_policy, ChildEnvironmentPolicy::Inherit)
}

#[cfg(unix)]
fn build_unix_env_wrapper_command(config: &PaneConfig) -> CommandBuilder {
    let mut command = CommandBuilder::new("/usr/bin/env");
    command.arg("-i");

    for (key, value) in effective_child_environment(config) {
        command.arg(env_assignment(&key, &value));
    }

    command.arg(config.program());
    for arg in config.args() {
        command.arg(arg);
    }

    if let Some(cwd) = &config.cwd {
        command.cwd(cwd);
    }

    command.env_clear();
    command
}

#[cfg(unix)]
fn effective_child_environment(
    config: &PaneConfig,
) -> std::collections::BTreeMap<OsString, OsString> {
    let mut envs = std::collections::BTreeMap::new();

    match &config.env_policy {
        ChildEnvironmentPolicy::Inherit => {
            envs.extend(env::vars_os());
        }
        ChildEnvironmentPolicy::Clear => {}
        ChildEnvironmentPolicy::Allowlist(keys) => {
            for key in keys {
                if let Some(value) = env::var_os(key.as_str()) {
                    envs.insert(OsString::from(key), value);
                }
            }
        }
    }

    if let Some(term) = &config.term_fallback {
        envs.entry(OsString::from("TERM"))
            .or_insert_with(|| OsString::from(term));
    }

    for (key, value) in &config.env {
        envs.insert(OsString::from(key), OsString::from(value));
    }

    envs
}

#[cfg(unix)]
fn env_assignment(key: &OsStr, value: &OsStr) -> OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let mut bytes = key.as_bytes().to_vec();
    bytes.push(b'=');
    bytes.extend(value.as_bytes());
    OsString::from_vec(bytes)
}

/// Concrete PTY process for [`PortablePtyBackend`].
pub struct PortablePtyProcess {
    process_id: String,
    master: Box<dyn MasterPty + Send>,
    writer: PtyWriter,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_pid: Option<u32>,
    kill_config: KillConfig,
    child_exited: Arc<AtomicBool>,
    frames: Arc<PtyFrameQueue>,
    reader_thread: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for PortablePtyProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortablePtyProcess")
            .field("process_id", &self.process_id)
            .finish_non_exhaustive()
    }
}

impl PtyProcess for PortablePtyProcess {
    fn id(&self) -> &str {
        &self.process_id
    }

    fn writer(&self) -> PtyWriter {
        self.writer.clone()
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        self.frames.try_recv()
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        Size::try_new(size.rows, size.cols)?;
        self.master
            .resize(to_pty_size(size))
            .map_err(|error| PaneError::Io {
                operation: IoOperation::Resize,
                message: error.to_string(),
            })
    }

    fn kill(&mut self) -> Result<()> {
        self.request_termination()?;
        self.join_reader_thread()
    }
}

impl Drop for PortablePtyProcess {
    fn drop(&mut self) {
        let _ = self.request_termination();
        let _ = self.join_reader_thread();
    }
}

impl PortablePtyProcess {
    /// Returns cumulative overflow statistics for this process.
    pub fn overflow_stats(&self) -> OverflowStats {
        self.frames.overflow_stats()
    }

    fn request_termination(&mut self) -> Result<()> {
        if self.reader_thread.is_none() {
            return Ok(());
        }

        terminate_child(
            &mut *self.killer,
            self.child_pid,
            self.kill_config,
            self.child_exited.as_ref(),
        )
    }

    fn join_reader_thread(&mut self) -> Result<()> {
        let Some(reader_thread) = self.reader_thread.take() else {
            return Ok(());
        };

        reader_thread.join().map_err(|_| PaneError::Io {
            operation: IoOperation::Kill,
            message: "pty reader thread panicked while waiting for process exit".into(),
        })
    }
}

struct PtyFrameQueue {
    state: Mutex<PtyFrameQueueState>,
}

struct PtyFrameQueueState {
    frames: VecDeque<QueuedPtyFrame>,
    output_capacity: usize,
    queued_output_frames: usize,
    overflow_summary_queued: bool,
    cumulative_dropped_frames: u64,
    cumulative_dropped_bytes: u64,
}

enum QueuedPtyFrame {
    Frame(PtyFrame),
    OverflowSummary {
        seq: u64,
        at: Instant,
        dropped_frames: u64,
        dropped_bytes: u64,
    },
}

impl PtyFrameQueue {
    fn new(output_capacity: usize) -> Self {
        Self {
            state: Mutex::new(PtyFrameQueueState {
                frames: VecDeque::new(),
                output_capacity: output_capacity.max(1),
                queued_output_frames: 0,
                overflow_summary_queued: false,
                cumulative_dropped_frames: 0,
                cumulative_dropped_bytes: 0,
            }),
        }
    }

    fn overflow_stats(&self) -> OverflowStats {
        let state = self.lock_state();
        OverflowStats {
            dropped_frames: state.cumulative_dropped_frames,
            dropped_bytes: state.cumulative_dropped_bytes,
        }
    }

    fn push(&self, frame: PtyFrame) {
        self.lock_state().push(frame);
    }

    fn try_recv(&self) -> Option<PtyFrame> {
        self.lock_state().try_recv()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, PtyFrameQueueState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                static WARNED: AtomicBool = AtomicBool::new(false);
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!("panesmith: pty frame queue mutex poisoned, recovering");
                }
                poisoned.into_inner()
            }
        }
    }
}

impl PtyFrameQueueState {
    fn push(&mut self, frame: PtyFrame) {
        match frame {
            PtyFrame::Output { seq, bytes, at } => self.push_output(seq, bytes, at),
            frame => self.frames.push_back(QueuedPtyFrame::Frame(frame)),
        }
    }

    fn push_output(&mut self, seq: u64, bytes: Vec<u8>, at: Instant) {
        if self.overflow_summary_queued || self.queued_output_frames >= self.output_capacity {
            self.record_output_overflow(seq, bytes.len() as u64, at);
            return;
        }

        self.frames
            .push_back(QueuedPtyFrame::Frame(PtyFrame::Output { seq, bytes, at }));
        self.queued_output_frames += 1;
    }

    fn record_output_overflow(&mut self, seq: u64, dropped_bytes: u64, at: Instant) {
        self.cumulative_dropped_frames += 1;
        self.cumulative_dropped_bytes += dropped_bytes;
        // When an OverflowSummary already exists in the queue, we coalesce
        // additional drops into it, retaining the original seq/at (which
        // represent the first dropped frame's position).
        for frame in &mut self.frames {
            if let QueuedPtyFrame::OverflowSummary {
                dropped_frames,
                dropped_bytes: total_dropped_bytes,
                ..
            } = frame
            {
                *dropped_frames += 1;
                *total_dropped_bytes += dropped_bytes;
                return;
            }
        }

        self.frames.push_back(QueuedPtyFrame::OverflowSummary {
            seq,
            at,
            dropped_frames: 1,
            dropped_bytes,
        });
        self.overflow_summary_queued = true;
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        match self.frames.pop_front()? {
            QueuedPtyFrame::Frame(frame) => {
                if matches!(frame, PtyFrame::Output { .. }) {
                    self.queued_output_frames = self.queued_output_frames.saturating_sub(1);
                }
                Some(frame)
            }
            QueuedPtyFrame::OverflowSummary {
                seq,
                at,
                dropped_frames,
                dropped_bytes,
            } => {
                self.overflow_summary_queued = false;
                Some(PtyFrame::Overflow {
                    seq,
                    dropped_frames,
                    dropped_bytes,
                    at,
                })
            }
        }
    }
}

struct ReaderThreadParts {
    reader: Box<dyn Read + Send>,
    child: Box<dyn Child + Send>,
    frames: Arc<PtyFrameQueue>,
    child_exited: Arc<AtomicBool>,
}

trait ReaderThreadSpawnCleanup {
    fn cleanup_after_reader_thread_spawn_failure(&mut self) -> Vec<String>;
}

impl ReaderThreadSpawnCleanup for Box<dyn Child + Send> {
    fn cleanup_after_reader_thread_spawn_failure(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        let mut killer = self.clone_killer();
        let child_exited = AtomicBool::new(false);
        let child_pid = self.process_id();
        if let Err(error) = terminate_child(
            &mut *killer,
            child_pid,
            KillConfig::new(Duration::ZERO, true),
            &child_exited,
        ) {
            failures.push(format!(
                "failed to kill spawned child after reader-thread spawn failure: {error}"
            ));
        }
        if let Err(error) = self.wait() {
            failures.push(format!(
                "failed to reap spawned child after reader-thread spawn failure: {error}"
            ));
        }
        failures
    }
}

fn spawn_reader_thread(
    process_id: String,
    reader: Box<dyn Read + Send>,
    child: Box<dyn Child + Send>,
    frames: Arc<PtyFrameQueue>,
    child_exited: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>> {
    let thread_name = format!("panesmith-pty-{process_id}");
    let parts = ReaderThreadParts {
        reader,
        child,
        frames,
        child_exited,
    };
    spawn_named_thread_with_state(
        thread_name,
        parts,
        run_reader_thread,
        |parts, error| reader_thread_spawn_error(error, &mut parts.child),
        |builder, task| builder.spawn(task),
    )
}

fn run_reader_thread(parts: ReaderThreadParts) {
    let ReaderThreadParts {
        mut reader,
        mut child,
        frames,
        child_exited,
    } = parts;
    let mut seq = 0_u64;
    let mut buffer = [0_u8; 4096];
    let mut pending_status = None;

    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                seq += 1;
                frames.push(PtyFrame::Output {
                    seq,
                    bytes: buffer[..read].to_vec(),
                    at: Instant::now(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => match child.try_wait() {
                Ok(Some(status)) => {
                    pending_status = Some(status);
                    break;
                }
                Ok(None) => {
                    seq += 1;
                    frames.push(PtyFrame::Error {
                        seq,
                        message: error.to_string(),
                        at: Instant::now(),
                    });
                    break;
                }
                Err(wait_error) => {
                    seq += 1;
                    frames.push(PtyFrame::Error {
                        seq,
                        message: format!(
                            "pty reader failed: {error}; child status poll failed: {wait_error}"
                        ),
                        at: Instant::now(),
                    });
                    break;
                }
            },
        }
    }

    let exit_status = match pending_status {
        Some(status) => Ok(status),
        None => child.wait(),
    };

    match exit_status {
        Ok(status) => {
            child_exited.store(true, Ordering::Release);
            let code = exit_code(status);
            seq += 1;
            frames.push(PtyFrame::Exited {
                seq,
                code,
                at: Instant::now(),
            });
        }
        Err(error) => {
            seq += 1;
            frames.push(PtyFrame::Error {
                seq,
                message: format!("child wait failed: {error}"),
                at: Instant::now(),
            });
        }
    }
}

fn terminate_child(
    killer: &mut (dyn ChildKiller + Send + Sync),
    child_pid: Option<u32>,
    kill_config: KillConfig,
    child_exited: &AtomicBool,
) -> Result<()> {
    #[cfg(unix)]
    {
        terminate_child_unix(killer, child_pid, kill_config, child_exited)
    }

    #[cfg(not(unix))]
    {
        let _ = child_pid;
        let _ = kill_config;
        let _ = child_exited;
        killer.kill().map_err(|error| PaneError::Io {
            operation: IoOperation::Kill,
            message: error.to_string(),
        })
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnixKillTarget {
    Process(libc::pid_t),
    ProcessGroup(libc::pid_t),
}

#[cfg(unix)]
impl UnixKillTarget {
    fn signal_pid(self) -> libc::pid_t {
        match self {
            Self::Process(pid) => pid,
            Self::ProcessGroup(pgid) => -pgid,
        }
    }
}

#[cfg(unix)]
fn terminate_child_unix(
    killer: &mut (dyn ChildKiller + Send + Sync),
    child_pid: Option<u32>,
    kill_config: KillConfig,
    child_exited: &AtomicBool,
) -> Result<()> {
    let Some(child_pid) = child_pid else {
        return killer.kill().map_err(|error| PaneError::Io {
            operation: IoOperation::Kill,
            message: error.to_string(),
        });
    };

    let child_pid = unix_pid(child_pid)?;
    let Some(target) =
        resolve_initial_unix_kill_target(child_pid, kill_config.kill_descendants, child_exited)?
    else {
        return Ok(());
    };

    if !send_unix_signal(target, libc::SIGTERM)? {
        return Ok(());
    }
    if wait_for_unix_kill_grace(target, child_exited, kill_config.term_grace)? {
        return Ok(());
    }
    let _ = send_unix_signal(target, libc::SIGKILL)?;
    Ok(())
}

#[cfg(unix)]
fn unix_pid(pid: u32) -> Result<libc::pid_t> {
    libc::pid_t::try_from(pid).map_err(|_| PaneError::Io {
        operation: IoOperation::Kill,
        message: format!("child pid {pid} exceeds the platform pid_t range"),
    })
}

#[cfg(unix)]
fn send_unix_signal(target: UnixKillTarget, signal: libc::c_int) -> Result<bool> {
    // SAFETY: `signal_pid` returns either a child pid or the negated process
    // group id prepared for `kill(2)`. `signal` is supplied by libc constants.
    let result = unsafe { libc::kill(target.signal_pid(), signal) };
    if result == 0 {
        return Ok(true);
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(io_failure(IoOperation::Kill, error))
    }
}

#[cfg(unix)]
fn resolve_initial_unix_kill_target(
    child_pid: libc::pid_t,
    kill_descendants: bool,
    child_exited: &AtomicBool,
) -> Result<Option<UnixKillTarget>> {
    let process_target = UnixKillTarget::Process(child_pid);
    if !kill_descendants {
        if child_exited.load(Ordering::Acquire) {
            return Ok(None);
        }
        return Ok(probe_unix_target_exists(process_target)?.then_some(process_target));
    }

    let process_group_target = UnixKillTarget::ProcessGroup(child_pid);
    if probe_unix_target_exists(process_group_target)? {
        return Ok(Some(process_group_target));
    }
    if should_skip_unix_sigkill(process_target, child_exited)? {
        return Ok(None);
    }

    Ok(Some(process_target))
}

#[cfg(unix)]
fn wait_for_unix_kill_grace(
    target: UnixKillTarget,
    child_exited: &AtomicBool,
    grace: Duration,
) -> Result<bool> {
    if should_skip_unix_sigkill(target, child_exited)? {
        return Ok(true);
    }
    if grace.is_zero() {
        return Ok(false);
    }

    let deadline = Instant::now() + grace;
    loop {
        if should_skip_unix_sigkill(target, child_exited)? {
            return Ok(true);
        }

        let now = Instant::now();
        if now >= deadline {
            break;
        }

        thread::sleep((deadline - now).min(UNIX_KILL_GRACE_POLL_INTERVAL));
    }

    should_skip_unix_sigkill(target, child_exited)
}

#[cfg(unix)]
fn should_skip_unix_sigkill(target: UnixKillTarget, child_exited: &AtomicBool) -> Result<bool> {
    if matches!(target, UnixKillTarget::Process(_)) && child_exited.load(Ordering::Acquire) {
        return Ok(true);
    }

    Ok(!probe_unix_target_exists(target)?)
}

#[cfg(unix)]
fn probe_unix_target_exists(target: UnixKillTarget) -> Result<bool> {
    // SAFETY: `kill(pid, 0)` performs permission/liveness probing without
    // sending a signal. `signal_pid` returns a pid value formatted for
    // `kill(2)`.
    let result = unsafe { libc::kill(target.signal_pid(), 0) };
    if result == 0 {
        return Ok(true);
    }

    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::ESRCH => Ok(false),
        Some(code) if code == libc::EPERM => Ok(true),
        _ => Err(io_failure(IoOperation::Kill, error)),
    }
}

fn to_pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn make_process_id(process_id: Option<u32>) -> String {
    process_id.map(|id| id.to_string()).unwrap_or_else(|| {
        NEXT_SYNTHETIC_PROCESS_ID
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    })
}

fn io_failure(operation: IoOperation, error: std::io::Error) -> PaneError {
    PaneError::Io {
        operation,
        message: error.to_string(),
    }
}

fn retry_flush(
    writer: &mut dyn Write,
    bytes_written: usize,
    max_transient_retries: usize,
    retry_delay: Duration,
) -> std::result::Result<(), PtyWriteFailure> {
    let mut retries = 0;

    loop {
        match writer.flush() {
            Ok(()) => return Ok(()),
            Err(error) if is_transient_io(error.kind()) && retries < max_transient_retries => {
                retries += 1;
                thread::sleep(retry_delay);
            }
            Err(error) => {
                return Err(PtyWriteFailure {
                    bytes_written,
                    error: io_failure(IoOperation::Flush, error),
                });
            }
        }
    }
}

fn is_transient_io(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted)
}

fn exit_code(status: portable_pty::ExitStatus) -> Option<i32> {
    if status.signal().is_some() {
        None
    } else {
        Some(status.exit_code() as i32)
    }
}

fn spawn_named_thread_with_state<T, F, C, S>(
    name: String,
    state: T,
    task: F,
    cleanup: C,
    spawner: S,
) -> Result<thread::JoinHandle<()>>
where
    T: Send + 'static,
    F: FnOnce(T) + Send + 'static,
    C: FnOnce(&mut T, std::io::Error) -> PaneError,
    S: FnOnce(thread::Builder, ThreadTask) -> std::io::Result<thread::JoinHandle<()>>,
{
    let state = Arc::new(Mutex::new(Some(state)));
    let task_state = Arc::clone(&state);
    let task: ThreadTask = Box::new(move || {
        if let Some(state) = take_thread_state(&task_state) {
            task(state);
        }
    });

    let builder = thread::Builder::new().name(name);
    match spawner(builder, task) {
        Ok(handle) => Ok(handle),
        Err(error) => {
            let mut state = take_thread_state(&state)
                .expect("reader thread state should still be available when spawn fails");
            Err(cleanup(&mut state, error))
        }
    }
}

fn take_thread_state<T>(state: &Arc<Mutex<Option<T>>>) -> Option<T> {
    match state.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => {
            eprintln!("panesmith: thread state mutex poisoned, recovering");
            poisoned.into_inner().take()
        }
    }
}

fn reader_thread_spawn_error<C>(error: std::io::Error, child: &mut C) -> PaneError
where
    C: ReaderThreadSpawnCleanup,
{
    let mut message = format!("failed to spawn PTY reader thread: {error}");
    for failure in child.cleanup_after_reader_thread_spawn_failure() {
        message.push_str("; ");
        message.push_str(&failure);
    }
    PaneError::Spawn { message }
}

/// Placeholder PTY handle exposed to keep the crate boundary visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PtyHandle;

#[cfg(test)]
mod tests;
