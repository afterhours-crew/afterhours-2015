// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

/// The installed distance calculation's publishers in one owned logic lifetime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding {
    pub role: u16,
    pub kilometres: usize,
    pub miles: usize,
    pub period_seconds: f32,
    pub metres_per_mile: f32,
}

impl Binding {
    pub(super) fn read(value: &Json, batch: &Batch) -> Result<Self, Failure> {
        keys(
            value,
            &[
                "role",
                "kilometres",
                "miles",
                "period_seconds",
                "metres_per_mile",
            ],
        )?;
        let role = ghost_id(&value["role"])?;
        let kilometres = uint(&value["kilometres"])?;
        let miles = uint(&value["miles"])?;
        let number = |key: &str| -> Result<f32, Failure> {
            let value = value[key].as_f64().ok_or(Failure::ProfileConfig)? as f32;
            if !value.is_finite() || value <= 0. {
                return Err(Failure::ProfileConfig);
            }
            Ok(value)
        };
        let result = Self {
            role,
            kilometres,
            miles,
            period_seconds: number("period_seconds")?,
            metres_per_mile: number("metres_per_mile")?,
        };
        let definition = batch
            .entities
            .iter()
            .find(|d| d.capture_id == role)
            .ok_or(Failure::ProfileConfig)?;
        if result.metres_per_mile < 1.
            || kilometres == miles
            || [kilometres, miles]
                .into_iter()
                .any(|index| definition.roles.get(index) != Some(&Role::EmptyVariant))
        {
            return Err(Failure::ProfileConfig);
        }
        Ok(result)
    }
}

impl GarageLogic {
    /// Resolve only this participant's current authored instance. Serializer
    /// positions identify fields; the live fields supply their RPC selectors.
    pub fn distance_publishers(
        &self,
        players: &Players,
        ghost: u16,
        participant: u16,
    ) -> Result<[u16; 2], replication::Error> {
        let binding = self
            .driving_distance
            .ok_or(replication::Error::Unsupported)?;
        let definition = self
            .arrived
            .entities
            .iter()
            .find(|d| d.capture_id == binding.role)
            .ok_or(replication::Error::Shape)?;
        let Some(replication::Initial::Entity { prefix, fields }) =
            players.objects().get(ghost).map(|object| &object.initial)
        else {
            return Err(replication::Error::UnknownObject);
        };
        if prefix.asset != definition.asset
            || !matches!(fields.first(),
            Some(Initial::Root { reference, .. }) if *reference == participant)
        {
            return Err(replication::Error::TypeMismatch);
        }
        let selector = |index| match fields.get(index) {
            Some(Initial::RpcVariant { rpc, .. }) => Ok(rpc.selector),
            _ => Err(replication::Error::TypeMismatch),
        };
        Ok([selector(binding.kilometres)?, selector(binding.miles)?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn binds_both_publishers_in_the_same_free_roam_lifetime() {
        let batch = Batch {
            entities: vec![Definition {
                capture_id: 8,
                asset: Asset {
                    bundle: 1,
                    type_id: 1,
                    local_index: 1,
                },
                blueprint: Blueprint::Scene(1),
                roles: vec![
                    Role::Root { bus_count: 1 },
                    Role::EmptyVariant,
                    Role::EmptyVariant,
                ],
            }],
            ..Batch::default()
        };
        let value = json!({"role":8,"kilometres":1,"miles":2,
            "period_seconds":1,"metres_per_mile":1609.344});
        assert_eq!(Binding::read(&value, &batch).unwrap().kilometres, 1);
        for (key, changed) in [
            ("role", json!(9)),
            ("kilometres", json!(0)),
            ("kilometres", json!(2)),
            ("miles", json!(3)),
            ("miles", json!(-1)),
            ("period_seconds", json!(0)),
            ("period_seconds", json!(1e99)),
            ("metres_per_mile", json!(-1)),
            ("metres_per_mile", json!(0.5)),
            ("metres_per_mile", json!("1609.344")),
        ] {
            let mut wrong = value.clone();
            wrong[key] = changed;
            assert!(Binding::read(&wrong, &batch).is_err(), "{key}");
        }
    }
}
