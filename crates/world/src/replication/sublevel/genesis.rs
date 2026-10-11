// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Content, Creation, Error, Rpc, root};
use crate::content::bindings::Bindings;

pub fn hierarchy(
    rpc: Rpc,
    bindings: &Bindings,
    content: &Content,
    traffic_keys: &[u32; root::REFERENCES],
) -> Result<Vec<Creation>, Error> {
    root::validate_traffic_keys(traffic_keys)?;
    if content.profile(1)? != root::content()?.profile(1)? {
        return Err(Error::TypeMismatch);
    }
    let mut links = [None; root::REFERENCES];
    for (slot, key) in traffic_keys.iter().copied().enumerate() {
        links[slot] = Some(
            bindings
                .root_child(key)
                .map_err(|_| Error::UnknownObject)?
                .id,
        );
    }
    let serial = rpc.serial;
    let mut result = vec![root::creation(rpc, links)?];
    for level in bindings.hierarchy() {
        let values = super::ordinary::unpopulated(content.profile(level.content_key)?)?;
        result.push(super::ordinary::creation(
            level.id,
            level.content_key,
            None,
            serial,
            &values,
            content,
        )?);
    }
    Ok(result)
}
