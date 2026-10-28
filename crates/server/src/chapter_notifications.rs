// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Mission-chapter values on scene notifications
//! (`nfs_content::chapter_notifications`): the intro objectives' state as the
//! Club/Garage markers and garage-entry box (spawnpoints s4), the entrance
//! prompt gate (s5) and the car-selection camera (garage s24). The values
//! follow the committed progression and are sent when they change. In the
//! participant's initial notification flush an Int value is always sent and
//! a Bool value only when it differs from the property default (official
//! E1026 36.319 s: s4 0, s5 0, s24 true; E912 32.336 s: s4 0, s5 0 and no
//! s24). The first written s4 sets the client's box: 0 disables it.
use nfs_content::chapter_notifications::{Kind, Notification};
use nfs_world::{
    game_mode::Arguments,
    participants::{Endpoint, HostRpc},
    replication::{Error, Initial, state::World},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    entries: Vec<Notification>,
}

/// Values last sent per notification, for one participant.
pub type Sent = BTreeMap<usize, i32>;

impl Content {
    pub fn prepare(
        index: &mut nfs_content::assets::AssetIndex,
    ) -> Result<Self, nfs_content::Error> {
        Ok(Self::new(index.chapter_notifications()?))
    }
    pub fn new(entries: Vec<Notification>) -> Self {
        Self { entries }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn call(world: &World, entry: &Notification, value: i32) -> Result<HostRpc, Error> {
        let scene = world.scene(entry.scene_key).ok_or(Error::UnknownObject)?;
        let Some(Initial::SubLevel { fields, .. }) = world.get(scene).map(|o| &o.initial) else {
            return Err(Error::TypeMismatch);
        };
        let rpc = fields
            .get(entry.serializer)
            .and_then(nfs_world::replication::sublevel::Initial::rpc)
            .ok_or(Error::TypeMismatch)?;
        Ok(match entry.kind {
            Kind::Bool => HostRpc::ModeNotification(nfs_world::garage::presence::Notification {
                endpoint: Endpoint {
                    scene,
                    selector: rpc.selector,
                    serial: rpc.serial,
                },
                enabled: value != 0,
            }),
            Kind::Int => HostRpc::GameMode(crate::event_logic::call(
                world,
                scene,
                rpc.selector,
                0,
                None,
                Arguments::Word(value as u32),
            )?),
        })
    }

    /// The garage-entry box value last sent, when the content has one.
    pub fn entry_box(&self, sent: &Sent) -> Option<bool> {
        let index = self.entries.iter().position(|e| e.entry_box)?;
        sent.get(&index).map(|&v| v != 0)
    }
    /// The calls that bring the client to the current values. `objective`
    /// answers (active, completed) per stable objective id. Scenes not yet
    /// in the world are left for a later pass.
    pub fn update(
        &self,
        world: &World,
        objective: impl Fn(u64) -> (bool, bool),
        sent: &mut Sent,
    ) -> Result<Vec<HostRpc>, Error> {
        let mut out = vec![];
        for (index, entry) in self.entries.iter().enumerate() {
            if world.scene(entry.scene_key).is_none() {
                continue;
            }
            let value = i32::from(entry.value.eval(&objective));
            let unchanged = match sent.get(&index) {
                Some(&last) => last == value,
                None => entry.kind == Kind::Bool && value == entry.default,
            };
            if unchanged {
                sent.insert(index, value);
                continue;
            }
            out.push(Self::call(world, entry, value)?);
            sent.insert(index, value);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_content::chapter_notifications::Expr;
    use nfs_protocol::world::rpc::Serial;
    use nfs_world::replication::{
        players::Players,
        sublevel::{self, Kind as SceneKind, Profile},
    };

    const SPAWN: u32 = 2_994_378_277;
    const GARAGE: u32 = 967_904_630;
    const DRIVE: u64 = 11;
    const RACE1: u64 = 13;
    const RACE2: u64 = 14;

    fn content() -> Content {
        Content::new(vec![
            Notification {
                kind: Kind::Int,
                value: Expr::Active(DRIVE),
                scene_key: SPAWN,
                serializer: 5,
                default: 0,
                entry_box: false,
            },
            Notification {
                kind: Kind::Int,
                value: Expr::Any(vec![Expr::Active(RACE1), Expr::Active(RACE2)]),
                scene_key: SPAWN,
                serializer: 6,
                default: 0,
                entry_box: false,
            },
            Notification {
                kind: Kind::Bool,
                value: Expr::Not(Box::new(Expr::Completed(RACE1))),
                scene_key: GARAGE,
                serializer: 25,
                default: 0,
                entry_box: false,
            },
        ])
    }
    fn world(scenes: &[u32]) -> Players {
        let mut players = Players::default();
        let mut startup = vec![SceneKind::RpcReferences; 30];
        startup[0] = SceneKind::Noop;
        let profiles = scenes
            .iter()
            .map(|&k| (k, Profile::new(false, &startup).unwrap()))
            .collect();
        let content = sublevel::Content::new(profiles).unwrap();
        let ids: Vec<u16> = (0..scenes.len() as u16).map(|i| 70 + i).collect();
        let creations = scenes
            .iter()
            .zip(&ids)
            .map(|(&key, &id)| {
                sublevel::ordinary::creation(
                    id,
                    key,
                    None,
                    Serial::new(0).unwrap(),
                    &sublevel::ordinary::unpopulated(content.profile(key).unwrap()).unwrap(),
                    &content,
                )
                .unwrap()
            })
            .collect();
        players.create_scenes(&ids, creations, &content).unwrap();
        players
    }
    /// (selector, value) of each call.
    fn values(calls: &[HostRpc]) -> Vec<(u16, i32)> {
        calls
            .iter()
            .map(|c| match c {
                HostRpc::ModeNotification(n) => (n.endpoint.selector, i32::from(n.enabled)),
                HostRpc::GameMode(call) => match call.arguments {
                    Arguments::Word(v) => (call.selector, v as i32),
                    _ => panic!("word argument"),
                },
                _ => panic!("notification"),
            })
            .collect()
    }

    #[test]
    fn the_intro_objectives_drive_markers_prompt_gate_and_camera() {
        let content = content();
        let players = world(&[SPAWN, GARAGE]);
        let world = players.objects();
        let selector = |key: u32, serializer: usize| -> u16 {
            let scene = world.scene(key).unwrap();
            let Some(Initial::SubLevel { fields, .. }) = world.get(scene).map(|o| &o.initial)
            else {
                panic!()
            };
            fields[serializer].rpc().unwrap().selector
        };
        let (s4, s5, s24) = (selector(SPAWN, 5), selector(SPAWN, 6), selector(GARAGE, 25));
        let mut state: BTreeMap<u64, (bool, bool)> = BTreeMap::new();
        let mut sent = Sent::new();
        let step = |state: &BTreeMap<u64, (bool, bool)>, sent: &mut Sent| {
            values(
                &content
                    .update(
                        world,
                        |id| state.get(&id).copied().unwrap_or_default(),
                        sent,
                    )
                    .unwrap(),
            )
        };
        // Fresh career flush: both ints (even 0) and the camera's true.
        assert_eq!(step(&state, &mut sent), [(s4, 0), (s5, 0), (s24, 1)]);
        assert!(
            step(&state, &mut sent).is_empty(),
            "unchanged values are not re-sent"
        );
        state.insert(DRIVE, (true, false));
        assert_eq!(step(&state, &mut sent), [(s4, 1)]);
        state.insert(DRIVE, (false, true));
        state.insert(RACE1, (true, false));
        assert_eq!(step(&state, &mut sent), [(s4, 0), (s5, 1)]);
        // Race 1 completes and race 2 starts in one commit: s5 stays 1.
        state.insert(RACE1, (false, true));
        state.insert(RACE2, (true, false));
        assert_eq!(step(&state, &mut sent), [(s24, 0)]);
        state.insert(RACE2, (false, true));
        assert_eq!(step(&state, &mut sent), [(s5, 0)]);
        // A session after the intro: the ints only, the camera stays at its
        // default and is not sent (E912).
        let mut after = Sent::new();
        assert_eq!(step(&state, &mut after), [(s4, 0), (s5, 0)]);
    }

    #[test]
    fn a_missing_scene_waits_and_participants_keep_their_own_values() {
        let content = content();
        let only_spawn = world(&[SPAWN]);
        let mut sent = Sent::new();
        let calls = content
            .update(only_spawn.objects(), |_| (false, false), &mut sent)
            .unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            !sent.contains_key(&2),
            "the garage value waits for its scene"
        );
        let both = world(&[SPAWN, GARAGE]);
        let calls = content
            .update(both.objects(), |_| (false, false), &mut sent)
            .unwrap();
        assert_eq!(values(&calls).len(), 1);
        let mut other = Sent::new();
        assert_eq!(
            content
                .update(both.objects(), |_| (false, false), &mut other)
                .unwrap()
                .len(),
            3
        );
    }
}
