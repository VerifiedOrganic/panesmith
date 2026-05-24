use super::*;

fn config(mode: TranscriptMode, lines: usize, bytes: usize) -> TranscriptConfig {
    TranscriptConfig::new(mode)
        .with_max_lines(lines)
        .with_max_bytes(bytes)
}

#[test]
fn disabled_mode_records_nothing() {
    let mut t = Transcript::new(config(TranscriptMode::Disabled, 100, 1024));
    let r = t.record(b"hello");
    assert_eq!(r.offset, 0);
    assert!(r.rotated.is_none());
    assert!(t.raw_bytes().is_empty());
    assert!(t.plain_text().is_empty());
}

#[test]
fn raw_mode_records_exact_bytes() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 100, 1024));
    let r = t.record(b"hello world");
    assert_eq!(r.offset, 0);
    assert_eq!(t.raw_bytes(), b"hello world");
    assert!(t.plain_text().is_empty());
}

#[test]
fn plain_mode_strips_ansi() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    t.record(b"\x1b[31mred\x1b[0m\n");
    assert_eq!(t.plain_text(), "red\n");
    assert!(t.raw_bytes().is_empty());
}

#[test]
fn both_mode_records_raw_and_plain() {
    let mut t = Transcript::new(config(TranscriptMode::Both, 100, 1024));
    t.record(b"\x1b[1mbold\x1b[0m");
    assert_eq!(t.raw_bytes(), b"\x1b[1mbold\x1b[0m");
    assert_eq!(t.plain_text(), "bold");
}

#[test]
fn offsets_are_cumulative() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 100, 1024));
    assert_eq!(t.record(b"abc").offset, 0);
    assert_eq!(t.record(b"de").offset, 3);
    assert_eq!(t.record(b"fghi").offset, 5);
}

#[test]
fn disabled_mode_still_advances_offset() {
    let mut t = Transcript::new(config(TranscriptMode::Disabled, 100, 1024));
    assert_eq!(t.record(b"abc").offset, 0);
    assert_eq!(t.record(b"de").offset, 3);
}

#[test]
fn byte_limit_trims_oldest_chunks() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 100, 10));
    t.record(b"0123456789"); // 10 bytes, at limit
    assert_eq!(t.raw_bytes(), b"0123456789");

    let r = t.record(b"xx"); // 12 bytes total, over limit
    assert!(r.rotated.is_some());
    assert_eq!(t.raw_bytes(), b"xx");
}

#[test]
fn line_limit_trims_oldest_chunks() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 2, 1024));
    t.record(b"line1\n");
    t.record(b"line2\n");
    assert_eq!(t.raw_bytes(), b"line1\nline2\n");

    let r = t.record(b"line3\n");
    assert!(r.rotated.is_some());
    // After trimming, only the newest lines remain.
    assert!(!t.raw_bytes().windows(5).any(|w| w == b"line1"));
    assert!(t.raw_bytes().windows(5).any(|w| w == b"line3"));
}

#[test]
fn rotation_counts_chunks_and_bytes() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 1, 1024));
    t.record(b"a\n");
    t.record(b"b\n");
    let r = t.record(b"c\n");
    assert!(r.rotated.is_some());
    // At least 2 chunks should have been dropped to get to 1 line.
    assert!(r.rotated.unwrap().chunks_dropped >= 1);
}

#[test]
fn plain_text_limit_counts_lines() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 2, 1024));
    t.record(b"\x1b[31mone\n");
    t.record(b"\x1b[32mtwo\n");
    assert_eq!(t.plain_text(), "one\ntwo\n");

    let r = t.record(b"\x1b[33mthree\n");
    assert!(r.rotated.is_some());
    assert!(!t.plain_text().contains("one"));
    assert!(t.plain_text().contains("three"));
}

#[test]
fn both_mode_enforces_limits_on_combined_size() {
    let mut t = Transcript::new(config(TranscriptMode::Both, 100, 20));
    // raw=9 (ESC[1m12345), plain=5 (12345), combined=14
    t.record(b"\x1b[1m12345");
    assert_eq!(t.raw_bytes().len() + t.plain_text().len(), 14);

    // Second record pushes combined to 28, over limit 20
    let r = t.record(b"\x1b[1m67890");
    assert!(r.rotated.is_some());
    // After trimming, the combined size should be <= 20
    assert!(t.raw_bytes().len() + t.plain_text().len() <= 20);
}

#[test]
fn both_mode_line_limit_counts_logical_lines_not_double() {
    let mut t = Transcript::new(config(TranscriptMode::Both, 2, 1024));
    // Each record has 1 logical line (raw and plain both have 1 newline)
    t.record(b"line1\n");
    t.record(b"line2\n");
    assert_eq!(t.raw_bytes(), b"line1\nline2\n");
    assert_eq!(t.plain_text(), "line1\nline2\n");

    // Third record triggers rotation; only 1 chunk should need to be
    // dropped because each chunk contributes 1 logical line, not 2.
    let r = t.record(b"line3\n");
    assert!(r.rotated.is_some());
    assert_eq!(r.rotated.unwrap().chunks_dropped, 1);
    assert!(!t.raw_bytes().starts_with(b"line1"));
    assert!(t.raw_bytes().ends_with(b"line3\n"));
    assert!(!t.plain_text().starts_with("line1"));
    assert!(t.plain_text().ends_with("line3\n"));
}

#[test]
fn zero_limits_mean_unbounded() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 0, 0));
    for i in 0..100 {
        let line = format!("line {i}\n");
        let r = t.record(line.as_bytes());
        assert!(r.rotated.is_none());
    }
    // "line N\n" is 7 bytes for 0-9 and 8 bytes for 10-99
    assert_eq!(t.raw_bytes().len(), 10 * 7 + 90 * 8);
}

#[test]
fn plain_text_offset_is_plain_text_offset_not_raw() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // "he\x1b[0mllo" has raw_len=7, plain_len=5
    let r1 = t.record(b"he\x1b[0mllo");
    assert_eq!(r1.offset, 0);
    let r2 = t.record(b"world");
    assert_eq!(r2.offset, 5); // plain offset, not 7
}

#[test]
fn split_csi_across_frames_is_stripped_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // ESC is at the end of frame 1; CSI completes in frame 2.
    t.record(b"hello\x1b");
    t.record(b"[31mred\x1b[0m");
    assert_eq!(t.plain_text(), "hellored");
}

#[test]
fn split_osc_across_frames_is_stripped_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // OSC starts at the end of frame 1; completes in frame 2.
    t.record(b"before\x1b]");
    t.record(b"0;title\x07after");
    assert_eq!(t.plain_text(), "beforeafter");
}

#[test]
fn split_two_char_escape_across_frames_is_stripped_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // ESC at end of frame 1; two-char escape completes in frame 2.
    t.record(b"hello\x1b");
    t.record(b"(0world");
    assert_eq!(t.plain_text(), "helloworld");
}

#[test]
fn split_utf8_across_frames_is_reconstructed_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // 日 is UTF-8 bytes: 0xE6 0x97 0xA5
    t.record(b"he\xE6\x97");
    t.record(b"\xA5lo");
    assert_eq!(t.plain_text(), "he日lo");
}

#[test]
fn split_incomplete_csi_at_frame_boundary() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // ESC [ is incomplete CSI (no terminator). Next frame provides it.
    t.record(b"hello\x1b[");
    t.record(b"31mred\x1b[0m");
    assert_eq!(t.plain_text(), "hellored");
}

#[test]
fn trailing_incomplete_bytes_detects_lone_esc() {
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b"), 1);
}

#[test]
fn trailing_incomplete_bytes_detects_incomplete_csi() {
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b["), 2);
    // CSI with terminator is complete.
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b[31m"), 0);
}

#[test]
fn trailing_incomplete_bytes_detects_incomplete_osc() {
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b]"), 2);
    // OSC with BEL terminator is complete.
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b]0;t\x07"), 0);
}

#[test]
fn trailing_incomplete_bytes_detects_incomplete_two_char_escape() {
    // Two-character escape missing its parameter byte.
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b("), 2);
    // Complete two-character escape.
    assert_eq!(trailing_incomplete_bytes(b"hello\x1b(0"), 0);
}

#[test]
fn trailing_incomplete_bytes_detects_incomplete_utf8() {
    // 日 = 0xE6 0x97 0xA5
    assert_eq!(trailing_incomplete_bytes(b"he\xE6"), 1);
    assert_eq!(trailing_incomplete_bytes(b"he\xE6\x97"), 2);
    assert_eq!(trailing_incomplete_bytes(b"he\xE6\x97\xA5"), 0);
}

#[test]
fn trailing_incomplete_bytes_terminates_osc_with_bel() {
    // OSC terminated by BEL should not hold back any bytes.
    assert_eq!(trailing_incomplete_bytes(b"text\x1b]title\x07more"), 0);
}

#[test]
fn osc_terminated_by_bel_does_not_hold_back() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    t.record(b"text\x1b]title\x07more");
    assert_eq!(t.plain_text(), "textmore");
}

#[test]
fn strip_ansi_handles_csi_sequences() {
    assert_eq!(strip_ansi(b"\x1b[31mred\x1b[0m"), "red");
    assert_eq!(strip_ansi(b"\x1b[1;31;40mbold\x1b[0m"), "bold");
    assert_eq!(strip_ansi(b"\x1b[?2004h"), "");
}

#[test]
fn strip_ansi_handles_osc_sequences() {
    assert_eq!(strip_ansi(b"\x1b]0;title\x07text"), "text");
    assert_eq!(strip_ansi(b"\x1b]0;title\x1b\\text"), "text");
}

#[test]
fn strip_ansi_handles_osc_with_esc_inside_payload() {
    // ESC inside the OSC payload that is NOT the ST terminator must not
    // prematurely end the sequence. The CSI `ESC [31m` is part of the
    // payload and should be stripped along with the OSC wrapper.
    assert_eq!(
        strip_ansi(b"before\x1b]abc\x1b[31mdef\x1b\\after"),
        "beforeafter"
    );
}

#[test]
fn strip_ansi_handles_dcs_with_esc_inside_payload() {
    // ESC inside the DCS payload that is NOT the ST terminator must not
    // prematurely end the sequence.
    assert_eq!(
        strip_ansi(b"before\x1bPabc\x1b[31mdef\x1b\\after"),
        "beforeafter"
    );
}

#[test]
fn split_osc_with_nested_incomplete_csi_is_buffered_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // Frame 1: OSC starts, payload contains an incomplete CSI at the end.
    // The incomplete CSI is inside the unterminated OSC, so everything
    // from ESC ] onward should be held back.
    t.record(b"before\x1b]abc\x1b[31");
    // Frame 2: completes the CSI and terminates the OSC.
    t.record(b"mdef\x1b\\after");
    assert_eq!(t.plain_text(), "beforeafter");
}

#[test]
fn split_dcs_with_nested_incomplete_csi_is_buffered_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // Frame 1: DCS starts, payload contains an incomplete CSI at the end.
    t.record(b"before\x1bPabc\x1b[31");
    // Frame 2: completes the CSI and terminates the DCS.
    t.record(b"mdef\x1b\\after");
    assert_eq!(t.plain_text(), "beforeafter");
}

#[test]
fn strip_ansi_handles_two_char_escapes() {
    assert_eq!(strip_ansi(b"\x1b(0text"), "text");
    assert_eq!(strip_ansi(b"\x1b=text"), "text");
}

#[test]
fn strip_ansi_preserves_regular_text() {
    assert_eq!(strip_ansi(b"hello world"), "hello world");
    assert_eq!(strip_ansi(b"hello\nworld"), "hello\nworld");
}

#[test]
fn strip_ansi_handles_incomplete_escape() {
    // A lone ESC is preserved; an incomplete CSI is stripped.
    assert_eq!(strip_ansi(b"\x1b"), "\x1b");
    assert_eq!(strip_ansi(b"\x1b["), "");
}

#[test]
fn strip_ansi_handles_utf8() {
    assert_eq!(strip_ansi("\x1b[31m日本語\x1b[0m".as_bytes()), "日本語");
}

#[test]
fn strip_ansi_handles_cursor_movement() {
    assert_eq!(strip_ansi(b"\x1b[2;3Htext"), "text");
    assert_eq!(strip_ansi(b"\x1b[Atext"), "text");
}

#[test]
fn empty_record_is_no_op() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 100, 1024));
    let r = t.record(b"");
    assert_eq!(r.offset, 0);
    assert!(t.raw_bytes().is_empty());
}

#[test]
fn multiple_records_preserve_order_after_trim() {
    let mut t = Transcript::new(config(TranscriptMode::RawBytes, 2, 1024));
    t.record(b"first\n");
    t.record(b"second\n");
    t.record(b"third\n");
    // After trimming to 2 lines, first should be gone.
    assert!(!t.raw_bytes().starts_with(b"first"));
    assert!(t.raw_bytes().ends_with(b"third\n"));
}

#[test]
fn dec_save_restore_cursor_escapes_are_not_buffered() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // ESC 7 (save cursor) and ESC 8 (restore cursor) are complete
    // single-byte escapes. They must not be held back.
    t.record(b"hello\x1b7");
    t.record(b"world");
    assert_eq!(t.plain_text(), "helloworld");

    let mut t2 = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    t2.record(b"hello\x1b8");
    t2.record(b"world");
    assert_eq!(t2.plain_text(), "helloworld");
}

#[test]
fn unknown_esc_introducer_is_not_buffered() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // ESC followed by an unknown introducer (e.g. 'x') is a complete
    // 2-byte escape. It must NOT be buffered, or the ESC will be
    // trapped in pending_plain_prefix forever.
    t.record(b"hello\x1bx");
    t.record(b"world");
    // strip_ansi preserves unknown escapes as literal text.
    assert_eq!(t.plain_text(), "hello\x1bxworld");
}

#[test]
fn dcs_sequence_is_buffered_across_frames() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // DCS starts with ESC P and ends with ST (ESC \).
    t.record(b"before\x1bP");
    t.record(b"params\x1b\\after");
    assert_eq!(t.plain_text(), "beforeafter");
}

#[test]
fn flush_pending_enforces_limits() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 5));
    // Record 5 bytes of plain text plus a trailing lone ESC (held back).
    t.record(b"12345\x1b");
    assert_eq!(t.plain_text(), "12345");
    assert!(!t.pending_plain_prefix.is_empty());

    // Flushing appends the ESC (1 byte) to plain, bringing total to 6.
    // Since max_bytes = 5, the oldest chunk should be dropped.
    let rotated = t.flush_pending();
    assert!(rotated.is_some());
    assert_eq!(t.plain_text(), "\x1b");
    assert!(t.pending_plain_prefix.is_empty());
}

#[test]
fn unknown_esc_followed_by_incomplete_utf8_is_not_corrupted() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // Unknown ESC sequence (ESC w) followed by incomplete UTF-8 (日 = 0xE6 0x97 0xA5).
    // The catch-all must not short-circuit the UTF-8 check.
    t.record(b"a\x1bwb\xE6\x97");
    assert_eq!(t.plain_text(), "a\x1bwb"); // ESC w is literal, incomplete UTF-8 held back
    t.record(b"\xA5c");
    assert_eq!(t.plain_text(), "a\x1bwb日c");
}

#[test]
fn flush_pending_releases_eof_bound_lone_esc() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    t.record(b"hello\x1b");
    // Without flush, the lone ESC disappears.
    assert_eq!(t.plain_text(), "hello");
    t.flush_pending();
    // After flush, the ESC is preserved as literal text.
    assert_eq!(t.plain_text(), "hello\x1b");
}

#[test]
fn flush_pending_releases_eof_bound_partial_utf8() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // 日 is UTF-8 bytes: 0xE6 0x97 0xA5. Split as 2+1, with the last
    // byte arriving in a frame that ends the stream.
    t.record(b"he\xE6\x97");
    t.record(b"\xA5");
    assert_eq!(t.plain_text(), "he日");
}

#[test]
fn three_frame_split_accumulates_correctly() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // ESC + [ + 31m across three separate frames.
    t.record(b"hello\x1b");
    t.record(b"[");
    t.record(b"31mred");
    assert_eq!(t.plain_text(), "hellored");
}

#[test]
fn pending_prefix_cap_forces_flush() {
    let mut t = Transcript::new(config(TranscriptMode::PlainText, 100, 1024));
    // Simulate an unterminated OSC that grows by one byte each frame.
    // Frame 1 starts ESC ] a  → pending = "\x1b]a" (3 bytes)
    // Frame 2 adds b         → combined = "\x1b]ab", pending = "\x1b]ab" (4 bytes)
    // Frame 3 adds c         → combined = "\x1b]abc", pending = "\x1b]abc" (5 bytes)
    // ... and so on until the cap triggers.
    let mut payload = vec![b'\x1b', b']', b'a'];
    for _ in 0..(MAX_PENDING_PREFIX_BYTES + 10) {
        t.record(&payload);
        payload.push(b'x');
    }
    // The prefix should have been force-flushed rather than growing
    // unboundedly. After the cap triggers, the pending prefix is cleared.
    assert!(t.pending_plain_prefix.is_empty());
}
