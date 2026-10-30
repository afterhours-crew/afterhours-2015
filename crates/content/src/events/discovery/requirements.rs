// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Start requirements are independent of map-marker visibility. Resolve the
//! notification's complete set expression and preserve each state dependency.
use super::*;
use crate::scoring::events::Identity;

/// A scene Boolean property and its authored progression identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BooleanInput {
    pub property: Identity,
    pub identifier: String,
}

/// An event requirement backed by a participant-membership notification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binding {
    pub event_id: String,
    pub volume: Endpoint,
    pub notification: Endpoint,
    /// Whether notification membership satisfies the client-side requirement.
    pub satisfied_when_member: bool,
    /// P is player membership outside these authored transition states.
    pub excluded_states: Vec<Endpoint>,
    /// Boolean participant-property sets in
    /// (((P \ (C \ B)) \ X) union M) \ (B \ X).
    /// These identities have no implied initial values.
    pub property_b: BooleanInput,
    pub property_c: BooleanInput,
    pub property_x: BooleanInput,
    /// M is an event-controlled copy whose input is P. It is not necessarily
    /// equal to P: Copy/Clear and listener gates belong to the state owner.
    pub mirrored_set: Identity,
    pub player_manager: Identity,
}

struct Dependency {
    states: Vec<Site>,
    b: Site,
    c: Site,
    x: Site,
    mirrored: Site,
    player_manager: Site,
}

pub(super) fn identified_bools<S: Source>(
    graph: &mut Graph<S>,
    roots: &[(String, Site)],
) -> Result<BTreeMap<Site, String>, Error> {
    let mut identified = BTreeMap::new();
    for (_, root) in roots {
        for node in graph.read(root.clone())? {
            if node.object.class != "IdentifiedBoolEntityData" {
                continue;
            }
            for property in graph.links(&node.site, 3_959_795_656)? {
                if property.port != 0
                    || graph.object(&property.site)?.class != "BoolParticipantPropertyEntityData"
                {
                    return Err(bad("identified Boolean property endpoint"));
                }
                let identifier = graph.target_field(&node.site, "BoolIdentifier")?.0;
                if identified
                    .insert(property.site, identifier.clone())
                    .is_some_and(|old| old != identifier)
                {
                    return Err(bad("ambiguous identified Boolean property"));
                }
            }
        }
    }
    Ok(identified)
}

fn only_link<S: Source>(g: &mut Graph<S>, site: &Site, port: u32) -> Result<Site, Error> {
    let links = g.links(site, port)?;
    match links.as_slice() {
        [link] if link.port == 0 => Ok(link.site.clone()),
        _ => Err(bad("requirement set link is missing or ambiguous")),
    }
}

fn class<S: Source>(g: &mut Graph<S>, site: &Site, wanted: &str) -> Result<(), Error> {
    if g.object(site)?.class != wanted {
        return Err(bad(&format!("requirement expected {wanted}")));
    }
    Ok(())
}

fn predicate<S: Source>(
    g: &mut Graph<S>,
    site: &Site,
    operation: &str,
) -> Result<[Site; 2], Error> {
    let object = g.object(site)?;
    if object.class != "ParticipantSetPredicateEntityData"
        || object
            .fields
            .get("NumberOfOperands")
            .and_then(Value::bits32)
            != Some(2)
        || object.fields.get("Operation").and_then(Value::as_enum_name) != Some(operation)
    {
        return Err(bad("unsupported requirement set predicate"));
    }
    Ok([
        only_link(g, site, name("0"))?,
        only_link(g, site, name("1"))?,
    ])
}

fn boolean_property<S: Source>(g: &mut Graph<S>, site: &Site) -> Result<Site, Error> {
    class(g, site, "ParticipantSetFromBPPEntityData")?;
    let property = only_link(g, site, 2_093_602_801)?;
    class(g, &property, "BoolParticipantPropertyEntityData")?;
    Ok(property)
}

/// Verify (A union copy(P)) \ (B \ X), where
/// A = (P \ (C \ B)) \ X. The copy is event-controlled, so retain its own
/// identity and value rather than simplifying it to its current input P.
/// Every repeated operand must resolve to the same prefab occurrence. Neither
/// property values nor identities are inferred from a neighbouring event.
fn dependency<S: Source>(g: &mut Graph<S>, notification: &Site) -> Result<Dependency, Error> {
    let set = only_link(g, notification, 2_664_990_019)?;
    let [union, selected_difference] = predicate(g, &set, "DIF")?;
    let [a, copied] = predicate(g, &union, "UNION")?;
    class(g, &copied, "ParticipantSetCopyEntityData")?;
    let p = only_link(g, &copied, name("ParticipantSet"))?;
    let [b, x] = predicate(g, &selected_difference, "DIF")?;
    let property_b = boolean_property(g, &b)?;
    let property_x = boolean_property(g, &x)?;
    let [a0, a_x] = predicate(g, &a, "DIF")?;
    let [a_p, c_minus_b] = predicate(g, &a0, "DIF")?;
    let [c, c_b] = predicate(g, &c_minus_b, "DIF")?;
    let property_c = boolean_property(g, &c)?;
    if a_x != x || a_p != p || c_b != b {
        return Err(bad(
            "requirement predicate operands do not share their authored instance",
        ));
    }
    let [players, states] = predicate(g, &p, "DIF")?;
    class(g, &players, "ParticipantSetPlayerAdapterEntityData")?;
    let manager = only_link(g, &players, name("ParticipantManager"))?;
    class(g, &manager, "ParticipantManagerEntityData")?;
    class(g, &states, "ParticipantSetStateAdapterEntityData")?;
    let links = g.links(&states, name("ParticipantStates"))?;
    if links.is_empty() || links.len() > 64 {
        return Err(bad("requirement transition-state bound"));
    }
    let mut states = BTreeSet::new();
    for link in links {
        if link.port != 0
            || !matches!(
                g.object(&link.site)?.class.as_str(),
                "ParticipantStateEntityData"
                    | "TeleportingParticipantStateEntityData"
                    | "StateSequenceParticipantStateEntityData"
            )
            || !states.insert(link.site)
        {
            return Err(bad("unsupported or duplicate requirement transition state"));
        }
    }
    Ok(Dependency {
        states: states.into_iter().collect(),
        b: property_b,
        c: property_c,
        x: property_x,
        mirrored: copied,
        player_manager: manager,
    })
}

/// Follow direct membership/negation gates. Other start requirements have
/// separate owners; this reader does not override them or claim their support.
fn notification<S: Source>(
    g: &mut Graph<S>,
    requirement: &Site,
) -> Result<Option<(Site, bool)>, Error> {
    let mut site = requirement.clone();
    let mut port = name("IsSatisfied");
    let mut member = true;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert((site.clone(), port)) || visited.len() > 64 {
            return Err(bad("requirement property cycle or bound"));
        }
        let inputs = g.inputs(&site, port)?;
        let [input] = inputs.as_slice() else {
            return if inputs.is_empty() {
                Ok(None)
            } else {
                Err(bad("ambiguous requirement property"))
            };
        };
        match g.object(&input.site)?.class.as_str() {
            NOTIFICATION if input.port == 513_251_846 => {
                return Ok(Some((input.site.clone(), member)));
            }
            "NotEntityData" if input.port == name("Out") => {
                site = input.site.clone();
                port = name("In");
                member = !member;
            }
            _ => return Ok(None),
        }
    }
}

pub(super) fn read(
    graph: &mut Graph<Assets>,
    scenes: &mut SceneLookup<'_>,
    start_area: &Site,
    entry: &Entry,
    identified: &BTreeMap<Site, String>,
) -> Result<Vec<Binding>, Error> {
    let mut requirements = BTreeSet::new();
    for port in graph.link_ports(start_area)? {
        for link in graph.links(start_area, port)? {
            if graph.object(&link.site)?.class == "RequirementEntityData" {
                if link.port != 0 {
                    return Err(bad("requirement link port"));
                }
                requirements.insert(link.site);
            }
        }
    }
    if requirements.len() > 16 {
        return Err(bad("event requirement bound"));
    }
    let mut bindings = BTreeMap::new();
    for requirement in requirements {
        let Some((notification, satisfied_when_member)) = notification(graph, &requirement)? else {
            continue;
        };
        let dependency = dependency(graph, &notification)?;
        let endpoint = |g: &mut Graph<Assets>,
                        s: &mut SceneLookup<'_>,
                        site: &Site|
         -> Result<Endpoint, Error> {
            let (scene_key, serializer) = s.endpoint(g, site)?;
            Ok(Endpoint {
                scene_key,
                serializer,
            })
        };
        let notification = endpoint(graph, scenes, &notification)?;
        let boolean = |site: &Site| -> Result<BooleanInput, Error> {
            Ok(BooleanInput {
                property: Identity::from(site),
                identifier: identified
                    .get(site)
                    .cloned()
                    .ok_or_else(|| bad("requirement Boolean has no authored identifier"))?,
            })
        };
        let binding = Binding {
            event_id: entry.id.clone(),
            volume: entry.volume,
            notification,
            satisfied_when_member,
            excluded_states: dependency
                .states
                .iter()
                .map(|site| endpoint(graph, scenes, site))
                .collect::<Result<_, _>>()?,
            property_b: boolean(&dependency.b)?,
            property_c: boolean(&dependency.c)?,
            property_x: boolean(&dependency.x)?,
            mirrored_set: Identity::from(&dependency.mirrored),
            player_manager: Identity::from(&dependency.player_manager),
        };
        if bindings.insert(notification, binding).is_some() {
            return Err(bad("duplicate requirement notification"));
        }
    }
    Ok(bindings.into_values().collect())
}

#[cfg(test)]
mod tests;
