//! Byte-level detach-chord matcher for attach mode.
//!
//! [`DetachMatcher`] intercepts stdin bytes during fullscreen attach and
//! recognises a configurable byte sequence that triggers detach.  Bytes that
//! do not match the chord are returned for forwarding to the child PTY.

use std::time::{Duration, Instant};

use panesmith_core::detach_encoding::parse_ctrl_detach_encoding;
use panesmith_core::DetachConfig;

/// Result of feeding bytes into a [`DetachMatcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchResult {
    /// Forward these bytes to the child PTY.
    Forward(Vec<u8>),
    /// The detach chord was fully matched.
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FeedStep {
    ForwardNone,
    ForwardByte(u8),
    ForwardBytes(Vec<u8>),
    Detach,
}

/// Result of feeding a byte slice into a [`DetachMatcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedBytesResult<'a> {
    /// Bytes that should be forwarded to the child PTY.
    pub forward: Vec<u8>,
    /// Whether the detach chord was matched.
    pub detached: bool,
    /// Bytes after the detach chord that were not consumed by the matcher.
    pub remaining: &'a [u8],
}

/// Byte-level matcher for detach chords.
///
/// Supports single-byte and multi-byte chords with partial-match timeout.
/// Non-matching bytes and timed-out partial matches are returned for forwarding
/// to the child PTY.
///
/// # Example
///
/// ```rust
/// use std::time::{Duration, Instant};
/// use panesmith_attach::detach::{DetachMatcher, MatchResult};
/// use panesmith_core::DetachConfig;
///
/// let mut config = DetachConfig::default();
/// config.chord = vec![0x1d]; // Ctrl-]
/// config.partial_timeout = Duration::from_millis(500);
/// let mut matcher = DetachMatcher::new(&config);
/// let now = Instant::now();
///
/// assert!(matches!(matcher.feed_byte(0x1d, now), MatchResult::Detach));
/// assert!(matches!(matcher.feed_byte(0x61, now), MatchResult::Forward(bytes) if bytes == vec![0x61]));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachMatcher {
    chord: Vec<u8>,
    timeout: Duration,
    held: Vec<u8>,
    held_at: Vec<Instant>,
}

impl DetachMatcher {
    /// Creates a new matcher from the given detach configuration.
    pub fn new(config: &DetachConfig) -> Self {
        Self {
            chord: config.chord.clone(),
            timeout: config.partial_timeout,
            held: Vec::new(),
            held_at: Vec::new(),
        }
    }

    /// Feeds a single byte into the matcher.
    ///
    /// Returns [`MatchResult::Detach`] if the configured chord was fully
    /// matched, or [`MatchResult::Forward`] with any bytes that should be
    /// sent to the child PTY. Any expired partial match is flushed before the
    /// new byte is processed.
    pub fn feed_byte(&mut self, byte: u8, now: Instant) -> MatchResult {
        let mut forward = self.check_timeout(now).unwrap_or_default();
        match self.feed_byte_inner(byte, now) {
            FeedStep::ForwardNone => MatchResult::Forward(forward),
            FeedStep::ForwardByte(byte) => {
                forward.push(byte);
                MatchResult::Forward(forward)
            }
            FeedStep::ForwardBytes(mut bytes) => {
                forward.append(&mut bytes);
                MatchResult::Forward(forward)
            }
            FeedStep::Detach => {
                debug_assert!(
                    forward.is_empty(),
                    "timed-out bytes cannot be pending when a single byte detaches"
                );
                MatchResult::Detach
            }
        }
    }

    fn feed_byte_inner(&mut self, byte: u8, now: Instant) -> FeedStep {
        if self.chord.is_empty() {
            return FeedStep::ForwardByte(byte);
        }

        // If we are not holding any bytes and this byte matches the first byte
        // of the chord, start (or complete) a match.
        if self.held.is_empty() {
            if byte == self.chord[0] {
                self.held.push(byte);
                self.held_at.push(now);
                if self.chord.len() == 1 {
                    self.reset();
                    return FeedStep::Detach;
                }
                return FeedStep::ForwardNone;
            }
            return FeedStep::ForwardByte(byte);
        }

        // We are holding bytes from a partial match.
        let next_idx = self.held.len();
        if next_idx < self.chord.len() && byte == self.chord[next_idx] {
            self.held.push(byte);
            self.held_at.push(now);
            if self.held.len() == self.chord.len() {
                self.reset();
                return FeedStep::Detach;
            }
            return FeedStep::ForwardNone;
        }

        // Mismatch: find the longest suffix of (held + byte) that is also a
        // prefix of the chord. Forward everything before that suffix, and hold
        // the suffix for a continued partial match.
        let mut combined = std::mem::take(&mut self.held);
        let mut combined_at = std::mem::take(&mut self.held_at);
        combined.push(byte);
        combined_at.push(now);

        // Detach chords are domain-bounded to a handful of bytes, so scanning
        // suffix lengths directly keeps the overlap logic simple.
        let mut suffix_len = 0;
        for k in (1..=combined.len().min(self.chord.len())).rev() {
            if combined[combined.len() - k..] == self.chord[0..k] {
                suffix_len = k;
                break;
            }
        }

        let forward = if suffix_len == 0 {
            combined
        } else {
            let split_idx = combined.len() - suffix_len;
            let held = combined.split_off(split_idx);
            let held_at = combined_at.split_off(split_idx);
            self.held = held;
            self.held_at = held_at;
            combined
        };

        if forward.is_empty() {
            FeedStep::ForwardNone
        } else if forward.len() == 1 {
            FeedStep::ForwardByte(forward[0])
        } else {
            FeedStep::ForwardBytes(forward)
        }
    }

    /// Feeds a slice of bytes into the matcher.
    ///
    /// Returns any bytes to forward, whether a detach was detected, and the
    /// unread suffix after the detach boundary. If `detached` is true,
    /// `remaining` contains the bytes after the chord that the caller should
    /// handle separately.
    ///
    /// Before processing the slice, any timed-out partial match is flushed.
    pub fn feed_bytes<'a>(&mut self, bytes: &'a [u8], now: Instant) -> FeedBytesResult<'a> {
        let mut forward = Vec::with_capacity(bytes.len());

        if let Some(held) = self.check_timeout(now) {
            forward.extend_from_slice(&held);
        }

        let mut idx = 0;
        while idx < bytes.len() {
            let (byte, consumed) =
                if let Some(matched) = parse_ctrl_detach_encoding(&bytes[idx..], &self.chord) {
                    (matched.normalized, matched.consumed)
                } else {
                    (bytes[idx], 1)
                };

            match self.feed_byte_inner(byte, now) {
                FeedStep::ForwardNone => {}
                FeedStep::ForwardByte(byte) => forward.push(byte),
                FeedStep::ForwardBytes(mut bytes) => forward.append(&mut bytes),
                FeedStep::Detach => {
                    return FeedBytesResult {
                        forward,
                        detached: true,
                        remaining: &bytes[idx + consumed..],
                    };
                }
            }
            idx += consumed;
        }

        FeedBytesResult {
            forward,
            detached: false,
            remaining: &[],
        }
    }

    /// Checks whether the current partial match has timed out.
    ///
    /// Returns held bytes for forwarding if the timeout expired.
    pub fn check_timeout(&mut self, now: Instant) -> Option<Vec<u8>> {
        debug_assert_eq!(
            self.held.len(),
            self.held_at.len(),
            "held bytes and timestamps must stay aligned"
        );

        if let Some(start) = self.held_at.first().copied() {
            debug_assert!(
                !self.held.is_empty(),
                "held timestamp set but held is empty"
            );
            if now.duration_since(start) >= self.timeout {
                self.held_at.clear();
                let held = std::mem::take(&mut self.held);
                if !held.is_empty() {
                    return Some(held);
                }
            }
        }
        None
    }

    /// Returns true if the matcher is currently holding bytes in a partial
    /// match.
    pub fn is_holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Returns the bytes currently held for a partial match.
    pub fn held_bytes(&self) -> &[u8] {
        &self.held
    }

    fn reset(&mut self) {
        self.held.clear();
        self.held_at.clear();
    }
}

#[cfg(test)]
mod tests;
