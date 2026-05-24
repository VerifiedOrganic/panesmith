//! Repro dump capture and deterministic replay helpers.

use std::path::PathBuf;

use crate::default_surface::DefaultSurfaceBackend;
use crate::mode_overlay::TerminalModeOverlay;
use crate::{
    OwnedSurfaceSnapshot, PaneConfig, PaneError, PaneEvent, PaneId, ReplayBackendKind, Result,
    ScrollbackConfig, Size, SurfaceBackend, SurfaceBackendMetadata,
};

const REDACTED: &str = "<redacted>";
const REPLAY_SENTINEL_PANE_ID: PaneId = PaneId::new(u64::MAX);

/// Controls how sensitive spawn metadata is redacted in repro dumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReproDumpOptions {
    /// Replace the executable/program with a redacted placeholder.
    pub redact_program: bool,
    /// Replace command arguments with redacted placeholders.
    pub redact_command_args: bool,
    /// Replace the child working directory with a redacted placeholder.
    pub redact_cwd: bool,
    /// Replace environment variable values with redacted placeholders.
    pub redact_env_values: bool,
}

impl Default for ReproDumpOptions {
    fn default() -> Self {
        Self {
            redact_program: true,
            redact_command_args: true,
            redact_cwd: true,
            redact_env_values: true,
        }
    }
}

/// A resize boundary in the raw PTY byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReproSizeEvent {
    /// Absolute raw-stream byte offset where this size took effect.
    pub transcript_offset: u64,
    /// Terminal size applied from this offset onward.
    pub size: Size,
}

impl ReproSizeEvent {
    /// Creates a size-history record.
    pub const fn new(transcript_offset: u64, size: Size) -> Self {
        Self {
            transcript_offset,
            size,
        }
    }
}

/// Retained raw PTY bytes captured in a repro dump.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReproRawTranscript {
    /// Absolute raw-stream offset of the first retained byte.
    pub start_offset: u64,
    /// Retained raw PTY bytes.
    pub bytes: Vec<u8>,
}

/// A serializable repro artifact for debugging rendering issues.
///
/// Dumps retain the manager's in-memory event log and size history for the
/// pane lifetime, so long-running panes can accumulate larger repro artifacts
/// until the host removes or rotates the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReproDump {
    /// Pane identifier the dump came from.
    pub pane_id: PaneId,
    /// Redacted spawn configuration and metadata.
    pub spawn_config: PaneConfig,
    /// Surface backend metadata.
    pub backend: SurfaceBackendMetadata,
    /// Initial size plus subsequent resize boundaries.
    pub size_history: Vec<ReproSizeEvent>,
    /// Full pane event log retained by the manager.
    pub events: Vec<PaneEvent>,
    /// Raw PTY bytes when raw transcript capture was enabled.
    pub raw_transcript: Option<ReproRawTranscript>,
    /// Final visible surface snapshot.
    pub final_surface: OwnedSurfaceSnapshot,
}

impl ReproDump {
    /// Builds a deterministic replay from this dump.
    ///
    /// # Errors
    ///
    /// Returns an error when the dump does not include raw transcript bytes or
    /// references a backend that this crate cannot currently replay.
    pub fn replay(&self) -> Result<PaneReplay> {
        PaneReplay::from_dump(self)
    }
}

/// Deterministic replay state reconstructed from a repro dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneReplay {
    backend: ReplayBackendKind,
    size_history: Vec<ReproSizeEvent>,
    raw_start_offset: u64,
    raw_bytes: Vec<u8>,
}

impl PaneReplay {
    /// Creates a replay from a full repro dump.
    ///
    /// # Errors
    ///
    /// Returns an error when the dump omits raw transcript bytes or the backend
    /// is not replayable in this crate. Replay currently also rejects dumps
    /// whose retained raw transcript starts after byte offset 0, because the
    /// earlier terminal state needed to resume parsing is unavailable.
    pub fn from_dump(dump: &ReproDump) -> Result<Self> {
        let raw = dump
            .raw_transcript
            .as_ref()
            .ok_or_else(|| PaneError::Surface {
                message: "repro dump does not include raw transcript bytes".into(),
            })?;
        if raw.start_offset > 0 {
            return Err(truncated_replay_error(raw.start_offset));
        }
        let backend = dump.backend.replay_kind.ok_or_else(|| PaneError::Surface {
            message: format!(
                "surface backend {} is not replayable in panesmith-core",
                dump.backend.name
            ),
        })?;

        Self::new(
            backend,
            dump.size_history.clone(),
            raw.start_offset,
            raw.bytes.clone(),
        )
    }

    /// Creates a replay from raw transcript bytes and size history.
    ///
    /// The provided transcript is assumed to start at raw offset `0`.
    ///
    /// # Errors
    ///
    /// Returns an error when the size history is empty.
    pub fn from_raw_transcript(
        bytes: impl Into<Vec<u8>>,
        size_history: Vec<ReproSizeEvent>,
        backend: ReplayBackendKind,
    ) -> Result<Self> {
        Self::new(backend, size_history, 0, bytes.into())
    }

    fn new(
        backend: ReplayBackendKind,
        mut size_history: Vec<ReproSizeEvent>,
        raw_start_offset: u64,
        raw_bytes: Vec<u8>,
    ) -> Result<Self> {
        if size_history.is_empty() {
            return Err(PaneError::Surface {
                message: "replay requires at least one size-history entry".into(),
            });
        }

        size_history.sort_by_key(|entry| entry.transcript_offset);

        Ok(Self {
            backend,
            size_history,
            raw_start_offset,
            raw_bytes,
        })
    }

    /// Reconstructs the final surface snapshot for this replay.
    pub fn final_surface(&self) -> Result<OwnedSurfaceSnapshot> {
        if self.raw_start_offset > 0 {
            return Err(truncated_replay_error(self.raw_start_offset));
        }

        let initial_size = current_size_at_offset(&self.size_history, self.raw_start_offset)
            .ok_or_else(|| PaneError::Surface {
                message: format!(
                    "replay size history must include an entry at or before offset {}",
                    self.raw_start_offset
                ),
            })?;
        let mut backend = replay_backend(self.backend, initial_size)?;
        let mut overlay = TerminalModeOverlay::default();

        let mut consumed = 0usize;
        let mut next_resize = 0usize;
        while next_resize < self.size_history.len()
            && self.size_history[next_resize].transcript_offset <= self.raw_start_offset
        {
            next_resize += 1;
        }

        while next_resize < self.size_history.len() {
            let resize = self.size_history[next_resize];
            let relative_end = resize
                .transcript_offset
                .saturating_sub(self.raw_start_offset);
            let end = usize::try_from(relative_end)
                .unwrap_or(usize::MAX)
                .min(self.raw_bytes.len());
            if end > consumed {
                let chunk = &self.raw_bytes[consumed..end];
                overlay.update_from_output(chunk);
                backend.feed(chunk)?;
                consumed = end;
            }
            backend.resize(resize.size)?;
            next_resize += 1;
        }

        if consumed < self.raw_bytes.len() {
            let chunk = &self.raw_bytes[consumed..];
            overlay.update_from_output(chunk);
            backend.feed(chunk)?;
        }

        let mut snapshot = backend.snapshot();
        snapshot.modes = overlay.merge_with(snapshot.modes);
        Ok(snapshot.to_owned_snapshot())
    }
}

pub(crate) fn redact_spawn_config(config: &PaneConfig, options: ReproDumpOptions) -> PaneConfig {
    let mut redacted = config.clone();

    if options.redact_program {
        redacted.command.program = REDACTED.into();
    }

    if options.redact_command_args {
        for arg in &mut redacted.command.args {
            *arg = REDACTED.into();
        }
    }

    if options.redact_cwd {
        redacted.cwd = redacted.cwd.as_ref().map(|_| PathBuf::from(REDACTED));
    }

    if options.redact_env_values {
        for value in redacted.env.values_mut() {
            *value = REDACTED.into();
        }
    }

    redacted
}

fn current_size_at_offset(size_history: &[ReproSizeEvent], offset: u64) -> Option<Size> {
    size_history
        .iter()
        .take_while(|entry| entry.transcript_offset <= offset)
        .last()
        .map(|entry| entry.size)
}

fn truncated_replay_error(start_offset: u64) -> PaneError {
    PaneError::Surface {
        message: format!(
            "repro dump replay requires an untruncated raw transcript; retained bytes start at offset {start_offset}"
        ),
    }
}

fn replay_backend(kind: ReplayBackendKind, size: Size) -> Result<Box<dyn SurfaceBackend + Send>> {
    match kind {
        ReplayBackendKind::DefaultSurfaceVt100 => Ok(Box::new(DefaultSurfaceBackend::new(
            REPLAY_SENTINEL_PANE_ID,
            size,
            ScrollbackConfig::default(),
        )?) as Box<dyn SurfaceBackend + Send>),
    }
}

#[cfg(test)]
mod tests;
