#![no_main]

use fix_codec::{Decoder, frame, frame_with_checksum, skip_garbled};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    const LIMIT: usize = 16 * 1024;
    let mut decoder = Decoder::new(256);
    let _ = decoder.decode(bytes);
    if let Ok(Some(boundary)) = frame_with_checksum(bytes, LIMIT) {
        assert!(boundary.len > 0 && boundary.len <= LIMIT);
        let raw = &bytes[..boundary.len];
        if boundary.checksum.is_ok() {
            assert_eq!(frame(raw, LIMIT), Ok(Some(raw.len())));
            let _ = decoder.decode(raw);
        }
    } else if !bytes.is_empty() {
        assert!((1..=bytes.len()).contains(&skip_garbled(bytes)));
    }
});
