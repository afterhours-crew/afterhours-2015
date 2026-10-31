// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Map seen-ID requests. The caller supplies the current published endpoint and
//! verifies that the second reference is its owned Participant, not a Player.
use crate::{
    game_mode::{Arguments, Call},
    participants::Endpoint,
    replication::Error,
};
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Seen {
    pub participant: u16,
    pub id: u32,
}

pub fn receive(
    endpoint: Endpoint,
    body: BitSpan<'_>,
    owns: impl FnOnce(u16) -> bool,
) -> Result<Option<Seen>, Error> {
    let envelope = Envelope::decode(
        body,
        Limits {
            max_input_bits: 8192,
            max_references: 32,
            max_payload_bytes: 256,
        },
    )
    .map_err(|_| Error::Shape)?;
    let route = envelope
        .route(RouteProfile::ClientSend)
        .map_err(|_| Error::Shape)?;
    if envelope.references().first() != Some(&endpoint.scene)
        || route.selector() != endpoint.selector
        || route.method_index() != 2
    {
        return Ok(None);
    }
    if envelope.references().len() != 2
        || envelope.words() != [0, 0]
        || !envelope.remaining().is_empty()
        || route.arguments().len() != 39
    {
        return Err(Error::Shape);
    }
    let participant = envelope.references()[1];
    if participant == 0 || !owns(participant) {
        return Err(Error::UnknownObject);
    }
    let id = route
        .arguments()
        .read_u32(0, 32)
        .map_err(|_| Error::Shape)?;
    Ok(Some(Seen { participant, id }))
}

/// Restore starts with a clear, then one add per committed ID. Add also
/// acknowledges a committed request. The host methods carry no owner reference.
pub fn publication(endpoint: Endpoint, id: Option<u32>) -> Call {
    Call {
        target: endpoint.scene,
        selector: endpoint.selector,
        serial: endpoint.serial,
        method: u32::from(id.is_some()),
        reference: None,
        arguments: id.map_or(Arguments::None, Arguments::Word),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::BitWriter;
    use nfs_protocol::world::rpc::Serial;

    const LIMITS: Limits = Limits {
        max_input_bits: 8192,
        max_references: 32,
        max_payload_bytes: 256,
    };

    fn endpoint() -> Endpoint {
        Endpoint {
            scene: 17,
            selector: 3,
            serial: Serial::new(4).unwrap(),
        }
    }
    fn request(
        scene: u16,
        selector: u16,
        method: u32,
        refs: &[u16],
        id: u32,
        padding: u8,
        extra: bool,
    ) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(method.into(), 32)
            .put(id.into(), 32)
            .put(padding.into(), 7);
        if extra {
            payload.put(0, 8);
        }
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put((refs.len() + 1) as u64, 8)
            .put(scene.into(), 13);
        for id in refs {
            body.put((*id).into(), 13);
        }
        body.put(payload.bytes().len() as u64, 9)
            .put_span(payload.span());
        body
    }

    #[test]
    fn owned_ids_accept_alignment_variation_and_host_encoding_uses_no_owner_reference() {
        for id in [0, 17, u32::MAX] {
            for padding in [0, 3, 127] {
                let input = request(17, 3, 2, &[29], id, padding, false);
                assert_eq!(
                    receive(endpoint(), input.span(), |p| p == 29).unwrap(),
                    Some(Seen {
                        participant: 29,
                        id
                    })
                );
            }
            let call = publication(endpoint(), Some(id)).encode().unwrap();
            let envelope = Envelope::decode(call.span(), LIMITS).unwrap();
            let route = envelope.route(RouteProfile::ClientReceive).unwrap();
            assert_eq!(envelope.references(), [17]);
            assert_eq!(route.method_index(), 1);
            assert_eq!(route.arguments().len(), 37);
            assert_eq!(route.arguments().read_u32(0, 32).unwrap(), id);
        }
        let clear = publication(endpoint(), None).encode().unwrap();
        let envelope = Envelope::decode(clear.span(), LIMITS).unwrap();
        let route = envelope.route(RouteProfile::ClientReceive).unwrap();
        assert_eq!(route.method_index(), 0);
        assert_eq!(route.arguments().len(), 5);
    }

    #[test]
    fn foreign_or_player_references_wrong_shapes_and_extra_data_cannot_be_accepted() {
        let valid = request(17, 3, 2, &[29], 91, 0, false);
        for end in 0..valid.span().len() {
            assert!(receive(endpoint(), valid.span().slice(0, end).unwrap(), |_| true).is_err());
        }
        for refs in [&[0][..], &[30][..], &[31][..]] {
            let input = request(17, 3, 2, refs, 91, 0, false);
            assert_eq!(
                receive(endpoint(), input.span(), |p| p == 29),
                Err(Error::UnknownObject)
            );
        }
        for refs in [&[][..], &[29, 30][..]] {
            let input = request(17, 3, 2, refs, 91, 0, false);
            assert_eq!(
                receive(endpoint(), input.span(), |_| true),
                Err(Error::Shape)
            );
        }
        let input = request(17, 3, 2, &[29], 91, 0, true);
        assert_eq!(
            receive(endpoint(), input.span(), |_| true),
            Err(Error::Shape)
        );
        for (scene, selector, method) in [(18, 3, 2), (17, 4, 2), (17, 3, 1)] {
            let input = request(scene, selector, method, &[29], 91, 0, false);
            assert_eq!(receive(endpoint(), input.span(), |_| true), Ok(None));
        }
    }
}
