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
    /// The free-roam HUD while its first-drive logic exists, then ViewWorld.
    fn driving(&self, participant: u16) -> Result<Vec<HostRpc>, replication::Error> {
        let (Some(logic), Some(ghosts)) = (self.logic, self.ghosts.get(&participant)) else {
            return Ok(Vec::new());
        };
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
            // 749427 ms: hide the world for the next teleport.
            (Vec::new(), ctx.fade(participant, &[HIDE_WORLD, BLACK])?)
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
                vec!["Leave", "fade:1", "fade:2"],
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
        // Once the return retires the first-drive HUD, driving shows the world only.
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
            vec!["Enter", "fade:0"]
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
