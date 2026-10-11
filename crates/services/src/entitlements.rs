// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Read-only entitlement state bound to an explicit durable local account.
//! Named grants and supported group scopes are loaded from a versioned state
//! document. No serialized queries/replies are retained. Grant mutations and
//! general entitlement search are outside this supported query subset.
use crate::{ContentError, SUPPORTED_BUILD_SHA256};
use nfs_fire2::{Fields, Frame};
use nfs_protocol::authentication::entitlements::*;
use nfs_storage::AccountId;
use serde_json::Value as Json;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};
pub const MAX_STATE_BYTES: usize = 512 * 1024;
pub const MAX_GRANTS: usize = 256;
pub const MAX_GROUPS: usize = 32;
pub const MAX_SCOPES: usize = 32;
pub const MAX_STRING_BYTES: usize = 1023;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Identity,
    Ineligible,
    Encode,
}
pub fn body_limits() -> nfs_heat2::Limits {
    nfs_heat2::Limits {
        max_bytes: 32 * 1024,
        max_depth: 4,
        max_values: 4096,
        max_collection: 128,
        max_byte_string: MAX_STRING_BYTES + 1,
    }
}
pub fn frame_limits() -> nfs_fire2::Limits {
    nfs_fire2::Limits::new(32 * 1024 + 16, 0, 32 * 1024).expect("constant limits")
}
pub fn owns(component: u16, command: u16) -> bool {
    (component, command) == (COMPONENT, LIST_USER_ENTITLEMENTS2)
}
struct Grant {
    device_uri: String,
    grant_date: String,
    group_name: String,
    id: u64,
    is_consumable: bool,
    persona_id: i64,
    project_id: String,
    product_catalog: i32,
    product_id: String,
    status: i32,
    status_reason_code: i32,
    entitlement_tag: String,
    termination_date: String,
    entitlement_type: i32,
    use_count: u32,
    version: u32,
}
impl Grant {
    fn wire(&self) -> Entitlement<'_> {
        Entitlement {
            device_uri: Some(self.device_uri.as_bytes()),
            grant_date: Some(self.grant_date.as_bytes()),
            group_name: Some(self.group_name.as_bytes()),
            id: Some(self.id),
            is_consumable: Some(self.is_consumable),
            persona_id: Some(self.persona_id),
            project_id: Some(self.project_id.as_bytes()),
            product_catalog: Some(self.product_catalog),
            product_id: Some(self.product_id.as_bytes()),
            status: Some(self.status),
            status_reason_code: Some(self.status_reason_code),
            entitlement_tag: Some(self.entitlement_tag.as_bytes()),
            termination_date: Some(self.termination_date.as_bytes()),
            entitlement_type: Some(self.entitlement_type),
            use_count: Some(self.use_count),
            version: Some(self.version),
            ..Default::default()
        }
    }
}
/// One account's explicit grant state. Clone/share only within that account;
/// every call supplies an authenticated account and current persona.
pub struct State {
    account: AccountId,
    scopes: Vec<Vec<String>>,
    grants: Vec<Grant>,
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntitlementState")
            .field("scopes", &self.scopes.len())
            .field("grants", &self.grants.len())
            .finish()
    }
}
fn fields(v: &Json, names: &[&str]) -> Result<(), ContentError> {
    let object = v.as_object().ok_or(ContentError::Invalid)?;
    if object.len() != names.len() || !names.iter().all(|n| object.contains_key(*n)) {
        return Err(ContentError::Invalid);
    }
    Ok(())
}
fn text(v: &Json) -> Result<String, ContentError> {
    v.as_str()
        .filter(|s| s.len() <= MAX_STRING_BYTES && !s.contains('\0'))
        .map(str::to_owned)
        .ok_or(ContentError::Invalid)
}
fn rows(v: &Json, max: usize) -> Result<&[Json], ContentError> {
    v.as_array()
        .filter(|v| v.len() <= max)
        .map(Vec::as_slice)
        .ok_or(ContentError::Invalid)
}
impl State {
    /// File I/O must run on a blocking worker at an async edge. Existing state is
    /// read without mutation; reopening preserves the same local grant IDs.
    pub fn load(path: &Path, account: AccountId) -> Result<Self, ContentError> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| ContentError::Io)?
            .take((MAX_STATE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ContentError::Io)?;
        if bytes.len() > MAX_STATE_BYTES {
            return Err(ContentError::TooLarge);
        }
        Self::from_json(
            &serde_json::from_slice(&bytes).map_err(|_| ContentError::Invalid)?,
            account,
        )
    }
    pub fn from_json(v: &Json, account: AccountId) -> Result<Self, ContentError> {
        fields(
            v,
            &[
                "format",
                "version",
                "build_sha256",
                "account",
                "scopes",
                "grants",
            ],
        )?;
        if v["format"] != "nfs-entitlement-state"
            || v["version"] != 1
            || v["build_sha256"] != SUPPORTED_BUILD_SHA256
            || v["account"].as_str() != Some(account.hex().as_str())
        {
            return Err(ContentError::Invalid);
        }
        let mut scopes = Vec::new();
        let mut distinct = BTreeSet::new();
        let mut known = BTreeSet::new();
        for scope in rows(&v["scopes"], MAX_SCOPES)? {
            let groups = rows(scope, MAX_GROUPS)?
                .iter()
                .map(text)
                .collect::<Result<Vec<_>, _>>()?;
            let mut unique = BTreeSet::new();
            if groups.is_empty()
                || groups.iter().any(|s| s.is_empty() || !unique.insert(s))
                || !distinct.insert(groups.clone())
            {
                return Err(ContentError::Invalid);
            }
            known.extend(groups.iter().cloned());
            scopes.push(groups);
        }
        if scopes.is_empty() {
            return Err(ContentError::Invalid);
        }
        let mut grants = Vec::new();
        let mut ids = BTreeSet::new();
        for row in rows(&v["grants"], MAX_GRANTS)? {
            fields(
                row,
                &[
                    "device_uri",
                    "grant_date",
                    "group_name",
                    "id",
                    "is_consumable",
                    "persona_id",
                    "project_id",
                    "product_catalog",
                    "product_id",
                    "status",
                    "status_reason_code",
                    "entitlement_tag",
                    "termination_date",
                    "entitlement_type",
                    "use_count",
                    "version",
                ],
            )?;
            let grant = Grant {
                device_uri: text(&row["device_uri"])?,
                grant_date: text(&row["grant_date"])?,
                group_name: text(&row["group_name"])?,
                id: row["id"].as_u64().ok_or(ContentError::Invalid)?,
                is_consumable: row["is_consumable"]
                    .as_bool()
                    .ok_or(ContentError::Invalid)?,
                persona_id: row["persona_id"].as_i64().ok_or(ContentError::Invalid)?,
                project_id: text(&row["project_id"])?,
                product_catalog: i32::try_from(
                    row["product_catalog"]
                        .as_i64()
                        .ok_or(ContentError::Invalid)?,
                )
                .map_err(|_| ContentError::Invalid)?,
                product_id: text(&row["product_id"])?,
                status: i32::try_from(row["status"].as_i64().ok_or(ContentError::Invalid)?)
                    .map_err(|_| ContentError::Invalid)?,
                status_reason_code: i32::try_from(
                    row["status_reason_code"]
                        .as_i64()
                        .ok_or(ContentError::Invalid)?,
                )
                .map_err(|_| ContentError::Invalid)?,
                entitlement_tag: text(&row["entitlement_tag"])?,
                termination_date: text(&row["termination_date"])?,
                entitlement_type: i32::try_from(
                    row["entitlement_type"]
                        .as_i64()
                        .ok_or(ContentError::Invalid)?,
                )
                .map_err(|_| ContentError::Invalid)?,
                use_count: u32::try_from(row["use_count"].as_u64().ok_or(ContentError::Invalid)?)
                    .map_err(|_| ContentError::Invalid)?,
                version: u32::try_from(row["version"].as_u64().ok_or(ContentError::Invalid)?)
                    .map_err(|_| ContentError::Invalid)?,
            };
            if grant.id == 0
                || !ids.insert(grant.id)
                || grant.persona_id != 0
                || !known.contains(&grant.group_name)
            {
                return Err(ContentError::Invalid);
            }
            grants.push(grant);
        }
        let state = Self {
            account,
            scopes,
            grants,
        };
        for scope in &state.scopes {
            state.body(scope).map_err(|_| ContentError::TooLarge)?;
        }
        Ok(state)
    }
    fn body(&self, scope: &[String]) -> Result<Vec<u8>, Error> {
        Entitlements {
            entitlements: Some(EntitlementList(
                self.grants
                    .iter()
                    .filter(|g| scope.contains(&g.group_name))
                    .map(Grant::wire)
                    .collect(),
            )),
            ..Default::default()
        }
        .encode(body_limits())
        .map_err(|_| Error::Encode)
    }
    pub fn reply(&self, wire: &[u8], account: AccountId, persona: i64) -> Result<Vec<u8>, Error> {
        if account != self.account || persona <= 0 {
            return Err(Error::Identity);
        }
        let decoded = nfs_fire2::decode(wire, frame_limits())
            .map_err(|_| Error::Ineligible)?
            .ok_or(Error::Ineligible)?;
        let f = decoded.frame;
        if decoded.consumed != wire.len()
            || !owns(f.fields.routing_a, f.fields.routing_b)
            || f.fields.category != 0
            || f.fields.slot != 0
            || f.fields.reserved != [0, 0]
            || !f.metadata.is_empty()
        {
            return Err(Error::Ineligible);
        }
        let query = ListUserEntitlements2Request::decode(f.body, body_limits())
            .map_err(|_| Error::Ineligible)?;
        if !selectors(&query)
            || query.user_id != Some(persona)
            || query.encode(body_limits()).map_err(|_| Error::Ineligible)? != f.body
        {
            return Err(Error::Ineligible);
        }
        let groups = &query.group_name_list.as_ref().ok_or(Error::Ineligible)?.0;
        let scope = self
            .scopes
            .iter()
            .find(|s| {
                s.len() == groups.len() && s.iter().zip(groups).all(|(a, b)| a.as_bytes() == *b)
            })
            .ok_or(Error::Ineligible)?;
        let body = self.body(scope)?;
        nfs_fire2::encode(
            Frame {
                fields: Fields {
                    category: 1,
                    ..f.fields
                },
                metadata: &[],
                body: &body,
            },
            frame_limits(),
        )
        .map_err(|_| Error::Encode)
    }
}

fn selectors(request: &ListUserEntitlements2Request<'_>) -> bool {
    request.unknown_field_count() == 0
        && request.user_id.is_some_and(|v| v > 0)
        && request.end_grant_date == Some(b"")
        && request.page_no == Some(0)
        && request.page_size == Some(0)
        && request.entitlement_tag == Some(b"")
        && request.end_termination_date == Some(b"")
        && request.has_authorized_persona == Some(false)
        && request.project_id == Some(b"")
        && request.product_id == Some(b"")
        && request.recursive_search == Some(false)
        && request.start_grant_date == Some(b"")
        && request.status == Some(0)
        && request.start_termination_date == Some(b"")
        && request.entitlement_type == Some(0)
}
