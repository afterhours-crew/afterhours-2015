// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Listener realms encoded separately in entity Flags. These do not determine
//! object construction or replace event connection delivery modes.
use crate::{Error, graph::bits32};
use nfs_frostbite::ebx::{Object, Value};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Realm {
    Neither,
    Client,
    Server,
    Both,
}
impl Realm {
    pub fn includes_server(self) -> bool {
        matches!(self, Self::Server | Self::Both)
    }
    pub fn includes_client(self) -> bool {
        matches!(self, Self::Client | Self::Both)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Listeners {
    pub events: Realm,
    pub properties: Realm,
}
impl Listeners {
    pub fn from_flags(flags: u32) -> Self {
        fn realm(bits: u32) -> Realm {
            match bits & 3 {
                0 => Realm::Neither,
                1 => Realm::Client,
                2 => Realm::Server,
                _ => Realm::Both,
            }
        }
        Self {
            events: realm(flags >> 25),
            properties: realm(flags >> 27),
        }
    }
    /// Non-entity objects may lack Flags; absence is not a guessed realm.
    pub fn read(object: &Object) -> Result<Option<Self>, Error> {
        // EntityBusData Flags are UInt16, distinct from EntityData flags.
        // SubWorldData derives from SpatialPrefabBlueprint; LevelData derives
        // from WorldData and ultimately SubWorldData.
        if matches!(object.fields.get("Flags"), Some(Value::UInt16(_)))
            && (object.class.ends_with("Blueprint")
                || matches!(
                    object.class.as_str(),
                    "SubWorldData" | "WorldData" | "LevelData" | "WorldPartData" | "LayerData"
                ))
        {
            return Ok(None);
        }
        object
            .fields
            .get("Flags")
            .map(|f| bits32(f).map(Self::from_flags))
            .transpose()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blueprint_bus_flags_are_not_entity_listener_flags() {
        use nfs_frostbite::ebx::Fields;
        let mut object = Object {
            class: "LogicPrefabBlueprint".into(),
            guid: None,
            fields: Fields(vec![("Flags".into(), Value::UInt16(15))]),
        };
        assert_eq!(Listeners::read(&object).unwrap(), None);
        for class in [
            "SubWorldData",
            "WorldData",
            "LevelData",
            "WorldPartData",
            "LayerData",
        ] {
            object.class = class.into();
            assert_eq!(Listeners::read(&object).unwrap(), None);
        }
        object.class = "BoolEntityData".into();
        assert!(Listeners::read(&object).is_err());
        object.fields = Fields(vec![("Flags".into(), Value::Bool(true))]);
        assert!(Listeners::read(&object).is_err());
        object.fields = Fields(vec![]);
        assert_eq!(Listeners::read(&object).unwrap(), None);
    }
    #[test]
    fn event_and_property_realms_are_independent_of_other_flag_bits() {
        for (a, event) in [Realm::Neither, Realm::Client, Realm::Server, Realm::Both]
            .into_iter()
            .enumerate()
        {
            for (b, property) in [Realm::Neither, Realm::Client, Realm::Server, Realm::Both]
                .into_iter()
                .enumerate()
            {
                let flags = ((a as u32) << 25) | ((b as u32) << 27) | 0xe1ffffff;
                assert_eq!(
                    Listeners::from_flags(flags),
                    Listeners {
                        events: event,
                        properties: property
                    }
                );
                assert_eq!(event.includes_server(), a >= 2);
                assert_eq!(event.includes_client(), a == 1 || a == 3);
            }
        }
    }
}
