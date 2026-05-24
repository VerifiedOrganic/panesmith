//! Manager-owned input transaction types.

use std::time::Duration;

use crate::{IoOperation, KeyInput};

const DEFAULT_CHUNK_SIZE: usize = 1024;
const DEFAULT_TRANSIENT_WRITE_RETRIES: usize = 3;
const DEFAULT_TRANSIENT_WRITE_RETRY_DELAY: Duration = Duration::from_millis(5);
const FNV1A_64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A_64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Operator intent for a manager-owned input transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InputIntent {
    /// Insert text without submitting it.
    InsertText(String),
    /// Insert text, then send the pane's configured Enter key sequence.
    SubmitText(String),
    /// Send a keyboard chord through Panesmith's terminal encoder.
    KeyChord(KeyInput),
    /// Send Ctrl-C.
    Interrupt,
    /// Send Ctrl-U to clear the current input line in common shells.
    ClearInput,
    /// Send explicit raw bytes as an escape hatch.
    RawBytes(Vec<u8>),
}

/// Echo verification policy for an input transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InputVerification {
    /// Do not verify echoed output.
    #[default]
    None,
    /// Succeed once the pane's owned surface text contains `needle`.
    EchoContains {
        /// Text to find in the pane surface or retained scrollback.
        needle: String,
        /// Maximum time to poll pane output and surface state.
        timeout: Duration,
    },
    /// Succeed once a surface line has the prefix or an FNV-1a hash matches.
    EchoPrefixOrHash {
        /// Text prefix to find in a surface line.
        prefix: String,
        /// Optional FNV-1a hash produced by [`input_echo_hash`].
        hash: Option<u64>,
        /// Maximum time to poll pane output and surface state.
        timeout: Duration,
    },
}

impl InputVerification {
    pub(crate) fn timeout(&self) -> Option<Duration> {
        match self {
            Self::None => None,
            Self::EchoContains { timeout, .. } | Self::EchoPrefixOrHash { timeout, .. } => {
                Some(*timeout)
            }
        }
    }

    pub(crate) fn matches_text(&self, text: &str) -> bool {
        match self {
            Self::None => true,
            Self::EchoContains { needle, .. } => text.contains(needle),
            Self::EchoPrefixOrHash { prefix, hash, .. } => {
                let prefix_matches =
                    !prefix.is_empty() && text.lines().any(|line| line.contains(prefix));
                if prefix_matches {
                    return true;
                }

                let Some(expected_hash) = hash else {
                    return false;
                };

                input_echo_hash(text) == *expected_hash
                    || text
                        .lines()
                        .any(|line| input_echo_hash(line) == *expected_hash)
            }
        }
    }
}

/// Retry policy for transient PTY write failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InputRetryConfig {
    /// Number of retries for `WouldBlock` and `Interrupted` write failures.
    pub max_transient_retries: usize,
    /// Delay between transient write retries.
    pub retry_delay: Duration,
}

impl Default for InputRetryConfig {
    fn default() -> Self {
        Self {
            max_transient_retries: DEFAULT_TRANSIENT_WRITE_RETRIES,
            retry_delay: DEFAULT_TRANSIENT_WRITE_RETRY_DELAY,
        }
    }
}

/// A manager-owned input transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InputTransaction {
    /// Input intent to apply.
    pub intent: InputIntent,
    /// Echo verification policy.
    pub verification: InputVerification,
    /// Maximum raw-typing chunk size for fallback text insertion.
    pub chunk_size: usize,
    /// Retry policy for transient PTY write failures.
    pub retry: InputRetryConfig,
}

impl InputTransaction {
    /// Creates a transaction from an input intent.
    pub fn new(intent: InputIntent) -> Self {
        Self {
            intent,
            verification: InputVerification::None,
            chunk_size: DEFAULT_CHUNK_SIZE,
            retry: InputRetryConfig::default(),
        }
    }

    /// Creates a text-insertion transaction.
    pub fn insert_text(text: impl Into<String>) -> Self {
        Self::new(InputIntent::InsertText(text.into()))
    }

    /// Creates a text-submit transaction.
    pub fn submit_text(text: impl Into<String>) -> Self {
        Self::new(InputIntent::SubmitText(text.into()))
    }

    /// Creates a key-chord transaction.
    pub fn key_chord(key: KeyInput) -> Self {
        Self::new(InputIntent::KeyChord(key))
    }

    /// Creates an interrupt transaction.
    pub fn interrupt() -> Self {
        Self::new(InputIntent::Interrupt)
    }

    /// Creates a clear-input transaction.
    pub fn clear_input() -> Self {
        Self::new(InputIntent::ClearInput)
    }

    /// Creates an explicit raw-byte transaction.
    pub fn raw_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::new(InputIntent::RawBytes(bytes.into()))
    }

    /// Sets echo verification for this transaction.
    pub fn with_verification(mut self, verification: InputVerification) -> Self {
        self.verification = verification;
        self
    }

    /// Sets the maximum raw-typing chunk size.
    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size.max(1);
        self
    }

    /// Sets the transient write retry policy.
    pub fn with_retry(mut self, retry: InputRetryConfig) -> Self {
        self.retry = retry;
        self
    }
}

/// Structured result from an input transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InputOutcome {
    /// Bytes successfully written to the PTY.
    pub bytes_sent: usize,
    /// Whether echo verification succeeded.
    pub echoed: bool,
    /// Whether a submit key sequence was sent.
    pub submitted: bool,
    /// Whether echo verification timed out.
    pub timed_out: bool,
    /// Whether the child exited before or during the transaction.
    pub child_exited: bool,
    /// Structured transaction failures.
    pub errors: Vec<InputTransactionError>,
}

impl InputOutcome {
    /// Returns true when the transaction completed without structured errors.
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && !self.timed_out
    }
}

/// Structured transaction failure details.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InputTransactionError {
    /// A PTY write or flush failed.
    Write {
        /// Failed I/O operation.
        operation: IoOperation,
        /// Bytes the transaction attempted to write in this chunk.
        bytes_attempted: usize,
        /// Bytes written from this chunk before failure was observed.
        bytes_written: usize,
        /// Human-readable failure message.
        message: String,
    },
    /// Echo verification did not succeed.
    VerificationFailed {
        /// Human-readable verification failure message.
        message: String,
    },
    /// The child exited before all transaction steps completed.
    ChildExited,
}

/// Computes the stable FNV-1a hash used by `EchoPrefixOrHash`.
pub fn input_echo_hash(text: &str) -> u64 {
    text.as_bytes().iter().fold(FNV1A_64_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV1A_64_PRIME)
    })
}
