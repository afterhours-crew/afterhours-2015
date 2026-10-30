use super::*;
use crate::graph::tests::{Memory, local, ptr};

fn fixture() -> (Vec<Node>, Vec<scenes::Instance>) {
    let mut source = Memory::default();
    source.put(
        "scene",
        0,
        "SubWorldData",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array(vec![local(1), local(2)])),
        ],
    );
    for i in [1, 2] {
        source.put(
            "scene",
            i,
            "LogicPrefabReferenceObjectData",
            &[("Blueprint", ptr("p", 0))],
        );
    }
    source.put(
        "p",
        0,
        "LogicPrefabBlueprint",
        &[
            ("Flags", Value::UInt16(1)),
            ("Objects", Value::Array(vec![local(1)])),
        ],
    );
    source.put("p", 1, "StateSequenceManagerEntityData", &[]);
    let nodes = Graph::new(&mut source).read(Site::root("scene")).unwrap();
    let instances = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| scenes::Instance {
            asset: n.site.key.0.clone(),
            index: n.site.key.1,
            serializer: (n.object.class == "StateSequenceManagerEntityData").then_some(i),
        })
        .collect();
    (nodes, instances)
}

#[test]
fn prepared_scene_preserves_occurrence_order_and_repeated_prefab_buses() {
    let (nodes, instances) = fixture();
    let prepared = Prepared::new(&nodes, &instances, 42).unwrap();
    let managers: Vec<_> = nodes
        .iter()
        .filter(|n| n.object.class == "StateSequenceManagerEntityData")
        .collect();
    for (i, manager) in managers.iter().enumerate() {
        assert_eq!(
            prepared.embedded_bus(&manager.site).unwrap(),
            (i + 2) as u32
        );
        assert_eq!(
            prepared.embedded_bus(&manager.site).unwrap(),
            embedded_bus(&nodes, &manager.site).unwrap()
        );
        let ordinal = nodes.iter().position(|n| n.site == manager.site).unwrap();
        assert_eq!(prepared.endpoints[&manager.site], (42, ordinal));
    }
    let mut foreign = managers[0].site.clone();
    foreign.parents[0].reference.1 = 99;
    assert!(prepared.embedded_bus(&foreign).is_err());
    let mut changed = instances.clone();
    changed[3].index += 1;
    assert!(Prepared::new(&nodes, &changed, 42).is_err());
    assert!(Prepared::new(&nodes, &instances[..instances.len() - 1], 42).is_err());
}

#[test]
fn cache_reuses_validated_tables_only_with_the_same_root_and_rules() {
    let (nodes, instances) = fixture();
    let prepared = Arc::new(Prepared::new(&nodes, &instances, 42).unwrap());
    let root = Site::root("scene");
    let rules = crate::BuildIdentity::supported().scenes;
    let mut cache = Cache::default();
    cache.insert(root.clone(), rules.clone(), prepared.clone());
    assert!(Arc::ptr_eq(&cache.get(&root, &rules).unwrap(), &prepared));
    let mut other_rules = rules.clone();
    other_rules.level = "other".into();
    assert!(cache.get(&root, &other_rules).is_none());
    let mut other_root = root.clone();
    other_root.parents.push(Frame {
        bus: ("parent".into(), 0),
        reference: ("parent".into(), 1),
    });
    assert!(cache.get(&other_root, &rules).is_none());
    // A separate installation cannot see another installation's entry.
    assert!(Cache::default().get(&root, &rules).is_none());
    for i in 0..MAX_CACHED_SCENES + 2 {
        cache.insert(
            Site::root(&format!("scene-{i}")),
            rules.clone(),
            prepared.clone(),
        );
    }
    assert_eq!(cache.entries.len(), MAX_CACHED_SCENES);
    assert!(cache.rows <= MAX_CACHED_ROWS);
    assert_eq!(
        cache.rows,
        cache.entries.values().map(|(_, p)| p.rows()).sum::<usize>()
    );
}

#[test]
fn ambiguous_or_invalid_buses_stay_errors_without_rejecting_endpoint_only_reads() {
    let (mut nodes, mut instances) = fixture();
    let manager = nodes
        .iter()
        .find(|n| n.object.class == "StateSequenceManagerEntityData")
        .unwrap()
        .site
        .clone();
    nodes.push(nodes[2].clone());
    instances.push(instances[2].clone());
    let prepared = Prepared::new(&nodes, &instances, 42).unwrap();
    assert!(prepared.embedded_bus(&manager).is_err());
    nodes[0].object.fields.0.clear();
    let prepared = Prepared::new(&nodes, &instances, 42).unwrap();
    assert!(prepared.endpoints.contains_key(&manager));
    assert!(prepared.embedded_bus(&manager).is_err());
}
