// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Cache decoding preserves the compiler's private postorder invariants.
use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    ops: Vec<Op>,
    #[serde(with = "crate::prepared_cache::ordered_map")]
    ranges: BTreeMap<Identity, IntegerRange>,
    empty_sets: BTreeSet<Identity>,
}
fn identity(id: &Identity) -> bool {
    !id.asset.is_empty()
        && id.asset.len() <= 512
        && id.instances.len() <= MAX_DEPTH
        && id
            .instances
            .iter()
            .all(|(asset, _)| !asset.is_empty() && asset.len() <= 512)
}
impl TryFrom<Source> for Program {
    type Error = Error;
    fn try_from(source: Source) -> Result<Self, Error> {
        if source.ops.is_empty() || source.ops.len() > MAX_NODES {
            return Err(bad("prepared membership node bound"));
        }
        let mut depths: Vec<usize> = Vec::new();
        let mut used = BTreeSet::new();
        let mut ranges = BTreeSet::new();
        let mut mutable = BTreeSet::new();
        for (i, op) in source.ops.iter().enumerate() {
            let depth = match op {
                Op::Fact(fact) => {
                    let valid = match fact {
                        Fact::Boolean(name) => !name.is_empty() && name.len() <= 512,
                        Fact::Player(id) | Fact::Copy(id) => identity(id),
                        Fact::Mutable(id) => {
                            mutable.insert(id.clone());
                            identity(id)
                        }
                        Fact::IntegerRange(id) => {
                            ranges.insert(id.clone());
                            identity(id)
                        }
                        Fact::States(sites) => {
                            !sites.is_empty()
                                && sites.len() <= 64
                                && sites.iter().all(|s| s.scene_key != 0)
                                && sites.iter().collect::<BTreeSet<_>>().len() == sites.len()
                        }
                    };
                    if !valid {
                        return Err(bad("prepared membership fact identity"));
                    }
                    1
                }
                Op::Combine(_, a, b) => {
                    if *a >= i || *b >= i {
                        return Err(bad("prepared membership forward input"));
                    }
                    used.extend([*a, *b]);
                    1 + depths[*a].max(depths[*b])
                }
            };
            if depth > MAX_DEPTH {
                return Err(bad("prepared membership depth"));
            }
            depths.push(depth);
        }
        if used != (0..source.ops.len() - 1).collect()
            || ranges != source.ranges.keys().cloned().collect()
            || !source.empty_sets.is_subset(&mutable)
            || source
                .ranges
                .values()
                .any(|r| r.input_port != 0 || !identity(&r.input))
        {
            return Err(bad("prepared membership ownership or unreachable input"));
        }
        Ok(Self {
            ops: source.ops,
            ranges: source.ranges,
            empty_sets: source.empty_sets,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn prepared_membership_preserves_set_order_and_rejects_invalid_indices_or_owners() {
        let good = json!({"ops":[{"Fact":{"Boolean":"a"}},{"Fact":{"Boolean":"b"}},{"Combine":["Difference",0,1]}],"ranges":[],"empty_sets":[]});
        let program: Program = serde_json::from_value(good.clone()).unwrap();
        assert!(
            program
                .evaluate(|f| Some(f == &Fact::Boolean("a".into())))
                .unwrap()
        );
        assert!(!program.evaluate(|_| Some(true)).unwrap());
        assert_eq!(serde_json::to_value(&program).unwrap(), good);
        for indices in [[2, 1], [0, 3]] {
            let mut value = good.clone();
            value["ops"][2] = json!({"Combine":["Difference",indices[0],indices[1]]});
            assert!(serde_json::from_value::<Program>(value).is_err());
        }
        for replacement in [
            json!([]),
            json!([{"Fact":{"Boolean":"a"}},{"Fact":{"Boolean":"b"}}]),
        ] {
            let mut value = good.clone();
            value["ops"] = replacement;
            assert!(serde_json::from_value::<Program>(value).is_err());
        }
        let mut value = good;
        value["empty_sets"] = json!([{"asset":"unrelated","index":1,"instances":[]}]);
        assert!(serde_json::from_value::<Program>(value).is_err());
    }
    #[test]
    fn prepared_range_requires_one_exact_owner_and_preserves_signed_bounds() {
        let id = json!({"asset":"range","index":8,"instances":[]});
        let input = json!({"asset":"input","index":3,"instances":[]});
        let range = json!({"input":input,"input_port":0,"minimum":-4,"maximum":2});
        let good =
            json!({"ops":[{"Fact":{"IntegerRange":id}}],"ranges":[[id,range]],"empty_sets":[]});
        let program: Program = serde_json::from_value(good.clone()).unwrap();
        let range = program.ranges.values().next().unwrap();
        assert!(range.contains(-4));
        assert!(!range.contains(-5));
        assert!(!range.contains(3));
        let mut missing = good.clone();
        missing["ranges"] = json!([]);
        assert!(serde_json::from_value::<Program>(missing).is_err());
        let mut duplicate = good.clone();
        duplicate["ranges"]
            .as_array_mut()
            .unwrap()
            .push(good["ranges"][0].clone());
        assert!(serde_json::from_value::<Program>(duplicate).is_err());
        let mut wrong = good;
        wrong["ranges"][0][1]["input_port"] = json!(7);
        assert!(serde_json::from_value::<Program>(wrong).is_err());
    }
}
