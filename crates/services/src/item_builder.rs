// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The item builder's begin-update call has no server-side effects or reply.
//! Its current scene binding and player ownership still require validation.
//! Commit and customization mutation methods are deliberately not accepted here.
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Binding,
    Shape,
    Ownership,
}

/// Resolve this binding from the caller's current scene, never from the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binding {
    scene: u16,
    selector: u16,
}

impl Binding {
    pub fn new(scene: u16, selector: u16) -> Result<Self, Error> {
        if scene == 0 || scene > 8191 || selector > 511 {
            return Err(Error::Binding);
        }
        Ok(Self { scene, selector })
    }

    /// Returns false for other routes. A handled call produces no output, keeps
    /// no retained state, and cannot modify inventory. Exact retries are silent.
    /// `owns` must validate a live player belonging to this connection.
    pub fn begin_update(
        self,
        body: BitSpan<'_>,
        owns: impl Fn(u16) -> bool,
    ) -> Result<bool, Error> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 4096,
                max_references: 32,
                max_payload_bytes: 256,
            },
        )
        .map_err(|_| Error::Shape)?;
        let route = envelope
            .route(RouteProfile::ClientSend)
            .map_err(|_| Error::Shape)?;
        if envelope.references().first() != Some(&self.scene)
            || route.selector() != self.selector
            || route.method_index() != 4
        {
            return Ok(false);
        }
        if envelope.words() != [0, 0]
            || !envelope.remaining().is_empty()
            || envelope.references().len() != 2
            || route.arguments().len() != 7
        {
            return Err(Error::Shape);
        }
        // The trailing seven bits align the native payload; their value is not
        // an argument. Do not confuse a nonzero padding bit with a mutation.
        let player = envelope.references()[1];
        if player == 0 || !owns(player) {
            return Err(Error::Ownership);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_world_core::bits::BitWriter;

    fn wire(scene: u16, selector: u16, method: u32, references: &[u16], pad: u8) -> BitWriter {
        let mut payload = BitWriter::new();
        payload
            .put(selector.into(), 9)
            .put(method.into(), 32)
            .put(pad.into(), 7);
        let mut body = BitWriter::new();
        body.put(0, 32)
            .put(0, 32)
            .put((1 + references.len()) as u64, 8)
            .put(scene.into(), 13);
        for &id in references {
            body.put(id.into(), 13);
        }
        body.put(6, 9).put_span(payload.span());
        body
    }

    #[test]
    fn current_binding_and_owner_are_required_and_retries_have_no_effects() {
        let binding = Binding::new(19, 5).unwrap();
        for pad in [0, 127] {
            let body = wire(19, 5, 4, &[23], pad);
            for _ in 0..3 {
                assert_eq!(binding.begin_update(body.span(), |p| p == 23), Ok(true));
            }
            assert_eq!(
                binding.begin_update(body.span(), |p| p == 24),
                Err(Error::Ownership)
            );
            assert_eq!(
                Binding::new(20, 5)
                    .unwrap()
                    .begin_update(body.span(), |_| true),
                Ok(false)
            );
        }
        assert_eq!(
            binding.begin_update(wire(19, 5, 4, &[0], 0).span(), |_| true),
            Err(Error::Ownership)
        );
        assert_eq!(
            binding.begin_update(wire(19, 6, 4, &[23], 0).span(), |_| true),
            Ok(false)
        );
        for method in [0, 3, 5, 14, 32, u32::MAX] {
            assert_eq!(
                binding.begin_update(wire(19, 5, method, &[23], 0).span(), |_| true),
                Ok(false)
            );
        }
        assert_eq!(Binding::new(0, 0), Err(Error::Binding));
        assert_eq!(Binding::new(8192, 0), Err(Error::Binding));
        assert_eq!(Binding::new(1, 512), Err(Error::Binding));
    }

    #[test]
    fn malformed_extents_references_and_concatenated_calls_are_rejected() {
        let binding = Binding::new(19, 5).unwrap();
        let body = wire(19, 5, 4, &[23], 0);
        for len in 0..body.span().len() {
            assert_eq!(
                binding.begin_update(body.span().slice(0, len).unwrap(), |_| true),
                Err(Error::Shape)
            );
        }
        for refs in [vec![], vec![23, 24]] {
            assert_eq!(
                binding.begin_update(wire(19, 5, 4, &refs, 0).span(), |_| true),
                Err(Error::Shape)
            );
        }
        let mut concat = body.clone();
        concat.put_span(body.span());
        assert_eq!(
            binding.begin_update(concat.span(), |_| true),
            Err(Error::Shape)
        );
        let mut header = body.clone().into_bytes();
        header[0] = 1;
        assert_eq!(
            binding.begin_update(BitSpan::new(&header, 0, body.span().len()).unwrap(), |_| {
                true
            }),
            Err(Error::Shape)
        );
        let oversized = vec![0; 513];
        assert_eq!(
            binding.begin_update(
                BitSpan::new(&oversized, 0, oversized.len() * 8).unwrap(),
                |_| true
            ),
            Err(Error::Shape)
        );
    }
}
