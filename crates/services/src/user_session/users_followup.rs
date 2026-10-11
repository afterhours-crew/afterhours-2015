// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Error as Failure, body_limits, frame_limits};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::users::{
    NetworkAddress, NetworkQosData, NotifyUserAddedInitial, UpdateHardwareFlagsRequest,
    UpdateNetworkInfoRequest, UserSessionExtendedDataInitial, UserSessionExtendedDataNetwork,
    UserSessionExtendedDataUpdate,
};

const DMAP_KEYS: [u32; 6] = [1, 0x70001, 0x70002, 0xe0001, 0xe0002, 0x78020001];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Network,
    Hardware,
    ObserveOnly,
}

pub struct Followup {
    baseline: Vec<u8>,
    persona: i64,
    state: State,
    completed: bool,
    previous: Option<(Vec<u8>, Vec<u8>)>,
}
fn frame(wire: &[u8]) -> Result<Frame<'_>, Failure> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Failure::IneligibleRequest)?
        .ok_or(Failure::IneligibleRequest)?;
    if d.consumed != wire.len() || !d.frame.metadata.is_empty() {
        return Err(Failure::IneligibleRequest);
    }
    Ok(d.frame)
}
fn header(fields: Fields, command: u16, category: u8) -> bool {
    fields.routing_a == 30722
        && fields.routing_b == command
        && fields.category == category
        && fields.slot == 0
        && fields.reserved == [0, 0]
}
fn fixed_qos(q: &NetworkQosData<'_>, nat: i64, bandwidth_errors: &[u32]) -> bool {
    q.unknown_field_count() == 0
        && q.bandwidth_error_code
            .is_some_and(|value| bandwidth_errors.contains(&value))
        && q.downstream_bits_per_second == Some(0)
        && q.nat_error_code == Some(0)
        && q.nat_type == Some(nat)
        && q.upstream_bits_per_second == Some(0)
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
        let f = frame(initial_user_added_frame).map_err(|_| Failure::ProfileShape)?;
        if !header(f.fields, 2, 2) || f.fields.correlation != 0 {
            return Err(Failure::ProfileShape);
        }
        let message = NotifyUserAddedInitial::decode(f.body, body_limits())
            .map_err(|_| Failure::ProfileShape)?;
        if message.unknown_field_count() != 0
            || message
                .encode(body_limits())
                .map_err(|_| Failure::ProfileShape)?
                != f.body
        {
            return Err(Failure::ProfileShape);
        }
        let data = message.extended_data.ok_or(Failure::ProfileShape)?;
        let user = message.user_info.ok_or(Failure::ProfileShape)?;
        let persona = user.blaze_id.ok_or(Failure::ProfileShape)?;
        let account = user.account_id.ok_or(Failure::ProfileShape)?;
        if persona <= 0
            || account <= 0
            || persona == account
            || user.external_id != Some(account as u64)
            || user.origin_persona_id != Some(persona as u64)
            || user.external_blob.as_ref().is_none_or(|b| !b.0.is_empty())
            || user.pid_id != Some(0)
            || user.name.is_none_or(|name| {
                name.is_empty()
                    || name.len() > 64
                    || std::str::from_utf8(name)
                        .map_or(true, |name| name.chars().any(char::is_control))
            })
            || user.account_locale.is_none()
            || user
                .persona_namespace
                .is_none_or(|v| v.is_empty() || v.len() > 64 || !v.iter().all(u8::is_ascii_graphic))
        {
            return Err(Failure::ProfileConfig);
        }
        if data.address.is_none()
            || data.client_data.is_none()
            || data.best_ping_site_alias != Some(b"")
            || data.country != Some(b"")
            || data.isp != Some(b"")
            || data.time_zone != Some(b"")
            || data.hardware_flags != Some(0)
            || data.user_info_attribute != Some(0)
            || data.latency_list.is_some()
            || data
                .qos_data
                .as_ref()
                .is_none_or(|q| !fixed_qos(q, 0, &[0]))
            || data
                .data_map
                .as_ref()
                .is_none_or(|m| m.0.as_slice() != DMAP_KEYS.map(|k| (k, 0)))
            || data
                .blaze_object_id_list
                .as_ref()
                .is_none_or(|v| v.0.len() != 1 || v.0[0].2 <= 0)
        {
            return Err(Failure::ProfileConfig);
        }
        Ok(Self {
            baseline: data
                .encode(body_limits())
                .map_err(|_| Failure::ProfileShape)?,
            persona,
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
        if !matches!(result, Ok(Some(_))) {
            self.state = State::ObserveOnly;
            self.previous = None;
        }
        result
    }
    fn next(&mut self, request: &[u8]) -> Result<Option<Vec<Vec<u8>>>, Failure> {
        if self.state == State::ObserveOnly {
            return Ok(None);
        }
        let f = frame(request)?;
        let mut reply_header = f.fields;
        reply_header.category = 1;
        match self.state {
            State::Network if header(f.fields, 20, 0) => {
                let message = UpdateNetworkInfoRequest::decode(f.body, body_limits())
                    .map_err(|_| Failure::IneligibleRequest)?;
                if message.unknown_field_count() != 0
                    || message.opts != Some(1)
                    || message
                        .encode(body_limits())
                        .map_err(|_| Failure::IneligibleRequest)?
                        != f.body
                {
                    return Err(Failure::IneligibleRequest);
                }
                let info = message.network_info.ok_or(Failure::IneligibleRequest)?;
                if info
                    .qos_data
                    .as_ref()
                    .is_none_or(|q| !fixed_qos(q, 5, &[0, 0xa083_fffe]))
                    || info.ping_site_latency_by_alias.as_ref().is_none_or(|m| {
                        m.0.len() != 4
                            || m.0.iter().any(|(alias, _)| {
                                alias.is_empty()
                                    || alias.len() > 64
                                    || !alias.iter().all(u8::is_ascii_graphic)
                            })
                    })
                {
                    return Err(Failure::IneligibleRequest);
                }
                let Some(NetworkAddress::IpPair(pair)) = info.address else {
                    return Err(Failure::IneligibleRequest);
                };
                if pair.machine_id.is_none()
                    || pair.external_address.as_ref().is_none_or(|a| {
                        a.ip.is_none() || a.machine_id.is_none() || a.port.is_none()
                    })
                    || pair.internal_address.as_ref().is_none_or(|a| {
                        a.ip.is_none() || a.machine_id.is_none() || a.port.is_none()
                    })
                {
                    return Err(Failure::IneligibleRequest);
                }
                let baseline =
                    UserSessionExtendedDataInitial::decode(&self.baseline, body_limits())
                        .map_err(|_| Failure::Reply)?;
                let body = UserSessionExtendedDataUpdate {
                    extended_data: Some(UserSessionExtendedDataNetwork {
                        address: Some(NetworkAddress::IpPair(pair)),
                        best_ping_site_alias: baseline.best_ping_site_alias,
                        country: baseline.country,
                        client_data: baseline.client_data,
                        data_map: baseline.data_map,
                        hardware_flags: baseline.hardware_flags,
                        isp: baseline.isp,
                        latency_list: baseline.latency_list,
                        qos_data: baseline.qos_data,
                        time_zone: baseline.time_zone,
                        user_info_attribute: baseline.user_info_attribute,
                        blaze_object_id_list: baseline.blaze_object_id_list,
                        ..Default::default()
                    }),
                    subscribed: Some(true),
                    user_id: Some(self.persona),
                    ..Default::default()
                }
                .encode(body_limits())
                .map_err(|_| Failure::Reply)?;
                let acknowledgement = encode(reply_header, &[])?;
                let notification = encode(
                    Fields {
                        routing_a: 30722,
                        routing_b: 1,
                        category: 2,
                        ..Default::default()
                    },
                    &body,
                )?;
                self.previous = Some((request.to_vec(), acknowledgement.clone()));
                self.state = State::Hardware;
                Ok(Some(vec![acknowledgement, notification]))
            }
            State::Hardware if header(f.fields, 8, 0) => {
                let message = UpdateHardwareFlagsRequest::decode(f.body, body_limits())
                    .map_err(|_| Failure::IneligibleRequest)?;
                if message.unknown_field_count() != 0
                    || message.hardware_flags != Some(0)
                    || message
                        .encode(body_limits())
                        .map_err(|_| Failure::IneligibleRequest)?
                        != f.body
                {
                    return Err(Failure::IneligibleRequest);
                }
                let acknowledgement = encode(reply_header, &[])?;
                self.previous = Some((request.to_vec(), acknowledgement.clone()));
                self.state = State::ObserveOnly;
                self.completed = true;
                Ok(Some(vec![acknowledgement]))
            }
            _ => Ok(None),
        }
    }
}
