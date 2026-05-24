//! Pane event consumption reference example.
//!
//! Drains ordered pane events from the manager and logs a few important kinds.

use std::{thread::sleep, time::Duration};

use panesmith::{PaneConfig, PaneEventKind, PaneManager, PaneManagerConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut manager = PaneManager::new(PaneManagerConfig::default());
    let pane_id = manager.spawn(PaneConfig::shell())?;
    // subscribe() only mirrors events emitted after this call, so Spawned and
    // the initial Running transition from spawn() still arrive through
    // drain_events() below.
    let rx = manager.subscribe();

    manager.write_bytes(pane_id, b"echo panesmith events\r\nexit\r\n")?;
    sleep(Duration::from_millis(150));

    let mut events = Vec::new();
    manager.drain_events(&mut events);
    for event in events {
        match event.kind {
            PaneEventKind::Spawned(spawned) => {
                println!("#{} spawned {}", event.seq, spawned.program)
            }
            PaneEventKind::InputSent(input) => {
                println!(
                    "#{} sent {:?} ({} bytes)",
                    event.seq, input.input_kind, input.bytes_len
                )
            }
            PaneEventKind::Output(output) => {
                println!("#{} output {} bytes", event.seq, output.bytes_len)
            }
            PaneEventKind::Exited(exit) => {
                println!("#{} exited with {:?}", event.seq, exit.code)
            }
            other => println!("#{} {other:?}", event.seq),
        }
    }

    let subscription_events = rx.try_iter().collect::<Vec<_>>();
    println!(
        "subscription observed {} future events",
        subscription_events.len()
    );
    Ok(())
}
