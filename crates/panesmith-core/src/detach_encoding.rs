//! Terminal-specific encodings for Ctrl-letter detach chords.

use std::fmt::Write as _;

const TRACE_PREVIEW_LIMIT: usize = 64;

/// A matched terminal encoding that is equivalent to a raw Ctrl-letter byte.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtrlDetachEncodingMatch {
    /// Normalized raw Ctrl-letter byte, e.g. `0x06` for Ctrl-F.
    pub normalized: u8,
    /// Number of original input bytes consumed by this encoded sequence.
    pub consumed: usize,
}

/// Parses a terminal-specific Ctrl-letter encoding for the configured detach chord.
#[doc(hidden)]
pub fn parse_ctrl_detach_encoding(
    bytes: &[u8],
    detach_chord: &[u8],
) -> Option<CtrlDetachEncodingMatch> {
    let raw_chord = ctrl_letter_detach_chord(detach_chord)?;
    parse_csi_u_detach_chord(bytes, raw_chord)
        .or_else(|| parse_xterm_modify_other_keys_detach_chord(bytes, raw_chord))
}

/// Normalizes all complete Ctrl-letter detach encodings in `bytes`.
#[doc(hidden)]
pub fn normalize_attach_detach_input(bytes: &[u8], detach_chord: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some(matched) = parse_ctrl_detach_encoding(&bytes[idx..], detach_chord) {
            normalized.push(matched.normalized);
            idx += matched.consumed;
        } else {
            normalized.push(bytes[idx]);
            idx += 1;
        }
    }

    normalized
}

/// Formats raw terminal bytes for opt-in trace diagnostics.
#[doc(hidden)]
pub fn format_attach_bytes_for_trace(bytes: &[u8]) -> String {
    let mut formatted = String::new();
    for (idx, byte) in bytes.iter().take(TRACE_PREVIEW_LIMIT).enumerate() {
        if idx > 0 {
            formatted.push(' ');
        }
        let _ = write!(formatted, "{byte:02x}");
    }
    if bytes.len() > TRACE_PREVIEW_LIMIT {
        let _ = write!(
            formatted,
            " ... +{} bytes",
            bytes.len() - TRACE_PREVIEW_LIMIT
        );
    }
    formatted
}

fn parse_csi_u_detach_chord(bytes: &[u8], raw_chord: u8) -> Option<CtrlDetachEncodingMatch> {
    if !bytes.starts_with(b"\x1b[") {
        return None;
    }

    let end = bytes.iter().position(|byte| *byte == b'u')?;
    let params = &bytes[2..end];
    let mut fields = params.split(|byte| *byte == b';');
    let key_field = fields.next()?;
    let key_code = parse_u32(main_param(key_field)?)?;
    let modifier_mask = fields.next().and_then(main_param).and_then(parse_u16);

    if csi_key_code_matches_detach_chord(key_code, modifier_mask, raw_chord) {
        Some(CtrlDetachEncodingMatch {
            normalized: raw_chord,
            consumed: end + 1,
        })
    } else {
        None
    }
}

fn parse_xterm_modify_other_keys_detach_chord(
    bytes: &[u8],
    raw_chord: u8,
) -> Option<CtrlDetachEncodingMatch> {
    if !bytes.starts_with(b"\x1b[") {
        return None;
    }

    let end = bytes.iter().position(|byte| *byte == b'~')?;
    let params = &bytes[2..end];
    let mut fields = params.split(|byte| *byte == b';');
    let introducer = parse_u32(main_param(fields.next()?)?)?;
    if introducer != 27 {
        return None;
    }
    let modifier_mask = parse_u16(main_param(fields.next()?)?)?;
    let key_code = parse_u32(main_param(fields.next()?)?)?;
    if fields.next().is_some() || !modifier_mask_has_control(modifier_mask) {
        return None;
    }

    if key_code_matches_letter_or_raw_ctrl(key_code, raw_chord) {
        Some(CtrlDetachEncodingMatch {
            normalized: raw_chord,
            consumed: end + 1,
        })
    } else {
        None
    }
}

fn csi_key_code_matches_detach_chord(
    key_code: u32,
    modifier_mask: Option<u16>,
    raw_chord: u8,
) -> bool {
    if key_code == u32::from(raw_chord) {
        return true;
    }
    modifier_mask.is_some_and(modifier_mask_has_control)
        && key_code_matches_letter_or_raw_ctrl(key_code, raw_chord)
}

fn key_code_matches_letter_or_raw_ctrl(key_code: u32, raw_chord: u8) -> bool {
    let expected_lower = u32::from(b'a' + raw_chord - 1);
    let expected_upper = u32::from(b'A' + raw_chord - 1);
    key_code == expected_lower || key_code == expected_upper || key_code == u32::from(raw_chord)
}

fn ctrl_letter_detach_chord(detach_chord: &[u8]) -> Option<u8> {
    match detach_chord {
        [byte @ 0x01..=0x1a] => Some(*byte),
        _ => None,
    }
}

fn modifier_mask_has_control(mask: u16) -> bool {
    mask.checked_sub(1)
        .is_some_and(|modifier_bits| modifier_bits & 0b100 != 0)
}

fn main_param(field: &[u8]) -> Option<&[u8]> {
    let main = field.split(|byte| *byte == b':').next()?;
    (!main.is_empty()).then_some(main)
}

fn parse_u32(bytes: &[u8]) -> Option<u32> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn parse_u16(bytes: &[u8]) -> Option<u16> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

#[cfg(test)]
mod tests;
