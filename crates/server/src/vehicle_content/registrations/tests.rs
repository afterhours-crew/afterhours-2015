// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

fn seed(names: &[(u16, &str)], entries: Vec<Registration>) -> Vec<Message> {
    vec![
        Message::Names(SubLevelNames {
            word: 1,
            entries: names
                .iter()
                .map(|(id, s)| (*id, s.as_bytes().to_vec()))
                .collect(),
        }),
        Message::Registrations(Registrations { header: 4, entries }),
    ]
}

#[test]
fn separate_namespaces_current_mesh_levels_and_silent_repeats() {
    let source = seed(&[(7, "SharedMesh")], vec![vehicle_entry(7, 41, 91)]);
    let mut state = Registry::from_messages(&source).unwrap();
    assert_eq!(state.mesh_level("sharedmesh"), Ok(41));
    let output = state.prepare(&["Car", "car"]).unwrap();
    let binding = Binding {
        name: 8,
        level: 42,
        stream: 92,
        asset_bundle: 89,
        content_key: name_key(b"car").unwrap(),
    };
    assert_eq!(output.bindings, [binding, binding]);
    assert_eq!(output.messages.len(), 2);
    assert!(matches!(&output.messages[0], Message::Names(_)));
    assert!(matches!(&output.messages[1], Message::Registrations(_)));
    let mut combined = source.clone();
    combined.extend(output.messages);
    assert_eq!(Registry::from_messages(&combined).unwrap(), state);
    assert!(state.prepare(&["CAR"]).unwrap().messages.is_empty());
    let mut other = Registry::from_messages(&source).unwrap();
    assert!(other.mesh_level("Car").is_err());
    assert_eq!(other.prepare(&["Different"]).unwrap().bindings[0].level, 42);
    assert!(state.mesh_level("Different").is_err());
}

#[test]
fn predeclared_names_are_used_without_resending_the_name_table() {
    let mut state = Registry::from_messages(&seed(
        &[(9, "World"), (3, "Car")],
        vec![vehicle_entry(9, 27, 83)],
    ))
    .unwrap();
    let result = state.prepare(&["CAR"]).unwrap();
    assert_eq!(result.bindings[0].name, 3);
    assert_eq!(result.bindings[0].level, 28);
    assert_eq!(result.bindings[0].stream, 84);
    assert_eq!(result.messages.len(), 1);
    assert!(matches!(result.messages[0], Message::Registrations(_)));
}

#[test]
fn later_failure_rolls_back_names_levels_streams_and_outputs() {
    let mut state = Registry::default();
    let before = state.clone();
    for invalid in ["", "bad\0name", "nonascii\u{100}"] {
        assert!(state.prepare(&["Valid", invalid]).is_err());
        assert_eq!(state, before);
    }
    assert!(
        state
            .prepare(&["Valid", &"x".repeat(content::MAX_STRING + 1)])
            .is_err()
    );
    assert_eq!(state, before);
    assert!(state.prepare(&["Car"; 33]).is_err());
    assert_eq!(state, before);
    let first = state.prepare(&["Car"]).unwrap().bindings[0];
    assert_eq!(
        (first.name, first.level, first.stream, first.asset_bundle),
        (0, 1, 4, 1)
    );
}

#[test]
fn ambiguous_root_and_unsupported_profile_are_not_reused() {
    let mut ambiguous = Registry::from_messages(&seed(
        &[(1, "Car")],
        vec![vehicle_entry(1, 2, 9), vehicle_entry(1, 3, 10)],
    ))
    .unwrap();
    let before = ambiguous.clone();
    assert_eq!(ambiguous.prepare(&["Car"]), Err(Error::Shape));
    assert_eq!(ambiguous.mesh_level("Car"), Err(Error::Shape));
    assert_eq!(ambiguous, before);
    let mut startup = vehicle_entry(1, 2, 9);
    startup.flag = false;
    let mut state = Registry::from_messages(&seed(&[(1, "Car")], vec![startup])).unwrap();
    assert_eq!(state.mesh_level("Car"), Ok(2));
    assert_eq!(state.prepare(&["Car"]), Err(Error::Unsupported));
    assert_eq!(state.mesh_level("Missing"), Err(Error::Unsupported));
    assert!(Registry::from_messages(&seed(&[(1, "Car"), (2, "CAR")], vec![])).is_err());
}

#[test]
fn reserved_asset_selector_is_skipped_and_exhaustion_is_atomic() {
    let mut state =
        Registry::from_messages(&seed(&[(1, "Old")], vec![vehicle_entry(1, 2, 2005)])).unwrap();
    let result = state.prepare(&["New"]).unwrap();
    assert_eq!(result.bindings[0].stream, 2007);
    assert_eq!(result.bindings[0].asset_bundle, 2004);
    for (name, level, stream) in [(u16::MAX, 2, 9), (1, u16::MAX - 1, 9), (1, 2, 2050)] {
        let mut state = Registry::from_messages(&seed(
            &[(name, "Old")],
            vec![vehicle_entry(name, level, stream)],
        ))
        .unwrap();
        let before = state.clone();
        assert!(state.prepare(&["New"]).is_err());
        assert_eq!(state, before);
    }
}

#[test]
fn namespace_collection_limits_apply_to_the_whole_world() {
    let mut state = Registry::default();
    for i in 0..content::MAX_ITEMS {
        state.prepare(&[&format!("Car{i}")]).unwrap();
    }
    let before = state.clone();
    assert_eq!(state.prepare(&["Overflow"]), Err(Error::Bound));
    assert_eq!(state, before);
    assert!(state.prepare(&["Car0"]).unwrap().messages.is_empty());
}

#[test]
fn bounded_wire_encoding_and_hash_collisions_fail_closed() {
    let mut state = Registry::default();
    let longest = "v".repeat(content::MAX_STRING);
    let output = state.prepare(&[&longest]).unwrap();
    for message in &output.messages {
        let bytes = message.encode().unwrap();
        assert_eq!(
            Message::decode(message.target(), &bytes).unwrap().message,
            *message
        );
    }
    assert_eq!(name_key(b"a6"), name_key(b"gp"));
    state.prepare(&["a6"]).unwrap();
    let before = state.clone();
    assert_eq!(state.prepare(&["gp"]), Err(Error::Shape));
    assert_eq!(state, before);
    assert!(Registry::from_messages(&seed(&[(1, "a6"), (2, "gp")], vec![])).is_err());
}
