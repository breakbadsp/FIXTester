//! Tests of the defaults for required fields (R29).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use fix_dict::{DefaultValue, Profile, VenueDictionary, Version, missing_defaults};

fn defaults<'a>(
    venue: &'a VenueDictionary,
    msg_type: &str,
    present: &'a [u32],
) -> Vec<(u32, DefaultValue<'a>)> {
    missing_defaults(venue, msg_type, present).collect()
}

#[test]
fn handl_inst_is_a_default_only_where_the_version_requires_it() {
    for version in Version::ALL {
        let venue = VenueDictionary::standard(version);
        let required = venue.message("D").unwrap().is_body_required(21);
        let found = defaults(&venue, "D", &[]);
        assert_eq!(
            found.contains(&(21, DefaultValue::Text("1"))),
            required,
            "{version:?}"
        );
    }
    // The FIX 4.2 repository requires HandlInst in NewOrderSingle.
    let fix42 = VenueDictionary::standard(Version::Fix42);
    assert!(fix42.message("D").unwrap().is_body_required(21));
}

#[test]
fn transact_time_is_a_time_default_for_each_message_that_requires_it() {
    for version in Version::ALL {
        let venue = VenueDictionary::standard(version);
        for msg_type in ["D", "F", "G"] {
            let required = venue.message(msg_type).unwrap().is_body_required(60);
            assert_eq!(
                defaults(&venue, msg_type, &[]).contains(&(60, DefaultValue::Time)),
                required,
                "{version:?} {msg_type}"
            );
        }
        assert!(venue.message("D").unwrap().is_body_required(60));
    }
}

#[test]
fn a_field_the_user_entered_has_no_default() {
    let venue = VenueDictionary::standard(Version::Fix42);
    assert_eq!(defaults(&venue, "D", &[21, 60]), []);
    assert_eq!(defaults(&venue, "D", &[21]), [(60, DefaultValue::Time)]);
}

#[test]
fn time_in_force_is_a_default_only_when_the_profile_asks() {
    let standard = VenueDictionary::standard(Version::Fix44);
    assert!(
        !defaults(&standard, "D", &[])
            .iter()
            .any(|(tag, _)| *tag == 59)
    );
    let venue = Profile::load("version = \"FIX.4.4\"\n[defaults]\nTimeInForce = \"0\"\n").unwrap();
    assert!(defaults(&venue, "D", &[]).contains(&(59, DefaultValue::Text("0"))));
    assert_eq!(defaults(&venue, "D", &[59]), [(60, DefaultValue::Time)]);
}

#[test]
fn time_in_force_defaults_to_day_when_the_profile_requires_it() {
    let required = "version = \"FIX.4.4\"\n[message.D]\nrequired = [\"TimeInForce\"]\n";
    let venue = Profile::load(required).unwrap();
    // Omitted: the default is Day (0).
    assert!(defaults(&venue, "D", &[]).contains(&(59, DefaultValue::Text("0"))));
    // Entered by the user: no default.
    assert!(
        !defaults(&venue, "D", &[59])
            .iter()
            .any(|(tag, _)| *tag == 59)
    );
    // The profile sets another value: it wins.
    let other = format!("{required}[defaults]\nTimeInForce = \"1\"\n");
    let venue = Profile::load(&other).unwrap();
    let found = defaults(&venue, "D", &[]);
    assert_eq!(found.iter().filter(|(tag, _)| *tag == 59).count(), 1);
    assert!(found.contains(&(59, DefaultValue::Text("1"))));
    // Not required and no profile default: no default.
    let standard = VenueDictionary::standard(Version::Fix44);
    assert!(
        !defaults(&standard, "D", &[])
            .iter()
            .any(|(tag, _)| *tag == 59)
    );
}

#[test]
fn a_profile_default_applies_to_the_messages_that_allow_the_field() {
    let venue = Profile::load(include_str!("profiles/valid.toml")).unwrap();
    let for_d = defaults(&venue, "D", &[]);
    assert!(for_d.contains(&(1, DefaultValue::Text("TEST01"))));
    assert!(for_d.contains(&(60, DefaultValue::Time)));
    // The 4.4 repository does not require HandlInst, but the profile gives it a value.
    assert!(for_d.contains(&(21, DefaultValue::Text("1"))));
    // HandlInst is not a field of OrderCancelRequest (F).
    let for_f = defaults(&venue, "F", &[]);
    assert!(!for_f.iter().any(|(tag, _)| *tag == 21));
    assert!(for_f.contains(&(1, DefaultValue::Text("TEST01"))));
}

#[test]
fn a_profile_default_replaces_a_standard_default() {
    let venue = Profile::load("version = \"FIX.4.2\"\n[defaults]\nHandlInst = \"2\"\n").unwrap();
    let found = defaults(&venue, "D", &[]);
    assert_eq!(found.iter().filter(|(tag, _)| *tag == 21).count(), 1);
    assert!(found.contains(&(21, DefaultValue::Text("2"))));
}

#[test]
fn a_forbidden_field_has_no_default() {
    let venue = Profile::load(
        "version = \"FIX.4.4\"\n[defaults]\nAccount = \"A\"\n[message.D]\nforbidden = [\"Account\"]\n",
    )
    .unwrap();
    assert!(!defaults(&venue, "D", &[]).iter().any(|(tag, _)| *tag == 1));
    assert!(defaults(&venue, "F", &[]).iter().any(|(tag, _)| *tag == 1));
}

#[test]
fn a_required_field_that_the_profile_adds_gets_its_default() {
    let venue = Profile::load(
        "version = \"FIX.4.4\"\n[defaults]\nVenueStrategy = \"VWAP\"\n\
         [[field]]\ntag = 9001\nname = \"VenueStrategy\"\ntype = \"STRING\"\n\
         [message.D]\nrequired = [9001]\n",
    )
    .unwrap();
    assert!(defaults(&venue, "D", &[]).contains(&(9001, DefaultValue::Text("VWAP"))));
    assert!(
        !defaults(&venue, "F", &[])
            .iter()
            .any(|(tag, _)| *tag == 9001)
    );
}

#[test]
fn an_unknown_message_type_has_no_defaults() {
    let venue = VenueDictionary::standard(Version::Fix44);
    assert_eq!(defaults(&venue, "U9", &[]), []);
}
