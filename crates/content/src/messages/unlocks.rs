// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Notification projections of committed category and purchasable-part grants.
//! This compiles selected senders, not REP eligibility, item awards or debug
//! grant events. The grant policy remains the authority for actual rewards.
use super::{conditional::*, *};
use nfs_frostbite::ebx::Object;
use std::{collections::BTreeSet, time::Duration};

const LOGIC: &str = "gameplay/progression/unlocksparticipantlogic";
const INVENTORY: u32 = (-1_682_645_067i32) as u32;
const INVENTORY_TARGET: u32 = 1_634_186_606;
const MANAGER_LINK: u32 = (-177_843_075i32) as u32;
const MANAGER_TARGET: u32 = 89_767_915;
const PART: u32 = 307_459_119;
const UNLOCKED_ID: u32 = (-210_769_687i32) as u32;
const UNLOCKED: u32 = 2_134_368_743;
const FILTER_ID: u32 = (-1_123_926_451i32) as u32;
const FILTER_IN: u32 = (-1_797_051_410i32) as u32;
const FILTER_OUT: u32 = 1_415_412_183;
const CATEGORY_SUCCESS: u32 = (-1_157_191_321i32) as u32;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct Notification {
    pub hash: u32,
    pub manager: usize,
    pub delay: Duration,
    pub trigger: Trigger,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// One success event from the category transaction, not one per mask bit.
    Categories { mask: u32 },
    /// The listener's current ItemData.Id is sampled when the sender fires.
    /// A passing event replaces the pending delay. A later filtered-out change
    /// still updates that property, without starting or replacing the timer.
    Part { include: bool, ids: BTreeSet<u32> },
}

impl AssetIndex {
    /// Compile every explicitly selected category/part notification. Unsupported
    /// attachment types, extra inputs and unresolved ownership fail this call.
    /// Unselected wrap/decal awards are outside this projection.
    pub fn unlock_messages(
        &mut self,
        selected: &BTreeSet<String>,
    ) -> Result<Vec<Notification>, Error> {
        check(
            !selected.is_empty()
                && selected.len() <= 64
                && selected.iter().all(|s| !s.is_empty() && s.len() <= 512),
            "unlock message selection bound",
        )?;
        let bindings = self
            .scene_bindings(GAMEPLAY)?
            .into_iter()
            .filter(|b| b.class == MANAGER)
            .map(|b| ((b.asset.to_lowercase(), b.object), b.serializer))
            .collect();
        let level = self.identity.scenes.level.clone();
        let progression = self.identity.progression.scene.clone();
        let startup = self.startup_entries(&level)?;
        let mut graph = Graph::new(&mut self.assets);
        let roots = crate::graph::scenes::scene_roots(&mut graph, &level, &startup)?;
        let nodes = graph.read(Site::root(LOGIC))?;
        let mut world = Vec::new();
        for scene in [GAMEPLAY, &progression] {
            world.extend(
                graph.read(
                    roots
                        .get(scene)
                        .ok_or_else(|| bad("unlock message startup scene"))?
                        .clone(),
                )?,
            );
        }
        read(&mut graph, &nodes, &world, &bindings, selected)
    }
}

fn read<S: Source>(
    g: &mut Graph<S>,
    nodes: &[Node],
    world: &[Node],
    bindings: &BTreeMap<Key, usize>,
    selected: &BTreeSet<String>,
) -> Result<Vec<Notification>, Error> {
    let mut found = BTreeMap::new();
    let mut hashes = BTreeSet::new();
    let mut delays = BTreeSet::new();
    for n in nodes
        .iter()
        .filter(|n| n.object.class == "DynamicMessageSenderEntityData")
    {
        let asset = g.target_field(&n.site, "MessageAsset")?;
        if !selected.contains(&asset.0) {
            continue;
        }
        listener(&n.object, true, false)?;
        let message = g.source.object(&asset)?;
        check(message.class == "MessageAsset", "unlock message asset")?;
        flag(&message, "Persisted", true)?;
        events(g, &n.site, &[SEND])?;
        links(g, &n.site, &[CONTEXT, MANAGER_LINK])?;
        let context = one_link(g, &n.site, CONTEXT, 0)?;
        class(
            g,
            &context.site,
            "ParticipantLogicParticipantProviderEntityData",
        )?;
        let channel = one_link(g, &n.site, MANAGER_LINK, MANAGER_TARGET)?;
        check(
            g.object(&channel.site)?.class == "LinkedSchematicChannelReferenceEntityData",
            "unlock message channel",
        )?;
        let delay = event(g, &n.site, SEND, OUT)?;
        check(
            delays.insert(delay.clone()),
            "shared unlock notification delay",
        )?;
        let seconds = delay_seconds(g, &delay)?;
        let sources = g.event_sources(&delay, IN)?;
        let [(source, output)] = sources.as_slice() else {
            return Err(bad("unlock notification source"));
        };
        let source_object = g.object(source)?;
        let attachments = array(&message, "Attachments")?;
        let parameters = array(&n.object, "AttachmentInputParameters")?;
        let trigger = match (source_object.class.as_str(), *output) {
            ("UnlockItemCategoryEntityData", CATEGORY_SUCCESS) => {
                check(
                    attachments.is_empty() && parameters.is_empty(),
                    "category notification attachments",
                )?;
                ports(g, &n.site, &[])?;
                ports(g, source, &[])?;
                ownership(g, source, &context, &channel)?;
                let mut mask = 0;
                for value in array(&source_object, "Categories")? {
                    let Value::Enum {
                        value,
                        name: Some(name),
                    } = value
                    else {
                        return Err(bad("category notification enum"));
                    };
                    let bit = *value as u32;
                    check(
                        *value > 0
                            && bit.is_power_of_two()
                            && name.starts_with("CategoryUnlockMask_")
                            && mask & bit == 0,
                        "category notification mask",
                    )?;
                    mask |= bit;
                }
                check(mask != 0, "empty category notification")?;
                Trigger::Categories { mask }
            }
            ("CarPartFilterEntityData", FILTER_OUT) => {
                part_attachment(g, &asset, attachments, parameters)?;
                ports(g, &n.site, &[PART])?;
                ports(g, source, &[FILTER_ID])?;
                events(g, source, &[FILTER_IN])?;
                listener(&source_object, true, true)?;
                ownership(g, source, &context, &channel)?;
                let part_listener = event(g, source, FILTER_IN, UNLOCKED)?;
                check(
                    g.object(&part_listener)?.class == "CarPartListenerEntityData",
                    "part notification listener",
                )?;
                ports(g, &part_listener, &[])?;
                events(g, &part_listener, &[])?;
                ownership(g, &part_listener, &context, &channel)?;
                check(
                    property(g, &n.site, PART, UNLOCKED_ID)? == part_listener
                        && property(g, source, FILTER_ID, UNLOCKED_ID)? == part_listener,
                    "part notification property and event source disagree",
                )?;
                let include = source_object
                    .fields
                    .get("IncludeFilter")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| bad("part filter mode"))?;
                let mut ids = BTreeSet::new();
                for pointer in array(&source_object, "FilterItems")? {
                    let key = g
                        .source
                        .target(
                            &source.key.0,
                            pointer
                                .as_pointer()
                                .ok_or_else(|| bad("part filter definition"))?,
                        )?
                        .ok_or_else(|| bad("null part filter definition"))?;
                    let item = g.source.object(&key)?;
                    check(item.class.ends_with("ItemData"), "part filter item class")?;
                    let id = item
                        .fields
                        .get("Id")
                        .and_then(Value::as_fields)
                        .and_then(|f| f.get("Id"))
                        .and_then(Value::bits32)
                        .ok_or_else(|| bad("part filter ItemData.Id"))?;
                    check(ids.insert(id), "duplicate part filter identity")?;
                }
                Trigger::Part { include, ids }
            }
            _ => return Err(bad("unsupported unlock notification trigger")),
        };
        let hash = message
            .fields
            .get("HashId")
            .and_then(Value::bits32)
            .filter(|&v| v != 0)
            .ok_or_else(|| bad("unlock message hash"))?;
        check(hashes.insert(hash), "duplicate unlock message hash")?;
        let notification = Notification {
            hash,
            manager: sender_manager(g, &n.site, world, bindings)?,
            delay: Duration::from_secs_f32(seconds),
            trigger,
        };
        check(
            found.insert(asset.0, notification).is_none(),
            "duplicate unlock sender",
        )?;
    }
    check(
        found.keys().cloned().collect::<BTreeSet<_>>() == *selected,
        "selected unlock sender missing",
    )?;
    Ok(found.into_values().collect())
}

fn array<'a>(o: &'a Object, name: &str) -> Result<&'a [Value], Error> {
    o.fields
        .get(name)
        .and_then(Value::as_array)
        .filter(|a| a.len() <= 4096)
        .ok_or_else(|| bad("unlock notification array"))
}
fn listener(o: &Object, events: bool, properties: bool) -> Result<(), Error> {
    let l = crate::scoring::realms::Listeners::read(o)?
        .ok_or_else(|| bad("unlock notification listener flags"))?;
    check(
        (!events || l.events.includes_server()) && (!properties || l.properties.includes_server()),
        "unlock notification listener realm",
    )
}
fn links<S: Source>(g: &mut Graph<S>, s: &Site, expected: &[u32]) -> Result<(), Error> {
    check(
        g.link_ports(s)?.into_iter().collect::<BTreeSet<_>>() == expected.iter().copied().collect(),
        "unlock notification link ports",
    )
}
fn one_link<S: Source>(
    g: &mut Graph<S>,
    s: &Site,
    port: u32,
    target: u32,
) -> Result<Endpoint, Error> {
    match g.links(s, port)?.as_slice() {
        [e] if e.port == target => Ok(e.clone()),
        _ => Err(bad("unlock notification link target")),
    }
}
fn ownership<S: Source>(
    g: &mut Graph<S>,
    s: &Site,
    context: &Endpoint,
    channel: &Endpoint,
) -> Result<(), Error> {
    links(g, s, &[CONTEXT, INVENTORY])?;
    let inventory = one_link(g, s, INVENTORY, INVENTORY_TARGET)?;
    check(
        one_link(g, s, CONTEXT, 0)? == *context && inventory.site == channel.site,
        "unlock notification participant or inventory ownership",
    )
}
fn delay_seconds<S: Source>(g: &mut Graph<S>, s: &Site) -> Result<f32, Error> {
    let o = class(g, s, "DelayEntityData")?;
    listener(&o, true, false)?;
    ports(g, s, &[])?;
    events(g, s, &[IN])?;
    links(g, s, &[])?;
    let c = crate::scoring::delay::compile(&o)?;
    check(
        !c.definition.auto_start
            && !c.definition.run_once
            && c.definition.remove_duplicates
            && c.time_domain_name == "TimeDeltaType_World"
            && c.delay > 0.
            && c.delay <= 60.,
        "unlock notification delay policy",
    )?;
    Ok(c.delay)
}
fn part_attachment<S: Source>(
    g: &mut Graph<S>,
    asset: &Key,
    attachments: &[Value],
    parameters: &[Value],
) -> Result<(), Error> {
    let ([attachment], [parameter]) = (attachments, parameters) else {
        return Err(bad("part notification attachment count"));
    };
    let key = g
        .source
        .target(
            &asset.0,
            attachment
                .as_pointer()
                .ok_or_else(|| bad("part attachment reference"))?,
        )?
        .ok_or_else(|| bad("null part attachment"))?;
    check(
        g.source.object(&key)?.class == "CarPartAttachmentAsset",
        "part attachment class",
    )?;
    let fields = parameter
        .as_fields()
        .ok_or_else(|| bad("part attachment parameters"))?;
    check(
        fields.get("LinkIds").and_then(Value::as_array) == Some(&[])
            && fields
                .get("PropertyIds")
                .and_then(Value::as_array)
                .is_some_and(|p| p.len() == 1 && p[0].bits32() == Some(PART)),
        "part attachment input mapping",
    )
}

#[cfg(test)]
mod tests;
