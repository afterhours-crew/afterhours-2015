// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Application, Section, application};
use application::delivery::{Status, Ticket};

#[derive(Clone, Debug, Default)]
pub(super) enum Entry {
    #[default]
    Waiting,
    Pending {
        player: u16,
        ticket: Ticket,
    },
    Ready {
        player: u16,
    },
    Entered,
    Rejected,
}

impl Entry {
    pub fn first_creation(&self, section: &Section) -> Option<u16> {
        if !matches!(self, Self::Waiting) {
            return None;
        }
        section.records.iter().find_map(|r| match &r.initial {
            Some(crate::replication::Initial::Player(p))
                if p.value8 == application::HOST_SELECTOR as u8 =>
            {
                Some(r.id)
            }
            _ => None,
        })
    }

    pub fn ready(&mut self, app: &mut Application, linked: bool) -> Option<u16> {
        if let Self::Pending { player, ticket } = *self {
            match app.delivery_status(ticket) {
                Some(Status::Pending) => return None,
                Some(Status::Accepted) => {
                    app.release_delivery(ticket);
                    *self = Self::Ready { player };
                }
                Some(Status::Rejected) | None => {
                    app.release_delivery(ticket);
                    *self = Self::Rejected;
                }
            }
        }
        match *self {
            Self::Ready { player } if linked => Some(player),
            _ => None,
        }
    }

    pub fn release(&self, app: &mut Application) {
        if let Self::Pending { ticket, .. } = self {
            app.release_delivery(*ticket);
        }
    }
}
