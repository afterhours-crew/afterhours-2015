// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bind the supported distance branch by authored connections, not EBX indices.
//! Other driver statistics remain outside this compiler's scope.
use super::{boolean, field, scalar};
use crate::{
    Error,
    assets::AssetIndex,
    graph::{Graph, Site, Source, bad, bits32},
    participant_logic::{Rules, shape},
};
use nfs_frostbite::ebx::{Fields, Value};
use nfs_world::replication::entity::Kind;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Binding {
    pub kilometres_serializer: usize,
    pub miles_serializer: usize,
    pub channel_serializer: usize,
    pub distance_property: u32,
    pub period_seconds: f32,
    pub metres_per_mile: f32,
}

const STAT_VALUE: u32 = 4062837084;
const CACHE_IN: u32 = 1658462601;
const CACHE_OUT: u32 = 993810272;
const OUT: u32 = 193453899;
const A: u32 = 193450866;
const B: u32 = 193450867;
const LOAD: u32 = 2089019715;
const STARTED: u32 = (-1635202463_i32) as u32;
const ACCUMULATE: u32 = 1034803477;
const ACCUMULATED: u32 = (-1669943495_i32) as u32;
const STAT_UPDATE: u32 = (-2023990426_i32) as u32;
const CHANGED: u32 = 613089920;
const IN: u32 = 5862146;

fn enum_is(value: &Value, expected: &str) -> bool {
    matches!(value, Value::Enum { name: Some(name), .. } if name == expected)
}

struct Trace<'a, 'b, S: Source> {
    graph: &'a mut Graph<'b, S>,
    inputs: BTreeMap<Site, BTreeSet<u32>>,
}
impl<S: Source> Trace<'_, '_, S> {
    fn node(&mut self, site: &Site, class: &str) -> Result<Fields, Error> {
        let object = self.graph.object(site)?;
        if object.class != class {
            return Err(bad("distance node class"));
        }
        self.inputs.entry(site.clone()).or_default();
        Ok(object.fields)
    }
    fn input(
        &mut self,
        target: &Site,
        input: u32,
        class: &str,
        output: u32,
    ) -> Result<Site, Error> {
        let sources = self.graph.property_sources(target, input)?;
        let [source] = sources.as_slice() else {
            return Err(bad("distance input ambiguity"));
        };
        if source.port != output || source.site.bus != target.bus {
            return Err(bad("distance output port or bus"));
        }
        self.inputs.entry(target.clone()).or_default().insert(input);
        let site = source.site.clone();
        self.node(&site, class)?;
        Ok(site)
    }
    fn op(&mut self, site: &Site, class: &str, operator: &str) -> Result<(), Error> {
        let fields = self.node(site, class)?;
        let values = field(&fields, "Operators")?
            .as_array()
            .ok_or_else(|| bad("distance operators"))?;
        if values.len() != 1 || !enum_is(&values[0], operator) {
            return Err(bad("distance arithmetic operation"));
        }
        Ok(())
    }
    fn vector_op(&mut self, site: &Site, operator: &str) -> Result<(), Error> {
        let fields = self.node(site, "VectorMathOpEntityData")?;
        if !enum_is(field(&fields, "MathOperator")?, operator) {
            return Err(bad("distance vector operation"));
        }
        Ok(())
    }
    fn cache(&mut self, site: &Site, class: &str, initial: bool) -> Result<(), Error> {
        let fields = self.node(site, class)?;
        if boolean(&fields, "LoadOnCreate")? != initial {
            return Err(bad("distance cache initialization"));
        }
        Ok(())
    }
    fn exact_inputs(&mut self) -> Result<(), Error> {
        for (site, expected) in &self.inputs {
            let ports = self.graph.input_ports(site)?;
            if ports.len() != expected.len()
                || ports.into_iter().collect::<BTreeSet<_>>() != *expected
            {
                return Err(bad("unexpected distance property input"));
            }
            if !expected.is_empty()
                && super::realms::Listeners::read(&self.graph.object(site)?)?
                    .is_none_or(|realm| !realm.properties.includes_server())
            {
                return Err(bad("distance property listener realm"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Event {
    source: Site,
    output: u32,
    target: Site,
    input: u32,
}

fn events<S: Source>(graph: &mut Graph<'_, S>, root: &Site) -> Result<Vec<Event>, Error> {
    let object = graph.object(root)?;
    let rows = field(&object.fields, "EventConnections")?
        .as_array()
        .ok_or_else(|| bad("distance event table"))?;
    if rows.len() > 4096 {
        return Err(bad("distance event bound"));
    }
    let mut result = Vec::new();
    for row in rows {
        let fields = row.as_fields().ok_or_else(|| bad("distance event row"))?;
        // Keep all targets: a non-server delivery into a modeled node fails
        // validation below rather than disappearing from the inventory.
        let mut endpoint = |name| -> Result<Site, Error> {
            let pointer = field(fields, name)?
                .as_pointer()
                .ok_or_else(|| bad("distance event pointer"))?;
            let key = graph
                .source
                .target(&root.key.0, pointer)?
                .ok_or_else(|| bad("distance null event pointer"))?;
            Ok(Site {
                key,
                ..root.clone()
            })
        };
        let source = endpoint("Source")?;
        let target = endpoint("Target")?;
        let id = |name| -> Result<u32, Error> {
            let fields = field(fields, name)?
                .as_fields()
                .ok_or_else(|| bad("distance event ID"))?;
            bits32(field(fields, "Id")?)
        };
        let input = if matches!(field(fields, "TargetType")?, Value::Enum { value: 3, name: Some(name) }
            if name == "EventConnectionTargetType_Server")
        {
            id("TargetEvent")?
        } else {
            // Preserve non-server rows with a value outside this contract.
            u32::MAX
        };
        result.push(Event {
            source,
            output: id("SourceEvent")?,
            target,
            input,
        });
    }
    Ok(result)
}

fn event_source(events: &[Event], target: &Site, input: u32, output: u32) -> Result<Site, Error> {
    let edges: Vec<_> = events
        .iter()
        .filter(|e| e.target == *target && e.input == input)
        .collect();
    match edges.as_slice() {
        [edge] if edge.output == output => Ok(edge.source.clone()),
        _ => Err(bad("distance event source ambiguity")),
    }
}

pub(crate) fn read<S: Source>(
    source: &mut S,
    blueprint: &str,
    rules: &Rules,
) -> Result<Binding, Error> {
    if blueprint.is_empty() || blueprint.len() > 512 {
        return Err(bad("distance blueprint bound"));
    }
    let root = Site::root(blueprint);
    let mut graph = Graph::new(source);
    let nodes = graph.read(root.clone())?;
    let node_sites: BTreeSet<_> = nodes.iter().map(|n| n.site.clone()).collect();
    let (_, fields) = shape(&nodes, rules)?;
    let mut publishers = BTreeMap::new();
    for node in &nodes {
        if node.object.class != "OnlineStatUpdateEntityData" {
            continue;
        }
        let key = graph.target_field(&node.site, "StatAsset")?;
        let asset = graph.source.object(&key)?;
        let name = field(&asset.fields, "StatName")?
            .as_str()
            .ok_or_else(|| bad("distance stat name"))?;
        if !["TotalDistanceDrivenKm", "TotalDistanceDriven"].contains(&name) {
            continue;
        }
        if asset.class != "OnlineStatAsset"
            || !enum_is(
                field(&asset.fields, "StatOperation")?,
                "EOnlineStatOperation_Replace",
            )
            || node.site.bus != root.bus
            || !node.site.parents.is_empty()
            || publishers
                .insert(name.to_owned(), node.site.clone())
                .is_some()
        {
            return Err(bad("distance publisher definition"));
        }
    }
    let km = publishers
        .remove("TotalDistanceDrivenKm")
        .ok_or_else(|| bad("missing kilometre publisher"))?;
    let miles = publishers
        .remove("TotalDistanceDriven")
        .ok_or_else(|| bad("missing mile publisher"))?;
    let event_edges = events(&mut graph, &root)?;
    let mut t = Trace {
        graph: &mut graph,
        inputs: BTreeMap::new(),
    };
    let km_divide = t.input(&km, STAT_VALUE, "MathIntOpEntityData", OUT)?;
    t.op(&km_divide, "MathIntOpEntityData", "IntMathOp_Divide")?;
    let total_cache = t.input(&km_divide, A, "IntCacheEntityData", CACHE_OUT)?;
    let metric = t.input(&km_divide, B, "IntEntityData", 225375086)?;
    if field(&t.node(&metric, "IntEntityData")?, "DefaultValue")? != &Value::Int32(1000) {
        return Err(bad("distance metric divisor"));
    }
    let mile_cast = t.input(&miles, STAT_VALUE, "PropertyCastEntityData", 245372264)?;
    let mile_divide = t.input(&mile_cast, 3123792190, "MathOpEntityData", OUT)?;
    t.op(&mile_divide, "MathOpEntityData", "MathOp_Divide")?;
    let total_cast = t.input(&mile_divide, A, "PropertyCastEntityData", 936870923)?;
    if t.input(&total_cast, 5685821, "IntCacheEntityData", CACHE_OUT)? != total_cache {
        return Err(bad("distance shared total"));
    }
    let imperial = t.input(&mile_divide, B, "FloatEntityData", 225375086)?;
    let metres_per_mile = scalar(&t.node(&imperial, "FloatEntityData")?, "DefaultValue")?;
    if metres_per_mile < 1. {
        return Err(bad("distance imperial divisor"));
    }

    let sum = t.input(&total_cache, CACHE_IN, "MathIntOpEntityData", OUT)?;
    t.op(&sum, "MathIntOpEntityData", "IntMathOp_Add")?;
    let initial_cache = t.input(&sum, A, "IntCacheEntityData", CACHE_OUT)?;
    let accumulated_cast = t.input(&sum, B, "PropertyCastEntityData", 245372264)?;
    let property = t.input(
        &initial_cache,
        CACHE_IN,
        "IntParticipantPropertyAccessorEntityData",
        731416433,
    )?;
    if t.input(&property, 225375086, "IntCacheEntityData", CACHE_OUT)? != total_cache {
        return Err(bad("distance property writeback"));
    }
    for cache in [&initial_cache, &total_cache] {
        t.cache(cache, "IntCacheEntityData", false)?;
    }
    let accumulator = t.input(
        &accumulated_cast,
        3123792190,
        "FloatAccumulatorEntityData",
        2895736442,
    )?;
    let length = t.input(
        &accumulator,
        214522259,
        "VectorMathOpEntityData",
        2595815478,
    )?;
    t.vector_op(&length, "VectorMathOp_Length")?;
    let builder = t.input(&length, 2412549647, "Vec3BuilderEntityData", OUT)?;
    let split = t.input(&builder, 177661, "Vec3SplitterEntityData", 177661)?;
    if t.input(&builder, 177663, "Vec3SplitterEntityData", 177663)? != split {
        return Err(bad("distance vector components"));
    }
    let conditional = t.input(&builder, 177660, "ConditionalFloatEntityData", 2895736442)?;
    if t.input(&conditional, 2915912540, "Vec3SplitterEntityData", 177660)? != split
        || scalar(
            &t.node(&conditional, "ConditionalFloatEntityData")?,
            "ValueIfTrue",
        )? != 0.
    {
        return Err(bad("distance vertical conditional"));
    }
    let flags = t.input(&conditional, 1800624758, "OrEntityData", OUT)?;
    let flag_config = super::boolean_fold::compile(&t.graph.object(&flags)?)?;
    if flag_config.all || flag_config.input_count != 2 {
        return Err(bad("distance flag count"));
    }
    let physics = t.input(
        &flags,
        193450867,
        "RaceVehiclePhysicsQueryEntityData",
        0x4e559cf1,
    )?;
    if t.input(
        &flags,
        193450864,
        "RaceVehiclePhysicsQueryEntityData",
        0x3c230e35,
    )? != physics
    {
        return Err(bad("distance flags owner"));
    }
    let subtract = t.input(&split, IN, "VectorMathOpEntityData", 976788535)?;
    t.vector_op(&subtract, "VectorMathOp_Subtract")?;
    let current = t.input(&subtract, 2412549647, "Vector3CacheEntityData", CACHE_OUT)?;
    let previous = t.input(&subtract, 2301908460, "Vector3CacheEntityData", CACHE_OUT)?;
    if current == previous
        || t.input(&previous, CACHE_IN, "Vector3CacheEntityData", CACHE_OUT)? != current
    {
        return Err(bad("distance previous-position cache"));
    }
    for cache in [&current, &previous] {
        t.cache(cache, "Vector3CacheEntityData", true)?;
    }
    let transform = t.input(&current, CACHE_IN, "TransformSplitterEntityData", 227190399)?;
    if t.input(
        &transform,
        2270319721,
        "RaceVehiclePhysicsQueryEntityData",
        2270319721,
    )? != physics
    {
        return Err(bad("distance position owner"));
    }

    let delay = event_source(&event_edges, &accumulator, ACCUMULATE, OUT)?;
    let delay_fields = t.node(&delay, "DelayEntityData")?;
    let period_seconds = scalar(&delay_fields, "Delay")?;
    if period_seconds <= 0.
        || boolean(&delay_fields, "AutoStart")?
        || boolean(&delay_fields, "RunOnce")?
        || !boolean(&delay_fields, "RemoveDuplicateEvents")?
        || !enum_is(
            field(&delay_fields, "TimeDeltaType")?,
            "TimeDeltaType_World",
        )
    {
        return Err(bad("distance sampling timer"));
    }
    let owner = event_source(&event_edges, &initial_cache, LOAD, STARTED)?;
    t.node(&owner, "ParticipantLogicEntityData")?;
    let km_changed = event_source(&event_edges, &km, STAT_UPDATE, CHANGED)?;
    let mile_changed = event_source(&event_edges, &miles, STAT_UPDATE, CHANGED)?;
    for (changed, expected, class, output) in [
        (&km_changed, &km_divide, "MathIntOpEntityData", OUT),
        (
            &mile_changed,
            &mile_cast,
            "PropertyCastEntityData",
            245372264,
        ),
    ] {
        let fields = t.node(changed, "PropertyChangedEntityData")?;
        if boolean(&fields, "AlwaysFire")?
            || boolean(&fields, "SuppressFirstChange")?
            || !boolean(&fields, "Enabled")?
            || t.input(changed, 910586961, class, output)? != *expected
        {
            return Err(bad("distance change publication"));
        }
    }
    let mut expected = Vec::new();
    let mut edge = |source: &Site, output, target: &Site, input| {
        expected.push(Event {
            source: source.clone(),
            output,
            target: target.clone(),
            input,
        })
    };
    // Synchronous order: current position, accumulate, previous position, rearm.
    edge(&delay, OUT, &current, LOAD);
    edge(&delay, OUT, &accumulator, ACCUMULATE);
    edge(&delay, OUT, &previous, LOAD);
    edge(&delay, OUT, &delay, IN);
    edge(&owner, STARTED, &initial_cache, LOAD);
    edge(&owner, STARTED, &current, LOAD);
    edge(&owner, STARTED, &previous, LOAD);
    edge(&owner, STARTED, &delay, IN);
    edge(&accumulator, ACCUMULATED, &total_cache, LOAD);
    edge(&km_changed, CHANGED, &km, STAT_UPDATE);
    edge(&mile_changed, CHANGED, &miles, STAT_UPDATE);
    let one_link =
        |t: &mut Trace<'_, '_, S>, site: &Site, port, target_port, class| -> Result<Site, Error> {
            let links = t.graph.link_targets(site, port)?;
            let [link] = links.as_slice() else {
                return Err(bad("distance owner link ambiguity"));
            };
            if link.port != target_port {
                return Err(bad("distance owner link port"));
            }
            let target = link.site.clone();
            t.node(&target, class)?;
            Ok(target)
        };
    let participant = one_link(
        &mut t,
        &property,
        3062566331,
        0,
        "ParticipantLogicParticipantProviderEntityData",
    )?;
    for publisher in [&km, &miles] {
        if one_link(
            &mut t,
            publisher,
            3062566331,
            0,
            "ParticipantLogicParticipantProviderEntityData",
        )? != participant
        {
            return Err(bad("distance publisher owner"));
        }
    }
    if one_link(
        &mut t,
        &participant,
        560080367,
        0,
        "ParticipantLogicEntityData",
    )? != owner
    {
        return Err(bad("distance participant root"));
    }
    let controllable = one_link(
        &mut t,
        &physics,
        4024553726,
        0,
        "ParticipantLogicControllableProviderEntityData",
    )?;
    if one_link(
        &mut t,
        &controllable,
        3867345844,
        0,
        "ParticipantLogicEntityData",
    )? != owner
    {
        return Err(bad("distance controllable root"));
    }
    let property_links = t.graph.link_targets(&property, 2249073098)?;
    let [property_link] = property_links.as_slice() else {
        return Err(bad("distance property link ambiguity"));
    };
    let channel = property_link.site.clone();
    t.node(&channel, "LinkedSchematicChannelReferenceEntityData")?;
    // Channel reference may also carry unrelated properties used by debug UI;
    // its consumers are outside the modeled scalar calculation.
    t.inputs.remove(&channel);
    if one_link(
        &mut t,
        &owner,
        440997395,
        0,
        "LinkedSchematicChannelReferenceEntityData",
    )? != channel
    {
        return Err(bad("distance channel owner"));
    }
    t.inputs.remove(&channel);
    let modeled: BTreeSet<_> = t
        .inputs
        .keys()
        .cloned()
        .chain([km.clone(), miles.clone()])
        .collect();
    let actual: Vec<_> = event_edges
        .into_iter()
        .filter(|e| modeled.contains(&e.target))
        .collect();
    if actual != expected {
        return Err(bad("distance event order or additional trigger"));
    }
    for event in &actual {
        if super::realms::Listeners::read(&t.graph.object(&event.target)?)?
            .is_none_or(|realm| !realm.events.includes_server())
        {
            return Err(bad("distance event listener realm"));
        }
    }

    t.exact_inputs()?;
    if t.inputs.keys().any(|site| !node_sites.contains(site)) || !node_sites.contains(&channel) {
        return Err(bad("distance node outside runtime composition"));
    }
    let serializer = |site: &Site, kind| {
        fields
            .iter()
            .position(|f| f == &(site.clone(), kind))
            .ok_or_else(|| bad("distance serializer binding"))
    };
    Ok(Binding {
        kilometres_serializer: serializer(&km, Kind::RpcVariant)?,
        miles_serializer: serializer(&miles, Kind::RpcVariant)?,
        channel_serializer: serializer(&channel, Kind::RpcReference)?,
        distance_property: property_link.port,
        period_seconds,
        metres_per_mile,
    })
}

impl AssetIndex {
    pub fn driving_distance_binding(&mut self, blueprint: &str) -> Result<Binding, Error> {
        let rules = Rules::from_json(&self.identity.participant_logic.to_json())?;
        read(&mut self.assets, blueprint, &rules)
    }
}

#[cfg(test)]
mod tests;
