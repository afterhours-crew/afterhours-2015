// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local static-cache format, not a game protocol. Preserve every float bit and
//! authored vector order. Counts and bytes are bounded before allocation; callers
//! must run the ordinary graph/network constructors before accepting the input.
use nfs_content::{Error, prepared_cache::MAX_PAYLOAD, road_network::GraphInput};
use nfs_world::{
    road_graph::{LaneInput, MAX_LANE_POINTS, MAX_LANES, MAX_NODES},
    scoring::road_network::{Direction, MAX_POINTS, MAX_SEGMENTS, Segment},
};

fn bad() -> Error {
    Error::Cache("prepared road bytes or bound")
}

struct Writer(Vec<u8>);
impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > MAX_PAYLOAD)
        {
            return Err(bad());
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn count(&mut self, value: usize, limit: usize) -> Result<(), Error> {
        if value > limit {
            return Err(bad());
        }
        self.bytes(&u32::try_from(value).map_err(|_| bad())?.to_le_bytes())
    }
    fn float(&mut self, value: f32) -> Result<(), Error> {
        self.bytes(&value.to_bits().to_le_bytes())
    }
    fn direction(&mut self, value: Direction) -> Result<(), Error> {
        self.bytes(&[match value {
            Direction::Any => 0,
            Direction::Forward => 1,
            Direction::Backward => 2,
        }])
    }
    fn points(&mut self, values: &[[f32; 3]], limit: usize) -> Result<(), Error> {
        self.count(values.len(), limit)?;
        for point in values {
            for &c in point {
                self.float(c)?;
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], magic: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_PAYLOAD || !bytes.starts_with(magic) {
            return Err(bad());
        }
        Ok(Self {
            bytes,
            at: magic.len(),
        })
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let end = self.at.checked_add(N).ok_or_else(bad)?;
        let value = self
            .bytes
            .get(self.at..end)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?;
        self.at = end;
        Ok(value)
    }
    fn count(&mut self, limit: usize) -> Result<usize, Error> {
        let count = u32::from_le_bytes(self.take()?) as usize;
        if count > limit {
            return Err(bad());
        }
        Ok(count)
    }
    fn float(&mut self) -> Result<f32, Error> {
        Ok(f32::from_bits(u32::from_le_bytes(self.take()?)))
    }
    fn direction(&mut self) -> Result<Direction, Error> {
        match self.take::<1>()?[0] {
            0 => Ok(Direction::Any),
            1 => Ok(Direction::Forward),
            2 => Ok(Direction::Backward),
            _ => Err(bad()),
        }
    }
    fn points(&mut self, limit: usize) -> Result<Vec<[f32; 3]>, Error> {
        let count = self.count(limit)?;
        let bytes = count.checked_mul(12).ok_or_else(bad)?;
        if bytes > self.bytes.len().saturating_sub(self.at) {
            return Err(bad());
        }
        (0..count)
            .map(|_| Ok([self.float()?, self.float()?, self.float()?]))
            .collect()
    }
    fn finish(self) -> Result<(), Error> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(bad())
        }
    }
}

pub(super) fn encode_graph(input: &GraphInput) -> Result<Vec<u8>, Error> {
    let mut out = Writer(b"RGR1".to_vec());
    out.points(&input.nodes, MAX_NODES)?;
    out.count(input.lanes.len(), MAX_LANES)?;
    for lane in &input.lanes {
        out.count(lane.start, MAX_NODES - 1)?;
        out.count(lane.end, MAX_NODES - 1)?;
        out.direction(lane.direction)?;
        out.points(&lane.points, MAX_LANE_POINTS)?;
        out.count(lane.widths.len(), MAX_LANE_POINTS - 1)?;
        for &width in &lane.widths {
            out.float(width)?;
        }
    }
    Ok(out.0)
}

pub(super) fn decode_graph(bytes: &[u8]) -> Result<GraphInput, Error> {
    let mut input = Reader::new(bytes, b"RGR1")?;
    let nodes = input.points(MAX_NODES)?;
    let count = input.count(MAX_LANES)?;
    let mut lanes = Vec::with_capacity(count);
    for _ in 0..count {
        let start = input.count(MAX_NODES - 1)?;
        let end = input.count(MAX_NODES - 1)?;
        let direction = input.direction()?;
        let points = input.points(MAX_LANE_POINTS)?;
        let count = input.count(MAX_LANE_POINTS - 1)?;
        let widths = (0..count)
            .map(|_| input.float())
            .collect::<Result<_, _>>()?;
        lanes.push(LaneInput {
            start,
            end,
            direction,
            points,
            widths,
        });
    }
    input.finish()?;
    Ok(GraphInput { nodes, lanes })
}

pub(super) fn encode_network(segments: &[Segment]) -> Result<Vec<u8>, Error> {
    let mut out = Writer(b"RSN1".to_vec());
    out.count(segments.len(), MAX_SEGMENTS)?;
    for segment in segments {
        out.bytes(&segment.speed_limit_mph.to_le_bytes())?;
        out.float(segment.width)?;
        out.direction(segment.direction)?;
        out.points(&segment.points, MAX_POINTS)?;
    }
    Ok(out.0)
}

pub(super) fn decode_network(bytes: &[u8]) -> Result<Vec<Segment>, Error> {
    let mut input = Reader::new(bytes, b"RSN1")?;
    let count = input.count(MAX_SEGMENTS)?;
    let mut segments = Vec::with_capacity(count);
    for _ in 0..count {
        segments.push(Segment {
            speed_limit_mph: u16::from_le_bytes(input.take()?),
            width: input.float()?,
            direction: input.direction()?,
            points: input.points(MAX_POINTS)?,
        });
    }
    input.finish()?;
    Ok(segments)
}
