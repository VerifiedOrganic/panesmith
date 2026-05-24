//! Internal default surface backend for the core PaneManager runtime path.
//!
//! This keeps the runtime-default `PaneManagerConfig` on a terminal-aware
//! parser without introducing a dependency on the public `panesmith-vt100`
//! crate (which already depends on `panesmith-core`).

use std::fmt;

use crate::vt100_surface::Vt100Surface;
use crate::{
    PaneId, ReplayBackendKind, Result, ScrollbackConfig, ScrollbackSnapshot, Size, SurfaceBackend,
    SurfaceBackendMetadata, SurfaceSnapshot, SurfaceUpdate, TerminalModes,
};

const VALIDATION_NAME: &str = "default surface backend";

/// Terminal-aware surface backend used by `PaneManagerConfig::default()`.
///
/// This is a lightweight fallback runtime surface. It delegates all vt100
/// parsing, snapshot extraction, scrollback handling, dirty-row detection, and
/// mode tracking to the shared core vt100 surface engine.
pub(crate) struct DefaultSurfaceBackend {
    inner: Vt100Surface,
}

impl fmt::Debug for DefaultSurfaceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultSurfaceBackend")
            .field("pane_id", &self.inner.pane_id())
            .field("size", &self.size())
            .finish()
    }
}

impl DefaultSurfaceBackend {
    pub(crate) fn new(pane_id: PaneId, size: Size, scrollback: ScrollbackConfig) -> Result<Self> {
        Ok(Self {
            inner: Vt100Surface::new(
                pane_id,
                size,
                configured_scrollback_rows(scrollback),
                VALIDATION_NAME,
            )?,
        })
    }

    fn size(&self) -> Size {
        self.inner.size()
    }
}

impl SurfaceBackend for DefaultSurfaceBackend {
    fn size(&self) -> Size {
        self.inner.size()
    }

    fn validate_resize(&self, size: Size) -> Result<()> {
        self.inner.validate_resize(size)
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.inner.resize(size)
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<SurfaceUpdate> {
        self.inner.feed(bytes)
    }

    fn snapshot(&self) -> SurfaceSnapshot<'_> {
        self.inner.snapshot()
    }

    fn cursor(&self) -> crate::CursorState {
        self.inner.cursor()
    }

    fn modes(&self) -> TerminalModes {
        self.inner.modes()
    }

    fn scrollback(&self) -> ScrollbackSnapshot<'_> {
        self.inner.scrollback()
    }

    fn metadata(&self) -> SurfaceBackendMetadata {
        SurfaceBackendMetadata::new(
            "panesmith-core/default-surface-vt100",
            env!("CARGO_PKG_VERSION"),
        )
        .with_replay_kind(ReplayBackendKind::DefaultSurfaceVt100)
    }
}

fn configured_scrollback_rows(scrollback: ScrollbackConfig) -> usize {
    scrollback.line_limit().unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests;
