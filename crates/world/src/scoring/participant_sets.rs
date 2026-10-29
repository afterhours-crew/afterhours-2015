// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Participant-set membership commands and accessor contract. Sets are keyed
//! by the authored channel port, separately from scalar participant properties.
pub use super::participant_properties::Property as Key;
pub const ADD: u32 = 0x9d36_347b;
pub const REMOVE: u32 = 0xd1fb_f23c;
pub const CONTAINS: u32 = 0xdaea_5fe0;
pub const MAX_SETS: usize = 256;
pub const MAX_MEMBERS: usize = 128;
pub const COUNT: u32 = 0xe6d6_7e59;
pub const MAX_OPERANDS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Channel(Key),
    Node(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Operation {
    Union,
    Intersection,
    Difference,
}

/// Native predicate bind folds authored operands. Difference update callbacks
/// address exactly two operands, so that is the supported difference shape.
pub fn evaluate(
    operation: Operation,
    operands: &[std::collections::BTreeSet<u64>],
) -> Result<std::collections::BTreeSet<u64>, super::Error> {
    if operands.len() > MAX_OPERANDS
        || (operation == Operation::Difference && operands.len() != 2)
        || operands.iter().any(|s| s.len() > MAX_MEMBERS)
    {
        return Err(super::Error::Bound);
    }
    let mut result = operands.first().cloned().unwrap_or_default();
    for operand in operands.iter().skip(1) {
        result = match operation {
            Operation::Union => result.union(operand).copied().collect(),
            Operation::Intersection => result.intersection(operand).copied().collect(),
            Operation::Difference => result.difference(operand).copied().collect(),
        };
        if result.len() > MAX_MEMBERS {
            return Err(super::Error::Bound);
        }
    }
    Ok(result)
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub set: Key,
    pub participant: u64,
    pub member: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[test]
    fn predicates_preserve_set_semantics_and_difference_operand_order() {
        let a = BTreeSet::from([1, 2]);
        let b = BTreeSet::from([2, 3]);
        assert_eq!(
            evaluate(Operation::Union, &[a.clone(), b.clone()]).unwrap(),
            BTreeSet::from([1, 2, 3])
        );
        assert_eq!(
            evaluate(Operation::Intersection, &[a.clone(), b.clone()]).unwrap(),
            BTreeSet::from([2])
        );
        assert_eq!(
            evaluate(Operation::Difference, &[a.clone(), b.clone()]).unwrap(),
            BTreeSet::from([1])
        );
        assert_eq!(
            evaluate(Operation::Difference, &[b, a]).unwrap(),
            BTreeSet::from([3])
        );
        assert!(evaluate(Operation::Union, &[]).unwrap().is_empty());
        assert!(evaluate(Operation::Intersection, &[]).unwrap().is_empty());
        assert!(evaluate(Operation::Difference, &[]).is_err());
        let a = (0..MAX_MEMBERS as u64).collect();
        let b = BTreeSet::from([MAX_MEMBERS as u64]);
        assert!(evaluate(Operation::Union, &[a, b]).is_err());
    }
}
