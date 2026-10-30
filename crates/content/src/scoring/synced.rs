// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synchronized scalar declarations; no replication profile or lifecycle policy.
use super::events::Identity;
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, bad},
};
use nfs_frostbite::ebx::Object;
use nfs_world::scoring::synced::Kind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub kind: Kind,
}

fn kind(class: &str) -> Option<Kind> {
    match class {
        "SyncedStringEntityData" => Some(Kind::Text),
        "SyncedBoolEntityData" => Some(Kind::Boolean),
        "SyncedFloatEntityData" => Some(Kind::Float),
        "SyncedIntEntityData" => Some(Kind::Integer),
        _ => None,
    }
}
/// These classes have no authored scalar defaults. Their native creation realm
/// is fixed to both; input binding/readiness and listener flags remain separate.
pub fn compile(object: &Object) -> Result<Kind, Error> {
    kind(&object.class).ok_or_else(|| bad("unsupported synchronized scalar class"))
}

impl AssetIndex {
    /// Read all occurrences of the supported scalar classes. Other classes
    /// are outside this reader's scope, not silently implemented behavior.
    pub fn scoring_synchronized(&mut self, blueprint: &str) -> Result<Vec<Node>, Error> {
        if blueprint.is_empty() || blueprint.len() > 512 {
            return Err(bad("scoring blueprint name"));
        }
        Ok(Graph::new(&mut self.assets)
            .read(Site::root(blueprint))?
            .into_iter()
            .filter_map(|row| {
                kind(&row.object.class).map(|kind| Node {
                    identity: Identity::from(&row.site),
                    kind,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_frostbite::ebx::{Fields, Value};
    #[test]
    fn synchronized_kind_has_no_guessed_authored_default_or_realm() {
        for (class, expected) in [
            ("SyncedStringEntityData", Kind::Text),
            ("SyncedBoolEntityData", Kind::Boolean),
            ("SyncedFloatEntityData", Kind::Float),
            ("SyncedIntEntityData", Kind::Integer),
        ] {
            let mut object = Object {
                class: class.into(),
                guid: None,
                fields: Fields(vec![]),
            };
            assert_eq!(compile(&object).unwrap(), expected);
            object
                .fields
                .0
                .push(("DefaultValue".into(), Value::Float32(100.0)));
            object.fields.0.push(("Realm".into(), Value::Bool(false)));
            assert_eq!(compile(&object).unwrap(), expected);
            object.class = "SyncedTransformEntityData".into();
            assert!(compile(&object).is_err());
        }
    }
}
