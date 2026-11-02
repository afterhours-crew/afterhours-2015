// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Phone messages on the gameplay scene's two message managers. At session
//! start each manager restores its stored messages (method 0, in message
//! order, with the saved read flag) and then reports itself initiated (method 1;
//! official
//! E1026 36.220 s, a fresh account: s106 m1, s107 m1; E912: the restores,
//! then m1). A message the account gains is committed first and then
//! announced (method 2, toast and inbox; E1026 906.624 s). The client's
//! messageRead (method 3) applies the reported read flag and gets no answer.
//! Each manager has its own table, and each restores before its initiation
//! (E912 32.336 s: s106 restores, s106 m1, s107 restore, s107 m1).
use crate::event_logic;
use nfs_content::messages::{Catalog, LicensedMessage, Managers};
use nfs_world::{
    game_mode::Arguments,
    participants::HostRpc,
    replication::{Error, state::World},
};

const RESTORED: u32 = 0;
const INITIATED: u32 = 1;
const ADDED: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd)]
pub enum Manager {
    Normal,
    Underground,
}
impl Manager {
    pub fn table(self) -> &'static str {
        match self {
            Self::Normal => nfs_services::messages::STATIC,
            Self::Underground => nfs_services::messages::UNDERGROUND,
        }
    }
}

/// A message the intro logic sends after the client's licence report and
/// an intro objective's completion (`nfs_content::messages::LicensedMessage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Licensed {
    pub accept: u32,
    pub objective: u64,
    pub hash: u32,
    pub manager: Manager,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub catalog: Catalog,
    pub managers: Managers,
    pub licensed: Vec<Licensed>,
    /// Strictly compiled direct Active-property invitations.
    pub active: Vec<nfs_content::messages::active::Invitation>,
    pub power: Vec<nfs_content::messages::power::Invitation>,
    pub result: Vec<nfs_content::messages::result::Invitation>,
    pub unlocks: Option<nfs_content::messages::unlocks::Delivery>,
}

impl Content {
    pub fn prepare(
        index: &mut nfs_content::assets::AssetIndex,
    ) -> Result<Self, nfs_content::Error> {
        let catalog = index.message_catalog()?;
        let managers = index.message_managers()?;
        let licensed = index
            .licensed_messages()?
            .into_iter()
            .map(|l| Self::licensed_from(&catalog, &managers, l))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            catalog,
            managers,
            licensed,
            active: vec![],
            power: vec![],
            result: vec![],
            unlocks: None,
        })
    }
    pub fn with_unlocks(
        mut self,
        delivery: nfs_content::messages::unlocks::Delivery,
    ) -> Result<Self, nfs_content::Error> {
        let mut hashes: std::collections::BTreeSet<_> = self
            .active
            .iter()
            .map(|m| m.hash)
            .chain(self.power.iter().flat_map(|m| m.hashes()))
            .chain(self.result.iter().flat_map(|m| m.hashes()))
            .collect();
        if self.manager(delivery.dynamic_manager).is_none()
            || !(2..=512).contains(&delivery.dynamic_capacity)
            || delivery.notifications.is_empty()
            || delivery.notifications.len() > 64
        {
            return Err(nfs_content::Error::Content("unlock delivery bounds".into()));
        }
        for n in &delivery.notifications {
            if self.manager(n.manager).is_none()
                || !hashes.insert(n.hash)
                || !self.catalog.by_hash(n.hash).is_some_and(|m| m.persisted)
                || n.delay.is_zero()
                || n.delay > std::time::Duration::from_secs(60)
                || matches!(
                    n.trigger,
                    nfs_content::messages::unlocks::Trigger::Part { .. }
                ) && n.manager != delivery.dynamic_manager
            {
                return Err(nfs_content::Error::Content(
                    "unlock delivery identity".into(),
                ));
            }
        }
        self.unlocks = Some(delivery);
        self.unlock_policy()?;
        Ok(self)
    }

    pub fn unlock_policy(
        &self,
    ) -> Result<Option<nfs_services::messages::unlocks::Policy>, nfs_content::Error> {
        let Some(delivery) = &self.unlocks else {
            return Ok(None);
        };
        use nfs_content::messages::unlocks::Trigger as Input;
        use nfs_services::messages::unlocks::{Policy, Trigger};
        let parts: std::collections::BTreeMap<_, _> = delivery
            .parts
            .iter()
            .map(|p| (p.definition, p.id))
            .collect();
        if parts.len() != delivery.parts.len() {
            return Err(nfs_content::Error::Content(
                "duplicate unlock definition".into(),
            ));
        }
        Policy::new(
            Some(delivery.controller),
            parts,
            delivery
                .notifications
                .iter()
                .map(|n| {
                    (
                        n.hash,
                        match &n.trigger {
                            Input::Categories { mask } => Trigger::Categories { mask: *mask },
                            Input::Part { include, ids } => Trigger::Part {
                                include: *include,
                                ids: ids.clone(),
                            },
                        },
                    )
                })
                .collect(),
        )
        .map(Some)
        .map_err(|_| nfs_content::Error::Content("unlock notification policy".into()))
    }

    pub(crate) fn dynamic_rows(
        &self,
        loaded: &crate::persistent::Loaded,
    ) -> Result<Vec<nfs_services::messages::dynamic::Row>, Error> {
        let table = loaded.tables().get(&crate::persistent::key(
            nfs_services::messages::dynamic::TABLE,
        ));
        let Some(delivery) = &self.unlocks else {
            return if table.is_none_or(|t| t.rows.is_empty()) {
                Ok(vec![])
            } else {
                Err(Error::Unsupported)
            };
        };
        nfs_services::messages::dynamic::rows(table, delivery.dynamic_capacity)
            .map_err(|_| Error::Shape)
    }

    pub(crate) fn dynamic_identities(
        &self,
        loaded: &crate::persistent::Loaded,
    ) -> Result<Vec<nfs_services::messages::dynamic::Identity>, Error> {
        let Some(delivery) = &self.unlocks else {
            self.dynamic_rows(loaded)?;
            return Ok(vec![]);
        };
        nfs_services::messages::dynamic::identities(
            loaded.tables().get(&crate::persistent::key(
                nfs_services::messages::dynamic::TABLE,
            )),
            loaded.owned(nfs_services::messages::dynamic::RECEIPTS),
            delivery.dynamic_capacity,
        )
        .map_err(|_| Error::Shape)
    }

    pub(crate) fn dynamic_message(
        &self,
        world: &World,
        row: &nfs_services::messages::dynamic::Row,
        restored: bool,
    ) -> Result<HostRpc, Error> {
        let delivery = self.unlocks.as_ref().ok_or(Error::Unsupported)?;
        if !delivery.notifications.iter().any(|n| {
            n.hash == row.hash
                && matches!(
                    n.trigger,
                    nfs_content::messages::unlocks::Trigger::Part { .. }
                )
        }) {
            return Err(Error::Unsupported);
        }
        let part =
            nfs_world::messages::CarPartAttachment::decode(&row.data).map_err(|_| Error::Shape)?;
        let message = self.catalog.by_hash(row.hash).ok_or(Error::Unsupported)?;
        let (scene, selector) = self.endpoint(world, delivery.dynamic_manager)?;
        Ok(HostRpc::GameMode(event_logic::call(
            world,
            scene,
            selector,
            if restored { RESTORED } else { ADDED },
            None,
            Arguments::CarPartMessage {
                bundle: self.catalog.bundle,
                flat: message.flat,
                flag: row.read,
                part,
            },
        )?))
    }
    pub fn with_active(
        mut self,
        invitations: Vec<nfs_content::messages::active::Invitation>,
    ) -> Result<Self, nfs_content::Error> {
        let mut unique: std::collections::BTreeSet<_> = self
            .power
            .iter()
            .flat_map(|p| p.hashes().map(|h| (p.manager(), h)))
            .chain(
                self.result
                    .iter()
                    .flat_map(|p| p.hashes().map(|h| (p.manager(), h))),
            )
            .collect();
        if invitations.len() > 64 {
            return Err(nfs_content::Error::Content(
                "active invitation bound".into(),
            ));
        }
        for invitation in &invitations {
            if self.manager(invitation.manager).is_none()
                || !self
                    .catalog
                    .by_hash(invitation.hash)
                    .is_some_and(|m| m.persisted)
                || !unique.insert((invitation.manager, invitation.hash))
            {
                return Err(nfs_content::Error::Content(
                    "invalid active invitation".into(),
                ));
            }
        }
        self.active = invitations;
        Ok(self)
    }
    pub fn with_power(
        mut self,
        invitations: Vec<nfs_content::messages::power::Invitation>,
    ) -> Result<Self, nfs_content::Error> {
        let mut objectives: std::collections::BTreeSet<_> =
            self.result.iter().map(|p| p.objective()).collect();
        let mut hashes: std::collections::BTreeSet<_> = self
            .active
            .iter()
            .map(|p| (p.manager, p.hash))
            .chain(
                self.result
                    .iter()
                    .flat_map(|p| p.hashes().map(|h| (p.manager(), h))),
            )
            .collect();
        if invitations.len() > 16 {
            return Err(nfs_content::Error::Content("power invitation bound".into()));
        }
        for p in &invitations {
            if self.manager(p.manager()).is_none()
                || !objectives.insert(p.objective())
                || p.hashes().iter().any(|&h| {
                    !self.catalog.by_hash(h).is_some_and(|m| m.persisted)
                        || !hashes.insert((p.manager(), h))
                })
            {
                return Err(nfs_content::Error::Content(
                    "invalid power invitation".into(),
                ));
            }
        }
        self.power = invitations;
        Ok(self)
    }
    pub fn with_result(
        mut self,
        invitations: Vec<nfs_content::messages::result::Invitation>,
    ) -> Result<Self, nfs_content::Error> {
        let mut objectives: std::collections::BTreeSet<_> =
            self.power.iter().map(|p| p.objective()).collect();
        let mut hashes: std::collections::BTreeSet<_> = self
            .active
            .iter()
            .map(|p| (p.manager, p.hash))
            .chain(
                self.power
                    .iter()
                    .flat_map(|p| p.hashes().map(|h| (p.manager(), h))),
            )
            .collect();
        if invitations.len() > 16 {
            return Err(nfs_content::Error::Content(
                "result invitation bound".into(),
            ));
        }
        for p in &invitations {
            if self.manager(p.manager()).is_none()
                || !objectives.insert(p.objective())
                || p.hashes().iter().any(|&h| {
                    !self.catalog.by_hash(h).is_some_and(|m| m.persisted)
                        || !hashes.insert((p.manager(), h))
                })
            {
                return Err(nfs_content::Error::Content(
                    "invalid result invitation".into(),
                ));
            }
        }
        self.result = invitations;
        Ok(self)
    }
    pub(crate) fn conditional_hashes(&self, objective: u64, manager: Manager) -> Option<[u32; 3]> {
        let mut groups = self
            .power
            .iter()
            .map(|p| (p.objective(), p.manager(), p.hashes()))
            .chain(
                self.result
                    .iter()
                    .map(|p| (p.objective(), p.manager(), p.hashes())),
            )
            .filter(|(o, m, _)| *o == objective && self.manager(*m) == Some(manager));
        let (_, _, hashes) = groups.next()?;
        groups.next().is_none().then_some(hashes)
    }
    pub(crate) fn manager(&self, serializer: usize) -> Option<Manager> {
        if serializer == self.managers.normal {
            Some(Manager::Normal)
        } else if serializer == self.managers.underground {
            Some(Manager::Underground)
        } else {
            None
        }
    }
    fn licensed_from(
        catalog: &Catalog,
        managers: &Managers,
        message: LicensedMessage,
    ) -> Result<Licensed, nfs_content::Error> {
        let manager = if message.manager == managers.normal {
            Manager::Normal
        } else if message.manager == managers.underground {
            Manager::Underground
        } else {
            return Err(nfs_content::Error::Content(
                "licensed message manager".into(),
            ));
        };
        if catalog.by_hash(message.hash).is_none() {
            return Err(nfs_content::Error::Content(
                "licensed message not installed".into(),
            ));
        }
        Ok(Licensed {
            accept: message.accept,
            objective: message.objective,
            hash: message.hash,
            manager,
        })
    }
    /// The licensed message a client EventSync id releases.
    pub fn licensed(&self, accept: u32) -> Option<Licensed> {
        self.licensed.iter().copied().find(|l| l.accept == accept)
    }
    fn serializer(&self, manager: Manager) -> usize {
        match manager {
            Manager::Normal => self.managers.normal,
            Manager::Underground => self.managers.underground,
        }
    }
    fn endpoint(&self, world: &World, serializer: usize) -> Result<(u16, u16), Error> {
        world
            .scene_endpoint(self.managers.scene_key, serializer)
            .ok_or(Error::UnknownObject)
    }
    fn message(
        &self,
        world: &World,
        manager: Manager,
        method: u32,
        hash: u32,
        read: bool,
    ) -> Result<HostRpc, Error> {
        let message = self.catalog.by_hash(hash).ok_or(Error::Unsupported)?;
        let (scene, selector) = self.endpoint(world, self.serializer(manager))?;
        Ok(HostRpc::GameMode(event_logic::call(
            world,
            scene,
            selector,
            method,
            None,
            Arguments::Message {
                bundle: self.catalog.bundle,
                flat: message.flat,
                flag: read,
            },
        )?))
    }
    /// Each manager's restores for its stored rows (rows whose message is
    /// not installed are skipped), then its initiation; normal first.
    pub fn session_start(
        &self,
        world: &World,
        normal: &[nfs_services::messages::Row],
        underground: &[nfs_services::messages::Row],
        dynamic: &[nfs_services::messages::dynamic::Row],
    ) -> Result<Vec<HostRpc>, Error> {
        let mut out = vec![];
        for (manager, rows) in [
            (Manager::Normal, normal),
            (Manager::Underground, underground),
        ] {
            for row in rows {
                if self.catalog.by_hash(row.hash).is_none() {
                    tracing::warn!(
                        hash = row.hash,
                        ?manager,
                        "stored message is not installed; not restored"
                    );
                    continue;
                }
                out.push(self.message(world, manager, RESTORED, row.hash, row.read)?);
            }
            if self
                .unlocks
                .as_ref()
                .is_some_and(|d| d.dynamic_manager == self.serializer(manager))
            {
                for row in dynamic {
                    out.push(self.dynamic_message(world, row, true)?);
                }
            }
            let (scene, selector) = self.endpoint(world, self.serializer(manager))?;
            out.push(HostRpc::GameMode(event_logic::call(
                world,
                scene,
                selector,
                INITIATED,
                None,
                Arguments::None,
            )?));
        }
        Ok(out)
    }
    /// A committed message's announcement on its manager.
    pub fn added(&self, world: &World, manager: Manager, hash: u32) -> Result<HostRpc, Error> {
        self.message(world, manager, ADDED, hash, false)
    }
    /// The message hash, participant, manager and reported read flag:
    /// `Ok(None)` when the RPC is not a messageRead.
    pub fn read(
        &self,
        world: &World,
        body: nfs_protocol::world::BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<(u32, u16, Manager, bool)>, Error> {
        let scene = |serializer| world.scene_endpoint(self.managers.scene_key, serializer);
        let (Some(normal), Some(underground)) = (
            scene(self.managers.normal),
            scene(self.managers.underground),
        ) else {
            return Ok(None);
        };
        let Some(read) =
            nfs_world::messages::Read::decode(body, normal.0, &[normal.1, underground.1])?
        else {
            return Ok(None);
        };
        if !owns(read.participant) {
            return Err(Error::UnknownObject);
        }
        let message = self
            .catalog
            .by_flat(read.flat)
            .filter(|_| read.bundle == self.catalog.bundle)
            .ok_or(Error::UnknownObject)?;
        let manager = if read.selector == underground.1 {
            Manager::Underground
        } else {
            Manager::Normal
        };
        Ok(Some((message.hash, read.participant, manager, read.flag)))
    }
}

#[cfg(test)]
mod tests;
