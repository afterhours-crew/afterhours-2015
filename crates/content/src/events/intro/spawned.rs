// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
pub(super) struct Context<'a> {
    pub mode_nodes: &'a [Node],
    pub mode_fields: &'a [FieldSite],
    pub parent_fields: &'a BTreeMap<Site, usize>,
    pub aliases: &'a BTreeMap<Site, Site>,
    pub bound: &'a BTreeMap<Site, (Site, Binding)>,
    pub rules: &'a Rules,
    pub sequences: &'a crate::sequences::Rules,
    pub lifecycle: &'a lifecycle::Lifecycle,
}

impl Context<'_> {
    fn binding(
        &self,
        g: &mut Graph<Assets>,
        scenes: &mut SceneLookup<'_>,
        target: &Site,
    ) -> Result<Binding, Error> {
        if let Some(serializer) = self.mode_fields.iter().position(|(s, _, _)| s == target) {
            Ok(Binding::GameMode { serializer })
        } else if let Some(&serializer) = self.parent_fields.get(target) {
            Ok(Binding::Participant { serializer })
        } else {
            let (content_key, serializer) = scenes.endpoint(g, target)?;
            Ok(Binding::Scene {
                content_key,
                serializer,
            })
        }
    }
}

pub(super) fn extract(
    g: &mut Graph<Assets>,
    scenes: &mut SceneLookup<'_>,
    c: Context<'_>,
) -> Result<(Vec<Sequence>, Vec<Layout>), Error> {
    let mut aliases = c.aliases.clone();
    aliases.extend(c.bound.iter().map(|(s, (t, _))| (s.clone(), t.clone())));
    let mut table = BTreeMap::new();
    for (site, class, _) in c.mode_fields {
        if class != "StateSequenceChannelReferenceEntityData" {
            continue;
        }
        let channel = &c
            .bound
            .get(site)
            .ok_or_else(|| bad("intro sequence table binding"))?
            .0;
        let object = g.object(channel)?;
        for row in object
            .fields
            .get("Sequences")
            .and_then(Value::as_array)
            .filter(|v| v.len() <= 64)
            .ok_or_else(|| bad("intro sequence table"))?
        {
            let fields = row.as_fields().ok_or_else(|| bad("intro sequence row"))?;
            let mut target = |field| -> Result<Key, Error> {
                let p = fields
                    .get(field)
                    .and_then(Value::as_pointer)
                    .ok_or_else(|| bad("intro sequence entry"))?;
                g.source
                    .target(&channel.key.0, p)?
                    .ok_or_else(|| bad("intro sequence null entry"))
            };
            let id = target("Id")?;
            let sequence = target("Sequence")?;
            if sequence.1 != 0 || table.insert(id, sequence.0).is_some() {
                return Err(bad("intro sequence table duplicate"));
            }
        }
    }
    let managers = g.read(
        scenes
            .roots
            .get(&c.sequences.manager_scene)
            .ok_or_else(|| bad("intro sequence manager scene"))?
            .clone(),
    )?;
    let manager_site = unique(&managers, "StateSequenceManagerEntityData")?;
    let manager = scenes.endpoint(g, manager_site)?;
    let manager_bus = embedded_bus(&managers, manager_site)?;
    let mut sequences = vec![];
    for state in &c.lifecycle.states {
        let id = match &state.configuration {
            lifecycle::StateConfiguration::Sequence { sequence }
            | lifecycle::StateConfiguration::Countdown { sequence, .. } => sequence,
            _ => continue,
        };
        let name = table
            .get(id)
            .ok_or_else(|| bad("intro sequence state missing from table"))?;
        let nodes = g.read(Site::root(name))?;
        let (buses, fields) = crate::sequences::authored::shape(&nodes, c.sequences)
            .map_err(|e| bad(&format!("intro sequence {name}: {e}")))?;
        let spawner = &c
            .mode_fields
            .get(state.serializer)
            .ok_or_else(|| bad("intro sequence state serializer"))?
            .0;
        let targets = links(
            g,
            spawner,
            c.rules.spawner_channels,
            &aliases,
            &mut BTreeSet::new(),
        )?;
        let refs: Vec<_> = fields
            .iter()
            .filter(|(_, _, k)| *k == Kind::RpcReference)
            .map(|(s, c, _)| (s.clone(), c.clone()))
            .collect();
        let bound = bind_targets(g, &refs, &fields[0].0, c.rules, &targets)?;
        let goto = sequence::goto(g, &nodes, &bound, &aliases)?;
        let mut bindings = BTreeMap::new();
        for ((source, _), target) in refs.iter().zip(bound) {
            bindings.insert(source.clone(), c.binding(g, scenes, &target)?);
        }
        let layout = layout(g, name, buses, manager_bus, 0, &fields, &bindings)?;
        sequences.push(Sequence {
            layout,
            manager,
            spawner: Some(state.serializer),
            goto,
            wander: sequence::wander(g, &nodes)?,
        });
    }
    let mut logics = vec![];
    for spawner in &c.lifecycle.bindings.spawners {
        let site = &c
            .mode_nodes
            .iter()
            .find(|n| crate::scoring::events::Identity::from(&n.site) == spawner.identity)
            .ok_or_else(|| bad("intro logic spawner occurrence"))?
            .site;
        let nodes = g.read(Site::root(&spawner.blueprint))?;
        let (buses, fields) = fields(&nodes, c.rules, "ParticipantLogicEntityData")?;
        let targets = links(
            g,
            site,
            c.rules.spawner_channels,
            &aliases,
            &mut BTreeSet::new(),
        )?;
        let refs: Vec<_> = fields
            .iter()
            .filter(|(_, _, k)| *k == Kind::RpcReference)
            .map(|(s, c, _)| (s.clone(), c.clone()))
            .collect();
        let bound = bind_targets(g, &refs, &fields[0].0, c.rules, &targets)?;
        let mut bindings = BTreeMap::new();
        for ((source, _), target) in refs.iter().zip(bound) {
            bindings.insert(source.clone(), c.binding(g, scenes, &target)?);
        }
        logics.push(layout(
            g,
            &spawner.blueprint,
            buses,
            embedded_bus(c.mode_nodes, site)?,
            spawner.occurrence,
            &fields,
            &bindings,
        )?);
    }
    Ok((sequences, logics))
}

fn layout<S: Source>(
    g: &mut Graph<S>,
    name: &str,
    bus_count: u16,
    sub_id: u32,
    occurrence: usize,
    fields: &[FieldSite],
    bindings: &BTreeMap<Site, Binding>,
) -> Result<Layout, Error> {
    Ok(Layout {
        blueprint: name.into(),
        asset: Asset {
            bundle: 0,
            type_id: 0,
            local_index: 0,
        },
        catalog: vec![],
        bus_count,
        sub_id,
        occurrence,
        fields: fields
            .iter()
            .map(|(s, c, k)| {
                Ok(Field {
                    kind: *k,
                    class: c.clone(),
                    asset: s.key.0.clone(),
                    object: s.key.1,
                    binding: bindings.get(s).copied(),
                    mirrors: mirrored(g, s)?,
                })
            })
            .collect::<Result<_, Error>>()?,
    })
}
