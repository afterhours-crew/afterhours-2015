// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Client-completable goals report completion on their published RPC component.
//! A report does not release the goal: its owning sequence/event does that.
use super::*;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Owner {
    connection: u8,
    persona: u64,
    actor: u16,
    participant: u16,
    selector: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    pub goal: u16,
    pub actor: u16,
    pub participant: u16,
    /// Repeated notifications do not complete the owning operation twice.
    pub first: bool,
    /// The owning operation already retired this goal; never advance it again.
    pub retired: bool,
}

impl Players {
    /// Accept the no-argument completion RPC only for an assigned spline or
    /// waypoint goal. AI reporting additionally requires current delegation.
    /// Owned human releases retain authority for an in-flight first report or
    /// repeat. Unknown retired IDs remain unsupported. AI repeats require an
    /// earlier accepted report while the goal had current delegation.
    pub fn receive_goal_completion(
        &mut self,
        connection: u8,
        persona: u64,
        body: BitSpan<'_>,
        delegated: &BTreeMap<u16, u64>,
    ) -> Result<Option<Completion>, Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        let Some(&goal) = envelope.references().first() else {
            return Ok(None);
        };
        let selector = match self.objects.get(goal).map(|o| &o.initial) {
            Some(Initial::ScriptedSplineGoal(g)) => g.selector,
            Some(Initial::GotoWaypointGoal(g)) => g.selector,
            None => match self
                .completed_goals
                .get(&goal)
                .or_else(|| self.retired_goals.get(&goal))
            {
                Some(owner) => owner.selector,
                None => return Ok(None),
            },
            _ => return Ok(None),
        };
        if route.selector() != selector || route.method_index() != 0 {
            return Ok(None);
        }
        if envelope.references().len() != 1
            || envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || route.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        // The residual seven bits are byte padding, not a Boolean argument.
        let owner = self
            .completed_goals
            .get(&goal)
            .copied()
            .or_else(|| self.retired_goals.get(&goal).copied())
            .or_else(|| {
                self.goal_assignment(connection, persona, goal, delegated)
                    .map(|(actor, participant)| Owner {
                        connection,
                        persona,
                        actor,
                        participant,
                        selector,
                    })
            })
            .ok_or(Error::UnknownObject)?;
        if owner.connection != connection || owner.persona != persona {
            return Err(Error::UnknownObject);
        }
        let first = !self.completed_goals.contains_key(&goal);
        self.completed_goals.insert(goal, owner);
        Ok(Some(Completion {
            goal,
            actor: owner.actor,
            participant: owner.participant,
            first,
            retired: self.objects.get(goal).is_none(),
        }))
    }

    pub(super) fn human_goal_owner(
        &self,
        connection: u8,
        persona: u64,
        goal: u16,
    ) -> Option<Owner> {
        let selector = match &self.objects.get(goal)?.initial {
            Initial::ScriptedSplineGoal(g) => g.selector,
            Initial::GotoWaypointGoal(g) => g.selector,
            _ => return None,
        };
        let (actor, participant) =
            self.goal_assignment(connection, persona, goal, &BTreeMap::new())?;
        Some(Owner {
            connection,
            persona,
            actor,
            participant,
            selector,
        })
    }

    fn goal_assignment(
        &self,
        connection: u8,
        persona: u64,
        goal: u16,
        delegated: &BTreeMap<u16, u64>,
    ) -> Option<(u16, u16)> {
        let assigned = |actor| {
            self.objects.get(actor).is_some_and(|o| {
                matches!(&o.current, Update::Actor(a) if a.references_a.iter()
                .chain(&a.references_b).flatten().any(|id| *id == goal))
            })
        };
        self.actors
            .iter()
            .find_map(|(&participant, &actor)| {
                (self.owns_participant(connection, persona, participant) && assigned(actor))
                    .then_some((actor, participant))
            })
            .or_else(|| {
                delegated.iter().find_map(|(&vehicle, &key)| {
                    let ids = self.ai_ids(connection, persona, key)?;
                    (ids.vehicle == vehicle && assigned(ids.actor))
                        .then_some((ids.actor, ids.participant))
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;

    fn request(goal: u16, selector: u16, method: u32, padding: u8, extra: bool) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(method.into(), 32)
            .put(padding.into(), 7);
        if extra {
            payload.put(0, 8);
        }
        let mut out = BitWriter::new();
        out.put(0, 32)
            .put(0, 32)
            .put(1, 8)
            .put(goal.into(), 13)
            .put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        out
    }
    fn human(players: &mut Players, connection: u8, persona: u64) -> u16 {
        let p = players
            .create(
                connection,
                persona,
                Request {
                    name: b"Goal".to_vec(),
                    flag: false,
                    slot: 0,
                },
            )
            .unwrap()
            .unwrap()
            .id;
        let participant = players.join(connection, persona, p).unwrap().unwrap().id;
        players
            .create_actor(connection, persona, participant)
            .unwrap();
        participant
    }
    #[test]
    fn first_completion_after_owned_retirement_cannot_affect_the_next_goal() {
        let mut players = Players::default();
        let participant = human(&mut players, 1, 101);
        let definition = spline::Definition {
            asset: entity::creation::Asset {
                bundle: 4,
                type_id: 5000,
                local_index: 0,
            },
            catalog: entity::creation::Catalog::new(4, &[(5000, 1)]).unwrap(),
            flag: false,
        };
        let (old, _) = players
            .start_spline(1, 101, participant, &definition)
            .unwrap();
        players.finish_spline(1, 101, participant, old.id).unwrap();
        let (next, _) = players
            .start_spline(1, 101, participant, &definition)
            .unwrap();
        let before = players.objects.snapshot();
        let delegates = BTreeMap::new();
        let callback = request(old.id, 0, 0, 127, false);
        assert!(
            players
                .receive_goal_completion(1, 101, callback.span(), &delegates)
                .unwrap()
                .expect("retired owned goal")
                .first
        );
        assert_eq!(
            players.receive_goal_completion(2, 202, callback.span(), &delegates),
            Err(Error::UnknownObject)
        );
        assert!(
            !players
                .receive_goal_completion(1, 101, callback.span(), &delegates)
                .unwrap()
                .unwrap()
                .first
        );
        assert_eq!(players.objects.snapshot(), before);
        assert!(players.objects.get(next.id).is_some());
        assert_eq!(
            Players::default().receive_goal_completion(1, 101, callback.span(), &delegates),
            Ok(None)
        );
    }
    #[test]
    fn delayed_waypoint_completions_share_owned_retirement_without_reviving_control() {
        for controlled in [false, true] {
            let mut players = Players::default();
            let p = human(&mut players, 1, 101);
            let route = goto_waypoint::Initial {
                selector: 7,
                serial: 1,
                allocation_serial: 0,
                waypoint: 0,
                word: 1,
                points: vec![[0, 0, 0]],
                flags: [false; 2],
                radius: 150f32.to_bits(),
                progress_updates: !controlled,
            };
            let goal = if controlled {
                let records = players
                    .start_controlled_goals(1, 101, p, vec![Initial::GotoWaypointGoal(route)])
                    .unwrap();
                let id = records[0].id;
                players.release_controlled_goals(1, 101, p, &[id]).unwrap();
                id
            } else {
                let changes = players.start_player_route(1, 101, p, route, None).unwrap();
                let id = changes.goals[0];
                players.release_player_route(1, 101, p, id, None).unwrap();
                id
            };
            let before = players.objects.snapshot();
            let result = players
                .receive_goal_completion(
                    1,
                    101,
                    request(goal, 7, 0, 85, false).span(),
                    &BTreeMap::new(),
                )
                .unwrap()
                .expect("retired waypoint");
            assert!(result.first && result.retired);
            assert_eq!(players.objects.snapshot(), before);
        }
    }
    #[test]
    fn assigned_goal_completion_is_owned_idempotent_and_lifetime_bound() {
        let mut players = Players::default();
        let a = human(&mut players, 1, 101);
        let _b = human(&mut players, 2, 202);
        let definition = spline::Definition {
            asset: entity::creation::Asset {
                bundle: 4,
                type_id: 5000,
                local_index: 0,
            },
            catalog: entity::creation::Catalog::new(4, &[(5000, 1)]).unwrap(),
            flag: false,
        };
        let (goal, _) = players.start_spline(1, 101, a, &definition).unwrap();
        let delegates = BTreeMap::new();
        let call = request(goal.id, 0, 0, 93, false);
        let before = players.objects.snapshot();
        for (c, p) in [(2, 101), (1, 202), (2, 202)] {
            assert_eq!(
                players.receive_goal_completion(c, p, call.span(), &delegates),
                Err(Error::UnknownObject)
            );
        }
        assert!(players.completed_goals.is_empty());
        for (selector, method) in [(1, 0), (0, 1)] {
            assert_eq!(
                players.receive_goal_completion(
                    1,
                    101,
                    request(goal.id, selector, method, 0, false).span(),
                    &delegates
                ),
                Ok(None)
            );
        }
        assert_eq!(
            players.receive_goal_completion(
                1,
                101,
                request(goal.id, 0, 0, 0, true).span(),
                &delegates
            ),
            Err(Error::Shape)
        );
        assert!(
            players
                .receive_goal_completion(1, 101, call.span(), &delegates)
                .unwrap()
                .unwrap()
                .first
        );
        assert!(
            !players
                .receive_goal_completion(1, 101, call.span(), &delegates)
                .unwrap()
                .unwrap()
                .first
        );
        assert_eq!(
            players.objects.snapshot(),
            before,
            "completion never releases the cinematic's goal"
        );
        players.finish_spline(1, 101, a, goal.id).unwrap();
        assert!(
            !players
                .receive_goal_completion(1, 101, call.span(), &delegates)
                .unwrap()
                .unwrap()
                .first
        );
        let (next, _) = players.start_spline(1, 101, a, &definition).unwrap();
        assert_ne!(goal.id, next.id);
        assert!(
            players
                .receive_goal_completion(
                    1,
                    101,
                    request(next.id, 0, 0, 127, false).span(),
                    &delegates
                )
                .unwrap()
                .unwrap()
                .first
        );
        assert_eq!(
            players.receive_goal_completion(
                1,
                101,
                request(8191, 0, 0, 0, false).span(),
                &delegates
            ),
            Ok(None)
        );
    }
}
