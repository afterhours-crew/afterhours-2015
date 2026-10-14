// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-connection MessageEX ordering and bounded chunk assembly. Application
//! receipts alone do not deduplicate messages retried in a new handler frame.
use super::{CHUNK_BYTES, EX_CHUNK, Group, MAX_GROUPS, Message, Messages, REGISTRY_SIZE};

pub const MAX_BYTES: usize = 256 * CHUNK_BYTES;
pub const RECEIVE_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Shape,
    Sequence,
    Interrupted,
    Timeout,
    Clock,
}

#[derive(Clone, Debug)]
struct Pending {
    target: u8,
    ordinal: usize,
    started: u64,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
struct Channel {
    next: Option<u8>,
    pending: Option<Pending>,
}

/// Owned reassembly storage lives as long as the decoded borrowed messages.
#[derive(Debug)]
pub enum Received<'a> {
    Direct(Message<'a>),
    Assembled { target: u8, bytes: Vec<u8> },
}

impl Received<'_> {
    pub fn decode(&self) -> Result<Message<'_>, super::Error> {
        match self {
            Self::Direct(message) => Ok(message.clone()),
            Self::Assembled { target, bytes } => super::assembled_message(*target, bytes),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Receiver {
    channels: [Channel; 4],
    now: u64,
}

impl Receiver {
    pub fn alive(&self, now: u64) -> bool {
        now >= self.now
            && self.channels.iter().all(|channel| {
                channel
                    .pending
                    .as_ref()
                    .is_none_or(|pending| now.saturating_sub(pending.started) < RECEIVE_MS)
            })
    }

    /// Prepare a complete handler-frame transition. The caller commits the
    /// returned receiver only after its message consumers accept the frame.
    /// A sequence gap is refused, allowing reliable retransmission to fill it;
    /// old messages and overlapping retransmissions are skipped modulo 128.
    pub fn prepare<'a>(
        &self,
        messages: &Messages<'a>,
        now: u64,
    ) -> Result<(Self, Vec<Received<'a>>), Error> {
        if now < self.now {
            return Err(Error::Clock);
        }
        if !self.alive(now) {
            return Err(Error::Timeout);
        }
        if messages.groups.len() > MAX_GROUPS
            || messages
                .groups
                .iter()
                .any(|group| group.messages.is_empty())
            || messages
                .groups
                .iter()
                .map(|g| g.messages.len())
                .sum::<usize>()
                > MAX_GROUPS
        {
            return Err(Error::Shape);
        }
        let mut next = self.clone();
        next.now = now;
        let mut output = Vec::new();
        for group in &messages.groups {
            next.group(group, now, &mut output)?;
        }
        Ok((next, output))
    }

    fn group<'a>(
        &mut self,
        group: &Group<'a>,
        now: u64,
        output: &mut Vec<Received<'a>>,
    ) -> Result<(), Error> {
        let channel = self
            .channels
            .get_mut(usize::from(group.channel))
            .ok_or(Error::Shape)?;
        if group.channel.is_multiple_of(2) != group.sequence.is_some()
            || group.sequence.is_some_and(|s| s > 127)
        {
            return Err(Error::Shape);
        }
        for (offset, message) in group.messages.iter().enumerate() {
            if let Some(first) = group.sequence {
                let sequence = first.wrapping_add(offset as u8) & 127;
                if let Some(expected) = channel.next {
                    let ahead = sequence.wrapping_sub(expected) & 127;
                    if ahead >= 64 {
                        continue;
                    }
                    if ahead != 0 {
                        return Err(Error::Sequence);
                    }
                }
                channel.next = Some(sequence.wrapping_add(1) & 127);
            }
            match message {
                Message::Chunk {
                    ordinal,
                    target,
                    last,
                    bytes,
                } => {
                    if bytes.is_empty()
                        || bytes.len() > CHUNK_BYTES
                        || (!last && bytes.len() != CHUNK_BYTES)
                        || (*ordinal == 0) != target.is_some()
                        || target.is_some_and(|t| t >= REGISTRY_SIZE || t == EX_CHUNK)
                    {
                        return Err(Error::Shape);
                    }
                    if let Some(target) = target {
                        if channel.pending.is_some() {
                            return Err(Error::Interrupted);
                        }
                        channel.pending = Some(Pending {
                            target: *target,
                            ordinal: 0,
                            started: now,
                            bytes: Vec::new(),
                        });
                    }
                    let pending = channel.pending.as_mut().ok_or(Error::Sequence)?;
                    if pending.ordinal != usize::from(*ordinal) {
                        return Err(Error::Sequence);
                    }
                    if pending.bytes.len() + bytes.len() > MAX_BYTES || (*ordinal == 255 && !last) {
                        return Err(Error::Shape);
                    }
                    pending.ordinal += 1;
                    pending.bytes.extend_from_slice(bytes);
                    if *last {
                        let pending = channel.pending.take().ok_or(Error::Shape)?;
                        output.push(Received::Assembled {
                            target: pending.target,
                            bytes: pending.bytes,
                        });
                    }
                }
                _ => {
                    // Refuse rather than silently lose a partial mutation and
                    // then accept its commit. The enclosing frame is retried.
                    if channel.pending.is_some() {
                        return Err(Error::Interrupted);
                    }
                    output.push(Received::Direct(message.clone()));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
