// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use nfs_protocol::world::BitSpan;
use replication::client_info::State;

impl PlayerListener {
    pub(super) fn prepare_client_info(
        &mut self,
        data: BitSpan<'_>,
        parsed: &mut frame::Frame<'_>,
    ) -> Result<State, ()> {
        let mut next = self.client_info.clone();
        if let Some(ghost) = &parsed.ghost {
            let input = data.after(ghost.start).map_err(|_| ())?;
            let bits = next.decode(input).map_err(|error| {
                if error == replication::Error::Unsupported {
                    self.unsupported
                        .record(diagnostics::Route::ClientGhostRecords);
                }
                if refusal::logged(self.stats.errors.saturating_add(1)) {
                    tracing::warn!(
                        connection = self.unsupported.connection(),
                        ?error,
                        "client information section rejected"
                    );
                }
            })?;
            if ghost.prefix.remaining_records() != 0 {
                parsed
                    .finish_ghost(
                        data,
                        bits,
                        self.stats.files.each_ref().map(|file| file.size()),
                    )
                    .map_err(|error| self.diagnose_decode(error))?;
            }
        }
        if !parsed.complete
            || parsed
                .consumed_bits
                .is_none_or(|bits| data.len().saturating_sub(bits) >= 8)
        {
            self.diagnose_decode(frame::Error::Shape);
            return Err(());
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world::{bits::BitWriter, files};
    use replication::{Initial, Record, Update, client_info::Update as Info};

    fn section(create: bool) -> BitWriter {
        Section {
            float_bits: None,
            flag: false,
            deleted: vec![],
            setup: Some(Setup::ContextOnly),
            records: vec![Record {
                id: 2,
                initial: create.then_some(Initial::ClientToServerInfo),
                update: Update::ClientToServerInfo(Info::Enabled(vec![None])),
            }],
        }
        .encode()
        .unwrap()
    }

    fn wire(section: BitSpan<'_>, player: Option<bool>, file: Option<files::Record>) -> BitWriter {
        let mut builder = frame::Builder::new();
        if let Some(flag) = player {
            builder = builder.messages(
                None,
                vec![frame::Group {
                    channel: 0,
                    sequence: Some(0),
                    messages: vec![frame::Message::CreatePlayer {
                        name: b"local".to_vec(),
                        flag,
                        slot: 0,
                    }],
                }],
            );
        }
        let mut wire = BitWriter::new();
        wire.put(
            u64::from(builder.mask() | (1 << frame::GHOST) | (u8::from(file.is_some()) << 4)),
            6,
        );
        builder.append(&mut wire).unwrap();
        wire.put_span(section);
        if let Some(file) = file {
            wire.put_span(file.encode(None).unwrap().span());
        }
        wire.align();
        wire
    }

    fn receive(listener: &mut PlayerListener, wire: &BitWriter) -> bool {
        listener.frame(Queued {
            advertised: None,
            frame: 0,
            data: wire.span(),
        })
    }

    fn player_count(listener: &PlayerListener) -> usize {
        listener
            .players
            .objects()
            .iter()
            .filter(|(_, object)| matches!(object.initial, Initial::Player(_)))
            .count()
    }

    #[test]
    fn invalid_object_or_file_tail_cannot_commit_player_or_receive_history() {
        let good = section(true);
        for bad in [
            wire(good.span().slice(0, 16).unwrap(), Some(false), None),
            wire(good.span(), Some(false), Some(files::Record::Finish)),
        ] {
            let mut listener = PlayerListener::new(7);
            assert!(!receive(&mut listener, &bad));
            assert!(listener.client_info.get(2).is_none());
            assert_eq!(player_count(&listener), 0);
            assert_eq!(listener.stats.file_records, 0);
            let valid = wire(good.span(), Some(false), None);
            assert!(receive(&mut listener, &valid));
            assert_eq!(player_count(&listener), 1);
            assert!(listener.client_info.get(2).unwrap().enabled);
            assert_eq!(listener.unsupported.total(), 0);
        }
    }

    #[test]
    fn domain_refusal_keeps_client_information_and_message_history_uncommitted() {
        let good = section(true);
        let mut listener = PlayerListener::new(7);
        assert!(!receive(
            &mut listener,
            &wire(good.span(), Some(true), None)
        ));
        assert!(listener.client_info.get(2).is_none());
        assert!(receive(
            &mut listener,
            &wire(good.span(), Some(false), None)
        ));
        assert_eq!(player_count(&listener), 1);
    }

    #[test]
    fn standalone_information_and_following_file_commit_only_at_complete_boundary() {
        let mut listener = PlayerListener::new(7);
        let mut first = wire(
            section(true).span(),
            None,
            Some(files::Record::Start {
                name: b"owned-test".to_vec(),
                bytes: 1,
            }),
        );
        let valid = first.clone();
        first.put(0xff, 8);
        assert!(!receive(&mut listener, &first));
        assert!(listener.client_info.get(2).is_none());
        assert_eq!(listener.stats.files[0].size(), None);
        assert!(receive(&mut listener, &valid));
        assert_eq!(listener.stats.files[0].size(), Some(1));
        assert_eq!(listener.stats.file_records, 1);
        assert!(receive(
            &mut listener,
            &wire(section(false).span(), None, None)
        ));
        let mut other = PlayerListener::new(8);
        assert!(!receive(
            &mut other,
            &wire(section(false).span(), None, None)
        ));
        assert!(other.client_info.get(2).is_none());
    }
}
