// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Current-session user lookup and headset state, with caller-owned external
//! directory resolution. Unsupported selectors do not imply absent records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    IneligibleRequest,
    ProfileShape,
    ProfileConfig,
    Reply,
}
use nfs_fire2::{Fields, Frame};
use nfs_protocol::{
    metadata::Fire2Metadata,
    users::{
        self, NotifyUserAddedInitial, UserData, UserIdentification, UserSessionExtendedDataNetwork,
        UserSessionExtendedDataUpdate,
    },
};

pub const USER_NOT_FOUND: i32 = 96258;
pub const MAX_BODY_BYTES: usize = 16 * 1024;
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(MAX_BODY_BYTES + 128 + 16, 128, MAX_BODY_BYTES).expect("constant limits")
}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: MAX_BODY_BYTES,
        max_depth: 6,
        max_values: 2048,
        max_collection: 128,
        max_byte_string: 1024,
    }
}
fn frame(wire: &[u8], command: u16, category: u8) -> Result<Frame<'_>, Error> {
    let d = nfs_fire2::decode(wire, frame_limits())
        .map_err(|_| Error::IneligibleRequest)?
        .ok_or(Error::IneligibleRequest)?;
    let f = d.frame;
    if d.consumed != wire.len()
        || !f.metadata.is_empty()
        || f.fields.routing_a != users::COMPONENT
        || f.fields.routing_b != command
        || f.fields.category != category
        || f.fields.slot != 0
        || f.fields.reserved != [0, 0]
    {
        return Err(Error::IneligibleRequest);
    }
    Ok(f)
}
fn complete_identity(u: &UserIdentification<'_>) -> bool {
    u.unknown_field_count() == 0
        && u.account_id.is_some()
        && u.account_locale.is_some()
        && u.external_blob.is_some()
        && u.external_id.is_some()
        && u.blaze_id.is_some_and(|id| id > 0)
        && u.name.is_some()
        && u.persona_namespace.is_some()
        && u.origin_persona_id.is_some()
        && u.pid_id.is_some()
}
fn complete_data(d: &UserSessionExtendedDataNetwork<'_>) -> bool {
    d.unknown_field_count() == 0
        && d.address.is_some()
        && d.best_ping_site_alias.is_some()
        && d.country.is_some()
        && d.client_data.is_some()
        && d.data_map.is_some()
        && d.hardware_flags.is_some()
        && d.isp.is_some()
        && d.time_zone.is_some()
        && d.user_info_attribute.is_some()
        && d.qos_data.as_ref().is_some_and(|q| {
            q.bandwidth_error_code.is_some()
                && q.downstream_bits_per_second.is_some()
                && q.nat_error_code.is_some()
                && q.nat_type.is_some()
                && q.upstream_bits_per_second.is_some()
        })
}

/// A caller-supplied result from its own local directory. Unsupported is the
/// default for unresolved external IDs. Absent is an explicit directory fact,
/// never inferred from an ID differing from this session. No Debug exposes data.
pub enum ExternalResolution<'a> {
    Unsupported,
    Absent,
    CurrentRecord(&'a UserData<'a>),
}

/// Snapshot of one current local session, without Debug or a global directory.
/// Rebuild when current session data changes; the caller supplies the latest
/// notification it actually generated, not an old capture from another session.
pub struct Profile {
    persona: i64,
    self_body: Vec<u8>,
}
impl Profile {
    pub fn from_current(
        initial_user_added: &[u8],
        latest_extended_update: &[u8],
    ) -> Result<Self, Error> {
        let added =
            frame(initial_user_added, users::USER_ADDED, 2).map_err(|_| Error::ProfileShape)?;
        let current = frame(
            latest_extended_update,
            users::USER_SESSION_EXTENDED_DATA_UPDATE,
            2,
        )
        .map_err(|_| Error::ProfileShape)?;
        if added.fields.correlation != 0 || current.fields.correlation != 0 {
            return Err(Error::ProfileShape);
        }
        let initial = NotifyUserAddedInitial::decode(added.body, body_limits())
            .map_err(|_| Error::ProfileShape)?;
        let latest = UserSessionExtendedDataUpdate::decode(current.body, body_limits())
            .map_err(|_| Error::ProfileShape)?;
        if initial.unknown_field_count() != 0
            || latest.unknown_field_count() != 0
            || initial
                .encode(body_limits())
                .map_err(|_| Error::ProfileShape)?
                != added.body
            || latest
                .encode(body_limits())
                .map_err(|_| Error::ProfileShape)?
                != current.body
        {
            return Err(Error::ProfileShape);
        }
        if initial.extended_data.is_none() || latest.subscribed != Some(true) {
            return Err(Error::ProfileConfig);
        }
        let user = initial.user_info.ok_or(Error::ProfileConfig)?;
        let data = latest.extended_data.ok_or(Error::ProfileConfig)?;
        if !complete_identity(&user) || !complete_data(&data) || latest.user_id != user.blaze_id {
            return Err(Error::ProfileConfig);
        }
        let persona = user.blaze_id.ok_or(Error::ProfileConfig)?;
        let self_body = UserData {
            extended_data: Some(data),
            status_flags: Some(2),
            user_info: Some(user),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::ProfileConfig)?;
        Ok(Self { persona, self_body })
    }
    pub fn persona_id(&self) -> i64 {
        self.persona
    }
    /// Repeated session-owned headset status update. Build both outputs
    /// before changing state, so validation/encoding failure cannot partly apply.
    /// Other hardware bits and notifications are outside this observed subset.
    pub fn update_hardware(&mut self, wire: &[u8]) -> Result<Vec<u8>, Error> {
        let f = frame(wire, users::UPDATE_HARDWARE_FLAGS, 0)?;
        let q = users::UpdateHardwareFlagsRequest::decode(f.body, body_limits())
            .map_err(|_| Error::IneligibleRequest)?;
        if !matches!(q.hardware_flags, Some(0 | 1))
            || q.unknown_field_count() != 0
            || q.encode(body_limits())
                .map_err(|_| Error::IneligibleRequest)?
                != f.body
        {
            return Err(Error::IneligibleRequest);
        }
        let mut current =
            UserData::decode(&self.self_body, body_limits()).map_err(|_| Error::Reply)?;
        current
            .extended_data
            .as_mut()
            .ok_or(Error::Reply)?
            .hardware_flags = q.hardware_flags;
        let body = current.encode(body_limits()).map_err(|_| Error::Reply)?;
        let reply = nfs_fire2::encode(
            Frame {
                fields: Fields {
                    category: 1,
                    ..f.fields
                },
                metadata: &[],
                body: &[],
            },
            frame_limits(),
        )
        .map_err(|_| Error::Reply)?;
        self.self_body = body;
        Ok(reply)
    }
    /// Pure repeatable selection. The caller owns authorization, directory
    /// lookup, sequencing and transport. Unsupported input never gets success.
    pub fn reply(
        &self,
        wire: &[u8],
        external: ExternalResolution<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        let f = frame(wire, users::LOOKUP_USER, 0)?;
        let q = UserIdentification::decode(f.body, body_limits())
            .map_err(|_| Error::IneligibleRequest)?;
        if q.unknown_field_count() != 0
            || q.encode(body_limits())
                .map_err(|_| Error::IneligibleRequest)?
                != f.body
        {
            return Err(Error::IneligibleRequest);
        }
        if q.account_id != Some(0)
            || q.account_locale != Some(0)
            || q.external_blob.as_ref().is_none_or(|b| !b.0.is_empty())
            || q.name != Some(b"")
            || q.persona_namespace != Some(b"")
            || q.origin_persona_id != Some(0)
            || q.pid_id != Some(0)
        {
            return Ok(None);
        }
        let (category, metadata, body) = match (q.blaze_id, q.external_id) {
            (Some(id), Some(0)) if id == self.persona => (1, Vec::new(), self.self_body.clone()),
            (Some(0), Some(id)) if id != 0 => match external {
                ExternalResolution::Unsupported => return Ok(None),
                ExternalResolution::Absent => (
                    3,
                    Fire2Metadata {
                        context: Some(0),
                        error_code: Some(USER_NOT_FOUND),
                        session_key: None,
                        ..Default::default()
                    }
                    .encode(body_limits())
                    .map_err(|_| Error::Reply)?,
                    Vec::new(),
                ),
                ExternalResolution::CurrentRecord(record) => {
                    if record.unknown_field_count() != 0
                        || record.status_flags != Some(0)
                        || record.user_info.as_ref().is_none_or(|u| {
                            !complete_identity(u)
                                || u.external_id != Some(id)
                                || u.blaze_id == Some(self.persona)
                        })
                        || record
                            .extended_data
                            .as_ref()
                            .is_none_or(|d| !complete_data(d))
                    {
                        return Err(Error::ProfileConfig);
                    }
                    (
                        1,
                        Vec::new(),
                        record
                            .encode(body_limits())
                            .map_err(|_| Error::ProfileConfig)?,
                    )
                }
            },
            _ => return Ok(None),
        };
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    category,
                    ..f.fields
                },
                metadata: &metadata,
                body: &body,
            },
            frame_limits(),
        )
        .map(Some)
        .map_err(|_| Error::Reply)
    }
}

/// Use only when the caller's local directory contains exactly this account.
/// Other nonzero external IDs are absent under that explicit deployment policy.
/// Resolving this account by external ID remains unsupported. Multi-account
/// directories must supply their own resolution to [`Profile::reply`].
/// Malformed frames resolve to Unsupported; reply validates the whole selector.
pub fn single_account_external_resolution(
    wire: &[u8],
    self_external: u64,
) -> ExternalResolution<'static> {
    let Ok(f) = frame(wire, users::LOOKUP_USER, 0) else {
        return ExternalResolution::Unsupported;
    };
    match UserIdentification::decode(f.body, body_limits())
        .ok()
        .and_then(|q| q.external_id)
    {
        Some(id) if id != 0 && id != self_external => ExternalResolution::Absent,
        _ => ExternalResolution::Unsupported,
    }
}
