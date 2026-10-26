use super::*;
use crate::bits::BitWriter;
use crate::replication::{Initial, Kind, Record, Update, state::World};

fn follow(parameters: Option<[u32; 3]>) -> Record {
    Record {
        id: 41,
        initial: Some(Initial::FollowTargetGoal(Follow {
            selector: 511,
            serial: 1023,
            allocation_serial: 63,
            target: 8191,
            parameters,
        })),
        update: Update::FollowTargetGoal,
    }
}

#[test]
fn follow_native_order_optional_branch_and_all_truncations() {
    for parameters in [None, Some([0x3f80_0000, 0xbf00_0000, 0x8000_0000])] {
        // Independently pack the paired native reader/writer field sequence.
        let mut wire = BitWriter::new();
        wire.put(41, 13)
            .put(1, 1)
            .put(8, 5)
            .put(511, 9)
            .put(1023, 10)
            .put(63, 6)
            .put(8191, 13)
            .put(u64::from(parameters.is_some()), 1);
        if let Some(values) = parameters {
            for value in values {
                wire.put(u64::from(value), 32);
            }
        }
        let expected_bits = if parameters.is_some() { 154 } else { 58 };
        let expected = follow(parameters);
        let decoded = Record::decode(wire.span(), None).unwrap();
        assert_eq!(decoded.record, expected);
        assert_eq!(
            (decoded.bits, decoded.initial_bits),
            (expected_bits, Some(expected_bits))
        );
        assert_eq!(expected.encode().unwrap().span(), wire.span());
        for bits in 0..expected_bits {
            assert_eq!(
                Record::decode(wire.span().slice(0, bits).unwrap(), None),
                Err(Error::Truncated)
            );
        }
        wire.put(0x5a, 8);
        assert_eq!(
            Record::decode(wire.span(), None).unwrap().bits,
            expected_bits
        );
    }
}

#[test]
fn chase_has_no_rpc_or_optional_triple_and_updates_are_empty() {
    let mut wire = BitWriter::new();
    wire.put(40, 13).put(1, 1).put(3, 5).put(63, 6).put(0, 13);
    let expected = Record {
        id: 40,
        initial: Some(Initial::ChaseTargetGoal(Chase {
            allocation_serial: 63,
            target: 0,
        })),
        update: Update::ChaseTargetGoal,
    };
    let decoded = Record::decode(wire.span(), None).unwrap();
    assert_eq!(decoded.record, expected);
    assert_eq!((decoded.bits, decoded.initial_bits), (38, Some(38)));
    assert_eq!(expected.encode().unwrap().span(), wire.span());
    for bits in 0..38 {
        assert_eq!(
            Record::decode(wire.span().slice(0, bits).unwrap(), None),
            Err(Error::Truncated)
        );
    }
    for (kind, update) in [
        (Kind::ChaseTargetGoal, Update::ChaseTargetGoal),
        (Kind::FollowTargetGoal, Update::FollowTargetGoal),
    ] {
        let record = Record {
            id: 40,
            initial: None,
            update,
        };
        let mut wire = record.encode().unwrap();
        assert_eq!(wire.len(), 14);
        wire.put(0x5a, 8);
        let decoded = Record::decode(wire.span(), Some(kind)).unwrap();
        assert_eq!(decoded.record, record);
        assert_eq!(decoded.bits, 14);
    }
}

#[test]
fn invalid_widths_values_and_mismatched_update_are_rejected() {
    for field in 0..7 {
        let mut record = follow(Some([0; 3]));
        let Some(Initial::FollowTargetGoal(goal)) = record.initial.as_mut() else {
            unreachable!()
        };
        match field {
            0 => goal.selector = 512,
            1 => goal.serial = 1024,
            2 => goal.allocation_serial = 64,
            3 => goal.target = 8192,
            4 => goal.parameters.as_mut().unwrap()[0] = f32::NAN.to_bits(),
            5 => goal.parameters.as_mut().unwrap()[1] = f32::INFINITY.to_bits(),
            _ => goal.parameters.as_mut().unwrap()[2] = f32::NEG_INFINITY.to_bits(),
        }
        assert!(record.encode().is_err());
    }
    let mut record = follow(None);
    record.update = Update::ChaseTargetGoal;
    assert_eq!(record.encode(), Err(Error::TypeMismatch));
}

#[test]
fn owned_goal_target_must_exist_and_cannot_be_retired_while_referenced() {
    let mut world = World::default();
    let initial = Initial::ChaseTargetGoal(Chase {
        allocation_serial: 1,
        target: 9,
    });
    assert_eq!(
        world.spawn(initial, Update::ChaseTargetGoal),
        Err(Error::UnknownObject)
    );
    assert!(world.is_empty());
    let target = world
        .spawn(
            Initial::PlayerManager,
            Update::PlayerManager(Default::default()),
        )
        .unwrap()
        .id;
    let goal = world
        .spawn(
            Initial::ChaseTargetGoal(Chase {
                allocation_serial: 1,
                target,
            }),
            Update::ChaseTargetGoal,
        )
        .unwrap()
        .id;
    assert_eq!(world.change(goal, Update::ChaseTargetGoal), Ok(None));
    assert_eq!(world.remove(target), Err(Error::UnknownObject));
    assert!(world.get(target).is_some());
    world.remove(goal).unwrap();
    world.remove(target).unwrap();
    assert!(world.is_empty());
}
