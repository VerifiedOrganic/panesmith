use super::Vt100Backend;
use panesmith_core::{
    PaneConfig, PaneId, ReplayBackendKind, ReproDump, ReproRawTranscript, ReproSizeEvent, Size,
    SurfaceBackend,
};

#[test]
fn backend_keeps_track_of_pane_id_and_size() {
    let backend = Vt100Backend::new(PaneId::new(5), Size::new(24, 80)).unwrap();

    assert_eq!(backend.pane_id(), PaneId::new(5));
    assert_eq!(backend.size(), Size::new(24, 80));
}

#[test]
fn backend_metadata_advertises_replay_compatibility() {
    let backend = Vt100Backend::new(PaneId::new(5), Size::new(24, 80)).unwrap();
    assert_eq!(
        backend.metadata().replay_kind,
        Some(ReplayBackendKind::DefaultSurfaceVt100)
    );
}

#[test]
fn repro_dump_from_public_vt100_backend_replays_simple_fixture() {
    let pane_id = PaneId::new(5);
    let size = Size::new(24, 80);
    let mut backend = Vt100Backend::new(pane_id, size).unwrap();
    backend.feed(b"hello world").unwrap();

    let dump = ReproDump {
        pane_id,
        spawn_config: PaneConfig::command("fixture").with_size(size),
        backend: backend.metadata(),
        size_history: vec![ReproSizeEvent::new(0, size)],
        events: Vec::new(),
        raw_transcript: Some(ReproRawTranscript {
            start_offset: 0,
            bytes: b"hello world".to_vec(),
        }),
        final_surface: backend.snapshot().to_owned_snapshot(),
    };

    let replayed = dump.replay().unwrap().final_surface().unwrap();
    assert_eq!(replayed, dump.final_surface);
}
