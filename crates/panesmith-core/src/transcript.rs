//! Transcript recording for pane PTY output.
//!
//! The [`Transcript`] captures raw bytes and/or plain text before lossy
//! surface parsing, enforces configurable line and byte limits, and reports
//! rotation metadata so callers can emit events.

use std::borrow::Cow;
use std::collections::VecDeque;

use crate::{TranscriptConfig, TranscriptMode};

/// Maximum bytes to hold back in the pending plain-text prefix buffer.
///
/// This bounds memory for pathological cases like an unterminated OSC
/// sequence split across many frames.
const MAX_PENDING_PREFIX_BYTES: usize = 1024;

/// Captures PTY output in configurable modes with bounded retention.
///
/// Limits are enforced by dropping the oldest chunks first. A limit of
/// `0` means unbounded for that dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    mode: TranscriptMode,
    raw: Vec<u8>,
    plain: String,
    chunks: VecDeque<Chunk>,
    max_bytes: usize,
    max_lines: usize,
    next_raw_offset: u64,
    next_plain_offset: u64,
    total_lines: usize,
    /// Bytes held back from the previous frame because they may be part of
    /// an incomplete ANSI escape sequence or incomplete UTF-8 multi-byte
    /// sequence. They are prepended to the next frame before stripping.
    pending_plain_prefix: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chunk {
    raw_len: usize,
    plain_len: usize,
    lines: usize,
}

/// Result of recording a single output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRecord {
    /// Absolute byte offset where this frame starts.
    ///
    /// For [`TranscriptMode::PlainText`] this is the offset into the plain
    /// text buffer; for all other modes it is the offset into the raw byte
    /// buffer.
    pub offset: u64,
    /// Rotation metadata if old chunks were dropped.
    pub rotated: Option<TranscriptRotation>,
}

/// Metadata about chunks dropped during limit enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptRotation {
    /// Number of chunks dropped.
    pub chunks_dropped: u64,
    /// Total bytes dropped (raw + plain).
    pub bytes_dropped: u64,
    /// Raw bytes dropped from the raw transcript buffer.
    pub raw_bytes_dropped: u64,
    /// Plain bytes dropped from the plain transcript buffer.
    pub plain_bytes_dropped: u64,
}

impl Transcript {
    /// Creates a new transcript with the given configuration.
    ///
    /// Limits are taken from `config.max_lines` and `config.max_bytes`.
    /// A value of `0` disables that limit.
    pub fn new(config: TranscriptConfig) -> Self {
        Self {
            mode: config.mode,
            raw: Vec::new(),
            plain: String::new(),
            chunks: VecDeque::new(),
            max_bytes: config.max_bytes,
            max_lines: config.max_lines,
            next_raw_offset: 0,
            next_plain_offset: 0,
            total_lines: 0,
            pending_plain_prefix: Vec::new(),
        }
    }

    /// Records a byte slice and returns its transcript offset.
    ///
    /// Depending on the configured mode, this appends raw bytes, plain text,
    /// or both. After appending, line and byte limits are enforced.
    pub fn record(&mut self, bytes: &[u8]) -> TranscriptRecord {
        if self.mode == TranscriptMode::Disabled || bytes.is_empty() {
            let offset = self.next_raw_offset;
            self.next_raw_offset += bytes.len() as u64;
            return TranscriptRecord {
                offset,
                rotated: None,
            };
        }

        let raw_len = if self.mode == TranscriptMode::RawBytes || self.mode == TranscriptMode::Both
        {
            self.raw.extend_from_slice(bytes);
            bytes.len()
        } else {
            0
        };

        let (plain_len, lines) =
            if self.mode == TranscriptMode::PlainText || self.mode == TranscriptMode::Both {
                let combined = if self.pending_plain_prefix.is_empty() {
                    Cow::Borrowed(bytes)
                } else {
                    let mut buf = self.pending_plain_prefix.clone();
                    buf.extend_from_slice(bytes);
                    Cow::Owned(buf)
                };

                let hold_back = trailing_incomplete_bytes(&combined);
                let to_strip = if hold_back > 0 {
                    &combined[..combined.len() - hold_back]
                } else {
                    &combined
                };

                let stripped = strip_ansi(to_strip);
                let stripped_lines = count_newlines(stripped.as_bytes());
                let len = stripped.len();
                self.plain.push_str(&stripped);

                // Update the pending prefix for the next frame.
                if hold_back > 0 {
                    self.pending_plain_prefix = combined[combined.len() - hold_back..].to_vec();
                } else {
                    self.pending_plain_prefix.clear();
                }

                // Bound the pending prefix so an unterminated escape sequence
                // cannot grow without limit.
                if self.pending_plain_prefix.len() > MAX_PENDING_PREFIX_BYTES {
                    // strip_ansi drops unterminated escape sequences, so the
                    // forced flush may lose trailing incomplete escapes rather
                    // than preserving them as literal text.
                    let forced = strip_ansi(&self.pending_plain_prefix);
                    self.plain.push_str(&forced);
                    self.pending_plain_prefix.clear();
                }

                let chunk_lines = if self.mode == TranscriptMode::Both {
                    // In Both mode, raw and plain represent the same logical
                    // content. Use the maximum so that line limits are driven
                    // by whichever representation has more newlines (e.g.
                    // when ANSI sequences span line boundaries).
                    count_newlines(bytes).max(stripped_lines)
                } else {
                    stripped_lines
                };

                (len, chunk_lines)
            } else {
                let raw_lines = count_newlines(bytes);
                (0, raw_lines)
            };

        let offset = if self.mode == TranscriptMode::PlainText {
            self.next_plain_offset
        } else {
            self.next_raw_offset
        };

        self.next_raw_offset += bytes.len() as u64;
        self.next_plain_offset += plain_len as u64;
        self.total_lines += lines;

        self.chunks.push_back(Chunk {
            raw_len,
            plain_len,
            lines,
        });

        let rotated = self.enforce_limits();

        TranscriptRecord { offset, rotated }
    }

    /// Returns the raw byte transcript, if recorded.
    pub fn raw_bytes(&self) -> &[u8] {
        &self.raw
    }

    /// Returns the plain text transcript, if recorded.
    pub fn plain_text(&self) -> &str {
        &self.plain
    }

    /// Returns the configured transcript mode.
    pub fn mode(&self) -> TranscriptMode {
        self.mode
    }

    /// Returns the absolute raw-stream offset that will be assigned next.
    pub fn next_raw_offset(&self) -> u64 {
        self.next_raw_offset
    }

    /// Returns the absolute raw-stream offset of the first retained byte.
    pub fn retained_raw_start_offset(&self) -> u64 {
        self.next_raw_offset.saturating_sub(self.raw.len() as u64)
    }

    /// Returns the absolute plain-text-stream offset of the first retained
    /// plain-text byte.
    pub fn retained_plain_start_offset(&self) -> u64 {
        self.next_plain_offset
            .saturating_sub(self.plain.len() as u64)
    }

    /// Returns `true` if raw PTY bytes are retained.
    pub fn records_raw_bytes(&self) -> bool {
        matches!(self.mode, TranscriptMode::RawBytes | TranscriptMode::Both)
    }

    fn enforce_limits(&mut self) -> Option<TranscriptRotation> {
        if self.max_bytes == 0 && self.max_lines == 0 {
            return None;
        }

        let mut chunks_dropped: u64 = 0;
        let mut raw_bytes_dropped: u64 = 0;
        let mut plain_bytes_dropped: u64 = 0;
        let mut raw_drop: usize = 0;
        let mut plain_drop: usize = 0;
        let mut remaining_raw = self.raw.len();
        let mut remaining_plain = self.plain.len();
        let mut remaining_lines = self.total_lines;

        while let Some(_chunk) = self.chunks.front() {
            let over_bytes =
                self.max_bytes > 0 && (remaining_raw + remaining_plain) > self.max_bytes;
            let over_lines = self.max_lines > 0 && remaining_lines > self.max_lines;

            if !over_bytes && !over_lines {
                break;
            }

            let chunk = self.chunks.pop_front().expect("chunk exists");
            chunks_dropped += 1;
            raw_bytes_dropped += chunk.raw_len as u64;
            plain_bytes_dropped += chunk.plain_len as u64;
            raw_drop += chunk.raw_len;
            plain_drop += chunk.plain_len;
            remaining_raw -= chunk.raw_len;
            remaining_plain -= chunk.plain_len;
            remaining_lines -= chunk.lines;
        }

        if chunks_dropped > 0 {
            if raw_drop > 0 {
                let drop_len = raw_drop.min(self.raw.len());
                self.raw.drain(..drop_len);
            }
            if plain_drop > 0 {
                let drop_len = plain_drop.min(self.plain.len());
                self.plain.drain(..drop_len);
            }
            self.total_lines = remaining_lines;

            Some(TranscriptRotation {
                chunks_dropped,
                bytes_dropped: raw_bytes_dropped + plain_bytes_dropped,
                raw_bytes_dropped,
                plain_bytes_dropped,
            })
        } else {
            None
        }
    }

    /// Drains any pending plain-text prefix that was held back for an
    /// incomplete escape or UTF-8 sequence.
    ///
    /// This should be called when the PTY stream ends (e.g., on pane exit,
    /// removal, or error). Incomplete ANSI escape sequences in the pending
    /// prefix are intentionally dropped by `strip_ansi()` rather than
    /// preserved as literal text, because their boundaries cannot be
    /// recovered once the stream has ended. Complete plain text and
    /// incomplete UTF-8 sequences are retained. The flushed bytes are
    /// recorded as a final chunk and limits are re-enforced so retention
    /// guarantees still hold after finalization.
    pub(crate) fn flush_pending(&mut self) -> Option<TranscriptRotation> {
        if self.pending_plain_prefix.is_empty() {
            return None;
        }
        let forced = strip_ansi(&self.pending_plain_prefix);
        let plain_len = forced.len();
        let lines = count_newlines(forced.as_bytes());
        self.plain.push_str(&forced);
        self.pending_plain_prefix.clear();
        self.next_plain_offset += plain_len as u64;
        self.total_lines += lines;

        self.chunks.push_back(Chunk {
            raw_len: 0,
            plain_len,
            lines,
        });

        self.enforce_limits()
    }
}

fn count_newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

/// Returns the number of trailing bytes that should be held back because
/// they may be part of an incomplete ANSI escape sequence or incomplete
/// UTF-8 multi-byte sequence.
///
/// This scans the buffer from the start, tracking open CSI and control-
/// string state. If the buffer ends inside an unterminated sequence, all
/// bytes from that sequence's introducer are held back so they can be
/// recombined with the next frame before stripping.
fn trailing_incomplete_bytes(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }

    let mut open_csi: Option<usize> = None;
    let mut open_control_string: Option<usize> = None;

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\x1b' {
            i += 1;
            continue;
        }

        // bytes[i] is ESC
        if i + 1 >= bytes.len() {
            // Lone ESC at the very end.
            return match (open_control_string, open_csi) {
                (Some(start), _) => bytes.len() - start,
                (None, Some(start)) => bytes.len() - start,
                (None, None) => 1,
            };
        }

        let next = bytes[i + 1];
        match next {
            b'[' => {
                // CSI sequence.
                if open_control_string.is_none() {
                    open_csi = Some(i);
                }
                i += 2; // past ESC [
                while i < bytes.len() {
                    if (b'@'..=b'~').contains(&bytes[i]) {
                        if open_control_string.is_none() {
                            open_csi = None;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b']' => {
                open_control_string = Some(i);
                i += 2; // past ESC ]
                while i < bytes.len() {
                    if bytes[i] == b'\x07' {
                        open_control_string = None;
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'\x1b' {
                        if i + 1 >= bytes.len() {
                            return bytes.len() - open_control_string.unwrap();
                        }
                        if bytes[i + 1] == b'\\' {
                            open_control_string = None;
                            i += 2;
                            break;
                        }
                        if bytes[i + 1] == b'[' {
                            i += 2; // past ESC [
                            while i < bytes.len() {
                                if (b'@'..=b'~').contains(&bytes[i]) {
                                    i += 1;
                                    break;
                                }
                                i += 1;
                            }
                            continue;
                        }
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
            }
            b'P' | b'_' | b'^' | b'X' => {
                if open_control_string.is_none() {
                    open_control_string = Some(i);
                }
                i += 2; // past ESC + introducer
                while i < bytes.len() {
                    if bytes[i] == b'\x1b' {
                        if i + 1 >= bytes.len() {
                            return bytes.len() - open_control_string.unwrap();
                        }
                        if bytes[i + 1] == b'\\' {
                            open_control_string = None;
                            i += 2;
                            break;
                        }
                        if bytes[i + 1] == b'[' {
                            i += 2; // past ESC [
                            while i < bytes.len() {
                                if (b'@'..=b'~').contains(&bytes[i]) {
                                    i += 1;
                                    break;
                                }
                                i += 1;
                            }
                            continue;
                        }
                        i += 2;
                        continue;
                    }
                    i += 1;
                }
            }
            b'(' | b')' | b'#' | b'%' | b'*' | b'+' | b'-' | b'.' | b'/' => {
                // Two-character escape: needs a parameter byte after the introducer.
                if i + 2 >= bytes.len() {
                    return match (open_control_string, open_csi) {
                        (Some(start), _) => bytes.len() - start,
                        (None, Some(start)) => bytes.len() - start,
                        (None, None) => bytes.len() - i,
                    };
                }
                i += 3;
            }
            b'=' | b'>' | b'<' | b'7' | b'8' => {
                // Single-character escape: complete.
                i += 2;
            }
            _ if (b'@'..=b'_').contains(&next) => {
                // Single-character escape: complete.
                i += 2;
            }
            _ => {
                // Unknown introducer: strip_ansi preserves as literal.
                // Nothing to hold back; skip both bytes.
                i += 2;
            }
        }
    }

    if let Some(start) = open_control_string {
        return bytes.len() - start;
    }
    if let Some(start) = open_csi {
        return bytes.len() - start;
    }

    // Check for incomplete multi-byte UTF-8 at the end.
    for j in (0..bytes.len()).rev() {
        let b = bytes[j];
        if b & 0b1000_0000 == 0 {
            break;
        }
        if b & 0b1100_0000 == 0b1100_0000 {
            let expected_len = if b & 0b1110_0000 == 0b1100_0000 {
                2
            } else if b & 0b1111_0000 == 0b1110_0000 {
                3
            } else if b & 0b1111_1000 == 0b1111_0000 {
                4
            } else {
                break;
            };
            let actual_len = bytes.len() - j;
            if actual_len < expected_len {
                return actual_len;
            }
            break;
        }
    }

    0
}

/// Strips common ANSI escape sequences from terminal output.
///
/// This is a best-effort stripper suitable for log-safe plain text. It
/// handles CSI sequences (`ESC [`), OSC sequences (`ESC ]`), and simple
/// two-character escapes. Invalid UTF-8 is replaced with the Unicode
/// replacement character.
///
/// **Frame-boundary caveat:** This function operates on a single byte
/// slice. ANSI escape sequences and multi-byte UTF-8 sequences that are
/// split across PTY frame boundaries may produce artifacts (replacement
/// characters or leaked escape fragments) when called independently on
/// each frame. For stateful stripping across arbitrary frame boundaries,
/// use [`Transcript::record`] which carries a continuation buffer.
pub fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if let Some(&next) = chars.peek() {
                match next {
                    '[' => {
                        chars.next(); // consume '['
                        while let Some(&c) = chars.peek() {
                            chars.next();
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                        continue;
                    }
                    ']' => {
                        chars.next(); // consume ']'
                        while let Some(&c) = chars.peek() {
                            if c == '\x07' {
                                chars.next();
                                break;
                            }
                            if c == '\x1b' {
                                chars.next(); // consume ESC
                                if chars.peek() == Some(&'\\') {
                                    chars.next(); // consume \
                                    break;
                                }
                                // ESC not followed by \ — keep scanning
                            } else {
                                chars.next();
                            }
                        }
                        continue;
                    }
                    'P' | '_' | '^' | 'X' => {
                        // DCS, APC, PM, SOS — multi-byte sequences ending in ST.
                        chars.next(); // consume introducer
                        while let Some(&c) = chars.peek() {
                            if c == '\x1b' {
                                chars.next(); // consume ESC
                                if chars.peek() == Some(&'\\') {
                                    chars.next(); // consume \
                                    break;
                                }
                                // ESC not followed by \ — keep scanning
                            } else {
                                chars.next();
                            }
                        }
                        continue;
                    }
                    '(' | ')' | '#' | '%' | '*' | '+' | '-' | '.' | '/' => {
                        chars.next(); // consume the introducer
                        chars.next(); // consume the parameter character
                        continue;
                    }
                    '=' | '>' | '<' => {
                        chars.next();
                        continue;
                    }
                    '7' | '8' => {
                        // DEC save/restore cursor — complete single-byte escapes.
                        chars.next();
                        continue;
                    }
                    _ if ('@'..='_').contains(&next) => {
                        chars.next();
                        continue;
                    }
                    _ => {}
                }
            }
        }
        result.push(ch);
    }

    result
}

#[cfg(test)]
mod tests;
