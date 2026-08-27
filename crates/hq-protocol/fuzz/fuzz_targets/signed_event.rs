#![no_main]

use hq_protocol::{MAX_EVENT_BYTES, RawEventBytes};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    // Text corpus files carry a repository terminator; inputs without one remain exact.
    exercise(bytes.strip_suffix(b"\n").unwrap_or(bytes));
});

fn exercise(bytes: &[u8]) {
    if bytes.len() > MAX_EVENT_BYTES {
        return;
    }
    let Ok(raw) = RawEventBytes::new(bytes.to_vec()) else {
        return;
    };
    let Ok(parsed) = raw.parse() else {
        return;
    };
    let Ok(verified) = parsed.verify() else {
        return;
    };
    let _ = verified.dispatch();
}
