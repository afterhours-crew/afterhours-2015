// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Client-authoritative chassis reports. Their frame header supplies the same
//! clock and revision used by host chassis updates; the body omits those words.
use super::{BitSpan, BitWriter, Error, Physics, Reader, Writer, codec};

pub const MAX_SAMPLE_BITS: usize = 1024;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sample {
    pub physics: Physics,
    pub flag: bool,
}
impl Sample {
    pub fn decode(header: BitSpan<'_>, body: BitSpan<'_>) -> Result<Self, Error> {
        if header.len() != 96 || body.len() > MAX_SAMPLE_BITS {
            return Err(Error::Bound);
        }
        let words = [
            header.read_u32(0, 32).map_err(|_| Error::Truncated)?,
            header.read_u32(32, 32).map_err(|_| Error::Truncated)?,
            header.read_u32(64, 32).map_err(|_| Error::Truncated)?,
        ];
        let mut r = Reader { span: body, pos: 0 };
        let sample = Self {
            physics: codec::movement_read(&mut r, words)?,
            flag: r.bit()?,
        };
        if r.pos != body.len() {
            return Err(Error::Shape);
        }
        sample.validate()?;
        Ok(sample)
    }
    pub fn encode(&self) -> Result<(BitWriter, BitWriter), Error> {
        self.validate()?;
        let mut header = BitWriter::new();
        for word in self.physics.identity_words {
            header.put(word.into(), 32);
        }
        let mut body = Writer(BitWriter::new());
        codec::movement_write(&mut body, &self.physics)?;
        body.bit(self.flag)?;
        Ok((header, body.0))
    }
    fn validate(&self) -> Result<(), Error> {
        let [high, low, _] = self.physics.identity_words;
        let seconds = f64::from_bits((u64::from(high) << 32) | u64::from(low));
        if !seconds.is_finite() || seconds < 0. {
            return Err(Error::Shape);
        }
        // Float escape representations still need the same finite-value guards
        // as locally constructed physics.
        if matches!(&self.physics.position, super::Vector::FloatBits(v) if v.iter().any(|x|!f32::from_bits(*x).is_finite()))
            || [&self.physics.velocity, &self.physics.angular_velocity].iter().any(|v|matches!(v, super::SparseVector::FloatBits(a) if a.iter().any(|x|!f32::from_bits(*x).is_finite()))) {
            return Err(Error::Shape);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replication::vehicle::{
        Vector,
        chassis::{Controls, Motion},
    };
    fn sample() -> Sample {
        let motion = Motion {
            identity: 1.25f64.to_bits(),
            revision: 38,
            position: [-30., 20., 400.],
            quaternion: [0., 0., 0., 1.],
            velocity: [4., 0., -2.],
            angular_velocity: [0., 0.5, 0.],
        };
        Sample {
            physics: motion.encode([0.; 3], &Controls::new(3).unwrap()).unwrap(),
            flag: true,
        }
    }
    #[test]
    fn movement_fields_share_the_physics_codec_and_require_exact_boundaries() {
        let sample = sample();
        let (h, b) = sample.encode().unwrap();
        assert_eq!(h.len(), 96);
        assert_eq!(Sample::decode(h.span(), b.span()), Ok(sample.clone()));
        for end in 0..b.len() {
            assert!(Sample::decode(h.span(), b.span().slice(0, end).unwrap()).is_err());
        }
        assert!(Sample::decode(h.span().slice(0, 95).unwrap(), b.span()).is_err());
        let mut joined = b.clone();
        joined.put_span(b.span());
        assert!(Sample::decode(h.span(), joined.span()).is_err());
        let mut invalid = sample;
        invalid.physics.identity_words[0] = 0x7ff80000;
        assert!(invalid.encode().is_err());
        invalid.physics.identity_words[0] = 0;
        invalid.physics.position = Vector::FloatBits([f32::INFINITY.to_bits(), 0, 0]);
        assert!(invalid.encode().is_err());
    }
}
