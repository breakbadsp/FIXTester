//! Tests of venue profiles: what a profile changes, and each profile error
//! with its place in the file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use fix_codec::Member;
use fix_dict::schema::FieldType;
use fix_dict::{Profile, ProfileError, ProfileErrorKind as Kind, Version, describe};
use std::fmt::Write as _;

fn errors(text: &str) -> Vec<ProfileError> {
    Profile::load(text).unwrap_err()
}

/// The `(line, column, kind)` of each error.
fn places(text: &str) -> Vec<(u32, u32, Kind)> {
    errors(text)
        .into_iter()
        .map(|error| (error.line, error.column, error.kind))
        .collect()
}

#[test]
fn the_example_profile_of_the_requirements_loads() {
    let venue = Profile::load(include_str!("profiles/valid.toml")).unwrap();
    assert_eq!(venue.version(), Version::Fix44);
    let strategy = venue.field(9001).unwrap();
    assert_eq!(strategy.name, "VenueStrategy");
    assert_eq!(strategy.ty, FieldType::String);
    assert_eq!(strategy.values().collect::<Vec<_>>(), ["VWAP", "TWAP"]);
    assert_eq!(venue.field_by_name("VenueStrategy").unwrap().tag, 9001);
    assert_eq!(venue.profile_default(1), Some("TEST01"));
    assert_eq!(venue.profile_default(21), Some("1"));
    let order = venue.message("D").unwrap();
    assert_eq!(order.profile_required(), [1, 9001]);
    assert_eq!(order.profile_forbidden(), [100]);
    assert!(order.is_required(9001) && order.is_required(11));
    assert!(!order.allows(100) && order.body_allows(9001));
    assert_eq!(venue.changed_messages().collect::<Vec<_>>(), ["AB", "D"]);
}

#[test]
fn names_of_groups_become_nested_groups_and_the_first_member_is_the_delimiter() {
    let venue = Profile::load(include_str!("profiles/valid.toml")).unwrap();
    let legs = venue.group("AB", 555).unwrap();
    assert_eq!(legs.count_tag, 555);
    let tags: Vec<_> = legs
        .members
        .iter()
        .map(|member| match member {
            Member::Field(tag) => *tag,
            Member::Group(nested) => nested.count_tag,
        })
        .collect();
    assert_eq!(tags, [600, 608, 624, 654, 604]);
    let Member::Group(alt) = legs.members[4] else {
        panic!("the last member is a group");
    };
    assert_eq!(alt.members.len(), 2);
    // The standard dictionary keeps its own, larger group.
    let standard = venue.standard_dictionary().group("AB", 555).unwrap();
    assert!(standard.members.len() > legs.members.len());
    // D has no profile group for tag 555: it has no group 555 at all.
    assert!(venue.group("D", 555).is_none());
    assert!(venue.group("AB", 453).is_some(), "standard groups stay");
}

#[test]
fn a_group_can_be_used_in_several_messages_and_in_other_groups() {
    let venue = Profile::load(
        "version = \"FIX.4.4\"\n\
         [group.NoPartyIDs]\ncount = 453\nmembers = [448, 447, 452, \"NoPartySubIDs\"]\n\
         [group.NoPartySubIDs]\ncount = 802\nmembers = [523, 803]\n\
         [message.D]\nadd = [\"NoPartyIDs\"]\n\
         [message.G]\nrequired = [\"NoPartyIDs\"]\n",
    )
    .unwrap();
    let in_d = venue.group("D", 453).unwrap();
    let in_g = venue.group("G", 453).unwrap();
    assert!(std::ptr::eq(in_d, in_g), "one definition for both messages");
    assert!(venue.message("G").unwrap().is_body_required(453));
    assert!(venue.message("D").unwrap().body_allows(453));
}

#[test]
fn a_profile_adds_values_to_a_standard_field() {
    let venue = Profile::load(
        "version = \"FIX.4.2\"\n[[field]]\ntag = 59\nvalues = [\"S\"]\n\
         [defaults]\nTimeInForce = \"S\"\n",
    )
    .unwrap();
    let tif = venue.field(59).unwrap();
    assert!(tif.accepts_value("0") && tif.accepts_value("S"));
    assert!(!tif.accepts_value("7"), "FIX 4.2 has no AtTheClose");
    assert_eq!(tif.name, "TimeInForce");
    let standard = Version::Fix42.dictionary().field(59).unwrap();
    assert!(
        !standard.accepts_value("S"),
        "the standard table does not change"
    );
}

#[test]
fn a_new_field_can_have_any_fix_type_in_any_letter_case() {
    let venue = Profile::load(
        "version = \"FIX.4.3\"\n[[field]]\ntag = 5001\nname = \"Slice\"\ntype = \"qty\"\n\
         [[field]]\ntag = 5002\nname = \"Flag\"\ntype = \"Boolean\"\n",
    )
    .unwrap();
    assert_eq!(venue.field(5001).unwrap().ty, FieldType::Qty);
    assert_eq!(venue.field(5002).unwrap().ty, FieldType::Boolean);
    assert_eq!(venue.custom_fields().count(), 2);
}

#[test]
fn an_unknown_version_is_an_error() {
    assert_eq!(
        places(include_str!("profiles/unknown_version.toml")),
        [(1, 11, Kind::UnknownVersion("FIX.5.0".to_owned()))]
    );
}

#[test]
fn an_unknown_field_name_is_an_error_with_its_place() {
    assert_eq!(
        places(include_str!("profiles/unknown_field.toml")),
        [(4, 24, Kind::UnknownField("NoSuchField".to_owned()))]
    );
    let in_defaults = places("version = \"FIX.4.4\"\n[defaults]\nNoSuch = \"1\"\n");
    assert_eq!(
        in_defaults,
        [(3, 10, Kind::UnknownField("NoSuch".to_owned()))]
    );
}

#[test]
fn an_unknown_tag_is_an_error() {
    let text = "version = \"FIX.4.4\"\n[message.D]\nrequired = [31337]\n";
    assert_eq!(places(text), [(3, 13, Kind::UnknownTag(31337))]);
}

#[test]
fn a_group_without_a_block_is_an_unknown_group() {
    assert_eq!(
        places(include_str!("profiles/unknown_group.toml")),
        [(5, 17, Kind::UnknownGroup("NoLegSecurityAltID".to_owned()))]
    );
}

#[test]
fn a_group_loop_is_an_error() {
    // Each group of the cycle is reported at its own member.
    let found = places(include_str!("profiles/group_loop.toml"));
    assert_eq!(
        found.iter().map(|(line, _, _)| *line).collect::<Vec<_>>(),
        [5, 9]
    );
    for (_, _, kind) in &found {
        let Kind::GroupLoop(path) = kind else {
            panic!("{kind:?}");
        };
        assert_eq!(path.first(), path.last(), "the path is a cycle");
        assert_eq!(path.len(), 3);
    }
    let own = "version = \"FIX.4.4\"\n[group.NoLegs]\ncount = 555\nmembers = [600, \"NoLegs\"]\n";
    assert!(matches!(&errors(own)[0].kind, Kind::GroupLoop(path) if path.len() == 2));
}

#[test]
fn a_tag_twice_in_a_group_is_an_error() {
    assert_eq!(
        places(include_str!("profiles/duplicate_tag.toml")),
        [(
            5,
            22,
            Kind::DuplicateInGroup {
                group: "NoLegSecurityAltID".to_owned(),
                member: "605".to_owned()
            }
        )]
    );
}

#[test]
fn a_group_without_members_is_an_error() {
    assert_eq!(
        places(include_str!("profiles/empty_group.toml")),
        [
            (
                3,
                1,
                Kind::GroupWithoutMembers("NoLegSecurityAltID".to_owned())
            ),
            (7, 1, Kind::GroupWithoutMembers("NoLegs".to_owned())),
        ]
    );
}

#[test]
fn a_group_cannot_start_with_a_group() {
    let text = "version = \"FIX.4.4\"\n[group.A]\ncount = 555\nmembers = [\"B\", 600]\n\
                [group.B]\ncount = 604\nmembers = [605]\n";
    assert_eq!(
        places(text),
        [(4, 12, Kind::GroupStartsWithGroup("A".to_owned()))]
    );
}

#[test]
fn a_count_tag_must_be_a_num_in_group_field() {
    let text = "version = \"FIX.4.4\"\n[group.A]\ncount = 600\nmembers = [605]\n";
    assert_eq!(
        places(text),
        [(
            3,
            9,
            Kind::CountNotNumInGroup {
                group: "A".to_owned(),
                tag: 600
            }
        )]
    );
}

#[test]
fn a_field_that_is_required_and_forbidden_is_an_error() {
    assert_eq!(
        places(include_str!("profiles/required_and_forbidden.toml")),
        [
            (
                5,
                14,
                Kind::RequiredAndForbidden {
                    msg_type: "D".to_owned(),
                    tag: 1
                }
            ),
            // ClOrdID is required by the standard.
            (
                5,
                25,
                Kind::RequiredAndForbidden {
                    msg_type: "D".to_owned(),
                    tag: 11
                }
            ),
        ]
    );
    let text =
        "version = \"FIX.4.4\"\n[message.D]\nadd = [\"Account\"]\nforbidden = [\"Account\"]\n";
    assert_eq!(
        places(text),
        [(
            4,
            14,
            Kind::AddedAndForbidden {
                msg_type: "D".to_owned(),
                tag: 1
            }
        )]
    );
}

#[test]
fn a_default_that_is_not_a_valid_value_is_an_error() {
    assert_eq!(
        places(include_str!("profiles/bad_default.toml")),
        [
            (
                4,
                13,
                Kind::EnumDefaultInvalid {
                    tag: 21,
                    value: "9".to_owned()
                }
            ),
            (
                6,
                9,
                Kind::ValueNotOfType {
                    tag: 44,
                    value: "abc".to_owned(),
                    ty: FieldType::Price
                }
            ),
        ]
    );
}

#[test]
fn an_unknown_message_type_and_an_unknown_field_type_are_errors() {
    let text = "version = \"FIX.4.4\"\n[message.ZZ]\nrequired = []\n\
                [[field]]\ntag = 5001\nname = \"A\"\ntype = \"BLOB\"\n";
    let found = places(text);
    assert_eq!(found[0], (2, 1, Kind::UnknownMessage("ZZ".to_owned())));
    assert_eq!(found[1], (7, 8, Kind::UnknownType("BLOB".to_owned())));
}

#[test]
fn field_entries_that_conflict_are_errors() {
    let text = "version = \"FIX.4.4\"\n\
                [[field]]\ntag = 5001\nname = \"Account\"\ntype = \"STRING\"\n\
                [[field]]\ntag = 5002\nname = \"B\"\ntype = \"STRING\"\n\
                [[field]]\ntag = 5002\nname = \"C\"\ntype = \"STRING\"\n\
                [[field]]\ntag = 5003\ntype = \"STRING\"\n\
                [[field]]\ntag = 54\nname = \"Side\"\ntype = \"INT\"\n";
    let found = places(text);
    assert_eq!(
        found,
        [
            (
                4,
                8,
                Kind::FieldConflict {
                    tag: 5001,
                    name: "Account".to_owned()
                }
            ),
            (11, 7, Kind::DuplicateField(5002)),
            (15, 7, Kind::MissingKey("name")),
            (
                20,
                8,
                Kind::FieldConflict {
                    tag: 54,
                    name: "Side".to_owned()
                }
            ),
        ]
    );
}

#[test]
fn a_toml_error_has_a_place() {
    let found = errors("version = \"FIX.4.4\"\n[defaults\nHandlInst = \"1\"\n");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 2);
    assert!(matches!(found[0].kind, Kind::Syntax(_)));
    // A key that the profile does not know.
    let found = errors("version = \"FIX.4.4\"\ncolour = \"red\"\n");
    assert_eq!((found[0].line, found[0].column), (2, 1));
    // A value of the wrong type.
    let found = errors("version = \"FIX.4.4\"\n[defaults]\nHandlInst = 1\n");
    assert_eq!(found[0].line, 3);
}

#[test]
fn all_errors_are_reported_in_file_order() {
    let found = places(
        "version = \"FIX.4.4\"\n[defaults]\nNoSuch = \"1\"\nHandlInst = \"9\"\n\
         [message.D]\nrequired = [\"AlsoNoSuch\"]\n",
    );
    let lines: Vec<u32> = found.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, [3, 4, 6]);
}

#[test]
fn an_error_prints_its_place() {
    let error = errors(include_str!("profiles/unknown_version.toml")).remove(0);
    assert_eq!(
        error.to_string(),
        "1:11: unknown version \"FIX.5.0\", use FIX.4.2, FIX.4.3, or FIX.4.4"
    );
}

#[test]
fn the_report_shows_the_group_tree_of_each_changed_message() {
    let venue = Profile::load(include_str!("profiles/valid.toml")).unwrap();
    let report = describe(&venue);
    assert!(report.contains("version FIX.4.4"));
    assert!(report.contains("default Account(1) = \"TEST01\""));
    assert!(report.contains("message AB (NewOrderMultileg)"));
    assert!(report.contains("  group NoLegs(555) [1]\n    LegSymbol(600)  (delimiter)\n"));
    assert!(report.contains(
        "    group NoLegSecurityAltID(604) [2]\n      LegSecurityAltID(605)  (delimiter)\n"
    ));
    assert!(
        report
            .contains("message D (NewOrderSingle)\n  required: Account(1), VenueStrategy(9001)\n")
    );
}

#[test]
fn a_fix_42_profile_can_replace_a_standard_group_whose_count_is_an_int() {
    // FIX 4.2 types NoOrders (73) as Int, but NewOrderList has it as a group.
    let venue = Profile::load(
        "version = \"FIX.4.2\"\n[group.NoOrders]\ncount = 73\nmembers = [11, 67, 55, 54]\n\
         [message.E]\nadd = [\"NoOrders\"]\n",
    )
    .unwrap();
    assert_eq!(venue.field(73).unwrap().ty, FieldType::Int);
    assert_eq!(venue.group("E", 73).unwrap().members.len(), 4);
    // A tag that no group of the version uses is still not a count tag.
    let found = places("version = \"FIX.4.2\"\n[group.A]\ncount = 55\nmembers = [11]\n");
    assert_eq!(
        found,
        [(
            3,
            9,
            Kind::CountNotNumInGroup {
                group: "A".to_owned(),
                tag: 55
            }
        )]
    );
}

#[test]
fn a_self_reference_is_reported_when_another_group_is_invalid() {
    let text = "version = \"FIX.4.4\"\n\
                [group.NoLegs]\ncount = 555\nmembers = [600, \"NoLegs\"]\n\
                [group.NoLegSecurityAltID]\ncount = 604\nmembers = []\n";
    let found = places(text);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|(line, _, kind)| *line == 4 && matches!(kind, Kind::GroupLoop(_)))
    );
    assert!(
        found
            .iter()
            .any(|(_, _, kind)| matches!(kind, Kind::GroupWithoutMembers(_)))
    );
}

#[test]
fn the_report_names_a_shared_group_after_its_first_use() {
    // Level i has the groups A_i and B_i. Both nest A_(i+1) and B_(i+1), so
    // the tree has 2^levels paths but 2*levels groups.
    let levels = 20;
    let mut text = String::from("version = \"FIX.4.4\"\n");
    for level in 0..levels {
        write!(
            text,
            "[[field]]\ntag = {}\nname = \"CountA{level}\"\ntype = \"NUMINGROUP\"\n\
             [[field]]\ntag = {}\nname = \"CountB{level}\"\ntype = \"NUMINGROUP\"\n",
            5000 + 2 * level,
            5001 + 2 * level
        )
        .unwrap();
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
    text.push_str("[message.D]\nadd = [\"CountA0\", \"CountB0\"]\n");
    let venue = Profile::load(&text).unwrap();
    let report = describe(&venue);
    assert!(report.contains("(see ["));
    assert!(
        report.lines().count() < 1000,
        "{} lines",
        report.lines().count()
    );
}

#[test]
fn every_group_in_a_cycle_is_reported() {
    // A -> B, A -> C, B -> A, C -> B. A depth-first search that reports only
    // the back edge misses C.
    let text = "version = \"FIX.4.4\"\n\
        [group.GA]\ncount = 555\nmembers = [600, \"GB\", \"GC\"]\n\
        [group.GB]\ncount = 604\nmembers = [605, \"GA\"]\n\
        [group.GC]\ncount = 453\nmembers = [455, \"GB\"]\n";
    let found = places(text);
    let lines: Vec<u32> = found.iter().map(|(line, _, _)| *line).collect();
    assert_eq!(lines, [4, 7, 10], "{found:?}");
    let paths: Vec<Vec<String>> = found
        .into_iter()
        .map(|(_, _, kind)| match kind {
            Kind::GroupLoop(path) => path,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(paths[0], ["GA", "GB", "GA"]);
    assert_eq!(paths[1], ["GB", "GA", "GB"]);
    assert_eq!(paths[2], ["GC", "GB", "GA", "GC"]);
}

#[test]
fn the_count_of_a_group_is_a_field_even_when_a_group_has_that_name() {
    // The block is named NoOrders, and so is the field. The count is a field.
    for count in ["73", "\"NoOrders\""] {
        let text = format!(
            "version = \"FIX.4.2\"\n[group.NoOrders]\ncount = {count}\nmembers = [11, 67]\n\
             [message.E]\nadd = [\"NoOrders\"]\n"
        );
        let venue = Profile::load(&text).unwrap();
        assert_eq!(venue.group("E", 73).unwrap().members.len(), 2, "{count}");
    }
}

#[test]
fn the_report_gives_each_definition_its_own_id() {
    // D has a profile NoPartyIDs. F and G have the standard one.
    let venue = Profile::load(
        "version = \"FIX.4.4\"\n\
         [group.NoPartyIDs]\ncount = 453\nmembers = [448, 447]\n\
         [message.D]\nadd = [\"NoPartyIDs\"]\n\
         [message.F]\nforbidden = [\"Text\"]\n\
         [message.G]\nforbidden = [\"Text\"]\n",
    )
    .unwrap();
    let report = describe(&venue);
    assert!(report.contains(
        "  group NoPartyIDs(453) [1]\n    PartyID(448)  (delimiter)\n    PartyIDSource(447)\n"
    ));
    let standard = report
        .lines()
        .position(|line| line.starts_with("  group NoPartyIDs(453) [") && !line.ends_with("[1]"))
        .expect("the standard definition is shown in F");
    let id = report
        .lines()
        .nth(standard)
        .unwrap()
        .rsplit('[')
        .next()
        .unwrap()
        .trim_end_matches(']')
        .to_owned();
    assert_ne!(id, "1");
    // G refers to the standard definition by its ID.
    assert!(
        report.contains(&format!("  group NoPartyIDs(453)  (see [{id}])")),
        "{report}"
    );
}
