// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Util component 9: preAuth and postAuth payload schemas.
use crate::{Error, Wire, check_string, integer, schema, unique_keys};
use nfs_heat2::{Encoder, Field, Fields, Item, Kind, Limits, Value};
use std::{collections::BTreeSet, fmt};

pub const COMPONENT: u16 = 9;
pub const FETCH_CLIENT_CONFIG: u16 = 1;
pub const PING: u16 = 2;
pub const PRE_AUTH: u16 = 7;
pub const POST_AUTH: u16 = 8;
pub const USER_SETTINGS_LOAD_ALL: u16 = 12;
pub const USER_SETTINGS_LOAD: u16 = 10;
pub const SET_CLIENT_STATE: u16 = 28;

mod text_filter;
pub use text_filter::{
    FILTER_FOR_PROFANITY, FilterResult, FilterUserTextResponse, FilteredUserText,
    FilteredUserTextList,
};

mod save;
pub use save::{USER_SETTINGS_SAVE, UserSettingsSaveRequest};

mod client_state;
pub use client_state::ClientState;

mod postauth;
pub use postauth::{
    GetTelemetryServerResponse, GetTickerServerResponse, PostAuthRequest, PostAuthResponse,
    UserOptions,
};

#[derive(Default)]
pub struct ComponentIds(pub Vec<u16>);
impl fmt::Debug for ComponentIds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ComponentIdsLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for ComponentIds {
    const KIND: Kind = Kind::List;
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::List {
                element: Kind::Integer,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        let values = item
            .elements()
            .ok_or(Error::WrongType { tag: None })?
            .map(|item| {
                u16::try_from(integer(item?)?).map_err(|_| Error::InvalidInteger { tag: None })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self(values))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.integer_list(tag, self.0.iter().map(|v| i64::from(*v)))
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 1)
    }
}

#[derive(Default)]
pub struct ConfigEntries<'a>(pub Vec<(&'a [u8], &'a [u8])>);
impl fmt::Debug for ConfigEntries<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ConfigEntriesLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for ConfigEntries<'a> {
    const KIND: Kind = Kind::Map;
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::Map {
                key: Kind::String,
                value: Kind::String,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        let mut elements = item.elements().ok_or(Error::WrongType { tag: None })?;
        let mut entries = Vec::new();
        while let Some(key) = elements.next() {
            let key = <&[u8]>::read(key?)?;
            let value = <&[u8]>::read(elements.next().ok_or(Error::WrongType { tag: None })??)?;
            entries.push((key, value));
        }
        unique_keys(entries.iter().map(|(key, _)| *key))?;
        Ok(Self(entries))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.string_map(tag, &self.0)
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 2)?;
        for (key, value) in &self.0 {
            check_string(key, budget.limits)?;
            check_string(value, budget.limits)?;
        }
        unique_keys(self.0.iter().map(|(key, _)| *key))
    }
}

#[derive(Default)]
pub struct PingSites<'a>(pub Vec<(&'a [u8], QosPingSiteInfo<'a>)>);
impl fmt::Debug for PingSites<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PingSitesLength")
            .field(&self.0.len())
            .finish()
    }
}
impl<'a> Wire<'a> for PingSites<'a> {
    const KIND: Kind = Kind::Map;
    fn unknown_count(&self) -> usize {
        self.0
            .iter()
            .map(|(_, site)| site.unknown_field_count())
            .sum()
    }
    fn read(item: Item<'a>) -> Result<Self, Error> {
        if !matches!(
            item.value(),
            Value::Map {
                key: Kind::String,
                value: Kind::Struct,
                ..
            }
        ) {
            return Err(Error::WrongType { tag: None });
        }
        let mut elements = item.elements().ok_or(Error::WrongType { tag: None })?;
        let mut entries = Vec::new();
        while let Some(key) = elements.next() {
            let key = <&[u8]>::read(key?)?;
            let value =
                QosPingSiteInfo::read(elements.next().ok_or(Error::WrongType { tag: None })??)?;
            entries.push((key, value));
        }
        unique_keys(entries.iter().map(|(key, _)| *key))?;
        Ok(Self(entries))
    }
    fn write(&self, tag: [u8; 3], w: &mut Encoder) -> Result<(), nfs_heat2::Error> {
        w.string_struct_map(tag, &self.0, |w, site| site.write_fields(w))
    }
    fn validate(&self, budget: &mut crate::Budget) -> Result<(), Error> {
        budget.take(1)?;
        budget.collection(self.0.len(), 1)?;
        for (key, _) in &self.0 {
            check_string(key, budget.limits)?;
        }
        unique_keys(self.0.iter().map(|(key, _)| *key))?;
        for (_, site) in &self.0 {
            site.validate(budget)?;
        }
        Ok(())
    }
}

schema!(ClientData {
    ignore_inactivity_timeout: bool => [0xa6,0x9d,0x2f],
    locale: u32 => [0xb2,0x1b,0xa7],
    service_name: &'a [u8] => [0xcf,0x68,0xee],
    client_type: i64 => [0xd3,0x9c,0x25],
});
schema!(ClientInfo {
    blaze_sdk_version: &'a [u8] => [0x8b,0x39,0x2b],
    blaze_sdk_build_date: &'a [u8] => [0x8b,0x4a,0x6d],
    client_name: &'a [u8] => [0x8e,0xcb,0xb4],
    platform: i64 => [0x8f,0x09,0xb4],
    client_sku_id: &'a [u8] => [0x8f,0x3a,0xf5],
    client_version: &'a [u8] => [0x8f,0x69,0x72],
    dirty_sdk_version: &'a [u8] => [0x93,0x39,0x2b],
    environment: &'a [u8] => [0x96,0xed,0x80],
    client_locale: u32 => [0xb2,0xf8,0xc0],
});
schema!(FetchClientConfigRequest { config_section: &'a [u8] => [0x8e,0x6a,0x64] });
schema!(FetchConfigResponse { config: ConfigEntries<'a> => [0x8e,0xfb,0xa6] });

// The SMAP member is a string-to-string map. ConfigEntries is the same bounded wire
// container; values here are player settings, not shared configuration.
// Persistence and authenticated player selection belong to the service layer.
schema!(UserSettingsLoadAllResponse { data_map: ConfigEntries<'a> => [0xce,0xd8,0x70] });

// native single-setting request and response; UID semantics are service policy.
schema!(UserSettingsLoadRequest {
    key: &'a [u8] => [0xae,0x5e,0x40],
    user_id: i64 => [0xd6,0x99,0x00],
});
schema!(UserSettingsResponse {
    data: &'a [u8] => [0x92,0x1d,0x21],
    key: &'a [u8] => [0xae,0x5e,0x40],
});

// : member initializer binds STIM to uint32_t. Clock policy stays outside
// the codec; epoch seconds are corroborated by two saved capture timestamps.
schema!(PingResponse { server_time: u32 => [0xcf,0x4a,0x6d] });
schema!(QosPingSiteInfo {
    address: &'a [u8] => [0xc3,0x38,0x40],
    port: u16 => [0xc3,0x3c,0x00],
    site_name: &'a [u8] => [0xce,0xe8,0x40],
});
schema!(QosConfigInfo {
    bandwidth_ping_site_info: QosPingSiteInfo<'a> => [0x8b,0x7c,0x33],
    num_latency_probes: u16 => [0xb2,0xec,0x00],
    ping_site_info_by_alias_map: PingSites<'a> => [0xb3,0x4c,0x33],
    service_id: u32 => [0xcf,0x6a,0x64],
    timeout: i64 => [0xd2,0x9b,0x65],
});
schema!(PreAuthRequest {
    client_data: ClientData<'a> => [0x8e,0x48,0x74],
    client_info: ClientInfo<'a> => [0x8e,0x9b,0xa6],
    fetch_client_config: FetchClientConfigRequest<'a> => [0x9a,0x38,0xf2],
    local_address: i64 => [0xb2,0x19,0x24],
});
schema!(PreAuthResponse {
    authentication_source: &'a [u8] => [0x87,0x3c,0xa3],
    component_ids: ComponentIds => [0x8e,0x99,0x33],
    client_id: &'a [u8] => [0x8e,0xca,0x64],
    config: FetchConfigResponse<'a> => [0x8e,0xfb,0xa6],
    entitlement_source: &'a [u8] => [0x97,0x3c,0xa3],
    service_name: &'a [u8] => [0xa6,0xec,0xf4],
    machine_id: u32 => [0xb6,0x1a,0x64],
    underage_supported: bool => [0xb6,0x9b,0xb2],
    persona_namespace: &'a [u8] => [0xba,0x1c,0xf0],
    legal_doc_game_identifier: &'a [u8] => [0xc2,0x9b,0x24],
    platform: &'a [u8] => [0xc2,0xc8,0x74],
    qos_settings: QosConfigInfo<'a> => [0xc6,0xfc,0xf3],
    registration_source: &'a [u8] => [0xcb,0x3c,0xa3],
    server_version: &'a [u8] => [0xcf,0x69,0x72],
});
