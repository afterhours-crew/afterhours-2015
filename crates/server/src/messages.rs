// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Phone messages on the gameplay scene's two message managers. At session
//! start each manager restores its stored messages (method 0, in message
//! order, flag false) and then reports itself initiated (method 1; official
//! E1026 36.220 s, a fresh account: s106 m1, s107 m1; E912: the restores,
//! then m1). A message the account gains is committed first and then
//! announced (method 2, toast and inbox; E1026 906.624 s). The client's
//! messageRead (method 3) marks the stored row read and gets no answer.
use crate::event_logic;
use nfs_content::messages::{Catalog, Managers};
use nfs_world::{
    game_mode::Arguments,
    participants::HostRpc,
    replication::{Error, state::World},
};

const RESTORED: u32 = 0;
const INITIATED: u32 = 1;
const ADDED: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub catalog: Catalog,
    pub managers: Managers,
}

impl Content {
    pub fn prepare(
        index: &mut nfs_content::assets::AssetIndex,
    ) -> Result<Self, nfs_content::Error> {
        Ok(Self {
            catalog: index.message_catalog()?,
            managers: index.message_managers()?,
        })
    }
    fn endpoint(&self, world: &World, serializer: usize) -> Result<(u16, u16), Error> {
        world
            .scene_endpoint(self.managers.scene_key, serializer)
            .ok_or(Error::UnknownObject)
    }
    fn message(&self, world: &World, method: u32, hash: u32) -> Result<HostRpc, Error> {
        let message = self.catalog.by_hash(hash).ok_or(Error::Unsupported)?;
        let (scene, selector) = self.endpoint(world, self.managers.normal)?;
        Ok(HostRpc::GameMode(event_logic::call(
            world,
            scene,
            selector,
            method,
            None,
            Arguments::Message {
                bundle: self.catalog.bundle,
                flat: message.flat,
                flag: false,
            },
        )?))
    }
    /// The normal manager's restores for its stored rows (rows whose message
    /// is not installed are skipped), then both managers' initiation.
    pub fn session_start(
        &self,
        world: &World,
        rows: &[nfs_services::messages::Row],
    ) -> Result<Vec<HostRpc>, Error> {
        let mut out = vec![];
        for row in rows {
            if self.catalog.by_hash(row.hash).is_none() {
                tracing::warn!(
                    hash = row.hash,
                    "stored message is not installed; not restored"
                );
                continue;
            }
            out.push(self.message(world, RESTORED, row.hash)?);
        }
        for serializer in [self.managers.normal, self.managers.underground] {
            let (scene, selector) = self.endpoint(world, serializer)?;
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
    /// A committed message's announcement on the normal manager.
    pub fn added(&self, world: &World, hash: u32) -> Result<HostRpc, Error> {
        self.message(world, ADDED, hash)
    }
    /// The read message's hash and the reading participant: `Ok(None)` when
    /// the RPC is not a messageRead.
    pub fn read(
        &self,
        world: &World,
        body: nfs_protocol::world::BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<Option<(u32, u16)>, Error> {
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
        Ok(Some((message.hash, read.participant)))
    }
}
