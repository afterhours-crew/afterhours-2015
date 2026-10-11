// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Rules, fields::*};
use crate::{
    Error,
    scan::Assets,
    scenes::{MAX_GRAPH_DEPTH, MAX_GRAPH_OBJECTS},
};
use nfs_frostbite::ebx::{Object, Pointer, Value};
use std::collections::BTreeSet;

pub(super) struct Node {
    pub key: Key,
    pub object: Object,
    pub path: Vec<u16>,
}
pub(super) fn read(assets: &mut Assets, root: Key, rules: &Rules) -> Result<Vec<Node>, Error> {
    struct Walk<'a> {
        assets: &'a mut Assets,
        rules: &'a Rules,
        active: BTreeSet<Key>,
        rows: Vec<Node>,
    }
    impl Walk<'_> {
        fn visit(&mut self, key: Key, path: Vec<u16>) -> Result<(), Error> {
            if path.len() >= MAX_GRAPH_DEPTH || self.rows.len() >= MAX_GRAPH_OBJECTS {
                return Err(bad("composition graph bound"));
            }
            if !self.active.insert(key.clone()) {
                return Err(bad("composition graph cycle"));
            }
            let object = object(self.assets, &key)?;
            if !self.rules.classes.contains_key(&object.class) {
                return Err(bad(&format!(
                    "unsupported composition class {}",
                    object.class
                )));
            }
            if object
                .fields
                .get("Excluded")
                .is_some_and(|v| v.as_bool() != Some(false))
            {
                return Err(bad("excluded component"));
            }
            self.rows.push(Node {
                key: key.clone(),
                object: object.clone(),
                path: path.clone(),
            });
            let mut ordinal = 0u16;
            for name in ["Objects", "Object", "Components", "Blueprint"] {
                // AudioBlueprint is a sound graph, not an EntityBusData instance.
                if name == "Blueprint" && object.class == "RaceVehicleAudioComponentData" {
                    continue;
                }
                let Some(value) = object.fields.get(name) else {
                    continue;
                };
                let pointers: Vec<Pointer> = match value {
                    Value::Pointer(p) => vec![*p],
                    Value::Array(a) if a.len() <= MAX_GRAPH_OBJECTS => a
                        .iter()
                        .map(|v| {
                            v.as_pointer()
                                .ok_or_else(|| bad("non-pointer component edge"))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err(bad("composition edge type or bound")),
                };
                for p in pointers {
                    if let Some(child) = self.assets.target(&key.0, p)? {
                        let mut child_path = path.clone();
                        child_path.push(ordinal);
                        ordinal = ordinal.checked_add(1).ok_or_else(|| bad("edge count"))?;
                        self.visit(child, child_path)?;
                    }
                }
            }
            self.active.remove(&key);
            Ok(())
        }
    }
    let mut walk = Walk {
        assets,
        rules,
        active: BTreeSet::new(),
        rows: Vec::new(),
    };
    walk.visit(root, Vec::new())?;
    Ok(walk.rows)
}

pub(super) fn unique<'a>(rows: &'a [Node], class: &str) -> Result<&'a Node, Error> {
    let mut matches = rows.iter().filter(|n| n.object.class == class);
    let first = matches
        .next()
        .ok_or_else(|| bad(&format!("missing {class}")))?;
    if matches.next().is_some() {
        return Err(bad(&format!("multiple {class}")));
    }
    Ok(first)
}
