//! vt100 reference surface backend implementation for Panesmith.

use std::fmt;

use panesmith_core::{
    vt100_surface::{Vt100Surface, DEFAULT_VT100_SCROLLBACK_ROWS},
    CursorState, PaneId, ReplayBackendKind, Result, ScrollbackSnapshot, Size, SurfaceBackend,
    SurfaceBackendMetadata, SurfaceSnapshot, SurfaceUpdate, TerminalModes,
};

const VALIDATION_NAME: &str = "vt100 backend";

/// Reference surface backend built on top of the shared vt100 parser engine.
pub struct Vt100Backend {
    inner: Vt100Surface,
}

impl fmt::Debug for Vt100Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vt100Backend")
            .field("pane_id", &self.pane_id())
            .field("size", &self.size())
            .finish()
    }
}

impl Vt100Backend {
    /// Creates a vt100-backed surface for the provided pane and size.
    pub fn new(pane_id: PaneId, size: Size) -> Result<Self> {
        Ok(Self {
            inner: Vt100Surface::new(
                pane_id,
                size,
                DEFAULT_VT100_SCROLLBACK_ROWS,
                VALIDATION_NAME,
            )?,
        })
    }

    /// Returns the pane identifier associated with this backend.
    pub const fn pane_id(&self) -> PaneId {
        self.inner.pane_id()
    }

    /// Returns the surface size tracked by the backend.
    pub fn size(&self) -> Size {
        self.inner.size()
    }
}

impl SurfaceBackend for Vt100Backend {
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

    fn cursor(&self) -> CursorState {
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
            "panesmith-vt100/reference-backend",
            env!("CARGO_PKG_VERSION"),
        )
        .with_replay_kind(ReplayBackendKind::DefaultSurfaceVt100)
    }
}

#[cfg(test)]
mod tests;
