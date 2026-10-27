// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The ordinary participant presentation the official host keeps driving
//! through the prologue (career start, E1183). Fade control (capture 1)
//! switches ViewWorld and BlackScreen at the intro's state boundaries and the
//! first drive shows the free-roam HUD. Each rule follows the startup or movie
//! notification it accompanies in the official stream; without the fade
//! logic (no garage logic configured) nothing is added.
use super::*;
use crate::garage_logic::{GarageLogic, Which};
use nfs_world::participants::{Call, Notification};

const VIEW_WORLD: u32 = 0;
const HIDE_WORLD: u32 = 1;
const BLACK: u32 = 2;
const UNBLACK: u32 = 3;

pub(in crate::net::players) struct Context<'a> {
    pub logic: Option<&'a GarageLogic>,
    pub ghosts: &'a BTreeMap<u16, BTreeMap<u16, u16>>,
    pub players: &'a Players,
    pub participants: &'a Lifecycle,
    pub content: Option<&'a crate::prologue::Content>,
}

impl Context<'_> {
    fn movie_playing(&self, endpoint: nfs_world::participants::Endpoint) -> bool {
        let Some(content) = self.content else {
            return false;
        };
        [&content.manu_movie, &content.diner_movie]
            .into_iter()
            .flatten()
            .any(|movie| {
                self.players
                    .objects()
                    .scene_endpoint(movie.data.state.0, movie.data.state.1)
                    == Some((endpoint.scene, endpoint.selector))
            })
    }
    fn fade(&self, participant: u16, events: &[u32]) -> Result<Vec<HostRpc>, replication::Error> {
        let (Some(logic), Some(ghosts)) = (self.logic, self.ghosts.get(&participant)) else {
            return Ok(Vec::new());
        };
        Ok(logic
            .fade(self.players, ghosts, events)?
            .into_iter()
            .map(HostRpc::Event)
            .collect())
    }
    /// The transfer presentation logic's two entity bursts (career start
    /// 39419, 563121, 749427 and 891523 ms, one per teleport or in-place
    /// start). The recovery batch carries the same authored events on the
    /// same logic (capture 21 in installed content); nothing while the intro
    /// has not created that logic.
    fn transfer(&self, participant: u16) -> Result<Vec<HostRpc>, replication::Error> {
        let (Some(logic), Some(ghosts)) = (self.logic, self.ghosts.get(&participant)) else {
            return Ok(Vec::new());
        };
        let transfer: BTreeSet<u16> = logic
            .batch(Which::InWorldTeleport)
            .entities
            .iter()
            .map(|d| d.capture_id)
            .collect();
        Ok(logic
            .present_events(Which::Recovered, self.players, ghosts)?
            .into_iter()
            .filter(|fire| {
                ghosts.iter().any(|(capture, &ghost)| {
                    ghost == fire.target.ghost && transfer.contains(capture)
                })
            })
            .map(HostRpc::Event)
            .collect())
    }
    /// The free-roam HUD while its first-drive logic exists, then ViewWorld.
    fn driving(&self, participant: u16) -> Result<Vec<HostRpc>, replication::Error> {
        let (Some(logic), Some(ghosts)) = (self.logic, self.ghosts.get(&participant)) else {
            return Ok(Vec::new());
        };
        // Only a drive is presented: the relocation's brief Alive carries no
        // drive logic and the official host shows nothing there (747325 ms).
        if !logic
            .arrived_captures()
            .iter()
            .all(|c| ghosts.contains_key(c))
        {
            return Ok(Vec::new());
        }
        let mut events = logic.present_events(Which::FreeRoam, self.players, ghosts)?;
        let fade = logic.fade_capture().and_then(|c| ghosts.get(&c)).copied();
        if !events
            .iter()
            .any(|f| f.event == VIEW_WORLD && Some(f.target.ghost) == fade)
        {
            events.extend(logic.fade(self.players, ghosts, &[VIEW_WORLD])?);
        }
        Ok(events.into_iter().map(HostRpc::Event).collect())
    }
}

/// `calls` with the official presentation events placed beside the intro
/// notifications they accompany. Calls for other participants are unchanged.
pub(in crate::net::players) fn present(
    ctx: &Context<'_>,
    calls: Vec<HostRpc>,
) -> Result<Vec<HostRpc>, replication::Error> {
    present_with_driving_tail(ctx, calls, &BTreeSet::new())
}

/// A newly constructed first drive publishes its HUD only after the complete
/// handoff, including mode callbacks and initial opponents. Ordinary Alive
/// transitions keep their existing presentation placement.
pub(super) fn present_with_driving_tail(
    ctx: &Context<'_>,
    calls: Vec<HostRpc>,
    driving_tail: &BTreeSet<u16>,
) -> Result<Vec<HostRpc>, replication::Error> {
    let (Some(intro), Some(lifecycle)) = (
        ctx.participants.intro_bindings(),
        ctx.participants.lifecycle_bindings(),
    ) else {
        return Ok(calls);
    };
    if ctx.logic.is_none() {
        return Ok(calls);
    }
    let mut out = Vec::with_capacity(calls.len());
    for (index, call) in calls.iter().enumerate() {
        let HostRpc::Participant(Notification {
            endpoint,
            participant,
            call: kind,
        }) = *call
        else {
            out.push(*call);
            continue;
        };
        if ctx.participants.intro_chapter(participant).is_none() {
            out.push(*call);
            continue;
        }
        let enter = matches!(kind, Call::Enter);
        let leave = matches!(kind, Call::Leave);
        let (before, after) = if endpoint == intro.render_wait && leave {
            // Career start 48920, 571921, 756022 ms: rendering ends the black.
            (ctx.fade(participant, &[UNBLACK])?, Vec::new())
        } else if endpoint == intro.alive && enter {
            // 125717 ms (with the first-drive HUD), 574121, 758326, 891523 ms.
            (
                Vec::new(),
                if driving_tail.contains(&participant) {
                    Vec::new()
                } else {
                    ctx.driving(participant)?
                },
            )
        } else if endpoint == intro.relocating && leave {
            // 749427 ms: ViewWorld, then hide the world for the next teleport,
            // then the transfer logic's two bursts.
            let mut after = ctx.fade(participant, &[VIEW_WORLD, HIDE_WORLD, BLACK])?;
            after.extend(ctx.transfer(participant)?);
            (Vec::new(), after)
        } else if endpoint == lifecycle.initial_teleport && enter {
            // 563121 ms: the first transfer's logic fires after its teleport
            // command; the final composite exit has no transfer logic left.
            (Vec::new(), ctx.transfer(participant)?)
        } else if ctx.movie_playing(endpoint) && enter {
            // 652722, 826725 ms.
            (Vec::new(), ctx.fade(participant, &[HIDE_WORLD, BLACK])?)
        } else if ctx.movie_playing(endpoint) && leave {
            // 747127, 889025 ms.
            (Vec::new(), ctx.fade(participant, &[UNBLACK])?)
        } else if endpoint == lifecycle.loading_garage && enter {
            // 298818, 1127627 ms.
            (Vec::new(), ctx.fade(participant, &[BLACK, UNBLACK, BLACK])?)
        } else if endpoint == lifecycle.loading_garage && leave {
            // 306219, 1135026 ms: the garage appears.
            (Vec::new(), ctx.fade(participant, &[UNBLACK, VIEW_WORLD])?)
        } else if endpoint == lifecycle.composite && leave {
            // 562123 ms toward the first race; 1247625 ms into customization.
            let next = calls[index + 1..].iter().find_map(|c| match c {
                HostRpc::Participant(n)
                    if n.participant == participant && n.call == Call::Enter =>
                {
                    Some(n.endpoint)
                }
                _ => None,
            });
            if next == Some(lifecycle.customization) {
                (
                    ctx.fade(participant, &[HIDE_WORLD, VIEW_WORLD])?,
                    Vec::new(),
                )
            } else {
                (
                    ctx.fade(participant, &[HIDE_WORLD, BLACK, UNBLACK, BLACK])?,
                    Vec::new(),
                )
            }
        } else {
            (Vec::new(), Vec::new())
        };
        out.extend(before);
        out.push(*call);
        out.extend(after);
    }
    for &participant in driving_tail {
        out.extend(ctx.driving(participant)?);
        // The return drive starts in place after its Enter alive: the
        // recreated transfer logic fires with the HUD (891523 ms).
        if ctx.participants.intro_chapter(participant) == Some(Chapter::ReturnDrive) {
            out.extend(ctx.transfer(participant)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::garage_logic::Part;

    fn label(fade: u16, hud: u16, call: &HostRpc) -> String {
        match call {
            HostRpc::Event(f) if f.target.ghost == fade => format!("fade:{}", f.event),
            HostRpc::Event(f) if f.target.ghost == hud => format!("hud:{}", f.event),
            HostRpc::Participant(n) => format!("{:?}", n.call),
            other => format!("{other:?}"),
        }
    }

    /// Career start 747725–756224 and 562–572 s: the relocation creates four
    /// transfer logics; the last fires its bursts after the relocating Leave
    /// (or the first transfer's initial-teleport Enter), two go at unblack,
    /// one with the arrival, and the burst logic retires with the next movie.
    #[test]
    fn transfer_logic_follows_the_relocation_and_retires_at_unblack_and_arrival() {
        let (mut listener, participant, _) = super::super::tests::setup();
        let key = listener.vehicles.as_ref().unwrap().layout.item_scene_key;
        let json = crate::garage_logic::tests::intro_json()
            .to_string()
            .replace(
                "\"scene_content_key\":101",
                &format!("\"scene_content_key\":{key}"),
            );
        let logic = GarageLogic::from_json(&serde_json::from_str(&json).unwrap()).unwrap();
        let ghosts = listener.logic_ghosts.entry(participant).or_default();
        logic
            .produce_intro_spawn(&mut listener.players, participant, ghosts)
            .unwrap();
        let created = logic
            .produce_present(
                Which::InWorldTeleport,
                &mut listener.players,
                participant,
                ghosts,
                Part::Creations,
            )
            .unwrap();
        assert_eq!(created.records.len(), 4);
        let fade = ghosts[&277];
        let transfer = ghosts[&336];
        let unblack = vec![ghosts[&333], ghosts[&334]];
        let arrival = ghosts[&335];
        listener.logic = Some(std::sync::Arc::new(logic));
        let intro = listener.participants.intro_bindings().unwrap();
        let life = listener.participants.lifecycle_bindings().unwrap();
        let label = |call: &HostRpc| match call {
            HostRpc::Event(f) if f.target.ghost == fade => format!("fade:{}", f.event),
            HostRpc::Event(f) if f.target.ghost == transfer => {
                format!("xfer:{}:{}", f.target.entity, f.event)
            }
            HostRpc::Participant(n) => format!("{:?}", n.call),
            other => format!("{other:?}"),
        };
        let note = |endpoint, call| {
            HostRpc::Participant(Notification {
                endpoint,
                participant,
                call,
            })
        };
        let run = |listener: &PlayerListener, calls: Vec<HostRpc>| {
            present(
                &Context {
                    logic: listener.logic.as_deref(),
                    ghosts: &listener.logic_ghosts,
                    players: &listener.players,
                    participants: &listener.participants,
                    content: listener.prologue.as_deref(),
                },
                calls,
            )
            .unwrap()
            .iter()
            .map(label)
            .collect::<Vec<_>>()
        };
        let bursts = [
            "xfer:1:17",
            "xfer:1:15",
            "xfer:1:13",
            "xfer:1:11",
            "xfer:2:9",
            "xfer:2:7",
            "xfer:2:5",
            "xfer:2:4",
        ];
        let mut expected = vec!["Leave", "fade:0", "fade:1", "fade:2"];
        expected.extend(bursts);
        assert_eq!(
            run(&listener, vec![note(intro.relocating, Call::Leave)]),
            expected
        );
        let mut expected = vec!["Enter"];
        expected.extend(bursts);
        assert_eq!(
            run(&listener, vec![note(life.initial_teleport, Call::Enter)]),
            expected
        );
        // Alive without the return-drive chapter: HUD rules only, no bursts.
        assert!(
            run(&listener, vec![note(intro.alive, Call::Enter)])
                .iter()
                .all(|l| !l.starts_with("xfer"))
        );
        let logic = listener.logic.clone().unwrap();
        let mut players = listener.players.clone();
        let mut staged = listener.logic_ghosts.clone();
        assert!(
            PlayerListener::intro_unblack(
                Some(&logic),
                &mut players,
                &listener.participants,
                &mut staged,
                &[note(intro.alive, Call::Enter)],
            )
            .unwrap()
            .is_none()
        );
        let section = PlayerListener::intro_unblack(
            Some(&logic),
            &mut players,
            &listener.participants,
            &mut staged,
            &[note(intro.render_wait, Call::Leave)],
        )
        .unwrap()
        .expect("unblack retires the holding logics");
        assert_eq!(section.deleted, unblack);
        assert!(section.records.is_empty());
        assert!(players.objects().get(unblack[0]).is_none());
        assert!(players.objects().get(transfer).is_some());
        // Repeated: nothing left to retire.
        assert!(
            PlayerListener::intro_unblack(
                Some(&logic),
                &mut players,
                &listener.participants,
                &mut staged,
                &[note(intro.render_wait, Call::Leave)],
            )
            .unwrap()
            .is_none()
        );
        let out = logic
            .produce_present(
                Which::Arrived,
                &mut players,
                participant,
                staged.get_mut(&participant).unwrap(),
                Part::All,
            )
            .unwrap();
        assert_eq!(out.deleted, vec![arrival]);
        assert!(!out.records.is_empty());
        assert!(players.objects().get(transfer).is_some());
        let out = logic
            .produce_present(
                Which::InWorldTeleport,
                &mut players,
                participant,
                staged.get_mut(&participant).unwrap(),
                Part::Deletions,
            )
            .unwrap();
        assert!(out.deleted.contains(&transfer));
        assert!(players.objects().get(transfer).is_none());
    }

    #[test]
    fn intro_states_carry_the_official_presentation_events() {
        let (mut listener, participant, _) = super::super::tests::setup();
        let key = listener.vehicles.as_ref().unwrap().layout.item_scene_key;
        let json = crate::garage_logic::tests::intro_json()
            .to_string()
            .replace(
                "\"scene_content_key\":101",
                &format!("\"scene_content_key\":{key}"),
            );
        let logic = GarageLogic::from_json(&serde_json::from_str(&json).unwrap()).unwrap();
        let ghosts = listener.logic_ghosts.entry(participant).or_default();
        logic
            .produce_intro_spawn(&mut listener.players, participant, ghosts)
            .unwrap();
        logic
            .produce_present(
                Which::Arrived,
                &mut listener.players,
                participant,
                ghosts,
                Part::Creations,
            )
            .unwrap();
        let (fade, hud) = (ghosts[&277], ghosts[&330]);
        listener.logic = Some(std::sync::Arc::new(logic));
        let intro = listener.participants.intro_bindings().unwrap();
        let life = listener.participants.lifecycle_bindings().unwrap();
        let note = |endpoint, participant, call| {
            HostRpc::Participant(Notification {
                endpoint,
                participant,
                call,
            })
        };
        let run = |listener: &PlayerListener, calls: Vec<HostRpc>| {
            present(
                &Context {
                    logic: listener.logic.as_deref(),
                    ghosts: &listener.logic_ghosts,
                    players: &listener.players,
                    participants: &listener.participants,
                    content: listener.prologue.as_deref(),
                },
                calls,
            )
            .unwrap()
            .iter()
            .map(|c| label(fade, hud, c))
            .collect::<Vec<_>>()
        };
        let cases = [
            (
                vec![(intro.render_wait, Call::Leave)],
                vec!["fade:3", "Leave"],
            ),
            (
                vec![(intro.alive, Call::Enter)],
                vec!["Enter", "hud:19", "hud:17", "hud:12", "fade:0"],
            ),
            (
                vec![(intro.relocating, Call::Leave)],
                vec!["Leave", "fade:0", "fade:1", "fade:2"],
            ),
            (
                vec![(life.loading_garage, Call::Enter)],
                vec!["Enter", "fade:2", "fade:3", "fade:2"],
            ),
            (
                vec![(life.loading_garage, Call::Leave)],
                vec!["Leave", "fade:3", "fade:0"],
            ),
            (
                vec![
                    (life.composite, Call::Leave),
                    (life.initial_teleport, Call::Enter),
                ],
                vec!["fade:1", "fade:2", "fade:3", "fade:2", "Leave", "Enter"],
            ),
            (
                vec![
                    (life.composite, Call::Leave),
                    (life.customization, Call::Enter),
                ],
                vec!["fade:1", "fade:0", "Leave", "Enter"],
            ),
            (vec![(intro.teleport, Call::Enter)], vec!["Enter"]),
        ];
        for (calls, expected) in cases {
            let calls = calls
                .into_iter()
                .map(|(endpoint, call)| note(endpoint, participant, call))
                .collect();
            assert_eq!(run(&listener, calls), expected);
        }
        let handoff = vec![
            note(intro.alive, participant, Call::Enter),
            HostRpc::Event(nfs_world::logic::Fire {
                target: nfs_world::logic::EntityRef {
                    ghost: fade,
                    entity: 1,
                },
                event: 77,
                player: None,
            }),
        ];
        let presented = present_with_driving_tail(
            &Context {
                logic: listener.logic.as_deref(),
                ghosts: &listener.logic_ghosts,
                players: &listener.players,
                participants: &listener.participants,
                content: listener.prologue.as_deref(),
            },
            handoff,
            &BTreeSet::from([participant]),
        )
        .unwrap();
        assert_eq!(
            presented
                .iter()
                .map(|c| label(fade, hud, c))
                .collect::<Vec<_>>(),
            ["Enter", "fade:77", "hud:19", "hud:17", "hud:12", "fade:0"]
        );
        // Another participant's identical notification is left alone.
        let other = vec![note(intro.alive, participant + 1, Call::Enter)];
        assert_eq!(run(&listener, other), vec!["Enter"]);
        // Without its drive logic an Alive presents nothing: the relocation's
        // brief Alive (official 747325 ms) shows the world only later, from
        // the relocating Leave (749427 ms).
        let ghosts = listener.logic_ghosts.get_mut(&participant).unwrap();
        let logic = listener.logic.clone().unwrap();
        logic
            .produce_present(
                Which::Returning,
                &mut listener.players,
                participant,
                ghosts,
                Part::All,
            )
            .unwrap();
        assert_eq!(
            run(&listener, vec![note(intro.alive, participant, Call::Enter)]),
            vec!["Enter"]
        );
        // Without garage logic nothing is added.
        listener.logic = None;
        assert_eq!(
            run(
                &listener,
                vec![note(life.loading_garage, participant, Call::Enter)]
            ),
            vec!["Enter"]
        );
    }
}
