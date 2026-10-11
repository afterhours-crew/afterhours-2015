// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use creation::{Asset, Catalog, Content, Creation, Prefix};

fn fixture() -> (Profile, Body) {
    let rpc = |selector| Rpc {
        selector,
        serial: Serial::new(1).unwrap(),
    };
    (
        Profile::new(&[
            Kind::SequenceRoot,
            Kind::RpcOptionalReference,
            Kind::RpcReference,
        ])
        .unwrap(),
        Body {
            initial: Some(vec![
                Initial::SequenceRoot {
                    value: 1,
                    rpc: rpc(0),
                    manager: 82,
                    manager_selector: 2,
                    sequence: 2,
                    participants: vec![260],
                    stopping: false,
                },
                Initial::RpcOptionalReference {
                    rpc: rpc(1),
                    reference: Some(260),
                },
                Initial::RpcReference {
                    rpc: rpc(2),
                    reference: 97,
                    target_selector: 22,
                },
            ]),
            updates: vec![Some(Update::Noop); 3],
        },
    )
}

#[test]
fn participant_count_null_provider_and_reference_bounds() {
    let (profile, mut body) = fixture();
    if let Initial::SequenceRoot {
        participants,
        stopping,
        ..
    } = &mut body.initial.as_mut().unwrap()[0]
    {
        *participants = vec![8191; 255];
        *stopping = true;
    }
    if let Initial::RpcOptionalReference { reference, .. } = &mut body.initial.as_mut().unwrap()[1]
    {
        *reference = None;
    }
    let wire = body.encode(&profile).unwrap();
    assert_eq!(
        Body::decode(wire.span(), &profile, true).unwrap().body,
        body
    );
    if let Initial::SequenceRoot { participants, .. } = &mut body.initial.as_mut().unwrap()[0] {
        participants.push(0);
    }
    assert_eq!(body.encode(&profile), Err(Error::Bound));
    if let Initial::SequenceRoot { participants, .. } = &mut body.initial.as_mut().unwrap()[0] {
        *participants = vec![8192];
    }
    assert!(body.encode(&profile).is_err());
    assert_eq!(
        Profile::new(&[Kind::Rpc, Kind::SequenceRoot]),
        Err(Error::Unsupported)
    );
    assert_eq!(
        Profile::new(&[Kind::Root, Kind::SequenceRoot]),
        Err(Error::Unsupported)
    );
    assert_eq!(
        Profile::new(&[Kind::SequenceRoot, Kind::Root]),
        Err(Error::Unsupported)
    );
    assert_eq!(
        Profile::new(&[Kind::SequenceRoot, Kind::SequenceRoot]),
        Err(Error::Unsupported)
    );
}

#[test]
fn sequence_state_requires_all_current_world_references() {
    let (profile, body) = fixture();
    let asset = Asset {
        bundle: 9,
        type_id: 3424,
        local_index: 0,
    };
    let content = Content::new(
        vec![Catalog::new(9, &[(3424, 1)]).unwrap()],
        vec![(asset, profile)],
    )
    .unwrap();
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint: 82,
            sub_id: 2,
            owner: None,
            asset,
        },
        body,
    };
    for missing in [82, 97, 260] {
        assert_eq!(
            state::State::new(creation.clone(), &content, |id| id != missing),
            Err(Error::UnknownObject)
        );
    }
    let mut second_participant = creation.clone();
    if let Initial::SequenceRoot { participants, .. } =
        &mut second_participant.body.initial.as_mut().unwrap()[0]
    {
        participants.push(261);
    }
    assert_eq!(
        state::State::new(second_participant, &content, |id| id != 261),
        Err(Error::UnknownObject)
    );
    let accepted = state::State::new(creation.clone(), &content, |_| true).unwrap();
    assert_eq!(accepted.snapshot(), creation);
}
