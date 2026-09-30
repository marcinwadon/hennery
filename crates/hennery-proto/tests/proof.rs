//! The bytes a host signs in `hello` (ACP core §3.5). The same vector is
//! checked by the host that signs it and the collector that verifies it.

use hennery_proto::hello_proof_message;

#[test]
fn the_proof_message_is_labelled_and_length_delimited() {
    let message = hello_proof_message(&[0xab; 4], "host-1", "1.0");
    let mut expected = b"hennery hello proof v1".to_vec();
    expected.extend_from_slice(&[0, 0, 0, 4, 0xab, 0xab, 0xab, 0xab]);
    expected.extend_from_slice(&[0, 0, 0, 6]);
    expected.extend_from_slice(b"host-1");
    expected.extend_from_slice(&[0, 0, 0, 3]);
    expected.extend_from_slice(b"1.0");
    assert_eq!(message, expected);
}

#[test]
fn moving_bytes_between_the_parts_changes_the_message() {
    // Plain concatenation would make these two the same bytes.
    assert_ne!(
        hello_proof_message(b"nonce", "host-1", "1.0"),
        hello_proof_message(b"nonceh", "ost-1", "1.0")
    );
    assert_ne!(
        hello_proof_message(b"n", "host-1", "1.0"),
        hello_proof_message(b"n", "host-11", ".0")
    );
}
