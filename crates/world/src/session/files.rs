// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Event, Host, Output, Stage, application};
use crate::{
    bits::BitWriter,
    files::{
        Completed, Record,
        sender::{Error, Policy, Sender},
    },
};
use application::delivery::{Status, Ticket};

pub(super) struct Outgoing {
    sender: Sender<Ticket>,
    pub receipt: Option<Ticket>,
}

impl Host {
    pub fn queue_file(&mut self, handler: u8, data: &Completed, now: u64) -> Result<(), Error> {
        let index = handler
            .checked_sub(4)
            .filter(|i| *i < 2)
            .ok_or(Error::Bound)? as usize;
        if !matches!(&self.stage, Stage::Running { application, .. }
            if application.phase_name() == application::PhaseName::Sequenced)
            || self.files[index].is_some()
        {
            return Err(Error::State);
        }
        if data.name.len() > crate::files::MAX_NAME || data.bytes.len() > crate::files::MAX_BYTES {
            return Err(Error::Bound);
        }
        self.files[index] = Some(Outgoing {
            sender: Sender::new(data.clone(), Policy::default(), now)?,
            receipt: None,
        });
        Ok(())
    }

    pub fn cancel_file(&mut self, handler: u8) -> bool {
        let Some(index) = handler.checked_sub(4).filter(|i| *i < 2) else {
            return false;
        };
        let Some(file) = self.files[index as usize].take() else {
            return false;
        };
        if let Stage::Running { application, .. } = &mut self.stage
            && let Some(ticket) = file.receipt
        {
            application.release_delivery(ticket);
        }
        true
    }

    pub(super) fn poll_files(
        files: &mut [Option<Outgoing>; 2],
        application: &mut application::Application,
        link: &mut crate::link::Link,
        now: u64,
        out: &mut Output,
    ) {
        for (index, slot) in files.iter_mut().enumerate() {
            let Some(file) = slot else { continue };
            let handler = index as u8 + 4;
            let result = (|| -> Result<(), Error> {
                if let Some(ticket) = file.receipt {
                    match application.delivery_status(ticket) {
                        Some(Status::Pending) => {}
                        Some(status) => {
                            application.release_delivery(ticket);
                            file.receipt = None;
                            file.sender
                                .acknowledge(ticket, status == Status::Accepted, now)?;
                        }
                        None => return Err(Error::State),
                    }
                }
                let Some(record) = file.sender.offer(now)? else {
                    return Ok(());
                };
                let body = record
                    .encode(file.sender.size())
                    .map_err(|_| Error::Bound)?;
                let mut bits = BitWriter::new();
                bits.put(1 << handler, 6).put_span(body.span()).align();
                if bits.len() > application::OUTBOUND_FRAME_BITS {
                    return Err(Error::Bound);
                }
                let (ticket, bodies) = match application
                    .send_tracked_frame(bits.span(), super::DEFAULT_FRAGMENT_BITS)
                {
                    Ok(sent) => sent,
                    Err(application::Error::Window) => return Ok(()),
                    Err(error) => {
                        out.events.push(Event::ApplicationError(error));
                        return Err(Error::State);
                    }
                };
                if let Err(error) = file.sender.sent(&record, ticket, now) {
                    application.release_delivery(ticket);
                    return Err(error);
                }
                if let Some(old) = file.receipt.replace(ticket) {
                    application.release_delivery(old);
                }
                for body in bodies {
                    match link.send_application(&body, now) {
                        Ok(wire) => out.send.push(wire),
                        Err(error) => out.events.push(Event::Link(error)),
                    }
                }
                let kind = match record {
                    Record::Start { .. } => 1,
                    Record::Blocks { .. } => 2,
                    Record::Finish => 3,
                };
                out.events.push(Event::FileRecord { handler, kind });
                Ok(())
            })();
            if let Err(error) = result {
                if let Some(ticket) = file.receipt.take() {
                    application.release_delivery(ticket);
                }
                *slot = None;
                out.events.push(Event::FileError { handler, error });
            } else if file.sender.done() {
                *slot = None;
                out.events.push(Event::FileComplete { handler });
            }
        }
    }
}
