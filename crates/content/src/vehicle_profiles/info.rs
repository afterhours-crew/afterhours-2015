// This Source Code Form is subject to the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::{Rules, fields::*, graph};
use crate::{Error, assets::AssetIndex};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::{value::Text, vehicle_info::Sample};

pub(super) fn read(object: &Object) -> Result<Sample, Error> {
    if object.class != "RaceVehicleEntityData" {
        return Err(bad("vehicle info class"));
    }
    let fields = &object.fields;
    let bundle = word(structure(fields, "VehicleBundleHash")?, "Hash")?;
    let name = get(fields, "RaceVehicleShortNameId")?
        .as_str()
        .ok_or_else(|| bad("vehicle short name"))?;
    let short_name = Text::new(name).map_err(|_| bad("vehicle short name bound"))?;
    Ok(Sample { bundle, short_name })
}

impl AssetIndex {
    /// Query data from the same validated installed vehicle composition used
    /// to construct the car. Native accessors 04168660/04168680 select these
    /// fields; the signed graph output reinterprets the identical bundle bits.
    pub fn vehicle_info(&mut self, bundle: &str, rules: &Rules) -> Result<Sample, Error> {
        let (_, _, nodes) = super::extract::composition(self, bundle, rules)?;
        read(&graph::unique(&nodes, "RaceVehicleEntityData")?.object)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use nfs_frostbite::ebx::{Fields, Value};
    #[test]
    fn vehicle_info_preserves_bundle_bits_and_owned_empty_or_localized_name() {
        let mut o = Object {
            class: "RaceVehicleEntityData".into(),
            guid: None,
            fields: Fields(vec![
                (
                    "VehicleBundleHash".into(),
                    Value::Struct(Fields(vec![("Hash".into(), Value::UInt32(0xf1234567))])),
                ),
                (
                    "RaceVehicleShortNameId".into(),
                    Value::String("ID_CAR_TEST".into()),
                ),
            ]),
        };
        let sample = read(&o).unwrap();
        assert_eq!(sample.bundle, 0xf1234567);
        assert_eq!(sample.short_name.as_str(), "ID_CAR_TEST");
        o.fields.0[1].1 = Value::String("".into());
        assert_eq!(read(&o).unwrap().short_name.as_str(), "");
        assert_eq!(sample.short_name.as_str(), "ID_CAR_TEST");
        o.fields.0[1].1 = Value::String("bad\0name".into());
        assert!(read(&o).is_err());
        o.class = "Other".into();
        assert!(read(&o).is_err());
    }
}
