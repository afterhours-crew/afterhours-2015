// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Client-selected scene location. The argument identifies a published RPC
//! selector in the referenced scene, not its serializer ordinal.
use crate::replication::Error;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    pub participant: u16,
    pub scene: u16,
    pub selector: u16,
}
impl Selection {
    pub fn decode(body: BitSpan<'_>, manager: (u16, u16)) -> Result<Option<Self>, Error> {
        let e = Envelope::decode(
            body,
            Limits {
                max_input_bits: 65_536,
                max_references: 32,
                max_payload_bytes: 8192,
            },
        )
        .map_err(|_| Error::Shape)?;
        let r = e
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if e.references().first() != Some(&manager.0) || r.selector() != manager.1 {
            return Ok(None);
        }
        if e.words() != [0, 0]
            || !e.remaining().is_empty()
            || e.references().len() != 3
            || e.references().contains(&0)
            || r.method_index() != 0
            || r.arguments().len() != 15
            || r.arguments().read_u32(9, 6) != Ok(0)
        {
            return Err(Error::Shape);
        }
        Ok(Some(Self {
            participant: e.references()[1],
            scene: e.references()[2],
            selector: r.arguments().read_u32(0, 9).map_err(|_| Error::Shape)? as u16,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    fn input(method: u32, refs: &[u16], selector: u16, pad: u8) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(3, 9)
            .put(method.into(), 32)
            .put(selector.into(), 9)
            .put(pad.into(), 6);
        let mut body = BitWriter::new();
        body.put(0, 32).put(0, 32).put(refs.len() as u64, 8);
        for &id in refs {
            body.put(id.into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }
    #[test]
    fn selects_received_scene_selector_and_rejects_shape_mutations() {
        let valid = input(0, &[10, 20, 30], 137, 0);
        assert_eq!(
            Selection::decode(valid.span(), (10, 3)).unwrap(),
            Some(Selection {
                participant: 20,
                scene: 30,
                selector: 137
            })
        );
        assert_eq!(Selection::decode(valid.span(), (10, 4)).unwrap(), None);
        for invalid in [
            input(1, &[10, 20, 30], 137, 0),
            input(0, &[10, 20], 137, 0),
            input(0, &[10, 0, 30], 137, 0),
            input(0, &[10, 20, 30], 137, 1),
        ] {
            assert!(Selection::decode(invalid.span(), (10, 3)).is_err());
        }
        let short = BitSpan::new(valid.bytes(), 0, valid.len() - 1).unwrap();
        assert!(Selection::decode(short, (10, 3)).is_err());
    }
}
