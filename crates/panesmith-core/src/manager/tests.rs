use std::borrow::Cow;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::*;
use crate::{
    AttachScreenPolicy, CellAttrs, CellStyle, CellWidth, ColorSpec, CursorPosition, CursorState,
    InputConfig, MouseMode, PasteNewlinePolicy, ReproDumpOptions, ScrollbackLine,
    ScrollbackSnapshot, SurfaceCell, SurfaceRow, SurfaceSnapshot, TerminalModes,
    TranscriptRotatedEvent,
};

#[derive(Debug, Default)]
struct FakeSurfaceShared {
    last_feed: Vec<Vec<u8>>,
    resize_calls: Vec<Size>,
}

#[derive(Debug, Default)]
struct FakePtyProcessShared {
    resize_calls: Vec<Size>,
    write_calls: Vec<Vec<u8>>,
    write_attempts: usize,
    write_failures: VecDeque<io::ErrorKind>,
    flush_failures: VecDeque<io::ErrorKind>,
}

#[derive(Debug, Default)]
struct LifecycleCounters {
    kill_calls: AtomicUsize,
    process_drops: AtomicUsize,
    surface_drops: AtomicUsize,
}

impl LifecycleCounters {
    fn kill_calls(&self) -> usize {
        self.kill_calls.load(Ordering::SeqCst)
    }

    fn process_drops(&self) -> usize {
        self.process_drops.load(Ordering::SeqCst)
    }

    fn surface_drops(&self) -> usize {
        self.surface_drops.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Default)]
struct TrackingResizeSurfaceShared {
    validate_calls: Vec<Size>,
    resize_calls: Vec<Size>,
}

#[derive(Debug)]
struct FakeSurfaceBackend {
    shared: Arc<Mutex<FakeSurfaceShared>>,
    size: Size,
    cursor: CursorState,
    modes: TerminalModes,
    snapshot_title: Option<&'static str>,
    rows: Vec<Vec<&'static str>>,
    scrollback: Vec<&'static str>,
    styled_scrollback: Vec<ScrollbackLine<'static>>,
    update: SurfaceUpdate,
}

impl FakeSurfaceBackend {
    fn new(shared: Arc<Mutex<FakeSurfaceShared>>, size: Size, update: SurfaceUpdate) -> Self {
        Self {
            shared,
            size,
            cursor: CursorState::new(Some(CursorPosition::new(2, 5)), true),
            modes: TerminalModes {
                bracketed_paste: true,
                mouse: MouseMode::AnyEvent,
                focus_events: true,
                application_cursor: true,
                alternate_screen: false,
            },
            snapshot_title: Some("surface title"),
            rows: vec![vec!["hello"], vec!["world"]],
            scrollback: vec!["older output"],
            styled_scrollback: Vec::new(),
            update,
        }
    }

    fn with_modes(mut self, modes: TerminalModes) -> Self {
        self.modes = modes;
        self
    }
}

impl SurfaceBackend for FakeSurfaceBackend {
    fn size(&self) -> Size {
        self.size
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.size = size;
        self.shared
            .lock()
            .expect("surface shared lock")
            .resize_calls
            .push(size);
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<SurfaceUpdate> {
        self.shared
            .lock()
            .expect("surface shared lock")
            .last_feed
            .push(bytes.to_vec());
        Ok(self.update)
    }

    fn snapshot(&self) -> SurfaceSnapshot<'_> {
        SurfaceSnapshot::new(
            self.size,
            self.rows
                .iter()
                .map(|row| {
                    SurfaceRow::new(
                        row.iter()
                            .map(|cell| {
                                SurfaceCell::new(*cell, CellWidth::Single, CellStyle::default())
                            })
                            .collect(),
                    )
                })
                .collect(),
            self.cursor,
            self.modes,
            self.snapshot_title.map(Cow::Borrowed),
        )
    }

    fn cursor(&self) -> CursorState {
        self.cursor
    }

    fn modes(&self) -> TerminalModes {
        self.modes
    }

    fn scrollback(&self) -> ScrollbackSnapshot<'_> {
        if !self.styled_scrollback.is_empty() {
            return ScrollbackSnapshot::new(self.styled_scrollback.clone());
        }
        ScrollbackSnapshot::new(
            self.scrollback
                .iter()
                .map(|line| ScrollbackLine::new(*line))
                .collect(),
        )
    }
}

#[derive(Debug, Default)]
struct EchoSurfaceShared {
    text: String,
}

#[derive(Debug)]
struct EchoSurfaceBackend {
    shared: Arc<Mutex<EchoSurfaceShared>>,
    size: Size,
    modes: TerminalModes,
}

impl EchoSurfaceBackend {
    fn new(shared: Arc<Mutex<EchoSurfaceShared>>, size: Size, modes: TerminalModes) -> Self {
        Self {
            shared,
            size,
            modes,
        }
    }
}

impl SurfaceBackend for EchoSurfaceBackend {
    fn size(&self) -> Size {
        self.size
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.size = size;
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<SurfaceUpdate> {
        self.shared
            .lock()
            .expect("echo surface lock")
            .text
            .push_str(&String::from_utf8_lossy(bytes));
        Ok(SurfaceUpdate {
            dirty_rows: DirtyRows::All,
            cursor_changed: true,
            title_changed: false,
            modes_changed: false,
            scrollback_changed: false,
        })
    }

    fn snapshot(&self) -> SurfaceSnapshot<'_> {
        let text = self.shared.lock().expect("echo surface lock").text.clone();
        let mut rows = text
            .lines()
            .map(|line| {
                SurfaceRow::new(vec![SurfaceCell::new(
                    line.to_string(),
                    CellWidth::Single,
                    CellStyle::default(),
                )])
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            rows.push(SurfaceRow::default());
        }
        SurfaceSnapshot::new(self.size, rows, CursorState::default(), self.modes, None)
    }

    fn cursor(&self) -> CursorState {
        CursorState::default()
    }

    fn modes(&self) -> TerminalModes {
        self.modes
    }

    fn scrollback(&self) -> ScrollbackSnapshot<'_> {
        ScrollbackSnapshot::new(Vec::new())
    }
}

#[derive(Debug)]
struct TrackingResizeSurfaceBackend {
    shared: Arc<Mutex<TrackingResizeSurfaceShared>>,
    size: Size,
    rejected_size: Option<Size>,
}

impl TrackingResizeSurfaceBackend {
    fn new(
        shared: Arc<Mutex<TrackingResizeSurfaceShared>>,
        size: Size,
        rejected_size: Option<Size>,
    ) -> Self {
        Self {
            shared,
            size,
            rejected_size,
        }
    }
}

impl SurfaceBackend for TrackingResizeSurfaceBackend {
    fn size(&self) -> Size {
        self.size
    }

    fn validate_resize(&self, size: Size) -> Result<()> {
        self.shared
            .lock()
            .expect("tracking surface shared lock")
            .validate_calls
            .push(size);
        if self.rejected_size == Some(size) {
            Err(PaneError::Surface {
                message: format!(
                    "tracking surface rejected rows={} cols={}",
                    size.rows, size.cols
                ),
            })
        } else {
            Ok(())
        }
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.shared
            .lock()
            .expect("tracking surface shared lock")
            .resize_calls
            .push(size);
        self.size = size;
        Ok(())
    }

    fn feed(&mut self, _bytes: &[u8]) -> Result<SurfaceUpdate> {
        Ok(SurfaceUpdate::default())
    }

    fn snapshot(&self) -> SurfaceSnapshot<'_> {
        SurfaceSnapshot::blank(self.size)
    }

    fn cursor(&self) -> CursorState {
        CursorState::default()
    }

    fn modes(&self) -> TerminalModes {
        TerminalModes::default()
    }

    fn scrollback(&self) -> ScrollbackSnapshot<'_> {
        ScrollbackSnapshot::new(vec![])
    }
}

#[derive(Debug)]
struct FakePtyProcess {
    id: String,
    frames: VecDeque<PtyFrame>,
}

impl PtyProcess for FakePtyProcess {
    fn id(&self) -> &str {
        &self.id
    }

    fn writer(&self) -> crate::PtyWriter {
        panic!("test writer should not be requested")
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        self.frames.pop_front()
    }

    fn resize(&mut self, _size: Size) -> Result<()> {
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct TrackingPtyProcess {
    id: String,
    shared: Arc<Mutex<FakePtyProcessShared>>,
}

impl TrackingPtyProcess {
    fn new(shared: Arc<Mutex<FakePtyProcessShared>>) -> Self {
        Self {
            id: "tracking-pty".into(),
            shared,
        }
    }
}

impl PtyProcess for TrackingPtyProcess {
    fn id(&self) -> &str {
        &self.id
    }

    fn writer(&self) -> crate::PtyWriter {
        panic!("test writer should not be requested")
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        None
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.shared
            .lock()
            .expect("tracking pty shared lock")
            .resize_calls
            .push(size);
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct LifecyclePtyProcess {
    counters: Arc<LifecycleCounters>,
}

impl Drop for LifecyclePtyProcess {
    fn drop(&mut self) {
        self.counters.process_drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl PtyProcess for LifecyclePtyProcess {
    fn id(&self) -> &str {
        "lifecycle-pty"
    }

    fn writer(&self) -> crate::PtyWriter {
        panic!("test writer should not be requested")
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        None
    }

    fn resize(&mut self, _size: Size) -> Result<()> {
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        self.counters.kill_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug)]
struct LifecycleSurfaceBackend {
    counters: Arc<LifecycleCounters>,
    size: Size,
}

impl Drop for LifecycleSurfaceBackend {
    fn drop(&mut self) {
        self.counters.surface_drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl SurfaceBackend for LifecycleSurfaceBackend {
    fn size(&self) -> Size {
        self.size
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.size = size;
        Ok(())
    }

    fn feed(&mut self, _bytes: &[u8]) -> Result<SurfaceUpdate> {
        Ok(SurfaceUpdate::default())
    }

    fn snapshot(&self) -> SurfaceSnapshot<'_> {
        SurfaceSnapshot::blank(self.size)
    }

    fn cursor(&self) -> CursorState {
        CursorState::default()
    }

    fn modes(&self) -> TerminalModes {
        TerminalModes::default()
    }

    fn scrollback(&self) -> ScrollbackSnapshot<'_> {
        ScrollbackSnapshot::new(vec![])
    }
}

#[derive(Debug)]
struct FailingResizePtyProcess {
    id: String,
    shared: Arc<Mutex<FakePtyProcessShared>>,
    error: PaneError,
}

impl FailingResizePtyProcess {
    fn new(shared: Arc<Mutex<FakePtyProcessShared>>, error: PaneError) -> Self {
        Self {
            id: "failing-resize-pty".into(),
            shared,
            error,
        }
    }
}

impl PtyProcess for FailingResizePtyProcess {
    fn id(&self) -> &str {
        &self.id
    }

    fn writer(&self) -> crate::PtyWriter {
        panic!("test writer should not be requested")
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        None
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.shared
            .lock()
            .expect("failing resize pty shared lock")
            .resize_calls
            .push(size);
        Err(self.error.clone())
    }

    fn kill(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct SharedPtyWriter {
    shared: Arc<Mutex<FakePtyProcessShared>>,
}

impl Write for SharedPtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut shared = self.shared.lock().expect("shared pty writer lock");
        shared.write_attempts += 1;
        if let Some(kind) = shared.write_failures.pop_front() {
            return Err(io::Error::from(kind));
        }
        shared.write_calls.push(buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(kind) = self
            .shared
            .lock()
            .expect("shared pty writer lock")
            .flush_failures
            .pop_front()
        {
            return Err(io::Error::from(kind));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct AttachPtyProcess {
    id: String,
    shared: Arc<Mutex<FakePtyProcessShared>>,
    frames: VecDeque<Option<PtyFrame>>,
    fail_resize: Option<(Size, PaneError)>,
}

impl AttachPtyProcess {
    fn new(
        shared: Arc<Mutex<FakePtyProcessShared>>,
        frames: Vec<Option<PtyFrame>>,
        fail_resize: Option<(Size, PaneError)>,
    ) -> Self {
        Self {
            id: "attach-pty".into(),
            shared,
            frames: VecDeque::from(frames),
            fail_resize,
        }
    }
}

impl PtyProcess for AttachPtyProcess {
    fn id(&self) -> &str {
        &self.id
    }

    fn writer(&self) -> crate::PtyWriter {
        crate::PtyWriter::new(Box::new(SharedPtyWriter {
            shared: Arc::clone(&self.shared),
        }))
    }

    fn try_recv(&mut self) -> Option<PtyFrame> {
        self.frames.pop_front().flatten()
    }

    fn resize(&mut self, size: Size) -> Result<()> {
        self.shared
            .lock()
            .expect("attach pty shared lock")
            .resize_calls
            .push(size);
        if let Some((rejected, error)) = &self.fail_resize {
            if *rejected == size {
                return Err(error.clone());
            }
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FakeAttachTerminal {
    stdin: VecDeque<PaneAttachInputChunk>,
    stdout: Arc<Mutex<Vec<Vec<u8>>>>,
    sizes: Arc<Mutex<VecDeque<Size>>>,
    stdout_retry_failures: Arc<Mutex<VecDeque<io::ErrorKind>>>,
}

impl FakeAttachTerminal {
    fn new(size: Size) -> Self {
        Self {
            stdin: VecDeque::new(),
            stdout: Arc::new(Mutex::new(Vec::new())),
            sizes: Arc::new(Mutex::new(VecDeque::from(vec![size]))),
            stdout_retry_failures: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn with_stdin(mut self, chunks: Vec<Vec<u8>>) -> Self {
        self.stdin = chunks
            .into_iter()
            .map(|bytes| PaneAttachInputChunk {
                at: Instant::now(),
                bytes,
            })
            .collect();
        self
    }

    fn with_stdout_retry_failures(mut self, failures: Vec<io::ErrorKind>) -> Self {
        self.stdout_retry_failures = Arc::new(Mutex::new(VecDeque::from(failures)));
        self
    }

    fn stdout(&self) -> Arc<Mutex<Vec<Vec<u8>>>> {
        Arc::clone(&self.stdout)
    }
}

impl PaneAttachTerminal for FakeAttachTerminal {
    type Error = &'static str;

    fn read_stdin(&mut self) -> std::result::Result<Option<PaneAttachInputChunk>, Self::Error> {
        Ok(self.stdin.pop_front())
    }

    fn write_stdout(&mut self, bytes: &[u8]) -> std::result::Result<(), Self::Error> {
        loop {
            let failure = self
                .stdout_retry_failures
                .lock()
                .expect("fake terminal stdout retry lock")
                .pop_front();
            match failure {
                Some(io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Some(_) => return Err("injected terminal stdout failure"),
                None => break,
            }
        }
        self.stdout
            .lock()
            .expect("fake terminal stdout lock")
            .push(bytes.to_vec());
        Ok(())
    }

    fn size(&self) -> std::result::Result<Size, Self::Error> {
        let mut sizes = self.sizes.lock().expect("fake terminal sizes lock");
        if sizes.len() > 1 {
            Ok(sizes.pop_front().expect("size should be present"))
        } else {
            Ok(*sizes
                .front()
                .expect("fake terminal needs at least one size"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FakeRestoreToken;

#[derive(Debug, Default)]
struct FakeAttachControl {
    suspended: Vec<AttachScreenPolicy>,
    restore_calls: u32,
    fail_suspend: bool,
    fail_restore_count: u32,
}

impl PaneAttachTerminalControl for FakeAttachControl {
    type Error = &'static str;
    type RestoreToken = FakeRestoreToken;

    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> std::result::Result<Self::RestoreToken, Self::Error> {
        if self.fail_suspend {
            return Err("suspend failed");
        }
        self.suspended.push(policy);
        Ok(FakeRestoreToken)
    }

    fn restore_after_attach(
        &mut self,
        _token: &mut Self::RestoreToken,
    ) -> std::result::Result<(), Self::Error> {
        self.restore_calls += 1;
        if self.fail_restore_count > 0 {
            self.fail_restore_count -= 1;
            return Err("restore failed");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmbeddedKeyboardEnhancementProfile {
    disambiguate_escape_codes: bool,
    report_event_types: bool,
}

impl EmbeddedKeyboardEnhancementProfile {
    fn dashboard() -> Self {
        Self {
            disambiguate_escape_codes: true,
            report_event_types: true,
        }
    }

    fn attach_owned() -> Self {
        Self {
            disambiguate_escape_codes: false,
            report_event_types: false,
        }
    }

    fn uses_csi_u_event_encoding(self) -> bool {
        self.disambiguate_escape_codes && self.report_event_types
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmbeddedTuiHostProfile {
    raw_mode: bool,
    alternate_screen: bool,
    mouse_capture: bool,
    bracketed_paste: bool,
    keyboard: EmbeddedKeyboardEnhancementProfile,
}

impl EmbeddedTuiHostProfile {
    fn dashboard() -> Self {
        Self {
            raw_mode: true,
            alternate_screen: true,
            mouse_capture: true,
            bracketed_paste: true,
            keyboard: EmbeddedKeyboardEnhancementProfile::dashboard(),
        }
    }

    fn attach_owned() -> Self {
        Self {
            raw_mode: true,
            alternate_screen: true,
            mouse_capture: false,
            bracketed_paste: false,
            keyboard: EmbeddedKeyboardEnhancementProfile::attach_owned(),
        }
    }

    fn ctrl_key_bytes(self, letter: u8) -> Vec<u8> {
        assert!(letter.is_ascii_lowercase());
        if self.keyboard.uses_csi_u_event_encoding() {
            format!("\x1b[{letter};5:1u").into_bytes()
        } else {
            vec![letter - b'a' + 1]
        }
    }

    fn parse_ctrl_key(self, bytes: &[u8]) -> Option<u8> {
        if !self.raw_mode {
            return None;
        }
        if let [control] = bytes {
            if (1..=26).contains(control) {
                return Some(control + b'a' - 1);
            }
            return None;
        }
        if !self.keyboard.uses_csi_u_event_encoding() {
            return None;
        }
        let text = std::str::from_utf8(bytes).ok()?;
        let body = text.strip_prefix("\x1b[")?.strip_suffix('u')?;
        let code = body.split([';', ':']).next()?.parse::<u8>().ok()?;
        if code.is_ascii_lowercase() {
            Some(code)
        } else if (1..=26).contains(&code) {
            Some(code + b'a' - 1)
        } else {
            None
        }
    }

    fn mouse_wheel_up_bytes(self) -> Option<Vec<u8>> {
        if self.alternate_screen && self.mouse_capture {
            Some(b"\x1b[<64;12;2M".to_vec())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EmbeddedTuiRestoreToken {
    profile: EmbeddedTuiHostProfile,
}

#[derive(Debug)]
struct EmbeddedTuiAttachControl {
    expected_profile: EmbeddedTuiHostProfile,
    profile: EmbeddedTuiHostProfile,
    suspended: Vec<AttachScreenPolicy>,
    restore_calls: u32,
    parent_mouse_events: u32,
    parent_scrollback_leaks: u32,
}

impl EmbeddedTuiAttachControl {
    fn new() -> Self {
        let expected_profile = EmbeddedTuiHostProfile::dashboard();
        Self {
            expected_profile,
            profile: expected_profile,
            suspended: Vec::new(),
            restore_calls: 0,
            parent_mouse_events: 0,
            parent_scrollback_leaks: 0,
        }
    }

    fn assert_dashboard_input_profile(
        &self,
        ctrl_f_before_attach: &[u8],
        ctrl_q_before_attach: &[u8],
    ) {
        assert_eq!(
            self.profile, self.expected_profile,
            "restore_after_attach must restore the caller-owned dashboard profile exactly"
        );
        let ctrl_f_after_attach = self.profile.ctrl_key_bytes(b'f');
        let ctrl_q_after_attach = self.profile.ctrl_key_bytes(b'q');
        assert_eq!(
            ctrl_f_after_attach, ctrl_f_before_attach,
            "Ctrl-f input shape changed after attach/detach"
        );
        assert_eq!(
            ctrl_q_after_attach, ctrl_q_before_attach,
            "Ctrl-q input shape changed after attach/detach"
        );
        assert_eq!(
            self.profile.parse_ctrl_key(&ctrl_f_after_attach),
            Some(b'f')
        );
        assert_eq!(
            self.profile.parse_ctrl_key(&ctrl_q_after_attach),
            Some(b'q')
        );
    }

    fn dispatch_parent_mouse_wheel(&mut self) {
        if self.profile.mouse_wheel_up_bytes().is_some() {
            self.parent_mouse_events += 1;
        } else {
            self.parent_scrollback_leaks += 1;
        }
    }
}

impl PaneAttachTerminalControl for EmbeddedTuiAttachControl {
    type Error = &'static str;
    type RestoreToken = EmbeddedTuiRestoreToken;

    fn suspend_for_attach(
        &mut self,
        policy: AttachScreenPolicy,
    ) -> std::result::Result<Self::RestoreToken, Self::Error> {
        assert_eq!(
            self.profile, self.expected_profile,
            "attach must start from the host dashboard profile"
        );
        self.suspended.push(policy);
        let token = EmbeddedTuiRestoreToken {
            profile: self.expected_profile,
        };
        self.profile = EmbeddedTuiHostProfile::attach_owned();
        Ok(token)
    }

    fn restore_after_attach(
        &mut self,
        token: &mut Self::RestoreToken,
    ) -> std::result::Result<(), Self::Error> {
        self.restore_calls += 1;
        self.profile = token.profile;
        Ok(())
    }
}

fn output_frame(seq: u64, bytes: &[u8]) -> PtyFrame {
    PtyFrame::Output {
        seq,
        bytes: bytes.to_vec(),
        at: Instant::now(),
    }
}

fn exit_frame(seq: u64, code: Option<i32>) -> PtyFrame {
    PtyFrame::Exited {
        seq,
        code,
        at: Instant::now(),
    }
}

fn manager_with_lifecycle_tracking() -> (PaneManager, PaneId, Arc<LifecycleCounters>) {
    let counters = Arc::new(LifecycleCounters::default());
    let process_counters = Arc::clone(&counters);
    let surface_counters = Arc::clone(&counters);

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(LifecyclePtyProcess {
                    counters: Arc::clone(&process_counters),
                }) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(LifecycleSurfaceBackend {
                    counters: Arc::clone(&surface_counters),
                    size: config.size,
                }) as Box<dyn SurfaceBackend + Send>)
            }),
    );
    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with lifecycle test runtime");
    (manager, pane_id, counters)
}

fn manager_with_attach_process(
    frames: Vec<Option<PtyFrame>>,
    embedded_size: Size,
    surface_update: SurfaceUpdate,
    fail_resize: Option<(Size, PaneError)>,
) -> (
    PaneManager,
    PaneId,
    Arc<Mutex<FakePtyProcessShared>>,
    Arc<Mutex<FakeSurfaceShared>>,
) {
    let pty_shared = Arc::new(Mutex::new(FakePtyProcessShared::default()));
    let surface_shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let frames_for_spawn = frames.clone();
    let fail_resize_for_spawn = fail_resize.clone();
    let pty_shared_for_spawn = Arc::clone(&pty_shared);
    let surface_shared_for_spawn = Arc::clone(&surface_shared);

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(AttachPtyProcess::new(
                    Arc::clone(&pty_shared_for_spawn),
                    frames_for_spawn.clone(),
                    fail_resize_for_spawn.clone(),
                )) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared_for_spawn),
                    config.size,
                    surface_update,
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );
    let pane_id = manager
        .spawn(
            PaneConfig::command("fake-command")
                .with_size(embedded_size)
                .with_transcript(crate::TranscriptConfig::new(crate::TranscriptMode::Both)),
        )
        .expect("spawn should succeed with attach test runtime");
    (manager, pane_id, pty_shared, surface_shared)
}

fn manager_with_transaction_process(
    frames: Vec<Option<PtyFrame>>,
    modes: TerminalModes,
) -> (
    PaneManager,
    PaneId,
    Arc<Mutex<FakePtyProcessShared>>,
    Arc<Mutex<FakeSurfaceShared>>,
) {
    manager_with_transaction_process_config(
        frames,
        modes,
        PaneConfig::command("fake-command").with_size(Size::new(10, 40)),
    )
}

fn manager_with_transaction_process_config(
    frames: Vec<Option<PtyFrame>>,
    modes: TerminalModes,
    pane_config: PaneConfig,
) -> (
    PaneManager,
    PaneId,
    Arc<Mutex<FakePtyProcessShared>>,
    Arc<Mutex<FakeSurfaceShared>>,
) {
    let pty_shared = Arc::new(Mutex::new(FakePtyProcessShared::default()));
    let surface_shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let frames_for_spawn = frames.clone();
    let pty_shared_for_spawn = Arc::clone(&pty_shared);
    let surface_shared_for_spawn = Arc::clone(&surface_shared);

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(AttachPtyProcess::new(
                    Arc::clone(&pty_shared_for_spawn),
                    frames_for_spawn.clone(),
                    None,
                )) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(
                    FakeSurfaceBackend::new(
                        Arc::clone(&surface_shared_for_spawn),
                        config.size,
                        SurfaceUpdate::default(),
                    )
                    .with_modes(modes),
                ) as Box<dyn SurfaceBackend + Send>)
            }),
    );
    let pane_id = manager
        .spawn(pane_config)
        .expect("spawn should succeed with transaction test runtime");
    (manager, pane_id, pty_shared, surface_shared)
}

fn manager_with_echo_transaction_process(
    frames: Vec<Option<PtyFrame>>,
    modes: TerminalModes,
) -> (
    PaneManager,
    PaneId,
    Arc<Mutex<FakePtyProcessShared>>,
    Arc<Mutex<EchoSurfaceShared>>,
) {
    let pty_shared = Arc::new(Mutex::new(FakePtyProcessShared::default()));
    let surface_shared = Arc::new(Mutex::new(EchoSurfaceShared::default()));
    let frames_for_spawn = frames.clone();
    let pty_shared_for_spawn = Arc::clone(&pty_shared);
    let surface_shared_for_spawn = Arc::clone(&surface_shared);

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(AttachPtyProcess::new(
                    Arc::clone(&pty_shared_for_spawn),
                    frames_for_spawn.clone(),
                    None,
                )) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(EchoSurfaceBackend::new(
                    Arc::clone(&surface_shared_for_spawn),
                    config.size,
                    modes,
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );
    let pane_id = manager
        .spawn(PaneConfig::command("fake-command").with_size(Size::new(10, 40)))
        .expect("spawn should succeed with echo transaction test runtime");
    (manager, pane_id, pty_shared, surface_shared)
}

fn transaction_modes(bracketed_paste: bool) -> TerminalModes {
    TerminalModes {
        bracketed_paste,
        ..TerminalModes::default()
    }
}

fn manager_with_attach_scrollback(
    frames: Vec<Option<PtyFrame>>,
    embedded_size: Size,
    rows: Vec<Vec<&'static str>>,
    styled_scrollback: Vec<ScrollbackLine<'static>>,
) -> (
    PaneManager,
    PaneId,
    Arc<Mutex<FakePtyProcessShared>>,
    Arc<Mutex<FakeSurfaceShared>>,
) {
    let pty_shared = Arc::new(Mutex::new(FakePtyProcessShared::default()));
    let surface_shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let frames_for_spawn = frames.clone();
    let pty_shared_for_spawn = Arc::clone(&pty_shared);
    let surface_shared_for_spawn = Arc::clone(&surface_shared);
    let rows_for_spawn = rows.clone();
    let scrollback_for_spawn = styled_scrollback.clone();

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(AttachPtyProcess::new(
                    Arc::clone(&pty_shared_for_spawn),
                    frames_for_spawn.clone(),
                    None,
                )) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                let mut backend = FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared_for_spawn),
                    config.size,
                    SurfaceUpdate::default(),
                );
                backend.rows = rows_for_spawn.clone();
                backend.styled_scrollback = scrollback_for_spawn.clone();
                Ok(Box::new(backend) as Box<dyn SurfaceBackend + Send>)
            }),
    );
    let pane_id = manager
        .spawn(
            PaneConfig::command("fake-command")
                .with_size(embedded_size)
                .with_transcript(crate::TranscriptConfig::new(crate::TranscriptMode::Both)),
        )
        .expect("spawn should succeed with attach scrollback test runtime");
    (manager, pane_id, pty_shared, surface_shared)
}

/// A surface backend that always rejects feed() with a surface error.
/// Used to verify that transcript recording happens before surface feeding.
#[derive(Debug)]
struct FailingSurfaceBackend;

impl SurfaceBackend for FailingSurfaceBackend {
    fn size(&self) -> Size {
        Size::new(24, 80)
    }

    fn resize(&mut self, _size: Size) -> Result<()> {
        Ok(())
    }

    fn feed(&mut self, _bytes: &[u8]) -> Result<SurfaceUpdate> {
        Err(PaneError::Surface {
            message: "injected feed failure".into(),
        })
    }

    fn snapshot(&self) -> SurfaceSnapshot<'_> {
        SurfaceSnapshot::blank(Size::new(24, 80))
    }

    fn cursor(&self) -> CursorState {
        CursorState::default()
    }

    fn modes(&self) -> TerminalModes {
        TerminalModes::default()
    }

    fn scrollback(&self) -> ScrollbackSnapshot<'_> {
        ScrollbackSnapshot::new(vec![])
    }
}

#[test]
fn pane_manager_new_accepts_config() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    assert_eq!(manager.alloc_placeholder_id(), PaneId::new(1));
}

#[test]
fn pane_manager_spawn_with_fake_runtime_emits_spawn_and_runtime_events() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let surface_shared = Arc::clone(&shared);
    let frames = vec![
        PtyFrame::Output {
            seq: 1,
            bytes: b"hello".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Overflow {
            seq: 2,
            dropped_frames: 3,
            dropped_bytes: 17,
            at: Instant::now(),
        },
        PtyFrame::Exited {
            seq: 3,
            code: Some(0),
            at: Instant::now(),
        },
    ];
    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(FakePtyProcess {
                    id: "fake-pty".into(),
                    frames: VecDeque::from(frames.clone()),
                }) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared),
                    config.size,
                    SurfaceUpdate {
                        dirty_rows: DirtyRows::Range { start: 0, end: 1 },
                        cursor_changed: true,
                        title_changed: false,
                        modes_changed: false,
                        scrollback_changed: false,
                    },
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with injected runtime");

    let mut events = Vec::new();
    manager.drain_events(&mut events);

    assert_eq!(
        shared.lock().expect("surface shared lock").last_feed,
        vec![b"hello".to_vec()]
    );
    assert!(matches!(
        events.as_slice(),
        [
            PaneEvent {
                kind: PaneEventKind::Spawned(SpawnedEvent { .. }),
                seq: 1,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::StateChanged(_),
                seq: 2,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::Output(OutputEvent { bytes_len: 5, .. }),
                seq: 3,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::SurfaceChanged(SurfaceChangedEvent {
                    generation: 1,
                    dirty_rows: DirtyRows::Range { start: 0, end: 1 },
                    cursor_changed: true,
                    ..
                }),
                seq: 4,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::Overflow(OverflowEvent {
                    dropped_frames: 3,
                    dropped_bytes: 17,
                    queue: OverflowQueue::PtyOutputFrames,
                }),
                seq: 5,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::Exited(ExitedEvent { code: Some(0) }),
                seq: 6,
                ..
            },
            PaneEvent {
                kind: PaneEventKind::StateChanged(_),
                seq: 7,
                ..
            },
        ]
    ));
    assert_eq!(
        manager.snapshot(pane_id).expect("pane should exist").state,
        PaneState::Exited { code: Some(0) }
    );
}

#[test]
fn pane_manager_remove_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .remove(pane_id)
        .expect_err("remove should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_remove_rejects_live_panes() {
    let mut manager = PaneManager::new(PaneManagerConfig::default().with_pty_spawner(|_config| {
        Ok(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::new(),
        }) as Box<dyn PtyProcess>)
    }));
    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with injected runtime");

    let err = manager
        .remove(pane_id)
        .expect_err("remove should reject live panes");
    assert!(
        matches!(
            err,
            PaneError::InvalidState { ref expected, ref actual }
            if expected == "Exited, Failed, or Killed" && actual == "Running"
        ),
        "expected InvalidState error, got {err:?}"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist")
            .state,
        PaneState::Running
    );
}

#[test]
fn pane_manager_remove_drains_exit_frames_before_state_validation() {
    let mut manager = PaneManager::new(PaneManagerConfig::default().with_pty_spawner(|_config| {
        Ok(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::new(),
        }) as Box<dyn PtyProcess>)
    }));
    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with injected runtime");

    {
        let pane = manager
            .panes
            .get_mut(&pane_id)
            .expect("pane should exist for test setup");
        pane.process = Some(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::from(vec![PtyFrame::Exited {
                seq: 1,
                code: Some(0),
                at: Instant::now(),
            }]),
        }));
    }

    assert_eq!(
        manager
            .remove(pane_id)
            .expect("remove should observe queued exit frames"),
        Some(PaneExit::Exited { code: Some(0) })
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            PaneEventKind::Exited(ExitedEvent { code: Some(0) })
        )),
        "expected remove() to preserve the queued Exited event, got {events:?}"
    );
}

#[test]
fn pane_manager_remove_flushes_killed_pane_exit_before_deletion() {
    let mut manager = PaneManager::new(PaneManagerConfig::default().with_pty_spawner(|_config| {
        Ok(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::new(),
        }) as Box<dyn PtyProcess>)
    }));
    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with injected runtime");

    {
        let pane = manager
            .panes
            .get_mut(&pane_id)
            .expect("pane should exist for test setup");
        pane.state = PaneState::Killed {
            reason: KillReason::UserRequested,
        };
        pane.exit = Some(PaneExit::Killed {
            reason: KillReason::UserRequested,
        });
        pane.process = Some(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::from(vec![PtyFrame::Exited {
                seq: 1,
                code: None,
                at: Instant::now(),
            }]),
        }));
        pane.exit_event_observed = false;
    }

    assert_eq!(
        manager.remove(pane_id).expect("remove should succeed"),
        Some(PaneExit::Killed {
            reason: KillReason::UserRequested,
        })
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            PaneEventKind::Exited(ExitedEvent { code: None })
        )),
        "expected remove() to preserve an Exited event for killed panes, got {events:?}"
    );
}

#[test]
fn pane_manager_remove_flushes_failed_pane_exit_before_deletion() {
    let mut manager = PaneManager::new(PaneManagerConfig::default().with_pty_spawner(|_config| {
        Ok(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::new(),
        }) as Box<dyn PtyProcess>)
    }));
    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with injected runtime");

    {
        let pane = manager
            .panes
            .get_mut(&pane_id)
            .expect("pane should exist for test setup");
        let error = PaneError::Spawn {
            message: "test failure".into(),
        };
        pane.state = PaneState::Failed {
            error: error.clone(),
        };
        pane.exit = Some(PaneExit::Failed { error });
        pane.process = Some(Box::new(FakePtyProcess {
            id: "fake-pty".into(),
            frames: VecDeque::from(vec![PtyFrame::Exited {
                seq: 1,
                code: None,
                at: Instant::now(),
            }]),
        }));
        pane.exit_event_observed = false;
    }

    assert_eq!(
        manager.remove(pane_id).expect("remove should succeed"),
        Some(PaneExit::Failed {
            error: PaneError::Spawn {
                message: "test failure".into(),
            },
        })
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            PaneEventKind::Exited(ExitedEvent { code: None })
        )),
        "expected remove() to preserve an Exited event for failed panes, got {events:?}"
    );
}

#[test]
fn pane_manager_kill_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .kill(pane_id, KillReason::UserRequested)
        .expect_err("kill should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_kill_leaves_killed_pane_until_explicit_remove_drops_resources() {
    let (mut manager, pane_id, counters) = manager_with_lifecycle_tracking();

    manager
        .kill(pane_id, KillReason::HostRequested)
        .expect("kill should succeed");

    assert_eq!(counters.kill_calls(), 1, "kill should reach the PTY once");
    assert_eq!(
        counters.process_drops(),
        0,
        "kill alone must not drop manager-owned process state"
    );
    assert_eq!(
        counters.surface_drops(),
        0,
        "kill alone must not drop manager-owned surface state"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("killed pane should remain inspectable")
            .state,
        PaneState::Killed {
            reason: KillReason::HostRequested
        }
    );

    let exit = manager
        .remove(pane_id)
        .expect("remove should drop terminal pane state");
    assert_eq!(
        exit,
        Some(PaneExit::Killed {
            reason: KillReason::HostRequested
        })
    );
    assert_eq!(counters.process_drops(), 1);
    assert_eq!(counters.surface_drops(), 1);
    assert!(
        matches!(
            manager.snapshot(pane_id),
            Err(PaneError::NotFound { pane_id: id }) if id == pane_id
        ),
        "remove should erase the runtime entry"
    );

    drop(manager);
    assert_eq!(
        counters.process_drops(),
        1,
        "removed process state must be dropped exactly once"
    );
    assert_eq!(
        counters.surface_drops(),
        1,
        "removed surface state must be dropped exactly once"
    );
}

#[test]
fn pane_manager_kill_and_remove_drops_owned_resources_once() {
    let (mut manager, pane_id, counters) = manager_with_lifecycle_tracking();

    let exit = manager
        .kill_and_remove(pane_id, KillReason::HostRequested)
        .expect("kill_and_remove should kill and remove the pane");

    assert_eq!(
        exit,
        Some(PaneExit::Killed {
            reason: KillReason::HostRequested
        })
    );
    assert_eq!(counters.kill_calls(), 1, "kill should reach the PTY once");
    assert_eq!(counters.process_drops(), 1);
    assert_eq!(counters.surface_drops(), 1);
    assert!(
        matches!(
            manager.snapshot(pane_id),
            Err(PaneError::NotFound { pane_id: id }) if id == pane_id
        ),
        "kill_and_remove should erase the runtime entry"
    );

    drop(manager);
    assert_eq!(
        counters.process_drops(),
        1,
        "removed process state must be dropped exactly once"
    );
    assert_eq!(
        counters.surface_drops(),
        1,
        "removed surface state must be dropped exactly once"
    );
}

#[test]
fn pane_manager_kill_and_remove_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .kill_and_remove(pane_id, KillReason::UserRequested)
        .expect_err("kill_and_remove should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[cfg(unix)]
fn open_fd_count_for_testing() -> io::Result<usize> {
    let path = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    Ok(std::fs::read_dir(path)?
        .filter(|entry| entry.is_ok())
        .count())
}

#[cfg(unix)]
fn spawn_and_close_live_pane_for_fd_smoke(manager: &mut PaneManager) -> Result<()> {
    let pane_id = manager.spawn(
        PaneConfig::command_with_args("sleep", ["60"])
            .with_kill(crate::KillConfig::new(Duration::ZERO, false)),
    )?;
    manager.kill_and_remove(pane_id, KillReason::HostRequested)?;
    let mut events = Vec::new();
    manager.drain_events(&mut events);
    Ok(())
}

#[test]
#[cfg(unix)]
#[ignore = "spawns live PTYs and inspects process file descriptor counts"]
fn repeated_live_spawn_kill_and_remove_does_not_grow_open_file_descriptors(
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut manager = PaneManager::new(PaneManagerConfig::default());

    for _ in 0..3 {
        spawn_and_close_live_pane_for_fd_smoke(&mut manager)?;
    }
    let baseline = open_fd_count_for_testing()?;

    for _ in 0..25 {
        spawn_and_close_live_pane_for_fd_smoke(&mut manager)?;
    }
    let final_count = open_fd_count_for_testing()?;

    assert!(
        final_count <= baseline,
        "open file descriptors grew across spawn/close cycles: baseline={baseline}, final={final_count}"
    );
    Ok(())
}

#[test]
fn pane_manager_resize_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .resize(pane_id, Size::new(24, 80))
        .expect_err("resize should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_write_bytes_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .write_bytes(pane_id, b"hello")
        .expect_err("write_bytes should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_send_input_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .send_input(pane_id, HostInput::Raw(vec![0x61]))
        .expect_err("send_input should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_send_input_transaction_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .send_input_transaction(pane_id, InputTransaction::submit_text("hello"))
        .expect_err("input transaction should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_input_transaction_single_line_submit_sends_content_then_enter() {
    let (mut manager, pane_id, pty_shared, _surface_shared) =
        manager_with_transaction_process(Vec::new(), transaction_modes(false));

    let outcome = manager
        .send_input_transaction(pane_id, InputTransaction::submit_text("hello"))
        .expect("transaction should succeed");

    assert_eq!(outcome.bytes_sent, 6);
    assert!(outcome.submitted);
    assert!(!outcome.echoed);
    assert!(outcome.errors.is_empty(), "unexpected errors: {outcome:?}");
    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"hello".to_vec(), b"\r".to_vec()],
        "submit must be sent only after all content bytes"
    );
}

#[test]
fn pane_manager_input_transaction_multiline_submit_uses_bracketed_paste() {
    let (mut manager, pane_id, pty_shared, _surface_shared) =
        manager_with_transaction_process(Vec::new(), transaction_modes(true));

    let outcome = manager
        .send_input_transaction(pane_id, InputTransaction::submit_text("hello\nworld"))
        .expect("transaction should succeed");

    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"\x1b[200~hello\nworld\x1b[201~".to_vec(), b"\r".to_vec()]
    );
    assert_eq!(outcome.bytes_sent, 24);
    assert!(outcome.submitted);
    assert!(outcome.errors.is_empty(), "unexpected errors: {outcome:?}");
}

#[test]
fn pane_manager_input_transaction_multiline_paste_applies_newline_policy() {
    let input = InputConfig {
        paste_newline: PasteNewlinePolicy::NormalizeToLf,
        ..InputConfig::default()
    };
    let (mut manager, pane_id, pty_shared, _surface_shared) =
        manager_with_transaction_process_config(
            Vec::new(),
            transaction_modes(true),
            PaneConfig::command("fake-command")
                .with_size(Size::new(10, 40))
                .with_input(input),
        );

    let outcome = manager
        .send_input_transaction(pane_id, InputTransaction::insert_text("a\r\nb\rc"))
        .expect("transaction should succeed");

    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"\x1b[200~a\nb\nc\x1b[201~".to_vec()]
    );
    assert_eq!(outcome.bytes_sent, 17);
    assert!(!outcome.submitted);
    assert!(outcome.errors.is_empty(), "unexpected errors: {outcome:?}");
}

#[test]
fn pane_manager_input_transaction_falls_back_to_chunked_typing_without_bracketed_paste() {
    let (mut manager, pane_id, pty_shared, _surface_shared) =
        manager_with_transaction_process(Vec::new(), transaction_modes(false));

    let outcome = manager
        .send_input_transaction(
            pane_id,
            InputTransaction::insert_text("abcdef\nz").with_chunk_size(3),
        )
        .expect("transaction should succeed");

    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"abc".to_vec(), b"def".to_vec(), b"\nz".to_vec()]
    );
    assert_eq!(outcome.bytes_sent, 8);
    assert!(!outcome.submitted);
    assert!(outcome.errors.is_empty(), "unexpected errors: {outcome:?}");
}

#[test]
fn pane_manager_input_transaction_echo_verification_success_uses_surface_text() {
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_echo_transaction_process(
        vec![None, Some(output_frame(1, b"prompt> hello"))],
        transaction_modes(false),
    );

    let outcome = manager
        .send_input_transaction(
            pane_id,
            InputTransaction::submit_text("hello").with_verification(
                InputVerification::EchoContains {
                    needle: "prompt> hello".into(),
                    timeout: Duration::from_millis(25),
                },
            ),
        )
        .expect("transaction should succeed");

    assert!(outcome.echoed);
    assert!(outcome.submitted);
    assert!(!outcome.timed_out);
    assert!(outcome.errors.is_empty(), "unexpected errors: {outcome:?}");
    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"hello".to_vec(), b"\r".to_vec()]
    );
}

#[test]
fn pane_manager_input_transaction_echo_verification_timeout_is_structured() {
    let (mut manager, pane_id, _pty_shared, _surface_shared) =
        manager_with_echo_transaction_process(Vec::new(), transaction_modes(false));

    let outcome = manager
        .send_input_transaction(
            pane_id,
            InputTransaction::insert_text("hello").with_verification(
                InputVerification::EchoContains {
                    needle: "missing".into(),
                    timeout: Duration::ZERO,
                },
            ),
        )
        .expect("transaction should return a structured timeout outcome");

    assert!(!outcome.echoed);
    assert!(outcome.timed_out);
    assert!(matches!(
        outcome.errors.as_slice(),
        [InputTransactionError::VerificationFailed { .. }]
    ));
}

#[test]
fn pane_manager_input_transaction_submit_waits_for_requested_echo_verification() {
    let (mut manager, pane_id, pty_shared, _surface_shared) =
        manager_with_echo_transaction_process(Vec::new(), transaction_modes(false));

    let outcome = manager
        .send_input_transaction(
            pane_id,
            InputTransaction::submit_text("hello").with_verification(
                InputVerification::EchoContains {
                    needle: "hello".into(),
                    timeout: Duration::ZERO,
                },
            ),
        )
        .expect("transaction should return a structured verification outcome");

    assert_eq!(outcome.bytes_sent, 5);
    assert!(!outcome.submitted);
    assert!(outcome.timed_out);
    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"hello".to_vec()],
        "Enter must not be sent when requested pre-submit echo verification fails"
    );
}

#[test]
fn pane_manager_input_transaction_stops_before_submit_when_child_exits() {
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_transaction_process(
        vec![None, Some(exit_frame(1, Some(0)))],
        transaction_modes(false),
    );

    let outcome = manager
        .send_input_transaction(pane_id, InputTransaction::submit_text("hello"))
        .expect("transaction should report child exit structurally");

    assert_eq!(outcome.bytes_sent, 5);
    assert!(!outcome.submitted);
    assert!(outcome.child_exited);
    assert!(matches!(
        outcome.errors.as_slice(),
        [InputTransactionError::ChildExited]
    ));
    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"hello".to_vec()]
    );
}

#[test]
fn pane_manager_input_transaction_retries_transient_write_failures() {
    let (mut manager, pane_id, pty_shared, _surface_shared) =
        manager_with_transaction_process(Vec::new(), transaction_modes(false));
    {
        let mut shared = pty_shared.lock().expect("pty shared lock");
        shared.write_failures =
            VecDeque::from(vec![io::ErrorKind::WouldBlock, io::ErrorKind::Interrupted]);
    }

    let outcome = manager
        .send_input_transaction(pane_id, InputTransaction::raw_bytes(b"x".to_vec()))
        .expect("transaction should retry transient failures");

    let shared = pty_shared.lock().expect("pty shared lock");
    assert_eq!(shared.write_attempts, 3);
    assert_eq!(shared.write_calls, vec![b"x".to_vec()]);
    assert_eq!(outcome.bytes_sent, 1);
    assert!(outcome.errors.is_empty(), "unexpected errors: {outcome:?}");
}

#[test]
fn pane_manager_snapshot_returns_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);
    let err = manager
        .snapshot(pane_id)
        .expect_err("snapshot should fail for unknown pane");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_manager_scrollback_transcript_and_last_seq_return_not_found() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(1);

    let scrollback_err = manager
        .scrollback(pane_id)
        .expect_err("scrollback should fail for unknown pane");
    assert!(
        matches!(scrollback_err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {scrollback_err:?}"
    );

    let transcript_err = manager
        .transcript(pane_id)
        .expect_err("transcript should fail for unknown pane");
    assert!(
        matches!(transcript_err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {transcript_err:?}"
    );

    let seq_err = manager
        .last_seq(pane_id)
        .expect_err("last_seq should fail for unknown pane");
    assert!(
        matches!(seq_err, PaneError::NotFound { pane_id: id } if id == pane_id),
        "expected NotFound error, got {seq_err:?}"
    );
}

#[test]
fn pane_manager_drain_events_is_no_op() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(events.is_empty());
}

#[test]
fn pane_handle_keeps_its_id() {
    let pane_id = PaneId::new(42);
    let handle = PaneHandle::new(pane_id);
    assert_eq!(handle.id, pane_id);
}

#[test]
fn pane_handle_write_bytes_returns_not_found() {
    let handle = PaneHandle::new(PaneId::new(1));
    let err = handle
        .write_bytes(vec![0x61])
        .expect_err("write_bytes should fail in skeleton");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == PaneId::new(1)),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_handle_resize_returns_not_found() {
    let handle = PaneHandle::new(PaneId::new(1));
    let err = handle
        .resize(Size::new(24, 80))
        .expect_err("resize should fail in skeleton");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == PaneId::new(1)),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_handle_kill_returns_not_found() {
    let handle = PaneHandle::new(PaneId::new(1));
    let err = handle
        .kill(KillReason::UserRequested)
        .expect_err("kill should fail in skeleton");
    assert!(
        matches!(err, PaneError::NotFound { pane_id: id } if id == PaneId::new(1)),
        "expected NotFound error, got {err:?}"
    );
}

#[test]
fn pane_exit_variants_are_constructible() {
    let _exited = PaneExit::Exited { code: Some(0) };
    let _killed = PaneExit::Killed {
        reason: KillReason::UserRequested,
    };
    let _failed = PaneExit::Failed {
        error: PaneError::Spawn {
            message: "test".into(),
        },
    };
}

#[test]
fn pane_manager_snapshot_uses_surface_backend_state() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(7);
    manager.insert_surface_for_testing(pane_id, Some("pane title".into()), Box::new(backend));

    let snapshot = manager.snapshot(pane_id).expect("test pane should exist");

    assert_eq!(snapshot.title.as_deref(), Some("pane title"));
    assert_eq!(snapshot.size, Size::new(24, 80));
    assert_eq!(snapshot.surface.title.as_deref(), Some("surface title"));
    assert_eq!(snapshot.surface.rows[0].cells[0].text.as_ref(), "hello");
    assert_eq!(snapshot.cursor.position, Some(CursorPosition::new(2, 5)));
    assert!(snapshot.cursor.visible);
    assert!(snapshot.modes.bracketed_paste);
    assert_eq!(snapshot.modes.mouse, MouseMode::AnyEvent);

    let owned = snapshot.to_owned_snapshot();
    assert_eq!(owned.surface.rows[1].cells[0].text.as_ref(), "world");
    assert_eq!(owned.cursor.position, Some(CursorPosition::new(2, 5)));
}

#[test]
fn pane_manager_resize_delegates_to_surface_backend_and_emits_event() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(9);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    manager
        .resize(pane_id, Size::new(40, 120))
        .expect("resize should delegate to the surface backend");

    assert_eq!(
        shared.lock().expect("surface shared lock").resize_calls,
        vec![Size::new(40, 120)],
        "backend should observe the resize"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("snapshot should reflect resized surface")
            .size,
        Size::new(40, 120)
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        matches!(
            events.as_slice(),
            [PaneEvent {
                kind: PaneEventKind::Resized(ResizedEvent { size }),
                seq: 1,
                ..
            }] if *size == Size::new(40, 120)
        ),
        "expected a single resized event, got {events:?}"
    );
}

#[test]
fn pane_manager_resize_validates_live_surface_before_resizing_running_pty() {
    let process_shared = Arc::new(Mutex::new(FakePtyProcessShared::default()));
    let surface_shared = Arc::new(Mutex::new(TrackingResizeSurfaceShared::default()));
    let process_shared_for_spawn = Arc::clone(&process_shared);
    let surface_shared_for_spawn = Arc::clone(&surface_shared);
    let rejected_size = Size::new(1, 80);

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(TrackingPtyProcess::new(Arc::clone(
                    &process_shared_for_spawn,
                ))) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(TrackingResizeSurfaceBackend::new(
                    Arc::clone(&surface_shared_for_spawn),
                    config.size,
                    Some(rejected_size),
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(PaneConfig::command("fake-command").with_size(Size::new(10, 80)))
        .expect("spawn should succeed with injected runtime");
    let mut _spawn_events = Vec::new();
    manager.drain_events(&mut _spawn_events);

    let err = manager
        .resize(pane_id, rejected_size)
        .expect_err("live surface validation should reject the resize");

    assert_eq!(
        err,
        PaneError::Surface {
            message: "tracking surface rejected rows=1 cols=80".to_string(),
        }
    );
    assert!(
        process_shared
            .lock()
            .expect("tracking pty shared lock")
            .resize_calls
            .is_empty(),
        "PTY resize should not run when live validation rejects the size"
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("tracking surface shared lock")
            .validate_calls,
        vec![rejected_size],
        "the live surface should validate the rejected resize"
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("tracking surface shared lock")
            .resize_calls,
        Vec::<Size>::new(),
        "the live surface must stay untouched when validation rejects the resize"
    );

    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist")
            .size,
        Size::new(10, 80)
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Resized(_))),
        "rejected resize should not emit a Resized event, got {events:?}"
    );
}

#[test]
fn pane_manager_resize_keeps_surface_untouched_when_running_pty_resize_fails() {
    let process_shared = Arc::new(Mutex::new(FakePtyProcessShared::default()));
    let surface_shared = Arc::new(Mutex::new(TrackingResizeSurfaceShared::default()));
    let process_shared_for_spawn = Arc::clone(&process_shared);
    let surface_shared_for_spawn = Arc::clone(&surface_shared);
    let resize_error = PaneError::Io {
        operation: IoOperation::Resize,
        message: "injected resize failure".into(),
    };
    let resize_error_for_spawn = resize_error.clone();

    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(FailingResizePtyProcess::new(
                    Arc::clone(&process_shared_for_spawn),
                    resize_error_for_spawn.clone(),
                )) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(TrackingResizeSurfaceBackend::new(
                    Arc::clone(&surface_shared_for_spawn),
                    config.size,
                    None,
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(PaneConfig::command("fake-command").with_size(Size::new(2, 80)))
        .expect("spawn should succeed with injected runtime");
    let mut _spawn_events = Vec::new();
    manager.drain_events(&mut _spawn_events);

    let err = manager
        .resize(pane_id, Size::new(4, 90))
        .expect_err("PTY resize failure should bubble up");

    assert_eq!(err, resize_error);
    assert_eq!(
        process_shared
            .lock()
            .expect("failing resize pty shared lock")
            .resize_calls,
        vec![Size::new(4, 90)]
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("surface shared lock")
            .validate_calls,
        vec![Size::new(4, 90)],
        "the live surface should validate the resize before the PTY is touched"
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("surface shared lock")
            .resize_calls,
        Vec::<Size>::new(),
        "the live surface must stay untouched when PTY resize fails"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist")
            .size,
        Size::new(2, 80)
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Resized(_))),
        "failed resize should not emit a Resized event, got {events:?}"
    );
}

#[test]
fn pane_manager_feed_surface_exposes_update_metadata_and_scrollback() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let update = SurfaceUpdate {
        dirty_rows: DirtyRows::Range { start: 1, end: 3 },
        cursor_changed: true,
        title_changed: true,
        modes_changed: true,
        scrollback_changed: true,
    };
    let backend = FakeSurfaceBackend::new(Arc::clone(&shared), Size::new(10, 20), update);
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(11);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    let observed = manager
        .feed_surface_for_testing(pane_id, b"\x1b[?2004hhello")
        .expect("feeding bytes should delegate to the backend");
    let scrollback = manager
        .scrollback_for_testing(pane_id)
        .expect("scrollback should be readable for registered panes");

    assert_eq!(observed, update);
    assert_eq!(
        shared.lock().expect("surface shared lock").last_feed,
        vec![b"\x1b[?2004hhello".to_vec()]
    );
    assert_eq!(scrollback.lines[0].text.as_ref(), "older output");

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    assert!(
        matches!(
            events.as_slice(),
            [PaneEvent {
                kind: PaneEventKind::SurfaceChanged(SurfaceChangedEvent {
                    generation: 1,
                    dirty_rows: DirtyRows::Range { start: 1, end: 3 },
                    cursor_changed: true,
                    title_changed: true,
                    modes_changed: true,
                    scrollback_changed: true,
                }),
                seq: 1,
                ..
            }]
        ),
        "expected a single surface-changed event, got {events:?}"
    );
}

#[test]
fn pane_manager_scrollback_reader_slices_retained_lines() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let update = SurfaceUpdate {
        dirty_rows: DirtyRows::Range { start: 0, end: 0 },
        cursor_changed: false,
        title_changed: false,
        modes_changed: false,
        scrollback_changed: true,
    };
    let mut backend = FakeSurfaceBackend::new(Arc::clone(&shared), Size::new(10, 20), update);
    backend.scrollback = vec!["line-1", "line-2", "line-3"];
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(12);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    let scrollback = manager
        .scrollback(pane_id)
        .expect("scrollback should be readable");
    let all_lines = scrollback
        .snapshot()
        .lines
        .iter()
        .map(|line| line.text.as_ref())
        .collect::<Vec<_>>();
    let tail_lines = scrollback
        .lines(1..)
        .iter()
        .map(|line| line.text.as_ref())
        .collect::<Vec<_>>();

    assert_eq!(all_lines, vec!["line-1", "line-2", "line-3"]);
    assert_eq!(tail_lines, vec!["line-2", "line-3"]);
    assert_eq!(
        scrollback.to_owned_snapshot().lines[0].text.as_ref(),
        "line-1"
    );
}

#[test]
fn pane_manager_last_seq_reports_latest_polled_event() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let surface_shared = Arc::clone(&shared);
    let frames = vec![
        PtyFrame::Output {
            seq: 1,
            bytes: b"hello".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Exited {
            seq: 2,
            code: Some(0),
            at: Instant::now(),
        },
    ];
    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(FakePtyProcess {
                    id: "fake-pty".into(),
                    frames: VecDeque::from(frames.clone()),
                }) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared),
                    config.size,
                    SurfaceUpdate {
                        dirty_rows: DirtyRows::Range { start: 0, end: 1 },
                        cursor_changed: true,
                        title_changed: false,
                        modes_changed: false,
                        scrollback_changed: false,
                    },
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(PaneConfig::command("fake-command"))
        .expect("spawn should succeed with injected runtime");

    assert_eq!(
        manager.last_seq(pane_id).expect("pane should exist"),
        6,
        "spawn, running, output, surface, exit, and exited-state events should all count"
    );
}

#[test]
fn pane_manager_attach_detach_happy_path_preserves_manager_state() {
    let embedded_size = Size::new(10, 20);
    let terminal_size = Size::new(40, 100);
    let update = SurfaceUpdate {
        dirty_rows: DirtyRows::All,
        cursor_changed: true,
        title_changed: false,
        modes_changed: false,
        scrollback_changed: false,
    };
    let (mut manager, pane_id, pty_shared, surface_shared) = manager_with_attach_process(
        vec![None, Some(output_frame(1, b"attached output"))],
        embedded_size,
        update,
        None,
    );
    let mut terminal = FakeAttachTerminal::new(terminal_size).with_stdin(vec![vec![0x1d]]);
    let stdout = terminal.stdout();
    let mut control = FakeAttachControl::default();

    let outcome = manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("attach should detach cleanly");

    assert_eq!(outcome.reason, DetachReason::UserChord);
    assert_eq!(outcome.child_exit_code, None);
    assert_eq!(outcome.terminal_size, terminal_size);
    assert_eq!(outcome.restored_size, embedded_size);
    assert!(outcome.remaining_input.is_empty());
    let stdout = stdout.lock().expect("stdout lock");
    assert!(
        stdout
            .first()
            .is_some_and(|chunk| chunk.starts_with(ATTACH_SCREEN_RESET)),
        "attach should render an initial reset-backed viewport, got {stdout:?}"
    );
    assert!(
        stdout.iter().any(|chunk| chunk == b"attached output"),
        "attached output should still forward at tail, got {stdout:?}"
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("surface shared lock")
            .last_feed,
        vec![b"attached output".to_vec()]
    );
    assert_eq!(
        pty_shared.lock().expect("pty shared lock").resize_calls,
        vec![terminal_size, embedded_size]
    );
    assert_eq!(control.restore_calls, 1);

    let snapshot = manager.snapshot(pane_id).expect("pane should remain");
    assert_eq!(snapshot.interaction_mode, PaneInteractionMode::Embedded);
    assert_eq!(snapshot.size, embedded_size);
}

#[test]
fn pane_manager_attach_initially_renders_existing_viewport_after_reset() {
    let scrollback = vec![ScrollbackLine::from_row(
        "history-before-attach",
        SurfaceRow::new(vec![SurfaceCell::new(
            "history-before-attach",
            CellWidth::Single,
            CellStyle::default(),
        )]),
    )];
    let (mut manager, pane_id, _pty_shared, _surface_shared) = manager_with_attach_scrollback(
        vec![None],
        Size::new(10, 24),
        vec![
            vec!["live-0"],
            vec!["live-1"],
            vec!["live-2"],
            vec!["live-3"],
            vec!["live-4"],
            vec!["live-5"],
            vec!["live-6"],
            vec!["live-7"],
            vec!["live-8"],
            vec!["live-9"],
        ],
        scrollback,
    );
    let mut terminal = FakeAttachTerminal::new(Size::new(12, 24))
        .with_stdin(vec![vec![0x1d]])
        .with_stdout_retry_failures(vec![
            io::ErrorKind::WouldBlock,
            io::ErrorKind::Interrupted,
            io::ErrorKind::WouldBlock,
        ]);
    let stdout = terminal.stdout();
    let mut control = FakeAttachControl::default();
    let options = AttachOptions {
        resize: AttachResizePolicy::KeepEmbeddedSize,
        ..AttachOptions::default()
    };

    manager
        .attach_blocking(pane_id, options, &mut terminal, &mut control)
        .expect("attach should render existing state before detach");

    let stdout = stdout.lock().expect("stdout lock");
    let first_chunk = stdout
        .first()
        .expect("attach should write an initial viewport");
    assert!(
        first_chunk.starts_with(ATTACH_SCREEN_RESET),
        "initial attach viewport should begin with screen reset, got {first_chunk:?}"
    );
    let first_render = String::from_utf8_lossy(first_chunk);
    assert!(
            first_render.contains("history-before-attach")
                && first_render.contains("live-0")
                && first_render.contains("live-9"),
            "initial attach viewport should include current scrollback and live snapshot, got {first_render:?}"
        );
}

#[test]
fn pane_manager_attach_detach_returns_trailing_input() {
    let (mut manager, pane_id, _pty_shared, _surface_shared) = manager_with_attach_process(
        vec![None],
        Size::new(10, 20),
        SurfaceUpdate::default(),
        None,
    );
    let mut terminal =
        FakeAttachTerminal::new(Size::new(40, 100)).with_stdin(vec![b"\x1dtrailing".to_vec()]);
    let mut control = FakeAttachControl::default();

    let outcome = manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("attach should return trailing input");

    assert_eq!(outcome.reason, DetachReason::UserChord);
    assert_eq!(outcome.remaining_input, b"trailing".to_vec());
}

#[test]
fn pane_manager_attach_child_exit_detaches_and_updates_state() {
    let (mut manager, pane_id, _pty_shared, _surface_shared) = manager_with_attach_process(
        vec![None, Some(exit_frame(1, Some(7)))],
        Size::new(10, 20),
        SurfaceUpdate::default(),
        None,
    );
    let mut terminal = FakeAttachTerminal::new(Size::new(40, 100));
    let mut control = FakeAttachControl::default();

    let outcome = manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("child exit should end attach cleanly");

    assert_eq!(outcome.reason, DetachReason::ChildExited);
    assert_eq!(outcome.child_exit_code, Some(7));
    let snapshot = manager.snapshot(pane_id).expect("pane should remain");
    assert_eq!(snapshot.state, PaneState::Exited { code: Some(7) });
    assert_eq!(snapshot.interaction_mode, PaneInteractionMode::Embedded);
}

#[test]
fn pane_manager_attach_output_reaches_surface_and_transcript_after_detach() {
    let (mut manager, pane_id, _pty_shared, surface_shared) = manager_with_attach_process(
        vec![None, Some(output_frame(1, b"hello from attach"))],
        Size::new(10, 20),
        SurfaceUpdate {
            dirty_rows: DirtyRows::Range { start: 0, end: 1 },
            cursor_changed: false,
            title_changed: false,
            modes_changed: false,
            scrollback_changed: true,
        },
        None,
    );
    let mut terminal = FakeAttachTerminal::new(Size::new(40, 100)).with_stdin(vec![vec![0x1d]]);
    let mut control = FakeAttachControl::default();

    manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("attach should detach cleanly");

    assert_eq!(
        surface_shared
            .lock()
            .expect("surface shared lock")
            .last_feed,
        vec![b"hello from attach".to_vec()]
    );
    assert_eq!(
        manager
            .plain_transcript(pane_id)
            .expect("transcript should be readable"),
        "hello from attach"
    );
}

#[test]
fn pane_manager_attach_stdout_only_replays_output_to_manager_on_detach() {
    let (mut manager, pane_id, _pty_shared, surface_shared) = manager_with_attach_process(
        vec![None, Some(output_frame(1, b"deferred output"))],
        Size::new(10, 20),
        SurfaceUpdate::default(),
        None,
    );
    let options = AttachOptions {
        output: AttachOutputPolicy::StdoutOnlyThenReplay,
        ..AttachOptions::default()
    };
    let mut terminal = FakeAttachTerminal::new(Size::new(40, 100)).with_stdin(vec![vec![0x1d]]);
    let stdout = terminal.stdout();
    let mut control = FakeAttachControl::default();

    manager
        .attach_blocking(pane_id, options, &mut terminal, &mut control)
        .expect("attach should replay output after detach");

    let stdout = stdout.lock().expect("stdout lock");
    assert!(
        stdout
            .first()
            .is_some_and(|chunk| chunk.starts_with(ATTACH_SCREEN_RESET)),
        "attach should render an initial reset-backed viewport, got {stdout:?}"
    );
    assert!(
        stdout.iter().any(|chunk| chunk == b"deferred output"),
        "deferred output should still forward at tail, got {stdout:?}"
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("surface shared lock")
            .last_feed,
        vec![b"deferred output".to_vec()]
    );
    assert_eq!(
        manager
            .plain_transcript(pane_id)
            .expect("transcript should be readable"),
        "deferred output"
    );
}

#[test]
fn pane_manager_attach_resize_failure_restores_terminal_and_mode() {
    let embedded_size = Size::new(10, 20);
    let terminal_size = Size::new(40, 100);
    let resize_error = PaneError::Io {
        operation: IoOperation::Resize,
        message: "injected attach resize failure".into(),
    };
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_attach_process(
        vec![None],
        embedded_size,
        SurfaceUpdate::default(),
        Some((terminal_size, resize_error.clone())),
    );
    let mut terminal = FakeAttachTerminal::new(terminal_size).with_stdin(vec![vec![0x1d]]);
    let mut control = FakeAttachControl::default();

    let err = manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect_err("initial attach resize should fail");

    assert!(matches!(err, PaneAttachError::PtyResize { .. }));
    assert_eq!(control.restore_calls, 1);
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should remain")
            .interaction_mode,
        PaneInteractionMode::Embedded
    );
    assert_eq!(
        pty_shared.lock().expect("pty shared lock").resize_calls,
        vec![terminal_size, embedded_size]
    );
}

#[test]
fn pane_manager_attach_events_keep_sequence_continuity() {
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_attach_process(
        vec![None],
        Size::new(10, 20),
        SurfaceUpdate::default(),
        None,
    );
    let mut terminal =
        FakeAttachTerminal::new(Size::new(40, 100)).with_stdin(vec![b"a\x1d".to_vec()]);
    let mut control = FakeAttachControl::default();

    manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("attach should detach after forwarding leading input");

    assert_eq!(
        pty_shared.lock().expect("pty shared lock").write_calls,
        vec![b"a".to_vec()]
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    let seqs = events.iter().map(|event| event.seq).collect::<Vec<_>>();
    assert_eq!(
        seqs,
        (1..=events.len() as u64).collect::<Vec<_>>(),
        "pane event sequence should be contiguous: {events:?}"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, PaneEventKind::AttachStarted(_))));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, PaneEventKind::InputSent(_))));
    assert!(events
        .iter()
        .any(|event| matches!(event.kind, PaneEventKind::AttachEnded(_))));
}

#[test]
fn pane_manager_attach_scroll_renders_styled_scrollback_and_clears_host_history() {
    let red_bold = CellStyle {
        fg: Some(ColorSpec::Indexed(1)),
        attrs: CellAttrs {
            bold: true,
            ..CellAttrs::default()
        },
        ..CellStyle::default()
    };
    let scrollback = vec![
        ScrollbackLine::from_row(
            "host history must not appear",
            SurfaceRow::new(vec![SurfaceCell::new(
                "panesmith oldest",
                CellWidth::Single,
                CellStyle::default(),
            )]),
        ),
        ScrollbackLine::from_row(
            "styled red",
            SurfaceRow::new(vec![SurfaceCell::new(
                "styled red",
                CellWidth::Single,
                red_bold,
            )]),
        ),
        ScrollbackLine::from_row(
            "panesmith newer",
            SurfaceRow::new(vec![SurfaceCell::new(
                "panesmith newer",
                CellWidth::Single,
                CellStyle::default(),
            )]),
        ),
    ];
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_attach_scrollback(
        vec![None],
        Size::new(3, 24),
        vec![vec!["live tail"], vec!["prompt"], vec!["cursor"]],
        scrollback,
    );
    let mut terminal =
        FakeAttachTerminal::new(Size::new(3, 24)).with_stdin(vec![b"\x1b[5~".to_vec(), vec![0x1d]]);
    let stdout = terminal.stdout();
    let mut control = FakeAttachControl::default();

    manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("attach should render scrollback and detach");

    let chunks = stdout.lock().expect("stdout lock");
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk.windows(b"\x1b[3J".len()).any(|w| w == b"\x1b[3J")),
        "attach should clear terminal emulator scrollback: {chunks:?}"
    );
    let rendered = String::from_utf8_lossy(&chunks.concat()).into_owned();
    assert!(
        rendered.contains("styled red"),
        "scrolled attach viewport should render Panesmith scrollback, got {rendered:?}"
    );
    assert!(
        rendered.contains("\u{1b}[0;1;38;5;1mstyled red"),
        "styled scrollback cells should preserve SGR style, got {rendered:?}"
    );
    assert!(
        !rendered.contains("host dashboard"),
        "attach renderer must not reveal host dashboard history"
    );
    assert!(
        pty_shared
            .lock()
            .expect("pty shared lock")
            .write_calls
            .is_empty(),
        "PageUp viewport control should not be forwarded to the child PTY"
    );
}

#[test]
fn pane_manager_attach_mouse_wheel_scrolls_owned_scrollback_with_live_rows() {
    let scrollback = ["history-0", "history-1"]
        .into_iter()
        .map(|text| {
            ScrollbackLine::from_row(
                text,
                SurfaceRow::new(vec![SurfaceCell::new(
                    text,
                    CellWidth::Single,
                    CellStyle::default(),
                )]),
            )
        })
        .collect::<Vec<_>>();
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_attach_scrollback(
        vec![None],
        Size::new(3, 24),
        vec![vec!["live-1"], vec!["live-2"], vec!["live-3"]],
        scrollback,
    );
    let mut terminal = FakeAttachTerminal::new(Size::new(4, 24))
        .with_stdin(vec![b"\x1b[<64;12;2M".to_vec(), vec![0x1d]]);
    let stdout = terminal.stdout();
    let mut control = FakeAttachControl::default();

    manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("mouse wheel should scroll the manager-owned attach viewport");

    let rendered =
        String::from_utf8_lossy(&stdout.lock().expect("stdout lock").concat()).into_owned();
    assert!(
        rendered.contains("history-0") && rendered.contains("history-1"),
        "mouse wheel should render retained Panesmith scrollback, got {rendered:?}"
    );
    assert!(
        rendered.contains("live-1") && rendered.contains("live-2"),
        "scrolled viewport should still compose live snapshot rows, got {rendered:?}"
    );
    assert!(
        pty_shared
            .lock()
            .expect("pty shared lock")
            .write_calls
            .is_empty(),
        "scroll-wheel viewport controls should not be forwarded to the child PTY"
    );
}

#[test]
fn pane_manager_attach_preserves_embedded_tui_terminal_profile_across_repeated_cycles() {
    let scrollback = ["history-0", "history-1"]
        .into_iter()
        .map(|text| {
            ScrollbackLine::from_row(
                text,
                SurfaceRow::new(vec![SurfaceCell::new(
                    text,
                    CellWidth::Single,
                    CellStyle::default(),
                )]),
            )
        })
        .collect::<Vec<_>>();
    let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_attach_scrollback(
        vec![None, None, None, None],
        Size::new(3, 24),
        vec![vec!["live-1"], vec!["live-2"], vec!["live-3"]],
        scrollback,
    );
    let mut control = EmbeddedTuiAttachControl::new();
    let ctrl_f_before_attach = control.profile.ctrl_key_bytes(b'f');
    let ctrl_q_before_attach = control.profile.ctrl_key_bytes(b'q');
    assert_eq!(
        control.profile.parse_ctrl_key(&ctrl_f_before_attach),
        Some(b'f')
    );
    assert_eq!(
        control.profile.parse_ctrl_key(&ctrl_q_before_attach),
        Some(b'q')
    );

    for cycle in 1..=2 {
        let mut options = AttachOptions::default();
        options.detach.chord = vec![0x06]; // Ctrl-F
        options.resize = AttachResizePolicy::KeepEmbeddedSize;
        let mut detach_input = ctrl_f_before_attach.clone();
        detach_input.extend_from_slice(format!("after-{cycle}").as_bytes());
        let mut terminal = FakeAttachTerminal::new(Size::new(3, 24))
            .with_stdin(vec![b"\x1b[<64;12;2M".to_vec(), detach_input]);

        let outcome = manager
            .attach_blocking(pane_id, options, &mut terminal, &mut control)
            .unwrap_or_else(|error| panic!("cycle {cycle} should detach cleanly: {error:?}"));

        assert_eq!(outcome.reason, DetachReason::UserChord, "cycle {cycle}");
        assert_eq!(
            outcome.remaining_input,
            format!("after-{cycle}").into_bytes(),
            "cycle {cycle}"
        );
        assert_eq!(
            control.restore_calls, cycle,
            "cycle {cycle} should restore the host terminal once"
        );
        control.assert_dashboard_input_profile(&ctrl_f_before_attach, &ctrl_q_before_attach);
        control.dispatch_parent_mouse_wheel();
        assert_eq!(
            control.parent_scrollback_leaks, 0,
            "cycle {cycle} restored mouse capture before parent input resumed"
        );
    }

    assert_eq!(
        control.suspended,
        vec![
            AttachScreenPolicy::ReuseHostAlternateScreen,
            AttachScreenPolicy::ReuseHostAlternateScreen,
        ]
    );
    assert_eq!(control.parent_mouse_events, 2);
    assert!(
        pty_shared
            .lock()
            .expect("pty shared lock")
            .write_calls
            .is_empty(),
        "attach mouse-wheel input and Ctrl-f detach sequences must not reach the child PTY"
    );
}

#[test]
fn pane_manager_attach_suppresses_output_while_scrolled_and_resumes_at_tail() {
    let scrollback = (0..4)
        .map(|idx| {
            let text = format!("history-{idx}");
            ScrollbackLine::from_row(
                text.clone(),
                SurfaceRow::new(vec![SurfaceCell::new(
                    text,
                    CellWidth::Single,
                    CellStyle::default(),
                )]),
            )
        })
        .collect::<Vec<_>>();
    let (mut manager, pane_id, pty_shared, surface_shared) = manager_with_attach_scrollback(
        vec![
            None,
            Some(output_frame(1, b"hidden while scrolled")),
            None,
            Some(output_frame(2, b"tail live")),
        ],
        Size::new(3, 24),
        vec![vec!["live-1"], vec!["live-2"], vec!["live-3"]],
        scrollback,
    );
    let mut terminal = FakeAttachTerminal::new(Size::new(3, 24)).with_stdin(vec![
        b"\x1b[5~".to_vec(),
        b"\x1b[F".to_vec(),
        vec![0x1d],
    ]);
    let stdout = terminal.stdout();
    let mut control = FakeAttachControl::default();

    manager
        .attach_blocking(
            pane_id,
            AttachOptions::default(),
            &mut terminal,
            &mut control,
        )
        .expect("attach should scroll away, return to tail, and detach");

    let rendered =
        String::from_utf8_lossy(&stdout.lock().expect("stdout lock").concat()).into_owned();
    assert!(
        !rendered.contains("hidden while scrolled"),
        "output produced while scrolled away must not be forwarded to stdout"
    );
    assert!(
        rendered.contains("tail live"),
        "output after returning to tail should resume live stdout forwarding, got {rendered:?}"
    );
    assert_eq!(
        surface_shared
            .lock()
            .expect("surface shared lock")
            .last_feed,
        vec![b"hidden while scrolled".to_vec(), b"tail live".to_vec()],
        "manager-owned surface still receives all output while attached"
    );
    let transcript = manager
        .plain_transcript(pane_id)
        .expect("transcript should keep attached output");
    assert!(
            transcript.contains("hidden while scrolled") && transcript.contains("tail live"),
            "manager-owned transcript should still receive all output while attached, got {transcript:?}"
        );
    assert!(
        pty_shared
            .lock()
            .expect("pty shared lock")
            .write_calls
            .is_empty(),
        "PageUp/End viewport controls should not be forwarded to the child PTY"
    );
}

#[test]
fn pane_manager_attach_detaches_with_custom_ctrl_f_terminal_encodings() {
    let cases: &[(&str, &[u8])] = &[
        ("raw ctrl-f", b"\x06after"),
        ("CSI-u letter modifier", b"\x1b[102;5uafter"),
        ("CSI-u letter modifier event", b"\x1b[102;5:1uafter"),
        ("CSI-u raw control", b"\x1b[6uafter"),
        ("CSI-u raw control modifier", b"\x1b[6;1uafter"),
        ("xterm modifyOtherKeys", b"\x1b[27;5;102~after"),
    ];

    for (name, input) in cases {
        let (mut manager, pane_id, pty_shared, _surface_shared) = manager_with_attach_process(
            vec![None],
            Size::new(3, 24),
            SurfaceUpdate::default(),
            None,
        );
        let mut options = AttachOptions::default();
        options.detach.chord = vec![0x06]; // Ctrl-F
        let mut terminal =
            FakeAttachTerminal::new(Size::new(3, 24)).with_stdin(vec![input.to_vec()]);
        let mut control = FakeAttachControl::default();

        let outcome = manager
            .attach_blocking(pane_id, options, &mut terminal, &mut control)
            .unwrap_or_else(|error| panic!("{name} should detach, got {error:?}"));

        assert_eq!(outcome.reason, DetachReason::UserChord, "{name}");
        assert_eq!(outcome.remaining_input, b"after".to_vec(), "{name}");
        assert!(
            pty_shared
                .lock()
                .expect("pty shared lock")
                .write_calls
                .is_empty(),
            "{name} detach sequence must not be forwarded to the child PTY"
        );
    }
}

#[test]
fn attach_viewport_control_parser_handles_mouse_page_home_and_end() {
    let (forward, actions) =
        split_attach_viewport_controls(b"a\x1b[<64;10;2M\x1b[5~\x1b[H\x1b[F\x1b[<65;10;2Mb");

    assert_eq!(forward, b"ab".to_vec());
    assert_eq!(
        actions,
        vec![
            AttachViewportAction::ScrollUp(3),
            AttachViewportAction::PageUp,
            AttachViewportAction::Home,
            AttachViewportAction::End,
            AttachViewportAction::ScrollDown(3),
        ]
    );
}

#[test]
fn pane_manager_event_subscriptions_and_sink_receive_future_events() {
    use std::sync::mpsc::TryRecvError;

    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let update = SurfaceUpdate {
        dirty_rows: DirtyRows::Range { start: 0, end: 0 },
        cursor_changed: false,
        title_changed: false,
        modes_changed: false,
        scrollback_changed: true,
    };
    let backend = FakeSurfaceBackend::new(Arc::clone(&shared), Size::new(8, 16), update);
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(13);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    manager.set_event_sink(move |event| {
        sink_seen.lock().expect("sink lock").push(event.seq);
    });

    let dropped = manager.subscribe();
    drop(dropped);
    let rx = manager.subscribe();

    manager
        .feed_surface_for_testing(pane_id, b"hello")
        .expect("surface feed should emit a surface event");
    let first = rx
        .recv()
        .expect("subscriber should receive the first event");
    assert!(matches!(first.kind, PaneEventKind::SurfaceChanged(_)));
    assert_eq!(first.seq, 1);
    assert_eq!(*seen.lock().expect("sink lock"), vec![1]);

    manager.clear_event_sink();
    manager
        .feed_surface_for_testing(pane_id, b"world")
        .expect("surface feed should emit a second surface event");
    let second = rx
        .recv()
        .expect("subscriber should receive the second event");
    assert_eq!(second.seq, 2);
    assert_eq!(
        *seen.lock().expect("sink lock"),
        vec![1],
        "clearing the sink should stop callback delivery"
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn pane_manager_output_event_includes_transcript_offset_when_enabled() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate {
            dirty_rows: DirtyRows::Range { start: 0, end: 1 },
            cursor_changed: false,
            title_changed: false,
            modes_changed: false,
            scrollback_changed: false,
        },
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(21);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(crate::TranscriptConfig::new(
            crate::TranscriptMode::RawBytes,
        ));
    }

    manager.feed_output_for_testing(pane_id, b"hello".to_vec());

    let mut events = Vec::new();
    manager.drain_events(&mut events);

    let output_event = events
        .iter()
        .find_map(|e| match &e.kind {
            PaneEventKind::Output(o) => Some(o),
            _ => None,
        })
        .expect("expected an Output event");
    assert_eq!(output_event.bytes_len, 5);
    assert_eq!(output_event.transcript_offset, Some(0));
}

#[test]
fn pane_manager_plain_text_offsets_align_with_transcript_reader_byte_ranges() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(29);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(crate::TranscriptConfig::new(
            crate::TranscriptMode::PlainText,
        ));
    }

    manager.feed_output_for_testing(pane_id, "he日".as_bytes().to_vec());
    manager.feed_output_for_testing(pane_id, b"llo\n".to_vec());

    let mut events = Vec::new();
    manager.drain_events(&mut events);

    let output_offsets = events
        .iter()
        .filter_map(|event| match &event.kind {
            PaneEventKind::Output(output) => output.transcript_offset,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(output_offsets, vec![0, 5]);

    let transcript = manager.transcript(pane_id).expect("pane should exist");
    assert_eq!(transcript.retained_plain_start_offset(), 0);
    assert_eq!(
        transcript.plain_text(output_offsets[0] as usize..output_offsets[1] as usize),
        "he日"
    );
    assert_eq!(
        transcript.plain_text(output_offsets[1] as usize..9),
        "llo\n"
    );
}

#[test]
fn pane_manager_output_event_has_none_transcript_offset_when_disabled() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(22);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    // Override the transcript to disabled by mutating the pane state directly.
    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(crate::TranscriptConfig::new(
            crate::TranscriptMode::Disabled,
        ));
    }

    manager.feed_output_for_testing(pane_id, b"hello".to_vec());

    let mut events = Vec::new();
    manager.drain_events(&mut events);

    let output_event = events
        .iter()
        .find_map(|e| match &e.kind {
            PaneEventKind::Output(o) => Some(o),
            _ => None,
        })
        .expect("expected an Output event");
    assert_eq!(output_event.transcript_offset, None);
}

#[test]
fn pane_manager_emits_transcript_rotated_event_when_limits_exceeded() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(23);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    // Tight transcript limits: 2 lines, small byte budget.
    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(
            crate::TranscriptConfig::new(crate::TranscriptMode::PlainText)
                .with_max_lines(2)
                .with_max_bytes(1024),
        );
    }

    manager.feed_output_for_testing(pane_id, b"line1\n".to_vec());
    manager.feed_output_for_testing(pane_id, b"line2\n".to_vec());
    manager.feed_output_for_testing(pane_id, b"line3\n".to_vec());

    let mut events = Vec::new();
    manager.drain_events(&mut events);

    assert!(
        events.iter().any(|e| matches!(
            e.kind,
            PaneEventKind::TranscriptRotated(TranscriptRotatedEvent {
                chunks_dropped: _,
                ..
            })
        )),
        "expected a TranscriptRotated event when limits are exceeded, got {events:?}"
    );

    let plain = manager
        .plain_transcript(pane_id)
        .expect("pane should exist");
    assert!(
        !plain.contains("line1"),
        "oldest line should have been trimmed"
    );
    assert!(plain.contains("line3"), "newest line should be retained");
}

#[test]
fn pane_manager_raw_and_plain_transcripts_match_recorded_data() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(24);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript =
            Transcript::new(crate::TranscriptConfig::new(crate::TranscriptMode::Both));
    }

    let input = b"\x1b[31mhello\x1b[0m\n";
    manager.feed_output_for_testing(pane_id, input.to_vec());

    let raw = manager.raw_transcript(pane_id).expect("pane should exist");
    assert_eq!(raw, input, "raw transcript should match exact PTY bytes");

    let plain = manager
        .plain_transcript(pane_id)
        .expect("pane should exist");
    assert_eq!(
        plain, "hello\n",
        "plain transcript should strip ANSI sequences"
    );

    let transcript = manager.transcript(pane_id).expect("pane should exist");
    assert_eq!(transcript.retained_raw_start_offset(), 0);
    assert_eq!(transcript.retained_plain_start_offset(), 0);
    assert_eq!(transcript.ansi_bytes(..), input);
    assert_eq!(transcript.ansi_bytes(0..5), &input[..5]);
    assert_eq!(transcript.plain_text(..), "hello\n");
    assert_eq!(transcript.plain_text(1..4), "ell");
}

#[test]
fn pane_manager_transcript_reader_preserves_utf8_byte_ranges() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(28);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript =
            Transcript::new(crate::TranscriptConfig::new(crate::TranscriptMode::Both));
    }

    manager.feed_output_for_testing(pane_id, "he日llo\n".as_bytes().to_vec());

    let transcript = manager.transcript(pane_id).expect("pane should exist");
    assert_eq!(transcript.plain_text(2..5), "日");
    assert_eq!(transcript.plain_text(2..7), "日ll");
    assert_eq!(transcript.plain_text(3..7), "ll");
}

#[test]
fn pane_manager_plain_text_reader_exposes_retained_plain_start_offset() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(30);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(
            crate::TranscriptConfig::new(crate::TranscriptMode::PlainText)
                .with_max_lines(2)
                .with_max_bytes(1024),
        );
    }

    manager.feed_output_for_testing(pane_id, b"line1\n".to_vec());
    manager.feed_output_for_testing(pane_id, b"line2\n".to_vec());
    manager.feed_output_for_testing(pane_id, b"line3\n".to_vec());

    let transcript = manager.transcript(pane_id).expect("pane should exist");
    assert_eq!(transcript.retained_plain_start_offset(), 6);
    let local_start = 6 - transcript.retained_plain_start_offset() as usize;
    let local_end = 12 - transcript.retained_plain_start_offset() as usize;
    assert_eq!(transcript.plain_text(local_start..local_end), "line2\n");
}

#[test]
fn pane_manager_surface_feed_failure_still_records_transcript() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(25);
    manager.insert_surface_for_testing(pane_id, None, Box::new(FailingSurfaceBackend));

    // Enable raw-byte transcript recording.
    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(crate::TranscriptConfig::new(
            crate::TranscriptMode::RawBytes,
        ));
    }

    // Feed output -- the surface will reject the frame, but the transcript
    // must still record the bytes because recording happens first.
    manager.feed_output_for_testing(pane_id, b"preserved".to_vec());

    let raw = manager.raw_transcript(pane_id).expect("pane should exist");
    assert_eq!(
        raw, b"preserved",
        "raw transcript should record bytes even when surface feed fails"
    );

    let mut events = Vec::new();
    manager.drain_events(&mut events);

    assert!(
        events.iter().any(|e| matches!(
            e.kind,
            PaneEventKind::Output(OutputEvent { bytes_len: 9, .. })
        )),
        "expected OutputEvent on surface feed failure, got {events:?}"
    );

    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, PaneEventKind::Error(ErrorEvent { .. }))),
        "expected ErrorEvent on surface feed failure, got {events:?}"
    );
}

#[test]
fn rejected_remove_on_live_pane_does_not_corrupt_pending_transcript() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(26);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    // Enable plain-text transcript recording.
    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(crate::TranscriptConfig::new(
            crate::TranscriptMode::PlainText,
        ));
    }

    // Feed an incomplete CSI -- the ESC [ is held back in pending_plain_prefix.
    manager.feed_output_for_testing(pane_id, b"hello\x1b[".to_vec());
    assert_eq!(
        manager
            .plain_transcript(pane_id)
            .expect("pane should exist"),
        "hello",
        "incomplete CSI should be held back"
    );

    // remove() on a live pane must fail WITHOUT flushing pending bytes.
    let err = manager
        .remove(pane_id)
        .expect_err("remove on live pane should fail");
    assert!(
        matches!(
            err,
            PaneError::InvalidState {
                ref expected,
                ref actual,
            } if expected == "Exited, Failed, or Killed" && actual == "Running"
        ),
        "expected InvalidState for live pane, got {err:?}"
    );

    // The pane is still alive and the pending prefix is intact.
    // Feed the rest of the CSI so it completes and is stripped.
    manager.feed_output_for_testing(pane_id, b"31mred\x1b[0m".to_vec());
    assert_eq!(
        manager
            .plain_transcript(pane_id)
            .expect("pane should exist"),
        "hellored",
        "pending prefix must survive the rejected remove()"
    );
}

#[test]
fn both_mode_rotation_preserves_raw_offset_correlation() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    );
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(27);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    // Both mode with a tight byte limit so rotation happens quickly.
    {
        let pane = manager.panes.get_mut(&pane_id).expect("pane should exist");
        pane.transcript = Transcript::new(
            crate::TranscriptConfig::new(crate::TranscriptMode::Both).with_max_bytes(20),
        );
    }

    let mut cumulative_raw_dropped: u64 = 0;

    // --- Frame 1: raw=9 (ESC[1m12345), plain=5 (12345) ---
    manager.feed_output_for_testing(pane_id, b"\x1b[1m12345".to_vec());
    {
        let mut events = Vec::new();
        manager.drain_events(&mut events);
        let output = events
            .iter()
            .find_map(|e| match &e.kind {
                PaneEventKind::Output(o) => Some(o),
                _ => None,
            })
            .expect("frame 1 should emit Output");
        assert_eq!(output.transcript_offset, Some(0));
        let raw = manager.raw_transcript(pane_id).expect("pane should exist");
        assert_eq!(raw, b"\x1b[1m12345");
        assert_eq!(
            output.transcript_offset.unwrap() - cumulative_raw_dropped,
            0
        );
    }

    // --- Frame 2: raw=9, plain=5. Combined 28 > 20 -> chunk 1 rotated. ---
    manager.feed_output_for_testing(pane_id, b"\x1b[1m67890".to_vec());
    {
        let mut events = Vec::new();
        manager.drain_events(&mut events);
        let rotated = events
            .iter()
            .find_map(|e| match &e.kind {
                PaneEventKind::TranscriptRotated(r) => Some(r),
                _ => None,
            })
            .expect("frame 2 should trigger rotation");
        assert_eq!(rotated.raw_bytes_dropped, 9);
        assert_eq!(rotated.plain_bytes_dropped, 5);
        cumulative_raw_dropped += rotated.raw_bytes_dropped;

        let output = events
            .iter()
            .find_map(|e| match &e.kind {
                PaneEventKind::Output(o) => Some(o),
                _ => None,
            })
            .expect("frame 2 should emit Output");
        // Offset is into the raw buffer; chunk 1 (9 raw bytes) was dropped.
        assert_eq!(output.transcript_offset, Some(9));
        let raw = manager.raw_transcript(pane_id).expect("pane should exist");
        assert_eq!(raw, b"\x1b[1m67890");
        // Effective offset in the current raw buffer: 9 - 9 = 0.
        assert_eq!(
            output.transcript_offset.unwrap() - cumulative_raw_dropped,
            0,
            "frame 2 should start at index 0 after rotation"
        );
    }

    // --- Frame 3: raw=3, plain=3. No rotation yet. ---
    manager.feed_output_for_testing(pane_id, b"abc".to_vec());
    {
        let mut events = Vec::new();
        manager.drain_events(&mut events);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, PaneEventKind::TranscriptRotated(..))),
            "frame 3 should not trigger rotation"
        );
        let output = events
            .iter()
            .find_map(|e| match &e.kind {
                PaneEventKind::Output(o) => Some(o),
                _ => None,
            })
            .expect("frame 3 should emit Output");
        assert_eq!(output.transcript_offset, Some(18)); // 9 + 9
        let raw = manager.raw_transcript(pane_id).expect("pane should exist");
        assert_eq!(raw, b"\x1b[1m67890abc");
        // Effective offset: 18 - 9 = 9, which is where "abc" starts.
        assert_eq!(
            output.transcript_offset.unwrap() - cumulative_raw_dropped,
            9,
            "frame 3 should start at index 9"
        );
    }

    // --- Frame 4: raw=3, plain=3. Combined 26 > 20 -> chunk 2 rotated. ---
    manager.feed_output_for_testing(pane_id, b"xyz".to_vec());
    {
        let mut events = Vec::new();
        manager.drain_events(&mut events);
        let rotated = events
            .iter()
            .find_map(|e| match &e.kind {
                PaneEventKind::TranscriptRotated(r) => Some(r),
                _ => None,
            })
            .expect("frame 4 should trigger rotation");
        assert_eq!(rotated.raw_bytes_dropped, 9);
        assert_eq!(rotated.plain_bytes_dropped, 5);
        cumulative_raw_dropped += rotated.raw_bytes_dropped;

        let output = events
            .iter()
            .find_map(|e| match &e.kind {
                PaneEventKind::Output(o) => Some(o),
                _ => None,
            })
            .expect("frame 4 should emit Output");
        assert_eq!(output.transcript_offset, Some(21)); // 18 + 3
        let raw = manager.raw_transcript(pane_id).expect("pane should exist");
        assert_eq!(raw, b"abcxyz");
        // Effective offset: 21 - 18 = 3, which is where "xyz" starts.
        assert_eq!(
            output.transcript_offset.unwrap() - cumulative_raw_dropped,
            3,
            "frame 4 should start at index 3"
        );
    }
}

#[test]
fn pane_manager_dump_repro_redacts_spawn_metadata_and_keeps_event_history() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let surface_shared = Arc::clone(&shared);
    let frames = vec![
        PtyFrame::Output {
            seq: 1,
            bytes: b"hello".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Exited {
            seq: 2,
            code: Some(0),
            at: Instant::now(),
        },
    ];
    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_pty_spawner(move |_config| {
                Ok(Box::new(FakePtyProcess {
                    id: "fake-pty".into(),
                    frames: VecDeque::from(frames.clone()),
                }) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared),
                    config.size,
                    SurfaceUpdate {
                        dirty_rows: DirtyRows::Range { start: 0, end: 1 },
                        cursor_changed: true,
                        title_changed: false,
                        modes_changed: false,
                        scrollback_changed: false,
                    },
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(
            PaneConfig::command_with_args("fixture-bin", ["--token", "super-secret"])
                .with_size(Size::new(24, 80))
                .with_cwd("/tmp/top-secret")
                .with_env("API_TOKEN", "super-secret")
                .with_transcript(crate::TranscriptConfig::new(crate::TranscriptMode::Both)),
        )
        .expect("spawn should succeed");

    let mut drained = Vec::new();
    manager.drain_events(&mut drained);
    assert!(
        !drained.is_empty(),
        "spawn and runtime events should be drainable before dump"
    );

    manager
        .resize(pane_id, Size::new(30, 90))
        .expect("resize after exit should still update surface history");

    let dump = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect("dump_repro should succeed");

    assert_eq!(dump.spawn_config.program(), "<redacted>");
    assert_eq!(
        dump.spawn_config.args(),
        ["<redacted>".to_string(), "<redacted>".to_string()]
    );
    assert_eq!(
        dump.spawn_config.cwd.as_deref(),
        Some(std::path::Path::new("<redacted>"))
    );
    assert_eq!(
        dump.spawn_config.env.get("API_TOKEN").map(String::as_str),
        Some("<redacted>")
    );
    assert_eq!(
        dump.size_history,
        vec![
            ReproSizeEvent::new(0, Size::new(24, 80)),
            ReproSizeEvent::new(5, Size::new(30, 90))
        ]
    );
    assert_eq!(
        dump.raw_transcript,
        Some(crate::ReproRawTranscript {
            start_offset: 0,
            bytes: b"hello".to_vec(),
        })
    );
    assert!(
        dump.events
            .iter()
            .any(|event| matches!(event.kind, PaneEventKind::Spawned(..))),
        "dump should keep events even after they were drained from the public queue"
    );
    let spawned_programs = dump
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            PaneEventKind::Spawned(SpawnedEvent { program }) => Some(program.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(spawned_programs, vec!["<redacted>"]);
    assert!(
        dump.events.iter().any(|event| matches!(
            event.kind,
            PaneEventKind::Exited(ExitedEvent { code: Some(0) })
        )),
        "dump should contain the exit event"
    );
    assert!(
        dump.backend.name.contains("FakeSurfaceBackend"),
        "expected backend metadata to identify the concrete surface, got {:?}",
        dump.backend
    );
    assert_eq!(dump.final_surface.size, Size::new(30, 90));
}

#[test]
fn pane_manager_dump_repro_returns_not_found_for_unknown_pane() {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let err = manager
        .dump_repro(PaneId::new(999), ReproDumpOptions::default())
        .expect_err("unknown panes should not dump repros");
    assert_eq!(
        err,
        PaneError::NotFound {
            pane_id: PaneId::new(999)
        }
    );
}

#[test]
fn pane_manager_dump_repro_drains_all_queued_frames_before_snapshotting() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let surface_shared = Arc::clone(&shared);
    let frames = vec![
        PtyFrame::Output {
            seq: 1,
            bytes: b"hello ".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Output {
            seq: 2,
            bytes: b"world".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Output {
            seq: 3,
            bytes: b"!\n".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Exited {
            seq: 4,
            code: Some(0),
            at: Instant::now(),
        },
    ];
    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_max_pty_frames_per_drain(1)
            .with_pty_spawner(move |_config| {
                Ok(Box::new(FakePtyProcess {
                    id: "fake-pty".into(),
                    frames: VecDeque::from(frames.clone()),
                }) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared),
                    config.size,
                    SurfaceUpdate::default(),
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(
            PaneConfig::command("fake-command")
                .with_transcript(crate::TranscriptConfig::new(crate::TranscriptMode::Both)),
        )
        .expect("spawn should succeed");

    let dump = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect("dump_repro should drain all queued frames");

    assert_eq!(
        dump.raw_transcript,
        Some(crate::ReproRawTranscript {
            start_offset: 0,
            bytes: b"hello world!\n".to_vec(),
        })
    );
    assert!(
        dump.events.iter().any(|event| matches!(
            event.kind,
            PaneEventKind::Exited(ExitedEvent { code: Some(0) })
        )),
        "dump should include the queued exit event after fully draining frames"
    );
    assert_eq!(
        manager
            .snapshot(pane_id)
            .expect("pane should still exist")
            .state,
        PaneState::Exited { code: Some(0) }
    );
}

#[test]
fn pane_manager_dump_repro_merges_overlay_modes_into_final_surface() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let backend = FakeSurfaceBackend::new(
        Arc::clone(&shared),
        Size::new(24, 80),
        SurfaceUpdate::default(),
    )
    .with_modes(TerminalModes::default());
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = PaneId::new(31);
    manager.insert_surface_for_testing(pane_id, None, Box::new(backend));

    manager.feed_output_for_testing(pane_id, b"\x1b[?1004h\x1b[?1006h".to_vec());

    let dump = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect("dump_repro should succeed");

    assert!(dump.final_surface.modes.focus_events);
    assert_eq!(dump.final_surface.modes.mouse, MouseMode::Sgr);
}

#[test]
fn pane_manager_dump_repro_errors_when_repro_drain_limit_is_exceeded() {
    let shared = Arc::new(Mutex::new(FakeSurfaceShared::default()));
    let surface_shared = Arc::clone(&shared);
    let frames = vec![
        PtyFrame::Output {
            seq: 1,
            bytes: b"one".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Output {
            seq: 2,
            bytes: b"two".to_vec(),
            at: Instant::now(),
        },
        PtyFrame::Output {
            seq: 3,
            bytes: b"three".to_vec(),
            at: Instant::now(),
        },
    ];
    let mut manager = PaneManager::new(
        PaneManagerConfig::default()
            .with_max_pty_frames_per_repro_dump(1)
            .with_pty_spawner(move |_config| {
                Ok(Box::new(FakePtyProcess {
                    id: "fake-pty".into(),
                    frames: VecDeque::from(frames.clone()),
                }) as Box<dyn PtyProcess>)
            })
            .with_surface_factory(move |_pane_id, config| {
                Ok(Box::new(FakeSurfaceBackend::new(
                    Arc::clone(&surface_shared),
                    config.size,
                    SurfaceUpdate::default(),
                )) as Box<dyn SurfaceBackend + Send>)
            }),
    );

    let pane_id = manager
        .spawn(
            PaneConfig::command("fake-command")
                .with_transcript(crate::TranscriptConfig::new(crate::TranscriptMode::Both)),
        )
        .expect("spawn should succeed");

    let err = manager
        .dump_repro(pane_id, ReproDumpOptions::default())
        .expect_err("dump_repro should fail when output never quiesces within the limit");
    assert!(matches!(
        err,
        PaneError::Surface { message }
            if message.contains("frame drain limit (1)")
                && message.contains("queued output")
    ));
}
