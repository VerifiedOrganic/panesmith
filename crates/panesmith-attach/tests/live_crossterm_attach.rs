#![cfg(all(feature = "crossterm", unix))]

use std::env;
use std::io;

use panesmith_attach::{
    AttachOptions, BlockingAttachSession, CrosstermTerminalControl, StdioAttachTerminal,
};
use panesmith_core::{
    DetachReason, PaneConfig, PaneId, PortablePtyBackend, PtyBackend, PtyProcess, Size,
};
use panesmith_vt100::Vt100Backend;

#[test]
#[ignore = "requires a real terminal and manual interaction"]
fn manual_live_shell_attach_restores_terminal() {
    if env::var_os("PANESMITH_RUN_MANUAL_ATTACH").is_none() {
        eprintln!("skipping manual attach smoke test; set PANESMITH_RUN_MANUAL_ATTACH=1 to run");
        return;
    }

    let shell = env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let embedded_size = Size::new(24, 80);
    let mut process = PortablePtyBackend
        .spawn(&PaneConfig::command(shell).with_size(embedded_size))
        .expect("shell should spawn for manual attach");
    let mut surface = Vt100Backend::new(PaneId::new(9001), embedded_size)
        .expect("surface backend should initialize");
    let mut terminal = StdioAttachTerminal::new(io::stdout())
        .expect("manual attach requires a readable TTY stdin");
    let mut control = CrosstermTerminalControl::new(io::stdout());
    let mut session = BlockingAttachSession::new(
        PaneId::new(9001),
        AttachOptions::default(),
        embedded_size,
        0,
    );

    eprintln!("manual attach smoke test: interact with the shell, then press Ctrl-] to detach");
    let outcome = session
        .run(&mut terminal, &mut process, &mut surface, &mut control)
        .expect("manual attach should detach cleanly");

    assert_eq!(outcome.reason, DetachReason::UserChord);
    let _ = process.kill();
}
