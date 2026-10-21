// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! AI pacing groups (registry index 1). Creation has no payload. The native
//! update reader (RVA 0x42810d0) reads a presence bit, then replaces the list
//! using 0x4274af0: an eight-bit count, each reference followed by an f32.
//! The float is retained without assigning an unverified gameplay meaning.
use super::{Error, Reader, Writer};

pub const MAX_ENTRIES: usize = 255;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    pub participant: u16,
    /// Exact f32 bits, including unknown values.
    pub value: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Update {
    /// None preserves the list; Some(empty) clears it.
    pub entries: Option<Vec<Entry>>,
}
impl Update {
    pub(super) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let entries = r.optional(|r| {
            let count = r.take(8)? as usize;
            if count > r.span.len().saturating_sub(r.pos) / 45 {
                return Err(Error::Truncated);
            }
            (0..count)
                .map(|_| {
                    Ok(Entry {
                        participant: r.reference()?,
                        value: r.take(32)?,
                    })
                })
                .collect::<Result<_, Error>>()
        })?;
        Ok(Self { entries })
    }
    pub(super) fn write(&self, w: &mut Writer) -> Result<(), Error> {
        w.optional(&self.entries, |w, entries| {
            if entries.len() > MAX_ENTRIES {
                return Err(Error::Bound);
            }
            w.put(entries.len() as u64, 8)?;
            for entry in entries {
                w.reference(&entry.participant)?;
                w.put(u64::from(entry.value), 32)?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::BitWriter,
        replication::{Initial, Kind, Record, Update as RecordUpdate, state::World},
    };

    fn record(entries: Option<Vec<Entry>>) -> Record {
        Record {
            id: 31,
            initial: Some(Initial::AiPacingGroup),
            update: RecordUpdate::AiPacingGroup(Update { entries }),
        }
    }

    #[test]
    fn native_layout_boundaries_truncation_and_arbitrary_float_bits() {
        let entries = vec![
            Entry {
                participant: 17,
                value: 0.5f32.to_bits(),
            },
            Entry {
                participant: 0,
                value: 0x7fc00001,
            },
        ];
        let mut wire = BitWriter::new();
        wire.put(31, 13).put(1, 1).put(1, 5);
        wire.put(1, 1).put(2, 8);
        wire.put(17, 13).put(u64::from(0.5f32.to_bits()), 32);
        wire.put(0, 13).put(0x7fc00001, 32);
        let decoded = Record::decode(wire.span(), None).unwrap();
        assert_eq!(decoded.record, record(Some(entries)));
        assert_eq!((decoded.initial_bits, decoded.bits), (Some(19), 118));
        assert_eq!(decoded.record.encode().unwrap(), wire);
        for end in 0..wire.len() {
            assert_eq!(
                Record::decode(wire.span().slice(0, end).unwrap(), None),
                Err(Error::Truncated)
            );
        }
        let first_len = wire.len();
        wire.put_span(record(None).encode().unwrap().span());
        let first = Record::decode(wire.span(), None).unwrap();
        assert_eq!(first.bits, first_len);
        assert_eq!(
            Record::decode(wire.span().after(first.bits).unwrap(), None)
                .unwrap()
                .record,
            record(None)
        );
    }

    #[test]
    fn update_presence_and_count_bounds() {
        for entries in [
            None,
            Some(vec![]),
            Some(vec![
                Entry {
                    participant: 8191,
                    value: 0
                };
                255
            ]),
        ] {
            let mut expected = record(entries);
            expected.initial = None;
            let wire = expected.encode().unwrap();
            let decoded = Record::decode(wire.span(), Some(Kind::AiPacingGroup)).unwrap();
            assert_eq!(decoded.record, expected);
            assert_eq!(decoded.bits, wire.len());
            assert_eq!(decoded.initial_bits, None);
            for end in 0..wire.len() {
                assert!(
                    Record::decode(
                        wire.span().slice(0, end).unwrap(),
                        Some(Kind::AiPacingGroup)
                    )
                    .is_err()
                );
            }
        }
        assert_eq!(
            record(Some(vec![
                Entry {
                    participant: 1,
                    value: 0
                };
                256
            ]))
            .encode(),
            Err(Error::Bound)
        );
        assert_eq!(
            record(Some(vec![Entry {
                participant: 8192,
                value: 0
            }]))
            .encode(),
            Err(Error::Shape)
        );
    }

    #[test]
    fn state_validates_participants_and_replaces_the_list_atomically() {
        use crate::replication::{Identity, NamedIdentity, ParticipantInit, ParticipantUpdate};
        let mut world = World::default();
        let participant = world
            .spawn(
                Initial::Participant(ParticipantInit {
                    selector: 0,
                    serial: 1,
                    player: 0,
                    identity: NamedIdentity {
                        identity: Identity {
                            persona: 0,
                            bytes: vec![],
                        },
                        name: vec![],
                    },
                }),
                RecordUpdate::Participant(ParticipantUpdate::default()),
            )
            .unwrap();
        let update = |entries| RecordUpdate::AiPacingGroup(Update { entries });
        let entry = Entry {
            participant: participant.id,
            value: 0.5f32.to_bits(),
        };
        let group = world
            .spawn(Initial::AiPacingGroup, update(Some(vec![entry])))
            .unwrap();
        assert!(world.change(group.id, update(None)).unwrap().is_none());
        let before = world.snapshot();
        assert_eq!(world.remove(participant.id), Err(Error::UnknownObject));
        for bad in [0, group.id, 8191] {
            assert_eq!(
                world.change(
                    group.id,
                    update(Some(vec![Entry {
                        participant: bad,
                        value: 0
                    }]))
                ),
                Err(Error::UnknownObject)
            );
            assert_eq!(world.snapshot(), before);
        }
        assert!(
            world
                .change(group.id, update(Some(vec![])))
                .unwrap()
                .is_some()
        );
        assert!(
            world
                .change(group.id, update(Some(vec![])))
                .unwrap()
                .is_none()
        );
        world.remove(participant.id).unwrap();
        let mut isolated = World::default();
        assert_eq!(
            isolated.spawn(Initial::AiPacingGroup, update(Some(vec![entry]))),
            Err(Error::UnknownObject)
        );
        assert!(isolated.is_empty());
        world.remove(group.id).unwrap();
        assert!(world.is_empty());
    }
}
