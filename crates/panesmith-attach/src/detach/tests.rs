use super::*;

fn make_config(chord: Vec<u8>, timeout_ms: u64) -> DetachConfig {
    let mut cfg = DetachConfig::default();
    cfg.chord = chord;
    cfg.partial_timeout = Duration::from_millis(timeout_ms);
    cfg
}

// ---------- Single-byte chord (Ctrl-]) ----------

#[test]
fn ctrl_bracket_detach_immediate() {
    let config = make_config(vec![0x1d], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(matcher.feed_byte(0x1d, now), MatchResult::Detach);
    assert!(!matcher.is_holding());
}

#[test]
fn ctrl_bracket_forwards_other_bytes() {
    let config = make_config(vec![0x1d], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(
        matcher.feed_byte(0x61, now),
        MatchResult::Forward(vec![0x61])
    );
    assert_eq!(
        matcher.feed_byte(0x0d, now),
        MatchResult::Forward(vec![0x0d])
    );
    assert!(!matcher.is_holding());
}

#[test]
fn ctrl_bracket_feed_bytes_chunk() {
    let config = make_config(vec![0x1d], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    let result = matcher.feed_bytes(&[0x61, 0x1d, 0x62], now);
    assert_eq!(result.forward, vec![0x61]);
    assert!(result.detached);
    assert_eq!(result.remaining, &[0x62]);
}

#[test]
fn ctrl_f_common_terminal_encodings_detach_without_forwarding() {
    let cases: &[(&str, &[u8])] = &[
        ("raw ctrl-f", b"\x06after"),
        ("CSI-u letter modifier", b"\x1b[102;5uafter"),
        ("CSI-u letter modifier event", b"\x1b[102;5:1uafter"),
        ("CSI-u raw control", b"\x1b[6uafter"),
        ("CSI-u raw control modifier", b"\x1b[6;1uafter"),
        ("xterm modifyOtherKeys", b"\x1b[27;5;102~after"),
    ];

    for (name, input) in cases {
        let config = make_config(vec![0x06], 500);
        let mut matcher = DetachMatcher::new(&config);
        let result = matcher.feed_bytes(input, Instant::now());

        assert_eq!(
            result.forward,
            Vec::<u8>::new(),
            "{name} should not forward"
        );
        assert!(result.detached, "{name} should detach");
        assert_eq!(
            result.remaining, b"after",
            "{name} should preserve trailing input"
        );
    }
}

// ---------- Multi-byte chord (Ctrl-A d) ----------

#[test]
fn ctrl_a_d_detach_on_full_sequence() {
    let config = make_config(vec![0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, now), MatchResult::Forward(vec![]));
    assert!(matcher.is_holding());
    assert_eq!(matcher.held_bytes(), &[0x01]);

    assert_eq!(matcher.feed_byte(0x64, now), MatchResult::Detach);
    assert!(!matcher.is_holding());
}

#[test]
fn ctrl_a_x_forwards_both_bytes() {
    let config = make_config(vec![0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, now), MatchResult::Forward(vec![]));
    assert_eq!(
        matcher.feed_byte(0x78, now),
        MatchResult::Forward(vec![0x01, 0x78])
    );
    assert!(!matcher.is_holding());
}

#[test]
fn ctrl_a_timeout_forwards_ctrl_a() {
    let config = make_config(vec![0x01, 0x64], 100);
    let mut matcher = DetachMatcher::new(&config);
    let start = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, start), MatchResult::Forward(vec![]));
    assert!(matcher.is_holding());

    let after_timeout = start + Duration::from_millis(150);
    let timed_out = matcher.check_timeout(after_timeout);
    assert_eq!(timed_out, Some(vec![0x01]));
    assert!(!matcher.is_holding());
}

#[test]
fn multi_byte_feed_bytes_chunk() {
    let config = make_config(vec![0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    let result = matcher.feed_bytes(&[0x01, 0x64], now);
    assert_eq!(result.forward, vec![]);
    assert!(result.detached);
    assert!(result.remaining.is_empty());
    assert!(!matcher.is_holding());
}

#[test]
fn multi_byte_feed_bytes_with_mismatch() {
    let config = make_config(vec![0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    let result = matcher.feed_bytes(&[0x01, 0x78, 0x62], now);
    assert_eq!(result.forward, vec![0x01, 0x78, 0x62]);
    assert!(!result.detached);
    assert!(result.remaining.is_empty());
}

// ---------- Edge cases ----------

#[test]
fn empty_chord_forwards_everything() {
    let config = make_config(vec![], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(
        matcher.feed_byte(0x1d, now),
        MatchResult::Forward(vec![0x1d])
    );
    assert_eq!(
        matcher.feed_byte(0x01, now),
        MatchResult::Forward(vec![0x01])
    );
}

#[test]
fn restart_partial_on_mismatch_when_byte_matches_start() {
    // Chord: Ctrl-A d  ([0x01, 0x64])
    // Input: Ctrl-A Ctrl-A d
    // First Ctrl-A is held. Second Ctrl-A mismatches (expected d), so the
    // first Ctrl-A is forwarded. The second Ctrl-A matches chord[0], so it
    // starts a new partial match. The final d completes it.
    let config = make_config(vec![0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    let result = matcher.feed_bytes(&[0x01, 0x01, 0x64], now);
    assert_eq!(result.forward, vec![0x01]);
    assert!(result.detached);
    assert!(result.remaining.is_empty());
}

#[test]
fn feed_bytes_flushes_timeout_before_processing() {
    let config = make_config(vec![0x01, 0x64], 100);
    let mut matcher = DetachMatcher::new(&config);
    let start = Instant::now();

    matcher.feed_byte(0x01, start);

    let after_timeout = start + Duration::from_millis(200);
    let result = matcher.feed_bytes(&[0x62], after_timeout);
    // The timed-out Ctrl-A is forwarded, then the new byte is forwarded.
    assert_eq!(result.forward, vec![0x01, 0x62]);
    assert!(!result.detached);
    assert!(result.remaining.is_empty());
}

#[test]
fn three_byte_chord_detaches() {
    let config = make_config(vec![0x01, 0x02, 0x03], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, now), MatchResult::Forward(vec![]));
    assert_eq!(matcher.feed_byte(0x02, now), MatchResult::Forward(vec![]));
    assert_eq!(matcher.feed_byte(0x03, now), MatchResult::Detach);
}

#[test]
fn held_bytes_cleared_after_detach() {
    let config = make_config(vec![0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    matcher.feed_byte(0x01, now);
    matcher.feed_byte(0x64, now);
    assert_eq!(matcher.held_bytes(), &[]);
    assert!(!matcher.is_holding());
}

#[test]
fn overlapping_chord_fallback_detaches_on_suffix_match() {
    // Regression test for KMP-style fallback on mismatch.
    // Chord: [0x01, 0x01, 0x64]
    // Input: 0x01 0x01 0x01 0x64
    // The first two bytes match. The third 0x01 mismatches (expected 0x64).
    // The suffix [0x01, 0x01] of [0x01, 0x01, 0x01] is also a prefix of the
    // chord, so only the first 0x01 should be forwarded. The last two bytes
    // complete the chord.
    let config = make_config(vec![0x01, 0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, now), MatchResult::Forward(vec![]));
    assert_eq!(matcher.feed_byte(0x01, now), MatchResult::Forward(vec![]));
    assert_eq!(
        matcher.feed_byte(0x01, now),
        MatchResult::Forward(vec![0x01])
    );
    assert!(matcher.is_holding());
    assert_eq!(matcher.held_bytes(), &[0x01, 0x01]);

    assert_eq!(matcher.feed_byte(0x64, now), MatchResult::Detach);
    assert!(!matcher.is_holding());
}

#[test]
fn overlapping_chord_feed_bytes_variant() {
    let config = make_config(vec![0x01, 0x01, 0x64], 500);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    let result = matcher.feed_bytes(&[0x01, 0x01, 0x01, 0x64], now);
    assert_eq!(result.forward, vec![0x01]);
    assert!(result.detached);
    assert!(result.remaining.is_empty());
}

#[test]
fn overlapping_chord_timeout_uses_retained_suffix_timestamp() {
    let config = make_config(vec![0x01, 0x01, 0x64], 100);
    let mut matcher = DetachMatcher::new(&config);
    let start = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, start), MatchResult::Forward(vec![]));
    assert_eq!(
        matcher.feed_byte(0x01, start + Duration::from_millis(80)),
        MatchResult::Forward(vec![])
    );
    assert_eq!(
        matcher.feed_byte(0x01, start + Duration::from_millis(90)),
        MatchResult::Forward(vec![0x01])
    );
    assert_eq!(matcher.held_bytes(), &[0x01, 0x01]);

    let result = matcher.feed_bytes(&[0x64], start + Duration::from_millis(185));
    assert_eq!(result.forward, vec![0x01, 0x01, 0x64]);
    assert!(!result.detached);
    assert!(result.remaining.is_empty());
    assert!(!matcher.is_holding());
}

#[test]
fn feed_byte_flushes_timeout_before_processing() {
    let config = make_config(vec![0x01, 0x64], 100);
    let mut matcher = DetachMatcher::new(&config);
    let start = Instant::now();

    assert_eq!(matcher.feed_byte(0x01, start), MatchResult::Forward(vec![]));
    assert_eq!(
        matcher.feed_byte(0x64, start + Duration::from_millis(150)),
        MatchResult::Forward(vec![0x01, 0x64])
    );
    assert!(!matcher.is_holding());
}

#[test]
fn check_timeout_without_hold_returns_none() {
    let config = make_config(vec![0x01, 0x64], 100);
    let mut matcher = DetachMatcher::new(&config);
    let now = Instant::now();

    assert_eq!(matcher.check_timeout(now), None);
}

#[test]
fn check_timeout_before_deadline_returns_none() {
    let config = make_config(vec![0x01, 0x64], 100);
    let mut matcher = DetachMatcher::new(&config);
    let start = Instant::now();

    matcher.feed_byte(0x01, start);
    assert_eq!(
        matcher.check_timeout(start + Duration::from_millis(50)),
        None
    );
    assert!(matcher.is_holding());
}
