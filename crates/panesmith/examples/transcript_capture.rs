//! Transcript recording reference example.
//!
//! Records both plain-text and raw-byte transcripts from a shell pane, then
//! prints the retained transcript buffers.

use std::{thread::sleep, time::Duration};

use panesmith::{PaneConfig, PaneManager, PaneManagerConfig, TranscriptConfig, TranscriptMode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager
        .spawn(PaneConfig::shell().with_transcript(TranscriptConfig::new(TranscriptMode::Both)))?;

    manager.write_bytes(pane_id, b"echo panesmith transcript\r\nexit\r\n")?;
    sleep(Duration::from_millis(150));
    let transcript = manager.transcript(pane_id)?;

    println!("plain transcript:\n{}", transcript.plain_text(..));
    println!("raw transcript bytes: {}", transcript.ansi_bytes(..).len());
    Ok(())
}
