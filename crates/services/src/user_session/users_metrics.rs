// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error as Failure, body_limits, frame_limits, users_followup};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::users::{
    LatencyList, NetworkAddress, UpdateNetworkInfoRequest, UserSessionExtendedDataUpdate,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Network,
    Hardware,
    Metrics,
    ObserveOnly,
}

pub struct Followup {
    initial: Option<users_followup::Followup>,
    current: Vec<u8>,
    aliases: [Vec<u8>; 4],
    state: State,
    completed: bool,
    previous: Option<(Vec<u8>, Vec<u8>)>,
}

fn frame(wire: &[u8]) -> Result<Frame<'_>, Failure> {
    let decoded = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Failure::IneligibleRequest)?
        .ok_or(Failure::IneligibleRequest)?;
    if decoded.consumed != wire.len() || !decoded.frame.metadata.is_empty() {
        return Err(Failure::IneligibleRequest);
    }
    Ok(decoded.frame)
}

fn encode(fields: Fields, body: &[u8]) -> Result<Vec<u8>, Failure> {
    nfs_fire2::encode(
        Frame {
            fields,
            metadata: &[],
            body,
        },
        frame_limits(),
    )
    .map_err(|_| Failure::Reply)
}

impl Followup {
    pub fn new(initial_user_added_frame: &[u8]) -> Result<Self, Failure> {
        Ok(Self {
            initial: Some(users_followup::Followup::new(initial_user_added_frame)?),
            current: Vec::new(),
            aliases: std::array::from_fn(|_| Vec::new()),
            state: State::Network,
            completed: false,
            previous: None,
        })
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn complete(&self) -> bool {
        self.completed
    }

    pub fn response(&mut self, request: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        if let Some((previous, acknowledgement)) = &self.previous
            && previous == request
        {
            return Ok(Some(vec![acknowledgement.clone()]));
        }
        let result = self.next(request);
        if let Ok(Some(output)) = &result {
            self.previous = Some((request.to_vec(), output[0].clone()));
        } else {
            self.stop();
            self.previous = None;
        }
        result
    }

    fn stop(&mut self) {
        self.state = State::ObserveOnly;
        self.initial = None;
        self.current = Vec::new();
        self.aliases = std::array::from_fn(|_| Vec::new());
    }

    fn next(&mut self, request: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        match self.state {
            State::Network | State::Hardware => {
                let initial = self.initial.as_mut().ok_or(Failure::Reply)?;
                let Some(output) = initial.response(request)? else {
                    return Ok(None);
                };
                if self.state == State::Network {
                    let f = frame(request)?;
                    let message = UpdateNetworkInfoRequest::decode(f.body, body_limits())
                        .map_err(|_| Failure::Reply)?;
                    let aliases = message
                        .network_info
                        .and_then(|i| i.ping_site_latency_by_alias)
                        .ok_or(Failure::Reply)?;
                    if aliases.0.len() != 4 || output.len() != 2 {
                        return Err(Failure::Reply);
                    }
                    self.aliases = std::array::from_fn(|i| aliases.0[i].0.to_vec());
                    self.current = frame(&output[1])?.body.to_vec();
                    self.state = State::Hardware;
                } else {
                    if !initial.complete() || output.len() != 1 {
                        return Err(Failure::Reply);
                    }
                    self.initial = None;
                    self.state = State::Metrics;
                }
                Ok(Some(output))
            }
            State::Metrics => self.metrics(request),
            State::ObserveOnly => Ok(None),
        }
    }

    fn metrics(&mut self, request: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        let f = frame(request)?;
        if f.fields.routing_a != 30722
            || f.fields.routing_b != 20
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
        {
            return Ok(None);
        }
        let message = UpdateNetworkInfoRequest::decode(f.body, body_limits())
            .map_err(|_| Failure::IneligibleRequest)?;
        if message.opts != Some(4)
            || message.unknown_field_count() != 0
            || message
                .encode(body_limits())
                .map_err(|_| Failure::IneligibleRequest)?
                != f.body
        {
            return Err(Failure::IneligibleRequest);
        }
        let info = message.network_info.ok_or(Failure::IneligibleRequest)?;
        let qos = info.qos_data.ok_or(Failure::IneligibleRequest)?;
        if qos.bandwidth_error_code.is_none()
            || qos.downstream_bits_per_second.is_none()
            || qos.nat_error_code.is_none()
            || qos.nat_type.is_none()
            || qos.upstream_bits_per_second.is_none()
        {
            return Err(Failure::IneligibleRequest);
        }
        let latency = info
            .ping_site_latency_by_alias
            .ok_or(Failure::IneligibleRequest)?;
        if latency.0.len() != 4
            || latency
                .0
                .iter()
                .zip(&self.aliases)
                .any(|((alias, millis), expected)| *alias != expected || *millis < 0)
        {
            return Err(Failure::IneligibleRequest);
        }
        let mut update = UserSessionExtendedDataUpdate::decode(&self.current, body_limits())
            .map_err(|_| Failure::Reply)?;
        let data = update.extended_data.as_mut().ok_or(Failure::Reply)?;
        let (Some(NetworkAddress::IpPair(old)), Some(NetworkAddress::IpPair(new))) =
            (&data.address, &info.address)
        else {
            return Err(Failure::IneligibleRequest);
        };
        if old.encode(body_limits()).map_err(|_| Failure::Reply)?
            != new
                .encode(body_limits())
                .map_err(|_| Failure::IneligibleRequest)?
        {
            return Err(Failure::IneligibleRequest);
        }
        let best = latency
            .0
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, millis))| *millis)
            .ok_or(Failure::IneligibleRequest)?
            .0;
        data.best_ping_site_alias = Some(&self.aliases[best]);
        data.latency_list = Some(LatencyList(latency.0.iter().map(|(_, ms)| *ms).collect()));
        data.qos_data = Some(qos);
        let body = update.encode(body_limits()).map_err(|_| Failure::Reply)?;
        let mut ack = f.fields;
        ack.category = 1;
        let output = vec![
            encode(ack, &[])?,
            encode(
                Fields {
                    routing_a: 30722,
                    routing_b: 1,
                    category: 2,
                    ..Default::default()
                },
                &body,
            )?,
        ];
        self.completed = true;
        self.stop();
        Ok(Some(output))
    }
}
