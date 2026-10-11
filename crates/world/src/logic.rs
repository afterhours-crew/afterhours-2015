// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{bits::BitWriter, frame::Error};
use nfs_protocol::world::BitSpan;

pub const MAX_EVENTS: usize = 128;
pub const MAX_NAME: usize = 1023;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntityRef {
    pub ghost: u16,
    pub entity: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Fire {
        events: Vec<u32>,
        player: Option<i32>,
        target: EntityRef,
    },
    Reached {
        event: u32,
        target: EntityRef,
        player: u8,
    },
    PlayerConnected {
        reason: u8,
        name: Vec<u8>,
    },
    FirstPlayerEntered,
}
struct Reader<'a> {
    bits: BitSpan<'a>,
    pos: usize,
}
impl Reader<'_> {
    fn take(&mut self, n: u8) -> Result<u32, Error> {
        let v = self.bits.read_u32(self.pos, n)?;
        self.pos += usize::from(n);
        Ok(v)
    }
    fn target(&mut self) -> Result<EntityRef, Error> {
        Ok(EntityRef {
            ghost: self.take(13)? as u16,
            entity: self.take(32)?,
        })
    }
}
impl Message {
    pub fn index(&self) -> u8 {
        match self {
            Self::Fire { player: None, .. } => 86,
            Self::Fire {
                player: Some(_), ..
            } => 91,
            Self::Reached { .. } => 82,
            Self::PlayerConnected { .. } => 35,
            Self::FirstPlayerEntered => 26,
        }
    }
    pub fn decode(index: u8, bits: BitSpan<'_>) -> Result<Self, Error> {
        if bits.len() > 15 + MAX_NAME * 8 {
            return Err(Error::Bound);
        }
        let mut r = Reader { bits, pos: 0 };
        if matches!(index, 82 | 86 | 91) && r.take(32)? as usize != bits.len() {
            return Err(Error::Shape);
        }
        let message = match index {
            86 | 91 => {
                let count = r.take(32)? as usize;
                if count > MAX_EVENTS {
                    return Err(Error::Bound);
                }
                let events = (0..count).map(|_| r.take(32)).collect::<Result<_, _>>()?;
                let player = if index == 91 {
                    Some(r.take(32)? as i32)
                } else {
                    None
                };
                Self::Fire {
                    events,
                    player,
                    target: r.target()?,
                }
            }
            82 => {
                let event = r.take(32)?;
                let target = r.target()?;
                let player = r.take(8)? as u8;
                if player > 7 && player != 255 {
                    return Err(Error::Shape);
                }
                Self::Reached {
                    event,
                    target,
                    player,
                }
            }
            35 => {
                let reason = r.take(5)? as u8;
                if reason > 17 {
                    return Err(Error::Shape);
                }
                let count = r.take(10)? as usize;
                let name = (0..count)
                    .map(|_| r.take(8).map(|b| b as u8))
                    .collect::<Result<_, _>>()?;
                Self::PlayerConnected { reason, name }
            }
            26 => Self::FirstPlayerEntered,
            _ => return Err(Error::Unsupported(index)),
        };
        if r.pos != bits.len() {
            return Err(Error::Shape);
        }
        Ok(message)
    }
    pub fn encode(&self) -> Result<BitWriter, Error> {
        let mut w = BitWriter::new();
        fn target(w: &mut BitWriter, v: EntityRef) -> Result<(), Error> {
            if v.ghost > 8191 {
                return Err(Error::Bound);
            }
            w.put(v.ghost.into(), 13).put(v.entity.into(), 32);
            Ok(())
        }
        match self {
            Self::Fire {
                events,
                player,
                target: v,
            } => {
                if events.len() > MAX_EVENTS {
                    return Err(Error::Bound);
                }
                let size = 32 + 32 + events.len() * 32 + usize::from(player.is_some()) * 32 + 45;
                w.put(size as u64, 32).put(events.len() as u64, 32);
                for event in events {
                    w.put((*event).into(), 32);
                }
                if let Some(player) = player {
                    w.put((*player as u32).into(), 32);
                }
                target(&mut w, *v)?;
            }
            Self::Reached {
                event,
                target: v,
                player,
            } => {
                if *player > 7 && *player != 255 {
                    return Err(Error::Shape);
                }
                w.put(117, 32).put((*event).into(), 32);
                target(&mut w, *v)?;
                w.put((*player).into(), 8);
            }
            Self::PlayerConnected { reason, name } => {
                if *reason > 17 {
                    return Err(Error::Shape);
                }
                if name.len() > MAX_NAME {
                    return Err(Error::Bound);
                }
                w.put((*reason).into(), 5)
                    .put(name.len() as u64, 10)
                    .put_bytes(name);
            }
            Self::FirstPlayerEntered => {}
        }
        Ok(w)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fire {
    pub event: u32,
    pub player: Option<i32>,
    pub target: EntityRef,
}
impl Fire {
    pub fn message(self) -> Message {
        Message::Fire {
            events: vec![self.event],
            player: self.player,
            target: self.target,
        }
    }
    pub fn index(self) -> u8 {
        if self.player.is_some() { 91 } else { 86 }
    }
    pub fn encode(self) -> Result<BitWriter, Error> {
        if self.target.ghost == 0 {
            return Err(Error::Bound);
        }
        self.message().encode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_event_layout_and_all_partial_reads() {
        let bytes = [0, 0, 0, 141, 0, 0, 0, 1, 0, 0, 0, 144, 2, 144, 0, 0, 0, 8];
        let bits = BitSpan::new(&bytes, 0, 141).unwrap();
        let expected = Message::Fire {
            events: vec![144],
            player: None,
            target: EntityRef {
                ghost: 82,
                entity: 1,
            },
        };
        assert_eq!(Message::decode(86, bits), Ok(expected.clone()));
        assert_eq!(expected.encode().unwrap().bytes(), &bytes);
        for cut in 0..141 {
            assert!(Message::decode(86, bits.slice(0, cut).unwrap()).is_err());
        }
        let mut trailing = expected.encode().unwrap();
        trailing.put(0, 1);
        assert!(Message::decode(86, trailing.span()).is_err());
    }
    #[test]
    fn player_context_unknown_events_and_entity_indices_are_preserved() {
        for player in [None, Some(-1), Some(4), Some(i32::MIN)] {
            let msg = Message::Fire {
                events: vec![0, u32::MAX, 7],
                player,
                target: EntityRef {
                    ghost: 8191,
                    entity: u32::MAX,
                },
            };
            assert_eq!(
                Message::decode(msg.index(), msg.encode().unwrap().span()),
                Ok(msg)
            );
        }
        for player in [0, 7, 255] {
            let msg = Message::Reached {
                event: 999,
                target: EntityRef {
                    ghost: 1,
                    entity: 0,
                },
                player,
            };
            assert_eq!(Message::decode(82, msg.encode().unwrap().span()), Ok(msg));
        }
        for reason in [0, 17] {
            let msg = Message::PlayerConnected {
                reason,
                name: vec![0xff; MAX_NAME],
            };
            assert_eq!(Message::decode(35, msg.encode().unwrap().span()), Ok(msg));
        }
        assert_eq!(
            Message::decode(26, BitWriter::new().span()),
            Ok(Message::FirstPlayerEntered)
        );
    }
    #[test]
    fn oversized_vectors_and_invalid_fields_fail_before_allocation() {
        let mut bad = BitWriter::new();
        bad.put(64, 32).put(u32::MAX.into(), 32);
        assert_eq!(Message::decode(86, bad.span()), Err(Error::Bound));
        assert_eq!(
            Message::Fire {
                events: vec![0; MAX_EVENTS + 1],
                player: None,
                target: EntityRef {
                    ghost: 1,
                    entity: 1
                }
            }
            .encode(),
            Err(Error::Bound)
        );
        assert_eq!(
            Message::Reached {
                event: 0,
                target: EntityRef {
                    ghost: 1,
                    entity: 1
                },
                player: 8
            }
            .encode(),
            Err(Error::Shape)
        );
        assert_eq!(
            Message::Fire {
                events: vec![],
                player: None,
                target: EntityRef {
                    ghost: 8192,
                    entity: 1
                }
            }
            .encode(),
            Err(Error::Bound)
        );
        for reason in 18..=31 {
            let mut bad = BitWriter::new();
            bad.put(reason, 5).put(0, 10);
            assert_eq!(Message::decode(35, bad.span()), Err(Error::Shape));
        }
        assert_eq!(
            Message::decode(1, BitWriter::new().span()),
            Err(Error::Unsupported(1))
        );
    }
}
