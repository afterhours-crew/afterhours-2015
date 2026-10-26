use super::*;
use crate::replication::{Update, client_info};

fn client_record(id: u16, create: bool) -> Record {
    Record {
        id,
        initial: create.then_some(Initial::ClientToServerInfo),
        update: Update::ClientToServerInfo(client_info::Update::Disabled),
    }
}

fn section(deleted: Vec<u16>, records: Vec<Record>) -> Section {
    Section {
        float_bits: None,
        flag: false,
        deleted,
        setup: (!records.is_empty()).then_some(Setup::ContextOnly),
        records,
    }
}

#[test]
fn absent_and_repeated_received_deletions_preserve_following_records() {
    let mut bindings = Bindings::from_client(1).unwrap();
    bindings
        .apply(&section(vec![], vec![client_record(7, true)]))
        .unwrap();
    let incoming = section(vec![99, 7, 7], vec![client_record(8, true)]);
    let wire = incoming.encode().unwrap();
    let decoded = bindings.decode(wire.span()).unwrap();
    assert_eq!(decoded.section, incoming);
    assert_eq!(decoded.bits, wire.len());
    assert_eq!(bindings.get(7), None);
    assert_eq!(bindings.get(8), Some(Kind::ClientToServerInfo));
    // Local producers must still account for every object they remove.
    let before = bindings.clone();
    assert_eq!(
        bindings.apply(&section(vec![99], vec![])),
        Err(Error::UnknownObject)
    );
    assert_eq!(bindings, before);
}

#[test]
fn later_failed_update_rolls_back_even_a_valid_received_deletion() {
    let mut bindings = Bindings::from_client(1).unwrap();
    bindings
        .apply(&section(vec![], vec![client_record(7, true)]))
        .unwrap();
    let before = bindings.clone();
    let wire = section(vec![99, 7], vec![client_record(88, false)])
        .encode()
        .unwrap();
    assert_eq!(bindings.decode(wire.span()), Err(Error::UnknownObject));
    assert_eq!(bindings, before);
    let wire = section(vec![99, 7], vec![client_record(8, true)])
        .encode()
        .unwrap();
    for end in 0..wire.len() {
        assert!(bindings.decode(wire.span().slice(0, end).unwrap()).is_err());
        assert_eq!(bindings, before);
    }
}
