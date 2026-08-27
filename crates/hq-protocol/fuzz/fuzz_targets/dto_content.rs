#![no_main]

use hq_protocol::{Bip340Signer, DispatchOutcome, MAX_CONTENT_BYTES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if bytes.len() > MAX_CONTENT_BYTES {
        return;
    }
    let mut secret = [0_u8; 32];
    secret[31] = 1;
    let Ok(signer) = Bip340Signer::from_secret_bytes(secret) else {
        return;
    };
    let created_at = if contains(bytes, b"\"time\":1000") {
        1
    } else {
        0
    };
    let Ok(verified) = signer.sign(created_at, bytes, [23; 32]) else {
        return;
    };
    let Ok(DispatchOutcome::Supported(prefix)) = verified.dispatch() else {
        return;
    };
    let _ = prefix.decode_v1();
});

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
