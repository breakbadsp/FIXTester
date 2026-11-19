//! Tests of `validate`: each kind of error, and the strictness switches.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{MULTILEG_BODY, SINGLE_BODY, wire, with_header};
use fix_codec::{Decoder, GroupError};
use fix_dict::schema::FieldType;
use fix_dict::{
    ERRORS_MAX, Profile, Strictness, ValidationError, ValidationErrors, VenueDictionary, Version,
    validate,
};
use std::fmt::Write as _;

/// Decodes `body` as a message of `msg_type` and validates it.
fn check(
    venue: &VenueDictionary,
    msg_type: &str,
    body: &str,
    strictness: Strictness,
) -> Result<(), Vec<ValidationError>> {
    let bytes = wire(venue.version().begin_string(), &with_header(msg_type, body));
    let mut decoder = Decoder::new(256);
    let message = decoder.decode(&bytes).unwrap();
    let mut errors = ValidationErrors::new();
    validate(venue, &message, strictness, &mut errors).map_err(|found| found.as_slice().to_vec())
}

fn fix44() -> VenueDictionary {
    VenueDictionary::standard(Version::Fix44)
}

#[test]
fn real_messages_are_valid_in_both_strictness_levels() {
    for strictness in [Strictness::LENIENT, Strictness::STRICT] {
        assert_eq!(check(&fix44(), "AB", MULTILEG_BODY, strictness), Ok(()));
        assert_eq!(check(&fix44(), "D", SINGLE_BODY, strictness), Ok(()));
    }
}

#[test]
fn a_missing_required_field_is_reported() {
    let body = "54=1|60=20261118-14:30:05.100|40=2|55=IBM|";
    assert_eq!(
        check(&fix44(), "D", body, Strictness::LENIENT),
        Err(vec![ValidationError::MissingRequired { tag: 11 }])
    );
}

#[test]
fn a_missing_header_field_is_reported() {
    let bytes = wire(
        "FIX.4.4",
        "35=D|49=FIXSIM|56=VENUE|52=20261118-14:30:05.123|11=A|54=1|60=20261118-14:30:05.100|40=1|",
    );
    let mut decoder = Decoder::new(64);
    let message = decoder.decode(&bytes).unwrap();
    let mut errors = ValidationErrors::new();
    let found = validate(&fix44(), &message, Strictness::LENIENT, &mut errors).unwrap_err();
    assert_eq!(
        found.as_slice(),
        [ValidationError::MissingRequired { tag: 34 }]
    );
}

#[test]
fn a_required_field_inside_a_group_does_not_count_as_present() {
    // The profile group NoOrders has ClOrdID (11) as a member. The message
    // has tag 11 only in its group entry, not at its top level.
    let venue = Profile::load(
        "version = \"FIX.4.4\"\n[group.NoOrders]\ncount = 73\nmembers = [11, 55]\n\
         [message.D]\nadd = [\"NoOrders\"]\n",
    )
    .unwrap();
    let body = "54=1|60=20261118-14:30:05.100|40=2|73=1|11=ORD1|55=IBM|";
    assert_eq!(
        check(&venue, "D", body, Strictness::STRICT),
        Err(vec![ValidationError::MissingRequired { tag: 11 }])
    );
    let with_top_level = format!("11=ORD0|{body}");
    assert_eq!(
        check(&venue, "D", &with_top_level, Strictness::STRICT),
        Ok(())
    );
}

#[test]
fn an_unlisted_field_is_an_error_only_in_strict_mode() {
    // BeginSeqNo (7) is a known tag, but NewOrderSingle does not list it.
    let body = format!("{SINGLE_BODY}7=1|");
    assert_eq!(check(&fix44(), "D", &body, Strictness::LENIENT), Ok(()));
    assert_eq!(
        check(&fix44(), "D", &body, Strictness::STRICT),
        Err(vec![ValidationError::FieldNotAllowed { tag: 7 }])
    );
    let only_unlisted = Strictness {
        reject_unlisted_fields: true,
        reject_unknown_tags: false,
    };
    assert_eq!(
        check(&fix44(), "D", &body, only_unlisted),
        Err(vec![ValidationError::FieldNotAllowed { tag: 7 }])
    );
}

#[test]
fn an_unknown_tag_is_an_error_when_the_session_says_so() {
    let body = format!("{SINGLE_BODY}9999=x|");
    assert_eq!(check(&fix44(), "D", &body, Strictness::LENIENT), Ok(()));
    let reject_unknown = Strictness {
        reject_unlisted_fields: false,
        reject_unknown_tags: true,
    };
    assert_eq!(
        check(&fix44(), "D", &body, reject_unknown),
        Err(vec![ValidationError::UnknownTag { tag: 9999 }])
    );
}

#[test]
fn a_value_that_is_not_in_the_list_of_values_is_an_error() {
    let body = SINGLE_BODY.replace("54=1", "54=Z");
    assert_eq!(
        check(&fix44(), "D", &body, Strictness::LENIENT),
        Err(vec![ValidationError::ValueNotInEnum { tag: 54 }])
    );
    // A value inside a group entry too: PartyIDSource (447).
    let body = SINGLE_BODY.replace("447=D", "447=!");
    assert_eq!(
        check(&fix44(), "D", &body, Strictness::LENIENT),
        Err(vec![ValidationError::ValueNotInEnum { tag: 447 }])
    );
}

#[test]
fn a_time_in_force_value_depends_on_the_version() {
    let body = "11=A|54=1|60=20261118-14:30:05.100|40=1|59=7|21=1|55=IBM|38=1|";
    let fix42 = VenueDictionary::standard(Version::Fix42);
    assert_eq!(
        check(&fix42, "D", body, Strictness::LENIENT),
        Err(vec![ValidationError::ValueNotInEnum { tag: 59 }])
    );
    assert_eq!(check(&fix44(), "D", body, Strictness::LENIENT), Ok(()));
}

#[test]
fn a_value_with_the_wrong_syntax_for_its_type_is_an_error() {
    let cases = [
        (38, "abc", FieldType::Qty),
        (44, "1e5", FieldType::Price),
        (43, "X", FieldType::Boolean),
        (52, "2026-11-18", FieldType::UtcTimestamp),
        (126, "20261318-10:00:00", FieldType::UtcTimestamp),
        (40, "12", FieldType::Char),
    ];
    for (tag, value, ty) in cases {
        let replaced = SINGLE_BODY.replace("59=0|", "");
        let body = format!("{replaced}{tag}={value}|");
        let errors = check(&fix44(), "D", &body, Strictness::LENIENT).unwrap_err();
        assert!(
            errors.contains(&ValidationError::ValueNotOfType { tag, ty }),
            "tag {tag} value {value}: {errors:?}"
        );
    }
}

#[test]
fn a_group_count_that_does_not_match_the_entries_is_an_error() {
    let too_many = SINGLE_BODY.replace("453=1", "453=2");
    assert_eq!(
        check(&fix44(), "D", &too_many, Strictness::STRICT),
        Err(vec![ValidationError::Group(GroupError::CountMismatch {
            count_tag: 453,
            declared: 2,
            found: 1
        })])
    );
    let too_few = MULTILEG_BODY.replace("555=2", "555=1");
    let errors = check(&fix44(), "AB", &too_few, Strictness::STRICT).unwrap_err();
    assert_eq!(
        errors,
        vec![ValidationError::Group(GroupError::CountMismatch {
            count_tag: 555,
            declared: 1,
            found: 2
        })],
        "the fields of the entries are not reported as fields of the message"
    );
}

#[test]
fn a_group_count_that_is_not_a_number_is_one_error() {
    let body = SINGLE_BODY.replace("453=1", "453=x");
    assert_eq!(
        check(&fix44(), "D", &body, Strictness::STRICT),
        Err(vec![ValidationError::ValueNotOfType {
            tag: 453,
            ty: FieldType::NumInGroup
        }])
    );
}

#[test]
fn an_unknown_message_type_is_an_error() {
    assert_eq!(
        check(&fix44(), "U9", "58=x|", Strictness::LENIENT),
        Err(vec![ValidationError::UnknownMessageType])
    );
    // AB is a FIX 4.3 and 4.4 message.
    let fix42 = VenueDictionary::standard(Version::Fix42);
    assert_eq!(
        check(&fix42, "AB", MULTILEG_BODY, Strictness::LENIENT),
        Err(vec![ValidationError::UnknownMessageType])
    );
}

#[test]
fn a_profile_adds_required_and_forbidden_fields_and_values() {
    let venue = Profile::load(include_str!("profiles/valid.toml")).unwrap();
    let valid = format!("{SINGLE_BODY}1=ACC|9001=VWAP|");
    assert_eq!(check(&venue, "D", &valid, Strictness::STRICT), Ok(()));

    let missing = format!("{SINGLE_BODY}1=ACC|");
    assert_eq!(
        check(&venue, "D", &missing, Strictness::LENIENT),
        Err(vec![ValidationError::MissingRequired { tag: 9001 }])
    );
    let bad_value = format!("{SINGLE_BODY}1=ACC|9001=XYZ|");
    assert_eq!(
        check(&venue, "D", &bad_value, Strictness::LENIENT),
        Err(vec![ValidationError::ValueNotInEnum { tag: 9001 }])
    );
    // The profile forbids ExDestination (100), also in lenient mode.
    let forbidden = format!("{SINGLE_BODY}1=ACC|9001=TWAP|100=XNAS|");
    assert_eq!(
        check(&venue, "D", &forbidden, Strictness::LENIENT),
        Err(vec![ValidationError::ForbiddenField { tag: 100 }])
    );
    // Without the profile, tag 9001 is unknown.
    let standard = fix44();
    assert_eq!(check(&standard, "D", &valid, Strictness::LENIENT), Ok(()));
    assert_eq!(
        check(&standard, "D", &valid, Strictness::STRICT),
        Err(vec![ValidationError::UnknownTag { tag: 9001 }])
    );
}

#[test]
fn a_profile_group_replaces_the_standard_group_of_its_message() {
    let venue = Profile::load(include_str!("profiles/valid.toml")).unwrap();
    // The profile NoLegs has no LegRatioQty (623) member, so each leg
    // entry ends at tag 623 and the count does not match.
    let errors = check(&venue, "AB", MULTILEG_BODY, Strictness::STRICT).unwrap_err();
    assert_eq!(
        errors[0],
        ValidationError::Group(GroupError::CountMismatch {
            count_tag: 555,
            declared: 2,
            found: 1
        })
    );
    // The same message without tag 623 matches the profile group. LegRefID
    // (654) is a member of the profile group only in the venue dictionary.
    let body = MULTILEG_BODY
        .replace("623=1|", "")
        .replace("624=1|", "624=1|654=LEGREF|");
    assert_eq!(check(&venue, "AB", &body, Strictness::STRICT), Ok(()));
    // The standard dictionary has LegRefID (654) in the group too, and 623.
    assert_eq!(
        check(&fix44(), "AB", MULTILEG_BODY, Strictness::STRICT),
        Ok(())
    );
}

#[test]
fn the_number_of_errors_is_limited() {
    let body = format!("{SINGLE_BODY}{}", "38=abc|".repeat(ERRORS_MAX + 6));
    let bytes = wire("FIX.4.4", &with_header("D", &body));
    let mut decoder = Decoder::new(256);
    let message = decoder.decode(&bytes).unwrap();
    let mut errors = ValidationErrors::new();
    let found = validate(&fix44(), &message, Strictness::LENIENT, &mut errors).unwrap_err();
    assert_eq!(found.as_slice().len(), ERRORS_MAX);
    assert_eq!(found.extra(), 6);
}

#[test]
fn the_error_buffer_is_reused_and_cleared_for_each_message() {
    let mut errors = ValidationErrors::new();
    let mut decoder = Decoder::new(256);
    let bad = wire(
        "FIX.4.4",
        &with_header("D", &SINGLE_BODY.replace("54=1", "54=Z")),
    );
    let good = wire("FIX.4.4", &with_header("D", SINGLE_BODY));
    let message = decoder.decode(&bad).unwrap();
    assert!(validate(&fix44(), &message, Strictness::LENIENT, &mut errors).is_err());
    let message = decoder.decode(&good).unwrap();
    assert!(validate(&fix44(), &message, Strictness::LENIENT, &mut errors).is_ok());
    assert!(errors.is_empty() && errors.extra() == 0);
}

#[test]
fn a_data_field_with_bytes_that_are_not_utf8_is_valid() {
    // A FIX 4.4 Logon with RawDataLength=1 and RawData=0xFF. Tag 96 has no
    // list of values, so the bytes are not text.
    let mut body = with_header("A", "98=0|108=30|95=1|96=").into_bytes();
    body.extend_from_slice(&[0xFF, b'|']);
    let bytes = common::wire_bytes("FIX.4.4", &body);
    let mut decoder = Decoder::new(64);
    let message = decoder.decode(&bytes).unwrap();
    for strictness in [Strictness::LENIENT, Strictness::STRICT] {
        let mut errors = ValidationErrors::new();
        assert!(validate(&fix44(), &message, strictness, &mut errors).is_ok());
    }
}

#[test]
fn an_empty_group_does_not_hide_the_next_field() {
    // 453=0 has no entries. TransactTime (60) after it is a top-level field,
    // and the forbidden ExDestination (100) after that must be reported.
    let venue =
        Profile::load("version = \"FIX.4.4\"\n[message.D]\nforbidden = [\"ExDestination\"]\n")
            .unwrap();
    let body = "11=A|54=1|40=1|453=0|60=20261118-14:30:05.100|100=XNAS|";
    assert_eq!(
        check(&venue, "D", body, Strictness::STRICT),
        Err(vec![ValidationError::ForbiddenField { tag: 100 }])
    );
    let without = "11=A|54=1|40=1|453=0|60=20261118-14:30:05.100|";
    assert_eq!(check(&venue, "D", without, Strictness::STRICT), Ok(()));
}

/// A profile with a group NoOrders(73) that nests an empty-able NoPartyIDs
/// (453), and a top-level NoPartyIDs in message D.
fn nested_party_venue(extra: &str) -> VenueDictionary {
    Profile::load(&format!(
        "version = \"FIX.4.4\"\n\
         [group.NoPartyIDs]\ncount = 453\nmembers = [448, 447, 452]\n\
         [group.NoOrders]\ncount = 73\nmembers = [11, \"NoPartyIDs\"]\n\
         [message.D]\nadd = [\"NoOrders\", \"NoPartyIDs\"]\n{extra}"
    ))
    .unwrap()
}

#[test]
fn a_group_is_read_where_it_is_when_its_count_tag_appears_twice() {
    // The nested 453 is empty. The top-level 453 has one entry.
    let body = "11=A|54=1|40=1|60=20261118-14:30:05.100|\
                73=1|11=B|453=0|44=1.5|453=1|448=P|447=D|452=1|";
    let venue = nested_party_venue("");
    assert_eq!(check(&venue, "D", body, Strictness::STRICT), Ok(()));
    // A forbidden field after the groups is still seen.
    let venue = nested_party_venue("forbidden = [\"ExDestination\"]\n");
    let with_forbidden = format!("{body}100=XNAS|");
    assert_eq!(
        check(&venue, "D", &with_forbidden, Strictness::STRICT),
        Err(vec![ValidationError::ForbiddenField { tag: 100 }])
    );
}

#[test]
fn the_recovery_after_a_group_error_is_fast_for_shared_nested_groups() {
    // Level i has the groups A_i and B_i. Both nest A_(i+1) and B_(i+1): a
    // walk of the tree has 2^levels paths. The lookup of the tags is linear.
    let levels = 30;
    let mut text = String::from("version = \"FIX.4.4\"\n");
    for level in 0..levels {
        for (side, tag) in [("A", 5000 + 2 * level), ("B", 5001 + 2 * level)] {
            write!(
                text,
                "[[field]]\ntag = {tag}\nname = \"Count{side}{level}\"\ntype = \"NUMINGROUP\"\n"
            )
            .unwrap();
        }
    }
    for level in 0..levels {
        for (side, tag) in [("A", 5000 + 2 * level), ("B", 5001 + 2 * level)] {
            let next = if level + 1 < levels {
                format!(", \"CountA{}\", \"CountB{}\"", level + 1, level + 1)
            } else {
                String::new()
            };
            write!(
                text,
                "[group.Count{side}{level}]\ncount = {tag}\nmembers = [605{next}]\n"
            )
            .unwrap();
        }
    }
    text.push_str("[message.D]\nadd = [\"CountA0\"]\n");
    let venue = Profile::load(&text).unwrap();
    // The count says 2, but there is one entry. Tag 55 is not in the group.
    let body = "11=A|54=1|40=1|60=20261118-14:30:05.100|5000=2|605=x|5002=1|605=z|55=IBM|";
    let errors = check(&venue, "D", body, Strictness::LENIENT).unwrap_err();
    assert_eq!(
        errors,
        [ValidationError::Group(GroupError::CountMismatch {
            count_tag: 5000,
            declared: 2,
            found: 1
        })]
    );
}
