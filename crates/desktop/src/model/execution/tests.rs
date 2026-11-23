#![allow(clippy::unwrap_used)]

use fix_codec::{Decoder, Encoder};
use proptest::prelude::*;

use super::automation::{Automation, Response, Rule, Settings};
use super::*;
use crate::store::{Direction, Entry};

fn test_frame(version: Version, kind: &str, body: &[(u32, &str)]) -> Vec<u8> {
    let mut encoder = Encoder::new(1024);
    let mut message = encoder.start(kind.as_bytes());
    message.field(34, b"1");
    for (tag, value) in body {
        message.field(*tag, value.as_bytes());
    }
    message
        .finish(version.begin_string().as_bytes())
        .unwrap()
        .to_vec()
}

fn received(version: Version, kind: &str, body: &[(u32, &str)]) -> Context {
    let raw = test_frame(version, kind, body);
    Context::read(
        version,
        &Entry {
            id: 7,
            time: UtcMillis(0),
            direction: Direction::Received,
            kind: kind.into(),
            seq: "1".into(),
            summary: String::new(),
            status: "Received".into(),
            transferred: raw.len(),
            raw,
        },
    )
    .unwrap()
}

proptest! {
    #[test]
    fn automated_order_chains_preserve_quantities_and_reject_stale_references(
        version in prop::sample::select(vec![Version::Fix42, Version::Fix43, Version::Fix44]),
        initial in 2u64..50,
        replacements in prop::collection::vec(1u64..30, 1..12),
    ) {
        let partial = Rule {
            response: Response::PartialFill,
            quantity: "1".into(),
            ..Rule::default()
        };
        let rules = Settings {
            enabled: true,
            rules: [partial.clone(), partial, Rule {
                response: Response::FullFill,
                ..Rule::default()
            }],
        }.compile().unwrap().unwrap();
        let mut automation = Automation::new(&rules);
        let mut decoder = Decoder::new(64);
        let mut current = "NEW".to_owned();
        let mut quantity = initial;
        let mut filled = 1;
        let raw = test_frame(version, "D", &[
            (11, &current), (55, "XYZ"), (54, "1"), (38, &initial.to_string()),
            (40, "2"), (44, "12"),
        ]);
        let message = decoder.decode(&raw).unwrap();
        let plan = automation.plan(version, &message, || Ok("ORDER".into())).unwrap().unwrap();
        prop_assert_eq!(plan.reports.len(), 2);
        prop_assert_eq!(plan.reports[1].value(14), "1");
        prop_assert_eq!(plan.reports[1].value(151), (initial - 1).to_string());
        automation.apply(plan);

        for (index, amount) in replacements.into_iter().enumerate() {
            let next = format!("REPLACE-{index}");
            let requested = if amount <= filled { amount } else { filled + amount + 2 };
            let raw = test_frame(version, "G", &[
                (11, &next), (41, &current), (37, "ORDER"), (55, "XYZ"), (54, "1"),
                (38, &requested.to_string()), (40, "2"), (44, "12"),
            ]);
            let message = decoder.decode(&raw).unwrap();
            let plan = automation.plan(version, &message, || Ok("unused".into())).unwrap().unwrap();
            let closed = requested <= filled;
            quantity = requested.max(filled);
            prop_assert_eq!(plan.reports.len(), if closed { 1 } else { 2 });
            if !closed {
                filled += 1;
            }
            let report = plan.reports.last().unwrap();
            prop_assert_eq!(report.value(38), quantity.to_string());
            prop_assert_eq!(report.value(14), filled.to_string());
            prop_assert_eq!(report.value(151), (quantity - filled).to_string());
            automation.apply(plan);
            let stale = test_frame(version, "F", &[
                (11, &format!("STALE-{index}")), (41, &current), (54, "1"), (55, "XYZ"),
            ]);
            let message = decoder.decode(&stale).unwrap();
            prop_assert!(automation.plan(version, &message, || Ok("unused".into())).is_err());
            current = next;
            if closed {
                break;
            }
        }

        let raw = test_frame(version, "F", &[(11, "CANCEL"), (41, &current), (54, "1"), (55, "XYZ")]);
        let message = decoder.decode(&raw).unwrap();
        let plan = automation.plan(version, &message, || Ok("unused".into()));
        if quantity == filled {
            prop_assert!(plan.is_err());
        } else {
            let plan = plan.unwrap().unwrap();
            prop_assert_eq!(plan.reports.len(), 2);
            let trade = &plan.reports[0];
            prop_assert_eq!(trade.value(32), (quantity - filled).to_string());
            prop_assert_eq!(trade.value(14), quantity.to_string());
            prop_assert_eq!(trade.value(151), "0");
        }
    }
}

fn fill_inputs(report: &mut Report) {
    let full_fill = report.action == Action::FullFill;
    let order_qty = report.value(38).to_owned();
    for field in report.fields_mut() {
        if field.required && field.value.is_empty() {
            field.value = match field.tag {
                6 | 31 => "10.25",
                14 if full_fill => &order_qty,
                14 | 32 => "2",
                151 => "3",
                19 => "TRADE-1",
                39 => "1",
                378 => "0",
                _ => "",
            }
            .into();
        }
    }
}

#[test]
fn all_execution_actions_use_standard_version_codes() {
    use Action::{
        Acknowledge, Bust, Calculated, Cancel, Correct, DoneForDay, Expire, FullFill, PartialFill,
        PendingCancel, PendingNew, PendingReplace, Reject, Replace, Restate, Status, Stopped,
        Suspended,
    };
    // Each row gives action, source, 4.2 transaction, 4.2 and later execution types, and order status.
    let cases = [
        (PendingNew, "D", "0", "A", "A", "A"),
        (Acknowledge, "D", "0", "0", "0", "0"),
        (Reject, "D", "0", "8", "8", "8"),
        (PartialFill, "D", "0", "1", "F", "1"),
        (FullFill, "D", "0", "2", "F", "2"),
        (PendingCancel, "F", "0", "6", "6", "6"),
        (Cancel, "F", "0", "4", "4", "4"),
        (PendingReplace, "G", "0", "E", "E", "E"),
        (Replace, "G", "0", "5", "5", "5"),
        (Expire, "D", "0", "C", "C", "C"),
        (DoneForDay, "D", "0", "3", "3", "3"),
        (Stopped, "D", "0", "7", "7", "7"),
        (Suspended, "D", "0", "9", "9", "9"),
        (Calculated, "D", "0", "B", "B", "B"),
        (Restate, "D", "0", "D", "D", "1"),
        (Status, "H", "3", "1", "I", "1"),
        (Correct, "D", "2", "1", "G", "1"),
        (Bust, "D", "1", "1", "H", "1"),
    ];
    for version in [Version::Fix42, Version::Fix43, Version::Fix44] {
        for (action, source, trans, legacy, modern, status) in cases {
            let context = received(
                version,
                source,
                &[
                    (11, "CURRENT"),
                    (37, "ORDER-1"),
                    (41, "ORIGINAL"),
                    (54, "1"),
                    (55, "XYZ"),
                    (38, "5"),
                ],
            );
            let mut report = Report::draft(&context, None, action).unwrap();
            for field in report.fields_mut() {
                if field.tag == 38 && field.value.is_empty() {
                    "5".clone_into(&mut field.value);
                }
            }
            fill_inputs(&mut report);
            let (exec_type, transaction) = if version == Version::Fix42 {
                (legacy, Some(trans))
            } else {
                (modern, None)
            };
            let status = if action == Replace && version == Version::Fix44 {
                "1"
            } else {
                status
            };
            assert_report(&report, version, exec_type, transaction, status);
        }
    }
}

fn assert_report(
    report: &Report,
    version: Version,
    exec_type: &str,
    transaction: Option<&str>,
    status: &str,
) {
    use Action::{Bust, Correct, FullFill, PartialFill, Restate, Status};
    let action = report.action;
    let exec_id = if action == Status { "0" } else { "EXEC-2" };
    let app = report.application(UtcMillis(0), exec_id.into()).unwrap();
    assert_eq!(app.kind, "8");
    let legacy_version = version == Version::Fix42;
    let value = |tag| {
        app.fields
            .iter()
            .find(|(num, _)| *num == tag)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(value(39), Some(status), "{version:?} {action:?}");
    assert_eq!(value(150), Some(exec_type));
    assert_eq!(value(20), transaction);
    assert_eq!(
        value(19),
        matches!(action, Correct | Bust).then_some("TRADE-1")
    );
    assert_eq!(value(378), (action == Restate).then_some("0"));
    assert_eq!(value(17), Some(exec_id));
    if matches!(action, PartialFill | FullFill | Correct) {
        assert_eq!(value(31), Some("10.25"));
        assert_eq!(value(32), Some("2"));
    } else if legacy_version && action != Status {
        assert_eq!(value(31), Some("0"));
        assert_eq!(value(32), Some("0"));
    } else {
        assert_eq!(value(31), None);
        assert_eq!(value(32), None);
    }
    let dict = version.dictionary();
    for tag in dict.message("8").unwrap().body.required {
        assert!(value(*tag).is_some(), "{version:?} {action:?} tag {tag}");
    }
    for (tag, data) in &app.fields {
        assert!(
            dict.is_valid_value(*tag, data),
            "{version:?} {action:?} tag {tag}"
        );
    }
}

#[test]
fn request_rejections_keep_chain_and_reject_the_request() {
    for version in [Version::Fix42, Version::Fix43, Version::Fix44] {
        for (kind, action, code) in [
            ("F", Action::RejectCancel, "1"),
            ("G", Action::RejectReplace, "2"),
            ("AC", Action::RejectReplace, "2"),
        ] {
            if version == Version::Fix42 && kind == "AC" {
                continue;
            }
            let context = received(
                version,
                kind,
                &[
                    (11, "REQUEST"),
                    (41, "WORKING"),
                    (37, "OID-3"),
                    (66, "LIST-7"),
                ],
            );
            assert!(Report::draft(&context, None, Action::Reject).is_err());
            let mut report = Report::draft(&context, None, action).unwrap();
            assert!(report.validate().is_err());
            fill_inputs(&mut report);
            let app = report.application(UtcMillis(0), String::new()).unwrap();
            assert_eq!(app.kind, "9");
            let find = |tag| {
                app.fields
                    .iter()
                    .find(|(num, _)| *num == tag)
                    .map(|(_, value)| value.as_str())
            };
            for (tag, value) in [
                (11, "REQUEST"),
                (37, "OID-3"),
                (41, "WORKING"),
                (39, "1"),
                (434, code),
            ] {
                assert_eq!(find(tag), Some(value));
            }
            assert_eq!(
                find(66),
                version
                    .dictionary()
                    .message("9")
                    .unwrap()
                    .body
                    .contains(66)
                    .then_some("LIST-7")
            );
            for tag in [17, 20, 150] {
                assert_eq!(find(tag), None);
            }
        }
    }
}

#[test]
fn fills_and_status_require_explicit_context_when_the_order_has_no_state() {
    let context = received(
        Version::Fix44,
        "D",
        &[
            (11, "REQUEST"),
            (55, "XYZ"),
            (54, "1"),
            (38, "5"),
            (44, "10"),
        ],
    );
    for action in [
        Action::PartialFill,
        Action::FullFill,
        Action::Status,
        Action::Correct,
        Action::Bust,
    ] {
        let mut report = Report::draft(&context, None, action).unwrap();
        assert!(report.validate().is_err());
        for field in report.fields_mut() {
            if matches!(field.tag, 14 | 6 | 31 | 32 | 19) {
                assert_eq!(field.value, "");
            }
        }
    }
    let status = received(
        Version::Fix44,
        "H",
        &[(11, "REQUEST"), (55, "XYZ"), (54, "1")],
    );
    let report = Report::draft(&status, None, Action::Status).unwrap();
    assert_eq!(report.value(37), "");
    assert!(Report::draft(&status, None, Action::Cancel).is_err());
}

fn set_field(report: &mut Report, tag: u32, value: &str) {
    value.clone_into(
        &mut report
            .fields_mut()
            .iter_mut()
            .find(|field| field.tag == tag)
            .unwrap()
            .value,
    );
}

#[test]
fn full_fill_checks_decimal_totals_exactly() {
    let context = received(
        Version::Fix44,
        "D",
        &[
            (11, "FULL"),
            (54, "1"),
            (55, "XYZ"),
            (38, "9007199254740993.000"),
        ],
    );
    let mut report = Report::draft(&context, None, Action::FullFill).unwrap();
    fill_inputs(&mut report);
    set_field(&mut report, 14, "9007199254740992.999999999");
    assert!(report.validate().is_err());
    set_field(&mut report, 14, "09007199254740993.0");
    assert!(report.validate().is_ok());
    set_field(&mut report, 151, "0.000000001");
    assert!(report.validate().is_err());
}

#[test]
fn gtd_requires_a_valid_expiry_and_accepts_either_supported_field() {
    for version in Version::ALL {
        let context = received(
            version,
            "D",
            &[
                (11, "GTD"),
                (54, "1"),
                (55, "XYZ"),
                (38, "5"),
                (59, "6"),
                (432, "20271231"),
            ],
        );
        let mut report = Report::draft(&context, None, Action::Acknowledge).unwrap();
        assert_eq!(report.value(432), "20271231");
        assert!(report.validate().is_ok());
        set_field(&mut report, 432, "");
        assert!(report.validate().is_err());
        set_field(&mut report, 126, "20271231-15:00:00");
        let app = report.application(UtcMillis(0), "EX-1".into()).unwrap();
        assert!(app.fields.contains(&(126, "20271231-15:00:00".into())));
        set_field(&mut report, 126, "20270229-15:00:00");
        assert!(report.validate().is_err());
    }
}

#[test]
fn cancel_reject_can_take_a_manual_list_id() {
    let context = received(
        Version::Fix44,
        "F",
        &[(11, "CANCEL"), (41, "CHILD"), (37, "VENUE")],
    );
    let mut report = Report::draft(&context, None, Action::RejectCancel).unwrap();
    fill_inputs(&mut report);
    set_field(&mut report, 66, "LIST-MANUAL");
    let app = report.application(UtcMillis(0), String::new()).unwrap();
    assert!(app.fields.contains(&(66, "LIST-MANUAL".into())));
}
