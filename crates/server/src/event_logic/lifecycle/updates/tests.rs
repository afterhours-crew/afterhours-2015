// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
use crate::event_logic::lifecycle::{
    Effect, Host, SceneInputs,
    graphs::{Driver, Instance, Reader, Runtime},
    tests::{definition, identity},
};
use nfs_content::{
    events::lifecycle::{StateKind, bindings::UpdateManager},
    scoring::events::{Composition, DataConnection, Node},
};
use nfs_world::{
    participant_states::Notification,
    scoring::{
        events::native::Operation,
        interpreter::{Inputs, Session, hosted::Action as GraphAction},
        program::{
            Behavior, Identity as ProgramIdentity, Node as ProgramNode, Program, Step as Startup,
        },
        properties::Port,
    },
};
use std::sync::Arc;

fn object(node: u16) -> Object {
    Object { instance: 10, node }
}
fn call(node: u16, call: Call) -> Request {
    Request {
        instance: 10,
        node,
        call,
    }
}
fn tick() -> Step {
    Step::Graph {
        id: 10,
        action: GraphAction::Graph(Operation::Update { node: 0 }),
    }
}
fn fixture() -> (
    Arc<Plan>,
    Registry,
    Bindings,
    Arc<Composition>,
    BTreeMap<Identity, Service>,
) {
    let mut data = definition();
    data.transitions.clear();
    data.registered_transitions.clear();
    data.bindings.update_managers = vec![UpdateManager {
        identity: identity(0),
        sets: vec![identity(4), identity(5)],
    }];
    for s in &mut data.states {
        s.kind = StateKind::Updateable;
        s.configuration = StateConfiguration::Updateable {
            update_set: identity(if s.serializer == 3 { 5 } else { 4 }),
        };
    }
    let plan = Arc::new(Plan::new(&data, |_| unreachable!()).unwrap());
    let c = Arc::new(Composition {
        nodes: (0..6)
            .map(|i| Node {
                identity: identity(i),
                class: match i {
                    0 => "UpdateableManagerEntityData",
                    1..=3 => "UpdateableParticipantStateEntityData",
                    _ => "UpdateSetEntityData",
                }
                .into(),
                listeners: None,
                forward: None,
            })
            .collect(),
        buses: vec![],
        bus_entities: vec![],
        properties: vec![],
        connections: vec![],
        links: [
            (0, MANAGER_SETS, 4),
            (0, MANAGER_SETS, 5),
            (1, STATE_SET, 4),
            (2, STATE_SET, 4),
            (3, STATE_SET, 5),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (source, field, target))| DataConnection {
            source: Port {
                node: source,
                field,
            },
            target: Port {
                node: target,
                field: 0,
            },
            index: i as u16,
            bus: 0,
            flags: None,
        })
        .collect(),
    });
    let mut links = Registry::default();
    links.install(10, c.clone(), &BTreeMap::new()).unwrap();
    let bindings = Bindings::new(
        10,
        plan.clone(),
        &c,
        &(0..6).map(identity).collect::<Vec<_>>(),
        &links,
        &BTreeMap::new(),
    )
    .unwrap();
    (
        plan,
        links,
        bindings,
        c,
        [4, 5].map(|n| (identity(n), Service::UpdateSet)).into(),
    )
}
#[derive(Clone, Copy, Debug)]
enum Mutation {
    DetachState,
    DestroySource,
    Nested,
    AddMember,
    ActivateLate,
    Fail,
}
#[derive(Clone, Debug)]
struct Owner {
    updates: Updates,
    links: Registry,
    trace: Vec<(StateId, u64)>,
    mutation: Option<Mutation>,
}
impl Driver for Owner {
    fn resume(&mut self, owner: u64, token: u64, _: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        self.updates
            .resume(owner, token)?
            .ok_or(Error::InvalidGraph)
    }
    fn invoke(&mut self, request: Request, _: &mut dyn Reader) -> Result<Vec<Step>, Error> {
        self.updates
            .invoke(&request, &self.links)?
            .ok_or(Error::UnsupportedEvent)
    }
    fn effect(
        &mut self,
        effect: &Effect,
        _: &dyn nfs_world::participant_states::Membership,
        _: &mut dyn Reader,
    ) -> Result<Vec<Step>, Error> {
        let Effect::State(Notification::Completed { state, participant }) = effect else {
            return Ok(vec![]);
        };
        self.trace.push((*state, *participant));
        if *state == 2
            && let Some(mutation) = self.mutation.take()
        {
            match mutation {
                Mutation::DetachState => {
                    self.updates.detach(object(1))?;
                }
                Mutation::DestroySource => {
                    self.updates.detach(object(4))?;
                }
                Mutation::Nested => return Ok(vec![tick()]),
                Mutation::AddMember => {
                    return Ok(vec![Step::Lifecycle(vec![Command::State(Action::Enter {
                        state: 1,
                        participant: 8,
                    })])]);
                }
                Mutation::ActivateLate => {
                    return self
                        .updates
                        .invoke(&call(1, Call::Activate), &self.links)?
                        .ok_or(Error::UnsupportedEvent);
                }
                Mutation::Fail => return Err(Error::UnsupportedEvent),
            }
        }
        Ok(vec![])
    }
}
fn runtime(mutation: Option<Mutation>) -> Runtime<Owner> {
    let (plan, links, bindings, c, services) = fixture();
    let mut updates = Updates::new(45).unwrap();
    updates
        .install(
            10,
            &plan,
            &(0..6).map(identity).collect::<Vec<_>>(),
            &services,
            &links,
            &bindings,
        )
        .unwrap();
    let nodes = c
        .nodes
        .iter()
        .map(|n| ProgramNode {
            identity: ProgramIdentity {
                asset: n.identity.asset.clone(),
                index: n.identity.index as u32,
                instances: vec![],
            },
            class: n.class.clone(),
            created: true,
            events: true,
            properties: false,
            forward: None,
            defaults: vec![],
            behavior: Behavior::Native,
        })
        .collect();
    let mut startup: Vec<_> = (0..6).map(Startup::Bind).collect();
    startup.extend(
        [0, 4, 5, 2, 1, 3]
            .into_iter()
            .filter(|node| *node != 1 || !matches!(mutation, Some(Mutation::ActivateLate)))
            .map(Startup::Activate),
    );
    let instance = Instance {
        session: Session::new(Arc::new(Program {
            nodes,
            cells: vec![],
            startup,
            connections: vec![],
            unmodeled: vec![],
        }))
        .unwrap(),
        inputs: Inputs {
            participant: 7,
            vehicle: None,
            others: vec![],
            participants: vec![],
            road: None,
            stats: None,
            now: 0.,
            time_scale: 1.,
        },
    };
    let mut r = Runtime::new(
        Host::prepare(plan, SceneInputs::default()).unwrap(),
        Owner {
            updates,
            links,
            trace: vec![],
            mutation,
        },
    );
    r.execute(vec![
        Step::Install {
            id: 10,
            instance: Box::new(instance),
        },
        Step::Graph {
            id: 10,
            action: GraphAction::Start,
        },
        Step::Lifecycle(
            (1..=3)
                .map(|state| {
                    Command::State(Action::Enter {
                        state,
                        participant: 7,
                    })
                })
                .collect(),
        ),
    ])
    .unwrap();
    r
}

#[test]
fn manager_updates_authored_sets_and_activation_ordered_states_without_recompletion() {
    let mut r = runtime(None);
    assert!(r.driver().trace.is_empty());
    r.execute(vec![tick()]).unwrap();
    assert_eq!(r.driver().trace, [(2, 7), (1, 7), (3, 7)]);
    r.execute(vec![tick()]).unwrap();
    assert_eq!(r.driver().trace.len(), 3);
    assert!(r.driver().updates.pending.is_empty());
}

#[test]
fn native_listener_removal_and_source_destruction_take_effect_during_dispatch() {
    for mutation in [Mutation::DetachState, Mutation::DestroySource] {
        let mut r = runtime(Some(mutation));
        r.execute(vec![tick()]).unwrap();
        assert_eq!(r.driver().trace, [(2, 7), (3, 7)]);
        assert!(r.lifecycle().machine().completed(1).unwrap().is_empty());
        assert!(r.driver().updates.pending.is_empty());
        if matches!(mutation, Mutation::DestroySource) {
            assert!(!r.driver().updates.sets.contains_key(&object(4)));
            assert_eq!(r.driver().updates.states[&object(1)].source, None);
            let mut updates = r.driver().updates.clone();
            assert!(updates.update_set(object(4)).unwrap().is_empty());
            assert_eq!(
                updates.update_set(object(99)).unwrap_err(),
                Error::UnknownNode
            );
        }
    }
}

#[test]
fn nested_updates_finish_before_outer_listener_iteration_resumes() {
    let mut r = runtime(Some(Mutation::Nested));
    r.execute(vec![tick()]).unwrap();
    // Nested manager completes both later states. Outer resumes without
    // duplicate completion, and all live-list continuations drain.
    assert_eq!(r.driver().trace, [(2, 7), (1, 7), (3, 7)]);
    assert!(r.driver().updates.pending.is_empty());
    assert_eq!(r.driver().updates.sets[&object(4)].busy, 0);
}

#[test]
fn later_state_update_sees_members_added_by_an_earlier_callback() {
    let mut r = runtime(Some(Mutation::AddMember));
    r.execute(vec![tick()]).unwrap();
    assert_eq!(r.driver().trace, [(2, 7), (1, 7), (1, 8), (3, 7)]);
}

#[test]
fn listener_appended_during_a_callback_is_visited_in_the_same_update() {
    let mut r = runtime(Some(Mutation::ActivateLate));
    assert!(!r.driver().updates.states[&object(1)].active);
    r.execute(vec![tick()]).unwrap();
    assert_eq!(r.driver().trace, [(2, 7), (1, 7), (3, 7)]);
    assert!(r.driver().updates.states[&object(1)].active);
}

#[test]
fn failed_update_rolls_back_membership_completion_driver_and_continuations() {
    let mut r = runtime(Some(Mutation::Fail));
    assert_eq!(
        r.execute(vec![tick()]).unwrap_err(),
        Error::UnsupportedEvent
    );
    assert!(r.driver().trace.is_empty());
    assert!(r.driver().updates.pending.is_empty());
    for state in 1..=3 {
        assert!(r.lifecycle().machine().completed(state).unwrap().is_empty());
    }
}

#[test]
fn unsupported_derived_state_and_mismatched_links_reject_installation_atomically() {
    let (plan, _, _, c, services) = fixture();
    for derived in [false, true] {
        let mut c = (*c).clone();
        if derived {
            c.nodes[1].class = "WaitForPendingItemUpdatesParticipantStateEntityData".into();
        } else {
            c.links[2].target.node = 5;
        }
        let mut links = Registry::default();
        links
            .install(10, Arc::new(c.clone()), &BTreeMap::new())
            .unwrap();
        let serializers = (0..6).map(identity).collect::<Vec<_>>();
        let bindings =
            Bindings::new(10, plan.clone(), &c, &serializers, &links, &BTreeMap::new()).unwrap();
        let mut updates = Updates::new(45).unwrap();
        assert!(
            updates
                .install(10, &plan, &serializers, &services, &links, &bindings)
                .is_err()
        );
        assert!(updates.sets.is_empty());
        assert!(updates.states.is_empty());
        assert!(updates.managers.is_empty());
    }
}
