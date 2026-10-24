// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Native BankTransaction defaults and explicit bank/provider bindings.
use super::{
    events::Identity,
    field,
    participant_properties::{Property, identifier},
};
use crate::{
    Error,
    graph::{Graph, Site, Source, bad},
};
use nfs_frostbite::ebx::{Object, Value};
use nfs_world::scoring::bank::Kind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub identity: Identity,
    pub bank: Property,
    pub provider: Identity,
    pub kind: Kind,
    pub amount: i32,
}

pub fn configuration(object: &Object) -> Result<(Kind, i32), Error> {
    if object.class != "BankTransactionEntityData" {
        return Err(bad("bank transaction class"));
    }
    let kind = match field(&object.fields, "TransactionType")? {
        Value::Enum {
            value: 0,
            name: Some(n),
        } if n == "Deposit_TransactionType" => Kind::Deposit,
        Value::Enum {
            value: 1,
            name: Some(n),
        } if n == "Withdraw_TransactionType" => Kind::Withdraw,
        _ => return Err(bad("bank transaction type")),
    };
    let Value::Int32(amount) = field(&object.fields, "Amount")? else {
        return Err(bad("bank transaction amount"));
    };
    Ok((kind, *amount))
}

pub(crate) fn read<S: Source>(source: &mut S, blueprint: &str) -> Result<Vec<Node>, Error> {
    let mut graph = Graph::new(source);
    let mut rows = Vec::new();
    for node in graph.read(Site::root(blueprint))? {
        if node.object.class != "BankTransactionEntityData" {
            continue;
        }
        let (kind, amount) = configuration(&node.object)?;
        let links = graph.links(&node.site, 0xb68b_0dbb)?;
        let [provider] = links.as_slice() else {
            return Err(bad("bank participant provider"));
        };
        if provider.port != 0
            || graph.object(&provider.site)?.class
                != "ParticipantLogicParticipantProviderEntityData"
        {
            return Err(bad("bank provider is not the owning participant"));
        }
        rows.push(Node {
            identity: Identity::from(&node.site),
            bank: identifier(&mut graph, &node.site, 0x7c80_a863)?,
            provider: Identity::from(&provider.site),
            kind,
            amount,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::tests::{Memory, edge, local, ptr};
    use nfs_frostbite::ebx::Fields;

    #[test]
    fn transaction_uses_the_declared_bank_and_own_provider_and_signed_amount() {
        let mut m = Memory::default();
        m.put(
            "r",
            0,
            "ParticipantLogicBlueprint",
            &[
                ("Objects", Value::Array(vec![local(1), local(2), local(3)])),
                (
                    "LinkConnections",
                    Value::Array(vec![
                        edge(local(1), 0x7c80_a863, local(2), 77),
                        edge(local(1), 0xb68b_0dbb, local(3), 0),
                    ]),
                ),
            ],
        );
        m.put(
            "r",
            1,
            "BankTransactionEntityData",
            &[
                ("Amount", Value::Int32(-100)),
                (
                    "TransactionType",
                    Value::Enum {
                        value: 0,
                        name: Some("Deposit_TransactionType".into()),
                    },
                ),
            ],
        );
        m.put(
            "r",
            2,
            "LinkedSchematicChannelReferenceEntityData",
            &[("ChannelAsset", ptr("c", 0))],
        );
        m.put("r", 3, "ParticipantLogicParticipantProviderEntityData", &[]);
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[(
                "Links",
                Value::Array(vec![Value::Struct(Fields(vec![(
                    "Id".into(),
                    Value::UInt32(77),
                )]))]),
            )],
        );
        let row = read(&mut m, "r").unwrap().remove(0);
        assert_eq!(
            row.bank,
            Property {
                channel: "c".into(),
                port: 77
            }
        );
        assert_eq!(row.provider.index, 3);
        assert_eq!((row.kind, row.amount), (Kind::Deposit, -100));
        m.put(
            "r",
            1,
            "BankTransactionEntityData",
            &[
                ("Amount", Value::Int32(100)),
                (
                    "TransactionType",
                    Value::Enum {
                        value: 1,
                        name: Some("Withdraw_TransactionType".into()),
                    },
                ),
            ],
        );
        assert_eq!(read(&mut m, "r").unwrap()[0].kind, Kind::Withdraw);
        m.put(
            "c",
            0,
            "SchematicChannelAsset",
            &[("Links", Value::Array(vec![]))],
        );
        assert!(read(&mut m, "r").is_err());
    }
}
