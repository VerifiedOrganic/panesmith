use super::*;

const CTRL_F: &[u8] = &[0x06];

#[test]
fn ctrl_f_common_terminal_encodings_normalize_to_raw_chord() {
    let cases: &[(&[u8], &[u8])] = &[
        (b"\x1b[102;5uafter", b"\x06after"),
        (b"\x1b[102;5:1uafter", b"\x06after"),
        (b"\x1b[6uafter", b"\x06after"),
        (b"\x1b[6;1uafter", b"\x06after"),
        (b"\x1b[27;5;102~after", b"\x06after"),
    ];

    for (encoded, expected) in cases {
        assert_eq!(
            normalize_attach_detach_input(encoded, CTRL_F),
            *expected,
            "encoded input {encoded:?} should normalize"
        );
    }
}

#[test]
fn ctrl_letter_csi_u_requires_control_modifier_for_printable_key_codes() {
    assert_eq!(
        normalize_attach_detach_input(b"\x1b[102u", CTRL_F),
        b"\x1b[102u"
    );
    assert_eq!(
        normalize_attach_detach_input(b"\x1b[102;1u", CTRL_F),
        b"\x1b[102;1u"
    );
}

#[test]
fn ctrl_letter_normalization_is_limited_to_configured_chord() {
    assert_eq!(
        normalize_attach_detach_input(b"\x1b[102;5u", &[0x07]),
        b"\x1b[102;5u"
    );
    assert_eq!(
        normalize_attach_detach_input(b"\x1b[102;5u", &[0x06, b'd']),
        b"\x1b[102;5u"
    );
}
