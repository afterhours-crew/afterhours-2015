// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_content::scoring::nodes::Configuration;
fn id(index: usize) -> Identity {
    Identity {
        asset: "constructed/mirror".into(),
        index,
        instances: vec![],
    }
}
fn property() -> Property {
    Property {
        channel: "constructed/control".into(),
        port: 9,
    }
}
fn signal(event: u32) -> Signal {
    Signal {
        source: id(20),
        event,
    }
}
fn definitions() -> BTreeMap<Identity, Definition> {
    BTreeMap::from([(
        id(2),
        Definition {
            input: id(1),
            channel: property(),
            comparison: nodes::Definition::CompareBoolean {
                on_change: true,
                on_start: false,
                always_send: false,
            },
        },
    )])
}
fn publishers() -> BTreeMap<Property, Vec<Publisher>> {
    BTreeMap::from([(
        property(),
        vec![Publisher {
            channel: id(10),
            input: id(11),
            configuration: Configuration {
                definition: nodes::Definition::Boolean { default: false },
                defaults: vec![],
            },
            writes: vec![(signal(1), nodes::SET_TRUE), (signal(2), nodes::SET_FALSE)],
        }],
    )])
}
fn project(members: &[u64]) -> BTreeMap<Identity, BTreeSet<u64>> {
    BTreeMap::from([(id(1), members.iter().copied().collect())])
}
#[test]
fn copy_enable_tracks_changes_disable_clears_and_reenable_uses_current_input() {
    let mut s = State::new(&definitions(), &publishers()).unwrap();
    s.sync(&project(&[7])).unwrap();
    assert!(
        !s.contains(&id(2), 7).unwrap(),
        "activation and input changes do not implicitly copy"
    );
    s.signal(&signal(1)).unwrap();
    assert!(s.contains(&id(2), 7).unwrap());
    let separate = s.clone();
    s.sync(&project(&[8])).unwrap();
    assert!(!s.contains(&id(2), 7).unwrap());
    assert!(s.contains(&id(2), 8).unwrap());
    assert!(separate.contains(&id(2), 7).unwrap());
    assert!(!separate.contains(&id(2), 8).unwrap());
    s.signal(&signal(2)).unwrap();
    assert!(!s.contains(&id(2), 8).unwrap());
    s.sync(&project(&[9])).unwrap();
    assert!(!s.contains(&id(2), 9).unwrap());
    s.signal(&signal(1)).unwrap();
    assert!(s.contains(&id(2), 9).unwrap());
    s.signal(&signal(1)).unwrap();
    s.sync(&project(&[])).unwrap();
    assert!(!s.contains(&id(2), 9).unwrap());
    assert!(
        !State::new(&definitions(), &publishers())
            .unwrap()
            .contains(&id(2), 7)
            .unwrap()
    );
}
#[test]
fn missing_or_oversized_projection_and_unknown_signals_leave_all_owners_unchanged() {
    let mut s = State::new(&definitions(), &publishers()).unwrap();
    s.sync(&project(&[7])).unwrap();
    s.signal(&signal(1)).unwrap();
    assert!(s.sync(&BTreeMap::new()).is_err());
    assert!(s.contains(&id(2), 7).unwrap());
    assert!(
        s.sync(&project(
            &(0..=participant_states::MAX_PARTICIPANTS as u64).collect::<Vec<_>>()
        ))
        .is_err()
    );
    assert!(s.contains(&id(2), 7).unwrap());
    assert!(s.signal(&signal(99)).is_err());
    assert!(s.contains(&id(2), 7).unwrap());
    assert!(s.contains(&id(99), 7).is_err());
    s.sync(&project(&[8])).unwrap();
    assert!(
        s.contains(&id(2), 8).unwrap(),
        "failed operations did not close the gates"
    );
    assert!(!s.contains(&id(2), 7).unwrap());
}
#[test]
fn shared_input_can_feed_separate_copies_but_missing_control_sources_are_errors() {
    let mut definitions = definitions();
    definitions.insert(id(3), definitions[&id(2)].clone());
    let mut s = State::new(&definitions, &publishers()).unwrap();
    s.sync(&project(&[7, 8])).unwrap();
    s.signal(&signal(1)).unwrap();
    for i in [2, 3] {
        assert!(s.contains(&id(i), 7).unwrap());
        assert!(s.contains(&id(i), 8).unwrap());
    }
    s.signal(&signal(2)).unwrap();
    for i in [2, 3] {
        assert!(!s.contains(&id(i), 7).unwrap());
        assert!(!s.contains(&id(i), 8).unwrap());
    }
    assert!(State::new(&definitions, &BTreeMap::new()).is_err());
    let mut unsupported = definitions.clone();
    unsupported.get_mut(&id(3)).unwrap().comparison = nodes::Definition::CompareBoolean {
        on_change: false,
        on_start: true,
        always_send: false,
    };
    assert!(State::new(&unsupported, &publishers()).is_err());
    definitions.get_mut(&id(3)).unwrap().input = id(2);
    assert!(State::new(&definitions, &publishers()).is_err());
}
