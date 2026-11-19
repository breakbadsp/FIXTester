#![allow(dead_code)]

/// Builds a message from text where `|` is SOH, and adds BodyLength and
/// CheckSum as the FIX specification defines them.
pub fn wire(begin_string: &str, body: &str) -> Vec<u8> {
    let body = body.replace('|', "\x01");
    let mut out = format!("8={begin_string}\x019={}\x01{body}", body.len()).into_bytes();
    let sum = out.iter().map(|byte| u32::from(*byte)).sum::<u32>() % 256;
    out.extend_from_slice(format!("10={sum:03}\x01").as_bytes());
    out
}

/// The standard header fields that every message needs, then `body`.
pub fn with_header(msg_type: &str, body: &str) -> String {
    format!("35={msg_type}|49=FIXSIM|56=VENUE|34=12|52=20261118-14:30:05.123|{body}")
}

/// A NewOrderMultileg (AB) of FIX 4.4 with two legs. The first leg has a
/// nested NoLegSecurityAltID group.
pub const MULTILEG_BODY: &str = "11=ORD0001|54=1|60=20261118-14:30:05.100|40=2|44=1.5|38=10|\
     555=2|\
     600=ESZ6|624=1|623=1|604=1|605=ESZ6-ALT|606=4|\
     600=ESH7|624=2|623=1|\
     59=0|";

/// A NewOrderSingle (D) of FIX 4.4 with a party that has two sub IDs.
pub const SINGLE_BODY: &str = "11=ORD0002|54=1|60=20261118-14:30:05.100|40=2|55=IBM|38=100|44=12.5|59=0|\
     453=1|448=PARTY1|447=D|452=1|802=2|523=SUB1|803=1|523=SUB2|803=2|";

/// Like [`wire`], for a body with bytes that are not UTF-8.
pub fn wire_bytes(begin_string: &str, body: &[u8]) -> Vec<u8> {
    let body: Vec<u8> = body
        .iter()
        .map(|byte| if *byte == b'|' { 0x01 } else { *byte })
        .collect();
    let mut out = format!("8={begin_string}\x019={}\x01", body.len()).into_bytes();
    out.extend_from_slice(&body);
    let sum = out.iter().map(|byte| u32::from(*byte)).sum::<u32>() % 256;
    out.extend_from_slice(format!("10={sum:03}\x01").as_bytes());
    out
}
