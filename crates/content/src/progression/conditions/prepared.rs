// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    ops: Vec<Op>,
}
impl TryFrom<Source> for Condition {
    type Error = Error;
    fn try_from(source: Source) -> Result<Self, Error> {
        if source.ops.is_empty() || source.ops.len() > MAX_NODES {
            return Err(bad("prepared mission condition bound"));
        }
        let mut depths: Vec<usize> = Vec::new();
        let mut used = BTreeSet::new();
        for (i, op) in source.ops.iter().enumerate() {
            let inputs = match op {
                Op::Fact(fact) => {
                    if matches!(
                        fact,
                        Fact::Active(0) | Fact::Completed(0) | Fact::PositiveInt(0)
                    ) {
                        return Err(bad("prepared mission fact identity"));
                    }
                    depths.push(1);
                    continue;
                }
                Op::Not(input) => std::slice::from_ref(input),
                Op::All(inputs) | Op::Any(inputs) => inputs.as_slice(),
            };
            if inputs.is_empty()
                || inputs.len() >= MAX_NODES
                || inputs.iter().any(|input| *input >= i)
                || inputs.iter().collect::<BTreeSet<_>>().len() != inputs.len()
            {
                return Err(bad("prepared mission condition input"));
            }
            let depth = 1 + inputs.iter().map(|input| depths[*input]).max().unwrap_or(0);
            if depth > MAX_DEPTH {
                return Err(bad("prepared mission condition depth"));
            }
            depths.push(depth);
            used.extend(inputs.iter().copied());
        }
        if used != (0..source.ops.len() - 1).collect() {
            return Err(bad("prepared mission condition unreachable input"));
        }
        Ok(Self { ops: source.ops })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn restored_condition_requires_every_fact_and_rejects_malformed_postorder() {
        let value = json!({"ops":[{"Fact":{"Active":12}},{"Fact":{"MessageRead":{"hash":81,"manager":3}}},{"Any":[0,1]}]});
        let condition: Condition = serde_json::from_value(value.clone()).unwrap();
        assert!(
            condition
                .evaluate(|f| match f {
                    Fact::Active(12) => Some(true),
                    _ => None,
                })
                .is_err()
        );
        assert!(
            condition
                .evaluate(|f| Some(matches!(f, Fact::Active(_))))
                .unwrap()
        );
        assert_eq!(serde_json::to_value(condition).unwrap(), value);
        for bad_ops in [
            json!([]),
            json!([{"Fact":{"Completed":0}}]),
            json!([{"Not":0}]),
            json!([{"All":[]}]),
            json!([{"Fact":{"Active":1}},{"Any":[0,0]}]),
            json!([{"Fact":{"Active":1}},{"Fact":{"Active":2}}]),
        ] {
            assert!(serde_json::from_value::<Condition>(json!({"ops":bad_ops})).is_err());
        }
        let mut too_deep = vec![json!({"Fact":{"Active":1}})];
        for i in 0..MAX_DEPTH {
            too_deep.push(json!({"Not":i}));
        }
        assert!(serde_json::from_value::<Condition>(json!({"ops":too_deep})).is_err());
    }
}
