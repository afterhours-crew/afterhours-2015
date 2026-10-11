// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed local matchmaking admission against an initialized group and explicit
//! deployment policy, followed by a bounded, write-committed acknowledgement.
use crate::bootstrap::{body_limits, frame_limits};
use nfs_protocol::gamemanager::StartMatchmakingResponse;
mod admission;
pub use admission::{AttributeRule, Config};
pub const SEED_BYTES: usize = 8;
pub const MAX_REQUESTS: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Context,
    Config,
    Ineligible,
    Pending,
    Closed,
    Limit,
    Encode,
}
#[derive(Clone)]
pub struct Generated {
    pub session_id: u64,
    pub external_session_name: Vec<u8>,
}
impl Generated {
    /// Raw unique entropy supplied by the edge. The high nibble is a local ID
    /// domain; no account identifier or captured seed is required.
    pub fn from_seed(seed: &[u8]) -> Result<Self, Error> {
        let word: [u8; 8] = seed.try_into().map_err(|_| Error::Context)?;
        if word.iter().all(|b| *b == 0) {
            return Err(Error::Context);
        }
        let session_id = (u64::from_le_bytes(word) & 0x0fff_ffff_ffff_ffff) | 0x7000_0000_0000_0000;
        Ok(Self {
            session_id,
            external_session_name: format!("local-mm-{session_id:016x}").into_bytes(),
        })
    }
    pub fn validate(&self, group: u64) -> Result<(), Error> {
        if group == 0
            || group > i64::MAX as u64
            || self.session_id == 0
            || self.session_id > i64::MAX as u64
            || self.session_id == group
            || self.external_session_name.is_empty()
            || self.external_session_name.len() > 64
            || !self.external_session_name.iter().all(u8::is_ascii_graphic)
        {
            return Err(Error::Context);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Accepted {
    pub group: u64,
    pub persona: i64,
    pub user_session: u64,
    pub connection: u64,
    pub scenario: u64,
}
pub struct Session {
    current: Accepted,
    allocation: Generated,
    pending: bool,
    published: bool,
    closed: bool,
    requests: usize,
}
impl Session {
    pub fn new(current: Accepted, allocation: Generated) -> Result<Self, Error> {
        allocation.validate(current.group)?;
        if [
            current.persona as u64,
            current.user_session,
            current.connection,
        ]
        .iter()
        .any(|v| *v == 0 || *v > i64::MAX as u64 || *v == allocation.session_id)
        {
            return Err(Error::Context);
        }
        Ok(Self {
            current,
            allocation,
            pending: false,
            published: false,
            closed: false,
            requests: 0,
        })
    }
    pub fn reply(&mut self, current: Accepted, correlation: u32) -> Result<Vec<u8>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.pending {
            self.abort_write();
            return Err(Error::Pending);
        }
        if self.current != current {
            self.abort_write();
            return Err(Error::Context);
        }
        if self.requests == MAX_REQUESTS {
            self.abort_write();
            return Err(Error::Limit);
        }
        let result = self.encode(correlation);
        match result {
            Ok(wire) => {
                self.requests += 1;
                self.pending = true;
                Ok(wire)
            }
            Err(e) => {
                self.abort_write();
                Err(e)
            }
        }
    }
    fn encode(&self, correlation: u32) -> Result<Vec<u8>, Error> {
        let body = StartMatchmakingResponse {
            external_session_correlation_id: Some(b""),
            external_session_name: Some(&self.allocation.external_session_name),
            session_id: Some(self.allocation.session_id),
            scid: Some(b""),
            external_session_template_name: Some(b""),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)?;
        nfs_fire2::encode(
            nfs_fire2::Frame {
                fields: nfs_fire2::Fields {
                    routing_a: 4,
                    routing_b: 13,
                    category: 1,
                    correlation,
                    ..Default::default()
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
    pub fn commit_after_write(&mut self) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.pending {
            self.pending = false;
            self.published = true;
        }
        Ok(())
    }
    pub fn published(&self) -> bool {
        self.published && !self.closed
    }
    pub fn abort_write(&mut self) {
        self.closed = true;
        self.pending = false;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn current() -> Accepted {
        Accepted {
            group: 10,
            persona: 11,
            user_session: 12,
            connection: 13,
            scenario: u64::MAX,
        }
    }
    fn allocation() -> Generated {
        Generated::from_seed(&[1; 8]).unwrap()
    }
    fn session() -> Session {
        Session::new(current(), allocation()).unwrap()
    }
    #[test]
    fn typed_ack_publishes_after_write_and_retries_keep_allocation() {
        let mut s = session();
        s.commit_after_write().unwrap();
        assert!(!s.published());
        for correlation in [1, 77, 0x00ff_ffff] {
            let wire = s.reply(current(), correlation).unwrap();
            let f = nfs_fire2::decode(&wire, frame_limits())
                .unwrap()
                .unwrap()
                .frame;
            assert_eq!(
                (
                    f.fields.routing_a,
                    f.fields.routing_b,
                    f.fields.category,
                    f.fields.correlation
                ),
                (4, 13, 1, correlation)
            );
            let r = StartMatchmakingResponse::decode(f.body, body_limits()).unwrap();
            assert_eq!(r.session_id, Some(allocation().session_id));
            assert_eq!(
                r.external_session_name,
                Some(allocation().external_session_name.as_slice())
            );
            s.commit_after_write().unwrap();
            assert!(s.published());
        }
    }
    #[test]
    fn failed_or_premature_batches_never_publish() {
        for premature in [false, true] {
            let mut s = session();
            s.reply(current(), 1).unwrap();
            assert!(!s.published());
            if premature {
                assert_eq!(s.reply(current(), 2), Err(Error::Pending));
            } else {
                s.abort_write();
            }
            assert_eq!(s.commit_after_write(), Err(Error::Closed));
            assert!(!s.published());
        }
    }
    #[test]
    fn owner_changes_and_request_budget_close_only_the_affected_session() {
        for field in 0..5 {
            let mut s = session();
            let mut c = current();
            match field {
                0 => c.group += 1,
                1 => c.persona += 1,
                2 => c.user_session += 1,
                3 => c.connection += 1,
                _ => c.scenario = 0,
            };
            assert_eq!(s.reply(c, 1), Err(Error::Context));
            assert_eq!(s.reply(current(), 1), Err(Error::Closed));
            assert!(session().reply(current(), 1).is_ok());
        }
        let mut s = session();
        for i in 0..MAX_REQUESTS {
            s.reply(current(), i as u32).unwrap();
            s.commit_after_write().unwrap();
        }
        assert_eq!(s.reply(current(), 9), Err(Error::Limit));
        assert!(!s.published());
    }
    #[test]
    fn allocation_and_current_inputs_are_bounded() {
        for n in 0..=9 {
            if n != 8 {
                assert!(Generated::from_seed(&vec![1; n]).is_err());
            }
        }
        assert!(Generated::from_seed(&[0; 8]).is_err());
        let mut c = current();
        c.persona = -1;
        assert!(Session::new(c, allocation()).is_err());
        let mut g = allocation();
        g.session_id = current().group;
        assert!(Session::new(current(), g).is_err());
        let mut g = allocation();
        g.external_session_name = vec![b'a'; 65];
        assert!(Session::new(current(), g).is_err());
        let mut g = allocation();
        g.external_session_name = b"bad name".to_vec();
        assert!(Session::new(current(), g).is_err());
        assert_ne!(
            allocation().session_id,
            Generated::from_seed(&[2; 8]).unwrap().session_id
        );
        let mut s = session();
        assert_eq!(s.reply(current(), u32::MAX), Err(Error::Encode));
        assert_eq!(s.commit_after_write(), Err(Error::Closed));
    }
}
