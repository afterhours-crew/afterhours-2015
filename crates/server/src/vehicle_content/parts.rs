// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{parse::*, *};

pub(super) fn components(v: &Value) -> Result<Vec<mesh::Component>, Failure> {
    array(v, MAX_MESHES)?
        .iter()
        .map(|c| {
            keys(c, &["index_bits", "extra", "variants"], &[])?;
            let kind = mesh_kind(c)?;
            let variants = array(&c["variants"], 256)?
                .iter()
                .map(|v| {
                    keys(v, &["guid", "bundle"], &[])?;
                    Ok(mesh::Variant {
                        guid: if v["guid"].is_null() {
                            None
                        } else {
                            Some(guid(&v["guid"])?)
                        },
                        bundle: if v["bundle"].is_null() {
                            None
                        } else {
                            Some(text(&v["bundle"])?.to_owned())
                        },
                    })
                })
                .collect::<Result<Vec<_>, Failure>>()?;
            if !(1..=16).contains(&kind.index_bits)
                || variants.len() > ((1usize << kind.index_bits) - 1)
            {
                return Err(Failure::ProfileConfig);
            }
            Ok(mesh::Component { kind, variants })
        })
        .collect()
}
