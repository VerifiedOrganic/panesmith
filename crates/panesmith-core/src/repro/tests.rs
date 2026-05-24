use super::*;
use crate::{MouseMode, PaneConfig, PaneEventKind, Size, SurfaceSnapshot, TerminalModes};

#[cfg(feature = "serde")]
use std::collections::VecDeque;
#[cfg(feature = "serde")]
use std::time::Instant;

#[cfg(feature = "serde")]
use crate::{PaneManager, PaneManagerConfig, PtyFrame, PtyProcess, Result as PaneResult};

#[cfg(feature = "serde")]
#[derive(Debug)]
struct ReplayTestPty {
    frames: VecDeque<PtyFrame>,
}

#[cfg(feature = "serde")]
impl PtyProcess for ReplayTestPty {
    fn id(&self) -> &str {
        "replay-test-pty"
    }

    fn writer(&self) -> crate::PtyWriter {
        panic!("writer should not be requested in replay serde tests")
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        self.frames.pop_front()
    }

    fn resize(&mut self, _size: Size) -> PaneResult<()> {
        Ok(())
    }

    fn kill(&mut self) -> PaneResult<()> {
        Ok(())
    }
}

fn sample_dump(
    raw_transcript: Option<ReproRawTranscript>,
    backend: SurfaceBackendMetadata,
) -> ReproDump {
    ReproDump {
        pane_id: PaneId::new(7),
        spawn_config: PaneConfig::command("fixture").with_size(Size::new(24, 80)),
        backend,
        size_history: vec![ReproSizeEvent::new(0, Size::new(24, 80))],
        events: vec![crate::PaneEvent::placeholder(PaneId::new(7))],
        raw_transcript,
        final_surface: SurfaceSnapshot::blank(Size::new(24, 80)).to_owned_snapshot(),
    }
}

#[test]
fn replay_from_dump_requires_raw_transcript() {
    let dump = sample_dump(
        None,
        SurfaceBackendMetadata::new("test-backend", "0.1.0")
            .with_replay_kind(ReplayBackendKind::DefaultSurfaceVt100),
    );

    let err = dump
        .replay()
        .expect_err("replay should fail without raw transcript bytes");
    assert!(matches!(
        err,
        PaneError::Surface { message } if message == "repro dump does not include raw transcript bytes"
    ));
}

#[test]
fn replay_from_dump_requires_replayable_backend() {
    let dump = sample_dump(
        Some(ReproRawTranscript {
            start_offset: 0,
            bytes: b"hello".to_vec(),
        }),
        SurfaceBackendMetadata::new("custom-backend", "0.1.0"),
    );

    let err = dump
        .replay()
        .expect_err("replay should fail without a replay selector");
    assert!(matches!(
        err,
        PaneError::Surface { message } if message.contains("custom-backend")
    ));
}

#[test]
fn replay_from_dump_rejects_truncated_raw_transcript() {
    let dump = sample_dump(
        Some(ReproRawTranscript {
            start_offset: 3,
            bytes: b"hello".to_vec(),
        }),
        SurfaceBackendMetadata::new("test-backend", "0.1.0")
            .with_replay_kind(ReplayBackendKind::DefaultSurfaceVt100),
    );

    let err = dump
        .replay()
        .expect_err("replay should reject truncated raw transcripts");
    assert!(matches!(
        err,
        PaneError::Surface { message }
            if message.contains("untruncated raw transcript")
                && message.contains("offset 3")
    ));
}

#[test]
fn replay_from_raw_transcript_requires_non_empty_size_history() {
    let err = PaneReplay::from_raw_transcript(
        b"hello".to_vec(),
        Vec::new(),
        ReplayBackendKind::DefaultSurfaceVt100,
    )
    .expect_err("replay requires at least one size entry");
    assert!(matches!(
        err,
        PaneError::Surface { message } if message == "replay requires at least one size-history entry"
    ));
}

#[test]
fn sample_dump_helper_keeps_event_shape_valid() {
    let dump = sample_dump(
        Some(ReproRawTranscript {
            start_offset: 0,
            bytes: Vec::new(),
        }),
        SurfaceBackendMetadata::new("test-backend", "0.1.0")
            .with_replay_kind(ReplayBackendKind::DefaultSurfaceVt100),
    );
    assert!(matches!(
        dump.events[0].kind,
        PaneEventKind::StateChanged(_)
    ));
}

#[test]
fn replay_final_surface_merges_overlay_tracked_modes() {
    let replay = PaneReplay::from_raw_transcript(
        b"\x1b[?1004h\x1b[?1006h".to_vec(),
        vec![ReproSizeEvent::new(0, Size::new(24, 80))],
        ReplayBackendKind::DefaultSurfaceVt100,
    )
    .expect("replay should build");

    let surface = replay.final_surface().expect("replay should succeed");
    assert_eq!(
        surface.modes,
        TerminalModes {
            focus_events: true,
            mouse: MouseMode::Sgr,
            ..TerminalModes::default()
        }
    );
}

#[cfg(feature = "serde")]
#[test]
fn serde_repro_dump_roundtrip_preserves_replay_surface() {
    let frames = vec![
        PtyFrame::Output {
            seq: 1,
            bytes: b"\x1b[?1004h\x1b[?1006hhello".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Exited {
            seq: 2,
            code: Some(0),
            at: Instant::now(),
        },
    ];
    let mut manager = PaneManager::new(PaneManagerConfig::default().with_pty_spawner(
        move |_config| {
            Ok(Box::new(ReplayTestPty {
                frames: VecDeque::from(frames.clone()),
            }) as Box<dyn PtyProcess>)
        },
    ));
    let pane_id = manager
        .spawn(
            PaneConfig::command("/tmp/fake-fixture")
                .with_transcript(crate::TranscriptConfig::new(crate::TranscriptMode::Both)),
        )
        .expect("spawn should succeed");

    let dump = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect("dump_repro should succeed");
    let json = serde_json::to_string(&dump).expect("serialize should succeed");
    let decoded: ReproDump = serde_json::from_str(&json).expect("deserialize should succeed");
    let replayed = decoded
        .replay()
        .expect("decoded dump should be replayable")
        .final_surface()
        .expect("replay should succeed");

    assert_eq!(decoded.final_surface, dump.final_surface);
    assert_eq!(replayed, dump.final_surface);
}
