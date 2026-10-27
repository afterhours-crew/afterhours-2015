// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Session-owned race recommendation cache. Requests merge by their entry key;
//! an empty list initializes the cache without clearing it or sending a reply.
//! This does not implement recommendation generation or event selection.
use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct State {
    received: bool,
    entries: BTreeMap<u32, frame::RecommendationEntry>,
}

#[derive(Clone, Copy, Debug)]
enum Rejected {
    Identity,
    Bound,
}

impl State {
    fn prepare(
        &self,
        persona: u64,
        messages: &[frame::Message<'_>],
    ) -> Result<Option<Self>, Rejected> {
        let mut next = None;
        for message in messages {
            let frame::Message::RecommendationSync(request) = message else {
                continue;
            };
            if persona == 0 || request.persona != persona {
                return Err(Rejected::Identity);
            }
            if request.entries.len() > frame::MAX_SPEED_WALL_ENTRIES {
                return Err(Rejected::Bound);
            }
            let state = next.get_or_insert_with(|| self.clone());
            for entry in &request.entries {
                if !state.entries.contains_key(&entry.key)
                    && state.entries.len() == frame::MAX_SPEED_WALL_ENTRIES
                {
                    return Err(Rejected::Bound);
                }
                state.entries.insert(entry.key, entry.clone());
            }
            state.received = true;
        }
        Ok(next)
    }
}

impl PlayerListener {
    pub(super) fn prepare_recommendations(
        &mut self,
        messages: &[frame::Message<'_>],
    ) -> Result<Option<State>, replication::Error> {
        self.recommendations
            .prepare(self.persona, messages)
            .map_err(|reason| {
                if refusal::logged(self.refused + 1) {
                    tracing::warn!(
                        connection = self.unsupported.connection(),
                        ?reason,
                        "recommendation sync refused"
                    );
                }
                match reason {
                    Rejected::Identity => replication::Error::Shape,
                    Rejected::Bound => replication::Error::Bound,
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::bits::BitWriter;

    fn entry(key: u32, word: u32) -> frame::RecommendationEntry {
        frame::RecommendationEntry {
            key,
            word,
            value_bits: 0x7fc01234,
            wide_word: 0x0123456789abcdef,
            flags: [true, false],
        }
    }
    fn request(persona: u64, entries: Vec<frame::RecommendationEntry>) -> frame::Message<'static> {
        frame::Message::RecommendationSync(frame::RecommendationSync { persona, entries })
    }
    fn wire(sequence: u8, messages: Vec<frame::Message<'_>>) -> BitWriter {
        frame::Builder::new()
            .messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(sequence),
                    messages,
                }],
            )
            .build()
            .unwrap()
    }
    fn receive(listener: &mut PlayerListener, wire: &BitWriter) -> bool {
        listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        })
    }
    #[test]
    fn recommendation_cache_merges_repeats_and_empty_requests_without_a_reply() {
        let mut listener = PlayerListener::new(7);
        let initial = wire(0, vec![request(7, vec![])]);
        assert!(receive(&mut listener, &initial));
        assert!(listener.recommendations.received);
        assert!(listener.recommendations.entries.is_empty());
        assert!(listener.observation(0, 0, 0, false).queues_drained());
        assert_eq!(listener.unsupported.total(), 0);
        assert!(receive(&mut listener, &initial));
        let first = wire(1, vec![request(7, vec![entry(3, 5), entry(4, 8)])]);
        assert!(receive(&mut listener, &first));
        assert!(receive(&mut listener, &first));
        assert!(receive(
            &mut listener,
            &wire(
                2,
                vec![
                    request(7, vec![entry(3, 6), entry(3, 7)]),
                    request(7, vec![])
                ]
            )
        ));
        assert_eq!(
            listener.recommendations.entries,
            BTreeMap::from([(3, entry(3, 7)), (4, entry(4, 8))])
        );
        assert!(listener.observation(0, 0, 0, false).queues_drained());
        let mut other = PlayerListener::new(8);
        assert!(receive(
            &mut other,
            &wire(0, vec![request(8, vec![entry(3, 99)])])
        ));
        assert_eq!(other.recommendations.entries[&3], entry(3, 99));
        assert_eq!(listener.recommendations.entries[&3], entry(3, 7));
    }
    #[test]
    fn recommendation_sync_rolls_back_with_late_frame_failures_and_retries() {
        let mut listener = PlayerListener::new(7);
        for request_owner in [0, 8] {
            assert!(!receive(
                &mut listener,
                &wire(0, vec![request(request_owner, vec![])])
            ));
            assert_eq!(listener.recommendations, State::default());
        }
        let messages = |bad| {
            vec![
                request(7, vec![entry(3, 5)]),
                frame::Message::CreatePlayer {
                    name: b"local".to_vec(),
                    flag: bad,
                    slot: 0,
                },
            ]
        };
        let good = wire(0, messages(false));
        let mut trailing = good.clone();
        trailing.put(0xff, 8);
        assert!(!receive(&mut listener, &trailing));
        assert!(!receive(&mut listener, &wire(0, messages(true))));
        assert_eq!(listener.recommendations, State::default());
        assert!(listener.players.objects().is_empty());
        assert!(receive(&mut listener, &good));
        assert_eq!(listener.recommendations.entries[&3], entry(3, 5));
        assert_eq!(listener.unsupported.total(), 0);
        // The same reliable sequence retries after failure; successful repeats
        // cannot apply a changed request body a second time.
        assert!(receive(
            &mut listener,
            &wire(0, vec![request(7, vec![entry(3, 99)])])
        ));
        assert_eq!(listener.recommendations.entries[&3], entry(3, 5));
    }
    #[test]
    fn recommendation_cache_union_bound_is_transactional_and_updates_still_fit() {
        let initial = State {
            received: true,
            entries: (0..frame::MAX_SPEED_WALL_ENTRIES as u32)
                .map(|key| (key, entry(key, 1)))
                .collect(),
        };
        let batch = vec![
            request(7, vec![entry(0, 2)]),
            request(7, vec![entry(frame::MAX_SPEED_WALL_ENTRIES as u32, 3)]),
        ];
        assert!(matches!(initial.prepare(7, &batch), Err(Rejected::Bound)));
        assert_eq!(initial.entries[&0], entry(0, 1));
        let next = initial
            .prepare(7, &[request(7, vec![entry(0, 2)])])
            .unwrap()
            .unwrap();
        assert_eq!(next.entries.len(), frame::MAX_SPEED_WALL_ENTRIES);
        assert_eq!(next.entries[&0], entry(0, 2));
        assert!(matches!(
            initial.prepare(0, &[request(0, vec![])]),
            Err(Rejected::Identity)
        ));
    }
}
