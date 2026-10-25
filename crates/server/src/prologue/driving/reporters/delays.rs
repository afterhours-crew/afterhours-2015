// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;
const MODE_STARTED: u32 = 4_234_712_132;
type Delays = (BTreeMap<u16, f32>, delay::Manager<Option<u64>>);
pub(super) fn prepare(
    index: &mut AssetIndex,
    composition: &nfs_content::scoring::events::Composition,
    name: &str,
    ids: &BTreeMap<Identity, u16>,
) -> Result<Delays, nfs_content::Error> {
    let mut values = BTreeMap::new();
    let mut definitions = vec![];
    for node in index.scoring_delays(name)? {
        let id = *ids
            .get(&node.identity)
            .ok_or_else(|| nfs_content::Error::Content("intro delay occurrence".into()))?;
        if composition.nodes[usize::from(id)]
            .listeners
            .is_some_and(|r| r.events == nfs_content::scoring::realms::Realm::Client)
        {
            continue;
        }
        let c = node.configuration;
        if c.time_domain_name != "TimeDeltaType_World"
            || !c.delay.is_finite()
            || !(0. ..=600.).contains(&c.delay)
            || composition
                .properties
                .iter()
                .any(|p| p.target.node == id && p.target.field == delay::DELAY)
            || composition
                .connections
                .iter()
                .filter(|e| e.target.node == id)
                .any(|e| {
                    e.target.event != delay::IN
                        || e.source.event != MODE_STARTED
                        || composition.nodes[usize::from(e.source.node)].class
                            != "GameModeEntityData"
                })
        {
            return Err(nfs_content::Error::Content(
                "unsupported intro delay clock or startup input".into(),
            ));
        }
        definitions.push((id, c.definition));
        values.insert(id, c.delay);
    }
    let manager = delay::Manager::new(composition.nodes.len(), &definitions)
        .map_err(|e| nfs_content::Error::Content(e.to_string()))?;
    Ok((values, manager))
}
impl delay::Handler for HandlerState {
    type Context = Option<u64>;
    fn delays(&mut self) -> &mut delay::Manager<Self::Context> {
        &mut self.delay_manager
    }
    fn delay_input(&self, node: u16) -> Result<f32, scoring::Error> {
        self.delay_values
            .get(&node)
            .copied()
            .ok_or(scoring::Error::UnknownNode)
    }
    fn delay_scale(&self, node: u16) -> Result<f32, scoring::Error> {
        self.delay_values
            .contains_key(&node)
            .then_some(1.)
            .ok_or(scoring::Error::UnknownNode)
    }
    fn delay_output(
        &mut self,
        node: u16,
        participant: Option<u64>,
        dispatch: &mut Dispatch<'_>,
    ) -> Result<(), scoring::Error> {
        let source = Endpoint {
            node,
            event: delay::OUT,
        };
        match participant {
            Some(p) => dispatch.emit_participant(self, source, p),
            None => dispatch.emit(self, source),
        }
    }
}
impl Reporters {
    pub(in crate::prologue::driving) fn start(
        &mut self,
        seconds: f64,
    ) -> Result<Vec<Command>, nfs_world::participant_states::Error> {
        if self.started {
            return Ok(vec![]);
        }
        if !seconds.is_finite() || seconds < 0. {
            return Err(nfs_world::participant_states::Error::Invalid);
        }
        let nodes = self.state.delay_values.keys().copied().collect::<Vec<_>>();
        let root = self.root;
        let actions = self.run(|s, d| {
            for node in nodes {
                d.initialize_delay(s, node, None)?;
            }
            d.emit(
                s,
                Endpoint {
                    node: root,
                    event: MODE_STARTED,
                },
            )
        })?;
        self.started = true;
        self.delay_clock = Some(seconds);
        Ok(actions)
    }
    pub(in crate::prologue::driving) fn tick(
        &mut self,
        seconds: f64,
    ) -> Result<Vec<Command>, nfs_world::participant_states::Error> {
        // Packet processing also updates Active's receipt clock. Keep our last
        // consumed tick separately so those inputs cannot erase elapsed time.
        let previous = self
            .delay_clock
            .ok_or(nfs_world::participant_states::Error::Invalid)?;
        let delta = seconds - previous;
        if !self.started || !delta.is_finite() || delta < 0. || delta > f64::from(f32::MAX) {
            return Err(nfs_world::participant_states::Error::Invalid);
        }
        let actions = self.run(|s, d| d.update_delays(s, delta as f32))?;
        self.delay_clock = Some(seconds);
        Ok(actions)
    }
    fn run(
        &mut self,
        operation: impl FnOnce(&mut HandlerState, &mut Dispatch<'_>) -> Result<(), scoring::Error>,
    ) -> Result<Vec<Command>, nfs_world::participant_states::Error> {
        let (mut next, remote) = self
            .graph
            .transaction(&self.state, operation)
            .map_err(|_| nfs_world::participant_states::Error::Invalid)?;
        if !remote.is_empty() {
            return Err(nfs_world::participant_states::Error::Invalid);
        }
        let actions = std::mem::take(&mut next.actions);
        self.state = next;
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(bad_sink: bool) -> Reporters {
        let mut r = super::super::tests::fixture(false);
        r.state.gates.clear();
        r.state.delay_values.insert(1, 3.);
        r.state.delay_manager = delay::Manager::new(
            4,
            &[(
                1,
                delay::Definition {
                    auto_start: false,
                    run_once: false,
                    remove_duplicates: true,
                },
            )],
        )
        .unwrap();
        let mut edges = vec![
            events::Connection {
                source: Endpoint {
                    node: 0,
                    event: MODE_STARTED,
                },
                target: Endpoint {
                    node: 1,
                    event: delay::IN,
                },
                delivery: events::Delivery::Local,
            },
            events::Connection {
                source: Endpoint {
                    node: 1,
                    event: delay::OUT,
                },
                target: Endpoint {
                    node: 2,
                    event: transitions::TRIGGER_ALL,
                },
                delivery: events::Delivery::Local,
            },
        ];
        if bad_sink {
            edges.push(events::Connection {
                source: Endpoint {
                    node: 1,
                    event: delay::OUT,
                },
                target: Endpoint { node: 3, event: 5 },
                delivery: events::Delivery::Local,
            });
        }
        r.graph = events::Graph::new(4, edges).unwrap();
        r
    }
    #[test]
    fn mode_started_uses_shared_delay_and_preserves_the_transition_once() {
        let mut r = fixture(false);
        assert!(r.tick(1.).is_err());
        assert!(r.start(100.).unwrap().is_empty());
        assert!(r.tick(102.5).unwrap().is_empty());
        assert!(r.start(102.5).unwrap().is_empty());
        for bad in [-1., f64::NAN, f64::INFINITY] {
            assert!(r.tick(bad).is_err());
        }
        assert_eq!(
            r.tick(103.).unwrap(),
            vec![Command::State(Action::MoveAll {
                sources: vec![7],
                target: Some(8)
            })]
        );
        assert!(r.tick(110.).unwrap().is_empty());
    }
    #[test]
    fn failed_delay_callback_rolls_back_removal_and_queued_transition() {
        let mut r = fixture(true);
        r.start(100.).unwrap();
        assert!(r.tick(103.).is_err());
        assert_eq!(r.state.delay_manager.pending(1).unwrap(), 1);
        assert!(r.state.actions.is_empty());
        r.graph = fixture(false).graph;
        assert_eq!(r.tick(103.).unwrap().len(), 1);
        assert_eq!(r.state.delay_manager.pending(1).unwrap(), 0);
    }
}
