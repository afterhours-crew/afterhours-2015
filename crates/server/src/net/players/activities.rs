// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Client crossing reports on the world volumes the host put the participant
//! into (`prologue::volumes`): event start areas, activities and spots. The
//! client reports a car it holds in the volume's set, m2 on entry and m3 on
//! exit, `[scene, participant]` with its clock and the assigned token (0 for
//! every activity family; official E1026, E912). Most families get no host
//! answer (official: missions, drift corners, speed runs, the other
//! narrative locations). The Club location gates the enter-location tutorial
//! (E1026 1117.801 s report, 1117.925 s creation, 1118.426 s deletion).
//! Reports after the membership left are late and dropped (E1026 a report
//! 6 ms after the 1119.228 s revoke).
use super::*;
use nfs_content::events::discovery::Endpoint as Site;
use nfs_protocol::world::{
    BitSpan,
    rpc::{Envelope, Limits, RouteProfile},
};

/// Bound on tracked (participant, volume) crossings.
const MAX_INSIDE: usize = 4096;

pub(super) type Inside = BTreeMap<(u16, Site), bool>;

impl PlayerListener {
    /// A client patrol request on an installed `CopSpawnPatrolEntityData`:
    /// consumed with no answer.
    pub(super) fn cop_patrol_request(
        &self,
        players: &Players,
        body: BitSpan<'_>,
    ) -> Result<bool, replication::Error> {
        let Some(content) = self.prologue.as_deref() else {
            return Ok(false);
        };
        let Some((site, envelope, method)) = Self::client_site(players, body) else {
            return Ok(false);
        };
        if !content.volumes.cop_patrols.contains(&site) {
            return Ok(false);
        }
        // Nothing answers a patrol request, so an unexpected one is only
        // logged: refusing the frame would drop the client's other input.
        let expected = method == 0
            && matches!(envelope.as_slice(), &[_, player]
                if players.owns(HOST_SELECTOR as u8, self.persona, player));
        if expected {
            tracing::debug!(
                scene_key = site.scene_key,
                serializer = site.serializer,
                "cop patrol request; no answer"
            );
        } else {
            tracing::warn!(
                scene_key = site.scene_key,
                serializer = site.serializer,
                method,
                references = ?envelope,
                "unexpected cop patrol call; dropped"
            );
        }
        Ok(true)
    }

    /// The installed scene serializer of a client call on `[scene, ...]`,
    /// with its references and method.
    fn client_site(players: &Players, body: BitSpan<'_>) -> Option<(Site, Vec<u16>, u32)> {
        let envelope = Envelope::decode(
            body,
            Limits {
                max_input_bits: 8192,
                max_references: 255,
                max_payload_bytes: 256,
            },
        )
        .ok()?;
        let route = envelope.route(RouteProfile::ClientSend).ok()?;
        let scene = *envelope.references().first()?;
        let Some(replication::Initial::SubLevel { prefix, fields }) =
            players.objects().get(scene).map(|o| &o.initial)
        else {
            return None;
        };
        let serializer = fields.iter().position(|f| {
            replication::sublevel::Initial::rpc(f).is_some_and(|r| r.selector == route.selector())
        })?;
        Some((
            Site {
                scene_key: prefix.content_key,
                serializer,
            },
            envelope.references().to_vec(),
            route.method_index(),
        ))
    }

    /// The installed volume a client call targets, when it is one the host
    /// assigns.
    fn activity_site(&self, players: &Players, body: BitSpan<'_>) -> Option<Site> {
        let content = self.prologue.as_deref()?;
        let (site, references, _) = Self::client_site(players, body)?;
        if references.len() != 2 {
            return None;
        }
        let volumes = &content.volumes;
        (volumes.events.contains(&site)
            || volumes.activities.contains(&site)
            || volumes.spots.contains(&site))
        .then_some(site)
    }

    /// Consume a crossing report on an assigned volume. `Ok(false)` when the
    /// call is not one.
    pub(super) fn activity_crossing(
        &self,
        players: &Players,
        tutorials: &mut BTreeMap<u16, crate::tutorials::Session>,
        inside: &mut Inside,
        body: BitSpan<'_>,
    ) -> Result<bool, replication::Error> {
        let Some(site) = self.activity_site(players, body) else {
            return Ok(false);
        };
        let Some(crossing) = nfs_world::world_activation::Crossing::decode(body)? else {
            return Ok(false);
        };
        if !players.owns_participant(HOST_SELECTOR as u8, self.persona, crossing.participant) {
            return Err(replication::Error::UnknownObject);
        }
        if crossing.token != 0 || !crossing.seconds.is_finite() || crossing.seconds < 0.0 {
            return Err(replication::Error::Shape);
        }
        let participant = crossing.participant;
        if !self.world_volumes.member(participant, site) {
            tracing::info!(
                participant,
                scene_key = site.scene_key,
                serializer = site.serializer,
                inside = crossing.inside,
                "late volume report after the membership left"
            );
            return Ok(true);
        }
        let previous = inside.insert((participant, site), crossing.inside);
        if inside.len() > MAX_INSIDE {
            return Err(replication::Error::Bound);
        }
        if previous == Some(crossing.inside) {
            return Ok(true);
        }
        tracing::debug!(
            participant,
            scene_key = site.scene_key,
            serializer = site.serializer,
            inside = crossing.inside,
            "volume crossing"
        );
        let volume = nfs_content::tutorials::Volume {
            scene_key: site.scene_key,
            serializer: site.serializer,
        };
        if let Some(content) = self.progression.as_ref().and_then(|p| p.tutorials.as_ref())
            && crate::tutorials::Session::watches(&content.0, volume)
            && let Some(session) = tutorials.get_mut(&participant)
        {
            session.location(volume, crossing.inside);
        }
        Ok(true)
    }
}
