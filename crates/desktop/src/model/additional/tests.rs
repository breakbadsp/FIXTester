#![allow(clippy::unwrap_used)]

use std::fmt::Write as _;

use fix_dict::Version;

use super::{Application, MESSAGE_MAX};

fn body(kind: &str, fields: &[(u32, &str)]) -> Application {
    Application::new(
        Version::Fix44,
        kind,
        fields
            .iter()
            .map(|(tag, value)| (*tag, (*value).to_owned()))
            .collect(),
    )
    .unwrap()
}

#[test]
fn additions_keep_order_and_preserve_value_bytes_inside_pairs() {
    let message = body("8", &[(150, "F"), (555, "1"), (600, "FUT")])
        .with_additional_tags(" 29 = 2 ; 58=Trade  price=7 ; 9001=Vendor value; ")
        .unwrap();
    assert_eq!(
        message.fields,
        [
            (150, "F"),
            (29, "2"),
            (58, "Trade  price=7"),
            (9001, "Vendor value"),
            (555, "1"),
            (600, "FUT"),
        ]
        .map(|(tag, value)| (tag, value.to_owned()))
    );
}

#[test]
fn additions_reject_reserved_duplicate_protected_and_group_tags() {
    for text in [
        "35=8",
        "34=7",
        "108=30",
        "112=REQUEST",
        "10=123",
        "95=4",
        "96=data",
        "0=zero",
        "29=café",
        "29=\x01",
        "29=\t2",
        "29=2;029=1",
        "29=2;;58=Text",
        "29",
        "29=",
        "39=2",
        "11=OTHER",
        "19=TRADE",
        "32=2",
        "55=OTHER",
        "555=1",
        "600=LEG",
        "605=ALT",
        "58=Second text",
    ] {
        let message = body("8", &[(55, "XYZ"), (58, "Form text")]);
        assert!(message.with_additional_tags(text).is_err(), "{text:?}");
    }
    for (kind, text) in [
        ("AB", "623=2"),
        ("E", "11=OTHER"),
        ("E", "73=2"),
        ("G", "448=PARTY"),
        ("F", "41=OTHER"),
        ("9", "434=2"),
    ] {
        assert!(
            body(kind, &[]).with_additional_tags(text).is_err(),
            "{kind} {text}"
        );
    }
}

#[test]
fn additions_obey_application_limits() {
    let value = format!("58={}", "A".repeat(super::super::TEXT_MAX + 1));
    assert!(body("D", &[]).with_additional_tags(&value).is_err());
    let spaces = " ".repeat(MESSAGE_MAX + 1);
    assert!(body("D", &[]).with_additional_tags(&spaces).is_err());
    let mut pairs = String::new();
    for tag in 9000..9256 {
        write!(pairs, "{tag}=A;").unwrap();
    }
    assert!(body("D", &[]).with_additional_tags(&pairs).is_err());
}
