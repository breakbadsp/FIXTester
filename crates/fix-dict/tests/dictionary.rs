//! Tests of the generated standard tables. The facts come from the FIX
//! Repository data in the tables, and the checks run for each version.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{MULTILEG_BODY, SINGLE_BODY, wire, with_header};
use fix_codec::{Decoder, GroupDef, Member, tag};
use fix_dict::schema::{Dictionary, FieldType, Section, delimiter};
use fix_dict::{VenueDictionary, Version};

fn dictionary(version: Version) -> &'static Dictionary {
    version.dictionary()
}

#[test]
fn new_order_multileg_exists_in_fix_43_and_44_but_not_in_42() {
    assert!(dictionary(Version::Fix42).message("AB").is_none());
    for version in [Version::Fix43, Version::Fix44] {
        let message = dictionary(version).message("AB").unwrap();
        assert_eq!(message.name, "NewOrderMultileg");
        assert!(dictionary(version).message("AC").is_some(), "{version:?}");
    }
}

#[test]
fn username_and_password_are_in_logon_from_fix_43() {
    let in_logon =
        |version: Version, tag: u32| dictionary(version).message("A").unwrap().body.contains(tag);
    for tag in [553, 554] {
        assert!(!in_logon(Version::Fix42, tag), "FIX 4.2 tag {tag}");
        assert!(in_logon(Version::Fix43, tag), "FIX 4.3 tag {tag}");
        assert!(in_logon(Version::Fix44, tag), "FIX 4.4 tag {tag}");
    }
    assert_eq!(
        dictionary(Version::Fix44).field(553).unwrap().name,
        "Username"
    );
    assert_eq!(
        dictionary(Version::Fix44).field(554).unwrap().name,
        "Password"
    );
}

#[test]
fn time_in_force_at_the_close_is_not_a_fix_42_value() {
    let at_the_close = |version: Version| {
        let field = dictionary(version).field(59).unwrap();
        (
            field.accepts_value("7"),
            field.values.iter().find(|v| v.value == "7").map(|v| v.name),
        )
    };
    assert_eq!(at_the_close(Version::Fix42), (false, None));
    assert_eq!(at_the_close(Version::Fix43), (true, Some("AtTheClose")));
    assert_eq!(at_the_close(Version::Fix44), (true, Some("AtTheClose")));
    for version in Version::ALL {
        for value in ["0", "1", "2", "3", "4", "6"] {
            assert!(
                dictionary(version).is_valid_value(59, value),
                "{version:?} {value}"
            );
        }
    }
}

#[test]
fn fix_44_new_order_single_has_party_ids_with_nested_party_sub_ids() {
    let parties = dictionary(Version::Fix44).group("D", 453).unwrap();
    assert_eq!(parties.count_tag, 453);
    assert_eq!(delimiter(parties), Some(448));
    let nested = parties
        .members
        .iter()
        .find_map(|member| match member {
            Member::Group(group) => Some(*group),
            Member::Field(_) => None,
        })
        .unwrap();
    assert_eq!(nested.count_tag, 802);
    assert_eq!(delimiter(nested), Some(523));
    assert!(dictionary(Version::Fix42).group("D", 453).is_none());
}

#[test]
fn raw_data_length_and_raw_data_are_a_pair_in_every_version() {
    for version in Version::ALL {
        let dict = dictionary(version);
        assert!(dict.data_pairs.contains(&(95, 96)), "{version:?}");
        assert_eq!(dict.field(95).unwrap().ty, FieldType::Length);
        assert_eq!(dict.field(96).unwrap().ty, FieldType::Data);
    }
}

#[test]
fn the_codec_knows_every_length_and_data_pair_of_the_dictionary() {
    for version in Version::ALL {
        for (length, data) in dictionary(version).data_pairs {
            assert_eq!(
                tag::data_tag_for(*length),
                Some(*data),
                "{version:?}: the decoder cannot read data field {data} by its length field {length}"
            );
            assert_eq!(
                dictionary(version).field(*data).unwrap().ty,
                FieldType::Data
            );
        }
        let data_fields = dictionary(version)
            .fields
            .iter()
            .filter(|field| field.ty == FieldType::Data)
            .count();
        assert_eq!(
            dictionary(version).data_pairs.len(),
            data_fields,
            "{version:?}"
        );
    }
}

fn check_group(dict: &Dictionary, group: &GroupDef<'_>, depth: usize, what: &str) {
    assert!(depth < 8, "{what}: groups nest too deep");
    assert!(dict.field(group.count_tag).is_some(), "{what}: count tag");
    let first = delimiter(group);
    assert!(
        first.is_some(),
        "{what}: group {} has no field delimiter",
        group.count_tag
    );
    let mut tags = Vec::new();
    for member in group.members {
        let tag = match member {
            Member::Field(tag) => *tag,
            Member::Group(nested) => {
                check_group(dict, nested, depth + 1, what);
                nested.count_tag
            }
        };
        assert!(
            dict.field(tag).is_some(),
            "{what}: member tag {tag} is unknown"
        );
        assert!(!tags.contains(&tag), "{what}: member {tag} twice");
        tags.push(tag);
    }
}

fn check_section(dict: &Dictionary, section: &Section, what: &str) {
    assert!(
        section.required.windows(2).all(|pair| pair[0] < pair[1]),
        "{what}: sorted"
    );
    assert!(
        section.optional.windows(2).all(|pair| pair[0] < pair[1]),
        "{what}: sorted"
    );
    for tag in section.required.iter().chain(section.optional) {
        assert!(dict.field(*tag).is_some(), "{what}: field {tag} is unknown");
    }
    for group in section.groups {
        assert!(
            section.contains(group.count_tag),
            "{what}: the count tag is a field"
        );
        check_group(dict, group, 0, what);
    }
}

#[test]
fn every_group_has_a_field_delimiter_and_every_reference_exists() {
    for version in Version::ALL {
        let dict = dictionary(version);
        assert!(dict.fields.windows(2).all(|pair| pair[0].tag < pair[1].tag));
        assert!(
            dict.messages
                .windows(2)
                .all(|pair| pair[0].msg_type < pair[1].msg_type)
        );
        check_section(dict, &dict.header, "header");
        check_section(dict, &dict.trailer, "trailer");
        for message in dict.messages {
            check_section(dict, &message.body, message.name);
            assert!(
                message
                    .body
                    .required
                    .iter()
                    .all(|tag| !dict.header.contains(*tag))
            );
        }
    }
}

#[test]
fn each_version_has_its_known_counts() {
    // The repository data decides: these sizes make a wrong generator run visible.
    let sizes: Vec<(usize, usize)> = Version::ALL
        .iter()
        .map(|v| (dictionary(*v).messages.len(), dictionary(*v).fields.len()))
        .collect();
    assert_eq!(sizes, vec![(46, 405), (68, 635), (93, 912)]);
}

#[test]
fn a_real_fix_44_multileg_order_decodes_with_nested_groups() {
    let dict = dictionary(Version::Fix44);
    let bytes = wire("FIX.4.4", &with_header("AB", MULTILEG_BODY));
    let mut decoder = Decoder::new(128);
    let message = decoder.decode(&bytes).unwrap();
    let legs_def = dict.group("AB", 555).unwrap();
    let legs = message.fields().group(legs_def).unwrap().unwrap();
    assert_eq!(legs.len(), 2);
    let entries: Vec<_> = legs.entries().collect();
    assert_eq!(entries[0].get(600), Some(&b"ESZ6"[..]));
    assert_eq!(entries[0].get(624), Some(&b"1"[..]));
    assert_eq!(entries[1].get(600), Some(&b"ESH7"[..]));
    assert_eq!(entries[1].len(), 3, "the second leg has no nested group");
    // The nested group comes from the definition of the first group.
    let alt_def = legs_def
        .members
        .iter()
        .find_map(|member| match member {
            Member::Group(group) if group.count_tag == 604 => Some(*group),
            _ => None,
        })
        .unwrap();
    let alt = entries[0].group(alt_def).unwrap().unwrap();
    assert_eq!(alt.len(), 1);
    let alt_entry = alt.entries().next().unwrap();
    assert_eq!(alt_entry.get(605), Some(&b"ESZ6-ALT"[..]));
    assert_eq!(alt_entry.get(606), Some(&b"4"[..]));
    assert!(entries[1].group(alt_def).unwrap().is_none());
    // The field after the groups is not part of the last leg.
    assert_eq!(message.fields().get(59), Some(&b"0"[..]));
}

#[test]
fn a_fix_44_single_order_decodes_party_ids_with_two_sub_ids() {
    let dict = dictionary(Version::Fix44);
    let bytes = wire("FIX.4.4", &with_header("D", SINGLE_BODY));
    let mut decoder = Decoder::new(128);
    let message = decoder.decode(&bytes).unwrap();
    let parties_def = dict.group("D", 453).unwrap();
    let parties = message.fields().group(parties_def).unwrap().unwrap();
    let entry = parties.entries().next().unwrap();
    let sub_def = dict
        .group("D", 453)
        .unwrap()
        .members
        .iter()
        .find_map(|m| match m {
            Member::Group(group) => Some(*group),
            Member::Field(_) => None,
        });
    let subs = entry.group(sub_def.unwrap()).unwrap().unwrap();
    assert_eq!(subs.len(), 2);
    let ids: Vec<_> = subs
        .entries()
        .map(|e| e.get(523).unwrap().to_vec())
        .collect();
    assert_eq!(ids, vec![b"SUB1".to_vec(), b"SUB2".to_vec()]);
}

#[test]
fn the_header_group_is_found_for_every_message_type() {
    let dict = dictionary(Version::Fix44);
    let hops = dict.group("D", 627).unwrap();
    assert_eq!(delimiter(hops), Some(628));
    assert!(
        dict.group("ZZ", 627).is_none(),
        "an unknown message type has no groups"
    );
}

#[test]
fn field_lookups_by_name_and_tag_agree() {
    for version in Version::ALL {
        let dict = dictionary(version);
        for field in dict.fields {
            assert_eq!(
                dict.field_by_name(field.name).map(|f| f.tag),
                Some(field.tag)
            );
        }
        let venue = VenueDictionary::standard(version);
        assert_eq!(venue.field_by_name("ClOrdID").unwrap().tag, 11);
        assert_eq!(venue.field(54).unwrap().name, "Side");
    }
}
