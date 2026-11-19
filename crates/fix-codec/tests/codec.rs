//! Behavior tests for the FIX codec. Expected values come from the FIX
//! specification, not from a run of the code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use fix_codec::{
    Decoder, Encoder, FrameError, GroupDef, GroupError, Member, UtcMillis, frame,
    parse_utc_timestamp, skip_garbled, write_utc_timestamp,
};
use proptest::prelude::*;

const LEN_MAX: usize = 4096;

/// Builds a message from text where `|` is SOH, and adds BodyLength and
/// CheckSum as the specification defines them.
fn wire(begin_string: &str, body: &str) -> Vec<u8> {
    let body = body.replace('|', "\x01");
    let mut out = format!("8={begin_string}\x019={}\x01{body}", body.len()).into_bytes();
    let sum = out.iter().map(|byte| u32::from(*byte)).sum::<u32>() % 256;
    out.extend_from_slice(format!("10={sum:03}\x01").as_bytes());
    out
}

#[test]
fn heartbeat_has_known_body_length_and_checksum() {
    let mut encoder = Encoder::new(4096);
    let mut body = encoder.start(b"0");
    body.field(49, b"A").field(56, b"B").field_u64(34, 7);
    let bytes = body.finish(b"FIX.4.4").expect("fits").to_vec();
    assert_eq!(bytes, wire("FIX.4.4", "35=0|49=A|56=B|34=7|"));
}

#[test]
fn frame_needs_the_whole_message() {
    let bytes = wire("FIX.4.2", "35=0|49=A|56=B|34=1|");
    for cut in 0..bytes.len() {
        assert_eq!(
            frame(&bytes[..cut], LEN_MAX),
            Ok(None),
            "prefix of {cut} bytes"
        );
    }
    assert_eq!(frame(&bytes, LEN_MAX), Ok(Some(bytes.len())));
}

#[test]
fn frame_returns_only_the_first_of_two_messages() {
    let first = wire("FIX.4.3", "35=0|34=1|");
    let mut stream = first.clone();
    stream.extend(wire("FIX.4.3", "35=0|34=2|"));
    assert_eq!(frame(&stream, LEN_MAX), Ok(Some(first.len())));
}

#[test]
fn frame_rejects_a_wrong_checksum_and_resyncs_to_the_next_message() {
    let mut bad = wire("FIX.4.4", "35=0|34=1|");
    let checksum_digit = bad.len() - 2;
    bad[checksum_digit] = if bad[checksum_digit] == b'9' {
        b'0'
    } else {
        bad[checksum_digit] + 1
    };
    assert!(matches!(
        frame(&bad, LEN_MAX),
        Err(FrameError::ChecksumMismatch { .. })
    ));

    let good = wire("FIX.4.4", "35=0|34=2|");
    let mut stream = bad.clone();
    stream.extend(&good);
    assert_eq!(skip_garbled(&stream), bad.len());
    assert_eq!(frame(&stream[bad.len()..], LEN_MAX), Ok(Some(good.len())));
}

#[test]
fn frame_rejects_a_wrong_body_length_and_a_long_message() {
    let mut bytes = wire("FIX.4.4", "35=0|34=1|");
    // BodyLength one too small: the trailer is not where it says.
    let text = String::from_utf8(bytes.clone())
        .unwrap_or_default()
        .replace("9=10", "9=9");
    bytes = text.into_bytes();
    assert_eq!(frame(&bytes, LEN_MAX), Err(FrameError::BadTrailer));
    let long = wire("FIX.4.4", "35=0|34=1|");
    assert_eq!(frame(&long, long.len() - 1), Err(FrameError::TooLong));
    assert_eq!(frame(b"9=5\x01", LEN_MAX), Err(FrameError::BadBeginString));
}

#[test]
fn decode_reads_header_and_body_fields() {
    let bytes = wire(
        "FIX.4.2",
        "35=D|49=BUY|56=SELL|34=12|11=ORD-1|54=1|38=100|43=Y|",
    );
    let mut decoder = Decoder::new(64);
    let message = decoder
        .decode(&bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(message.begin_string(), b"FIX.4.2");
    assert_eq!(message.msg_type(), b"D");
    let fields = message.fields();
    assert_eq!(fields.get(11), Some(&b"ORD-1"[..]));
    assert_eq!(fields.get_u64(34), Some(12));
    assert_eq!(fields.get_bool(43), Some(true));
    assert_eq!(fields.get_bool(97), Some(false), "an absent Boolean is N");
    assert_eq!(fields.get(44), None);
}

#[test]
fn decode_reads_a_data_field_that_contains_soh() {
    // RawDataLength(95)=5, RawData(96) holds an SOH in the middle.
    let bytes = wire("FIX.4.4", "35=A|95=5|96=ab|cd|98=0|108=30|");
    let mut decoder = Decoder::new(64);
    let message = decoder
        .decode(&bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(message.fields().get(96), Some(&b"ab\x01cd"[..]));
    assert_eq!(message.fields().get_u64(108), Some(30));
}

#[test]
fn encoder_writes_data_fields_that_the_decoder_reads_back() {
    let mut encoder = Encoder::new(4096);
    let mut body = encoder.start(b"A");
    body.data_field(95, b"x\x01y").field_u64(108, 30);
    let bytes = body.finish(b"FIX.4.4").expect("fits").to_vec();
    assert_eq!(frame(&bytes, LEN_MAX), Ok(Some(bytes.len())));
    let mut decoder = Decoder::new(64);
    let message = decoder
        .decode(&bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(message.fields().get(96), Some(&b"x\x01y"[..]));
}

#[test]
fn decode_rejects_bad_fields() {
    let mut decoder = Decoder::new(64);
    for body in ["35=0|x1=2|", "35=0|5|", "35=0|58=|", "35=0|0=1|"] {
        let bytes = wire("FIX.4.4", body);
        assert!(decoder.decode(&bytes).is_err(), "{body}");
    }
    let bytes = wire("FIX.4.4", "49=A|35=0|");
    assert!(
        decoder.decode(&bytes).is_err(),
        "MsgType must be the third field"
    );
    let bytes = wire("FIX.4.4", "35=0|1=a|2=b|3=c|");
    assert!(Decoder::new(5).decode(&bytes).is_err(), "too many fields");
}

// NoLegs(555) with LegSymbol(600), LegSide(624), and the nested
// NoLegSecurityAltID(604) with LegSecurityAltID(605), LegSecurityAltIDSource(606).
static ALT_IDS: GroupDef = GroupDef {
    count_tag: 604,
    members: &[Member::Field(605), Member::Field(606)],
};
static LEGS: GroupDef = GroupDef {
    count_tag: 555,
    members: &[
        Member::Field(600),
        Member::Group(&ALT_IDS),
        Member::Field(624),
    ],
};

#[test]
fn nested_groups_are_split_into_entries() {
    let bytes = wire(
        "FIX.4.4",
        "35=AB|11=M1|555=2|600=IBM|604=2|605=a|606=1|605=b|606=2|624=1|600=MSFT|624=2|38=10|",
    );
    let mut decoder = Decoder::new(64);
    let message = decoder
        .decode(&bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let legs = message
        .fields()
        .group(&LEGS)
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("no legs"));
    assert_eq!(legs.len(), 2);
    let entries: Vec<_> = legs.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].get(600), Some(&b"IBM"[..]));
    assert_eq!(entries[0].get(624), Some(&b"1"[..]));
    assert_eq!(entries[1].get(600), Some(&b"MSFT"[..]));
    assert_eq!(entries[1].get(604), None);
    let alt_ids = entries[0]
        .group(&ALT_IDS)
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("no ids"));
    let ids: Vec<_> = alt_ids
        .entries()
        .filter_map(|entry| entry.get(605))
        .collect();
    assert_eq!(ids, [&b"a"[..], &b"b"[..]]);
    assert_eq!(
        message.fields().get_u64(38),
        Some(10),
        "the field after the group is read"
    );
}

#[test]
fn a_group_is_read_from_a_cursor_when_its_count_tag_appears_twice() {
    // The first 555 is an empty group. The second one has an entry.
    let bytes = wire("FIX.4.4", "35=AB|555=0|555=1|600=IBM|38=10|");
    let mut decoder = Decoder::new(64);
    let message = decoder
        .decode(&bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    let fields = message.fields();
    let first = fields.group(&LEGS).ok().flatten();
    assert_eq!(first.map(|group| group.len()), Some(0));
    let second = fields.skip(2).group(&LEGS).ok().flatten();
    assert_eq!(second.map(|group| group.len()), Some(1));
    assert_eq!(fields.skip(2).len(), 3);
    assert!(fields.skip(99).is_empty());
}

#[test]
fn group_count_must_match_the_entries() {
    let mut decoder = Decoder::new(64);
    for (body, declared, found) in [
        ("35=AB|555=3|600=IBM|600=MSFT|", 3, 2),
        ("35=AB|555=1|600=IBM|600=MSFT|", 1, 2),
        ("35=AB|555=1|600=IBM|604=2|605=a|", 2, 1),
    ] {
        let bytes = wire("FIX.4.4", body);
        let message = decoder
            .decode(&bytes)
            .unwrap_or_else(|error| panic!("{error:?}"));
        let error = message.fields().group(&LEGS).err();
        let count_tag = if body.contains("604") { 604 } else { 555 };
        assert_eq!(
            error,
            Some(GroupError::CountMismatch {
                count_tag,
                declared,
                found
            }),
            "{body}"
        );
    }
    let bytes = wire("FIX.4.4", "35=AB|11=M1|");
    let message = decoder
        .decode(&bytes)
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert!(matches!(message.fields().group(&LEGS), Ok(None)));
}

#[test]
fn timestamps_match_the_calendar() {
    let mut out = [0u8; 21];
    let cases = [
        (0, "19700101-00:00:00.000"),
        (1_000_000_000_000, "20010909-01:46:40.000"),
        (1_709_210_096_789, "20240229-12:34:56.789"),
        (4_102_444_799_999, "20991231-23:59:59.999"),
    ];
    for (millis, text) in cases {
        assert_eq!(
            write_utc_timestamp(UtcMillis(millis), &mut out),
            text.as_bytes()
        );
        assert_eq!(
            parse_utc_timestamp(text.as_bytes()),
            Some(UtcMillis(millis))
        );
    }
    assert_eq!(
        parse_utc_timestamp(b"20240229-12:34:56"),
        Some(UtcMillis(1_709_210_096_000))
    );
    assert_eq!(
        parse_utc_timestamp(b"20240229-12:34:56.789123"),
        Some(UtcMillis(1_709_210_096_789))
    );
    for bad in [
        "20230229-00:00:00",
        "20241301-00:00:00",
        "20240101-24:00:00",
        "20240101 00:00:00",
    ] {
        assert_eq!(parse_utc_timestamp(bad.as_bytes()), None, "{bad}");
    }
}

/// A tag that is neither a data field nor its length field.
fn plain_tag() -> impl Strategy<Value = u32> {
    (100u32..10_000).prop_filter("not a data field", |tag| {
        fix_codec::tag::DATA_FIELDS
            .iter()
            .all(|(length, data)| tag != length && tag != data)
    })
}

proptest! {
    #[test]
    fn encoded_fields_decode_back(
        fields in prop::collection::vec((plain_tag(), "[ -~]{1,20}"), 0..20)
    ) {
        let mut encoder = Encoder::new(4096);
        let mut body = encoder.start(b"8");
        for (tag, value) in &fields {
            body.field(*tag, value.as_bytes());
        }
        let bytes = body.finish(b"FIX.4.4").expect("fits").to_vec();
        prop_assert_eq!(frame(&bytes, 1 << 20), Ok(Some(bytes.len())));
        let mut decoder = Decoder::new(64);
        let message = decoder.decode(&bytes).map_err(|error| TestCaseError::fail(format!("{error:?}")))?;
        let decoded: Vec<(u32, Vec<u8>)> =
            message.fields().iter().skip(1).map(|(tag, value)| (tag, value.to_vec())).collect();
        let expected: Vec<(u32, Vec<u8>)> =
            fields.iter().map(|(tag, value)| (*tag, value.as_bytes().to_vec())).collect();
        prop_assert_eq!(decoded, expected);
    }

    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let _ = frame(&bytes, LEN_MAX);
        let _ = skip_garbled(&bytes);
        let _ = Decoder::new(64).decode(&bytes);
        let _ = parse_utc_timestamp(&bytes);
    }

    #[test]
    fn timestamps_round_trip(millis in 0u64..253_402_300_799_999) {
        let mut out = [0u8; 21];
        let text = write_utc_timestamp(UtcMillis(millis), &mut out).to_vec();
        prop_assert_eq!(parse_utc_timestamp(&text), Some(UtcMillis(millis)));
    }
}

#[test]
fn encoder_refuses_a_message_above_its_limit_without_growing() {
    let mut encoder = Encoder::new(128);
    let mut body = encoder.start(b"0");
    body.field(58, &[b'x'; 200]);
    assert_eq!(body.finish(b"FIX.4.4"), Err(fix_codec::TooLarge));
    let mut body = encoder.start(b"0");
    body.field(58, b"short");
    assert!(
        body.finish(b"FIX.4.4").is_ok(),
        "the encoder works again after an error"
    );
}
