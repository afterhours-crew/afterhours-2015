// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The owned event selection: which installed event is offered and which of its
//! logics and sequences the host creates, in the order the official host creates
//! them (E994/E995: Sprint_Race_007, "In The Driver's Seat", normal variant, solo).
//! Every layout, binding and value source is resolved from the installation.
use nfs_content::events::Request;

pub const START_AREA: &str = "startareavehiclecheckfacingandspeed_set";
pub const SHOW_START: &str = "gameplay/gamemodes/startevent/showstartraceandinviteset";
pub const SHOW_PARTICIPANTS: &str =
    "gameplay/gamemodes/startevent/showparticipantstostartwithlogic";
pub const EXTERNAL_FILTER: &str = "gameplay/gamemodes/externalsetfilter";
pub const STARTING: &str = "gameplay/eventstartinglogic";
pub const LOGIC_TELEMETRY: &str = "gameplay/eventlogictelemetrylogic";
pub const SOUND: &str = "gameplay/gamemodes/gamemodesoundplb";
pub const INTRO: &str = "gameplay/gamemodes/genericintroparticipantlogic";
pub const TELEMETRY: &str = "gameplay/eventtelemetrylogic";
pub const RESTART: &str = "gameplay/gamemodes/restarteventlogic";
pub const LEAVE: &str = "gameplay/gamemodes/leaveeventlogic";
pub const RESULT_TRACKING: &str = "gameplay/scoring/raceeventresulttrackinglogic";
pub const ONGOING_SCORE: &str = "gameplay/scoring/raceeventongoingscore";
pub const ADD_SET: &str = "gameplay/addsetparticipantnoevent";
pub const DRIVING_STYLE: &str = "gameplay/scoring/eventdrivingstylescoringparticipantlogic";
pub const AWARDS: &str = "gameplay/raceeventawardsselectionparticipantlogic";
pub const OUTRO: &str = "gameplay/scoring/raceeventoutrologic";
pub const INTRO_SEQUENCE: &str = "gameplay/sequences/gotowaypointraceintrosequence";
pub const COUNTDOWN_SEQUENCE: &str = "gameplay/sequences/countdownsequence";
pub const OUTRO_SEQUENCE: &str = "gameplay/sequences/wanderoutro";
pub const EVENTS_SCENE: &str = "levels/genesis01/events";
/// Boolean client notifications the host sets while the participant is in the
/// game mode and clears when it leaves (E997: calls on Events selectors 1 and 9,
/// UI selector 1, Collectibles selector 1 and Activities selector 0 in every
/// official attempt), in the official order.
pub const GAME_MODE_FLAGS: [(&str, &str, usize); 5] = [
    (
        EVENTS_SCENE,
        "levels/genesis01/events/layer0_events_schematic",
        20,
    ),
    (
        EVENTS_SCENE,
        "levels/genesis01/events/layer0_events_schematic",
        19,
    ),
    (
        "levels/genesis01/ui",
        "levels/genesis01/ui/layer0_notifications_schematic",
        6,
    ),
    (
        "levels/genesis01/collectibles",
        "levels/genesis01/collectibles/layer0_collectibles_schematic",
        4,
    ),
    (
        "levels/genesis01/activities",
        "levels/genesis01/activities/layer0_activities_schematic",
        8,
    ),
];
pub const IN_GAME_MODE: &str = "gameplay/gamemodes/participantingamemodelogic";
pub const COOLDOWN: &str = "gameplay/gamemodes/participantincooldownlogic";

/// Sprint_Race_007's normal variant. Its start-area eligibility notifications are
/// objects 70 and 72 of the area's event-logic prefab (participant set: REP level in
/// the event's range or the override set; E996), and its free-roam notification is
/// object 25 of the event schematic's mission-switching prefab (set while the
/// participant is in free roam and not on its prestige path; E996).
pub fn sprint_race_007() -> Request {
    let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    Request {
        bundle: "levels/genesis01/district02_sprintraces".into(),
        scene: "levels/genesis01/district02_sprintraces_sublevel2".into(),
        game_mode_id: "gameplay/gamemodes/sprint_race_007".into(),
        game_mode: "gameplay/gamemodes/racegamemode".into(),
        manager_scene: "levels/genesis01/gameplay".into(),
        start_logics: names(&[
            START_AREA,
            SHOW_START,
            SHOW_PARTICIPANTS,
            EXTERNAL_FILTER,
            STARTING,
            LOGIC_TELEMETRY,
        ]),
        game_mode_logics: names(&[
            SOUND,
            INTRO,
            TELEMETRY,
            RESTART,
            LEAVE,
            RESULT_TRACKING,
            ONGOING_SCORE,
            ADD_SET,
            DRIVING_STYLE,
            AWARDS,
            OUTRO,
        ]),
        sequences: names(&[INTRO_SEQUENCE, COUNTDOWN_SEQUENCE, OUTRO_SEQUENCE]),
        scene_logics: vec![
            (EVENTS_SCENE.into(), IN_GAME_MODE.into()),
            (EVENTS_SCENE.into(), COOLDOWN.into()),
        ],
        linked_logic: Some(EXTERNAL_FILTER.into()),
        area_notifications: vec![
            ("eventlogic(set)".into(), 70),
            ("eventlogic(set)".into(), 72),
        ],
        scene_notifications: vec![(
            "gameplay/gamemodes/missionswitchingprefabprestige".into(),
            25,
        )],
        game_mode_notifications: GAME_MODE_FLAGS
            .iter()
            .map(|(scene, asset, object)| ((*scene).to_owned(), (*asset).to_owned(), *object))
            .collect(),
    }
}
