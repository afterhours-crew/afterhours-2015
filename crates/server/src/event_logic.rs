// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned race event (E994–E996): one installed event's start-area logics, game mode,
//! game-mode and scene logics and sequences as replicated creations for one
//! participant, and the host calls on them. Pure: the listener owns the lifecycle,
//! timing and delivery. Layouts, bindings and reward values come from the
//! installation; synchronized values come from the participant's own event state,
//! and a value without an owned source is sent absent, never invented.
use nfs_content::{
    events::{Binding, Event, Field, Layout, Rewards, SceneLogic, Sequence},
    scoring::participant_properties::Property,
};
use nfs_protocol::world::rpc::Serial;
use nfs_world::{
    game_mode::{Arguments, Call},
    replication::{
        Error, Initial as RecordInitial,
        entity::{
            Body, Initial, Kind, Rpc, Update,
            creation::{Asset, Catalog, Content as CreationContent, Creation, Prefix},
        },
        goto_waypoint, speed_limit,
        state::World,
        sublevel, wander,
    },
};
use std::collections::{BTreeMap, BTreeSet};

pub mod catalog;
pub mod lifecycle;
pub mod request;

/// Native waypoint-channel command, shared by races and solo story drives.
pub(crate) fn route_goal(
    mode: &nfs_content::events::GameMode,
) -> Result<goto_waypoint::Initial, Error> {
    if mode.drift != Some(false) {
        return Err(Error::Unsupported);
    }
    if mode.points.is_empty()
        || mode.points.len() > goto_waypoint::MAX_WAYPOINTS
        || mode.points.len() != mode.checkpoints.len()
        || mode.points.iter().flatten().any(|v| !v.is_finite())
    {
        return Err(Error::Shape);
    }
    Ok(goto_waypoint::Initial {
        selector: 0,
        serial: 1,
        allocation_serial: 0,
        waypoint: 0,
        word: mode.laps,
        points: mode.points.iter().map(|p| p.map(f32::to_bits)).collect(),
        flags: [false, false],
        radius: 150f32.to_bits(),
        progress_updates: true,
    })
}

/// The waypoints state's method-3 car ID: the client name hash (djb2 with XOR) of the
/// car's lower-case blueprint path (E996: 762201586 is
/// `vehicles/player/car_subaru_brzpremium_2014/car_subaru_brzpremium_2014`, the car
/// raced in E994 and the account's favourite-car ID).
pub fn car_id(blueprint: &str) -> u32 {
    nfs_content::events::record_id(blueprint)
}

/// Game-mode serializers the lifecycle calls, by declaring asset and class.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum State {
    /// Root (selector 0): participant join/leave notifications and the end call.
    Root,
    WaitForAi,
    Main,
    Final,
    Countdown,
    Waypoints,
    Dnf,
    DnfTriggered,
    ResultReporting,
    Finish,
    /// EventInfoPresentationDataCollector: a participant's race position.
    Positions,
    /// IntParticipantPropertyClientNotification: the local participant's position.
    LocalPosition,
    DnfState,
}
const STATES: [(State, &str, &str); 11] = [
    (
        State::Main,
        "gameplay/gamemodes/racegamemode",
        "GameModeStateEntityData",
    ),
    (
        State::Final,
        "gameplay/gamemodes/gamemodefinalstateprefab",
        "SimpleGameModeStateEntityData",
    ),
    (
        State::Countdown,
        "gameplay/gamemodes/countdowngamemodestateprefab",
        "CountdownGameModeStateEntityData",
    ),
    (
        State::Waypoints,
        "gameplay/gamemodes/waypointgamemodestateprefab",
        "WaypointsGameModeStateEntityData",
    ),
    (
        State::Dnf,
        "gameplay/gamemodes/waypointgamemodestateprefab",
        "DNFGameModeStateTransitionEntityData",
    ),
    (
        State::DnfTriggered,
        "gameplay/gamemodes/controlgamemodeleavingallowedstates",
        "DNFTriggeredGameModeStateTransitionEntityData",
    ),
    (
        State::ResultReporting,
        "gameplay/gamemodes/states/resultreportingstate",
        "GameModeStateEntityData",
    ),
    (
        State::Finish,
        "gameplay/gamemodes/raceeventfinishstateprefab",
        "GameModeStateEntityData",
    ),
    (
        State::Positions,
        "gameplay/gamemodes/ongoingscoringprefab",
        "EventInfoPresentationDataCollectorEntityData",
    ),
    (
        State::LocalPosition,
        "gameplay/gamemodes/ongoingscoringprefab",
        "IntParticipantPropertyClientNotificationEntityData",
    ),
    (
        State::DnfState,
        "gameplay/gamemodes/states/dnfstateprefab",
        "GameModeStateEntityData",
    ),
];

/// What the results logic's synchronized values present (E996: each source traced
/// in the installed result and reward prefabs; the E994 win's values in brackets).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Presented {
    /// A driving-style category's event REP [1284, 3749, 1200, 0, 4557]: the
    /// participant property the field mirrors, as the event's driving-style
    /// session wrote it (#138, E1000).
    Category,
    /// Event REP plus category REP plus awards [20,790].
    TotalRep,
    /// The event REP for the position [9,000].
    EventRep,
    /// The event cash for the position [2,000].
    Cash,
    /// The race time in milliseconds [148,129].
    RaceTimeMs,
    /// The finishing position, one-based [1].
    Position,
    /// The group-play participant count [1].
    GroupSize,
    /// A bonus or multiplier this host does not offer [0].
    Zero,
}
const RESULTS: [(&str, usize, Presented); 27] = [
    (
        "gameplay/scoring/eventresultscorelogic",
        7,
        Presented::Category,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        5,
        Presented::Category,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        11,
        Presented::Category,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        1,
        Presented::Category,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        10,
        Presented::Category,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        4,
        Presented::TotalRep,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        13,
        Presented::EventRep,
    ),
    ("gameplay/scoring/eventresultscorelogic", 2, Presented::Zero),
    ("gameplay/scoring/eventresultscorelogic", 9, Presented::Zero),
    (
        "gameplay/scoring/eventresultscorelogic",
        12,
        Presented::Zero,
    ),
    ("gameplay/scoring/eventresultscorelogic", 8, Presented::Zero),
    (
        "gameplay/scoring/eventresultscorelogic",
        3,
        Presented::EventRep,
    ),
    (
        "gameplay/scoring/eventresultscorelogic",
        6,
        Presented::EventRep,
    ),
    (
        "gameplay/scoring/raceeventoutrologic",
        6,
        Presented::Position,
    ),
    ("gameplay/scoring/raceeventoutrologic", 2, Presented::Cash),
    (
        "gameplay/scoring/raceeventoutrologic",
        9,
        Presented::RaceTimeMs,
    ),
    ("gameplay/scoring/raceeventoutrologic", 3, Presented::Zero),
    ("gameplay/scoring/raceeventoutrologic", 4, Presented::Zero),
    (
        "gameplay/progression/groupplaybonuslogic",
        2,
        Presented::Cash,
    ),
    (
        "gameplay/progression/groupplaybonuslogic",
        1,
        Presented::Zero,
    ),
    (
        "gameplay/progression/groupplaybonuslogic",
        3,
        Presented::GroupSize,
    ),
    (
        "gameplay/scoring/speedlistcashrewardsprefab",
        1,
        Presented::Zero,
    ),
    (
        "gameplay/scoring/speedlistcashrewardsprefab",
        2,
        Presented::Cash,
    ),
    (
        "gameplay/scoring/speedlistcashrewardsprefab",
        3,
        Presented::Zero,
    ),
    (
        "gameplay/scoring/speedlistcashrewardsprefab",
        4,
        Presented::Cash,
    ),
    (
        "gameplay/scoring/speedlistcashrewardsprefab",
        5,
        Presented::Zero,
    ),
    (
        "gameplay/scoring/speedlistcashrewardsprefab",
        6,
        Presented::Zero,
    ),
];

/// One participant's finished race, the results logic's inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Finish {
    pub race_ms: i32,
    /// One-based finishing position.
    pub position: i32,
    /// Largest membership reached during this race, retained after departures.
    pub participants: usize,
}

/// The results logic's inputs: the finish and the participant properties the
/// race wrote (absent ones present zero).
#[derive(Clone, Copy, Debug)]
pub struct Results<'a> {
    pub finish: Finish,
    pub properties: &'a BTreeMap<Property, i32>,
    pub awards: Option<&'a nfs_content::events::awards::ResultValues>,
}

fn presented(field: &Field) -> Option<Presented> {
    RESULTS
        .iter()
        .find(|(asset, object, _)| field.asset == *asset && field.object == *object)
        .map(|(_, _, role)| *role)
}

/// Typed synchronized value of a creation field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
}

/// Where a logic is spawned: the event scene's start area, the game mode, or another
/// startup scene.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Owner {
    Scene,
    GameMode,
    Linked,
}

#[derive(Clone, Debug)]
pub struct Content {
    event: Event,
    category: String,
    record_id: u32,
    creation: CreationContent,
    states: BTreeMap<State, usize>,
    ai: Option<std::sync::Arc<crate::event_ai::Content>>,
    navigation: Option<std::sync::Arc<crate::event_ai::navigation::Navigation>>,
    awards: Option<std::sync::Arc<nfs_content::events::awards::Content>>,
    discovery: BTreeSet<nfs_content::events::discovery::Endpoint>,
}

fn bad<T>(_: T) -> Error {
    Error::Shape
}
fn rpc(selector: u16) -> Result<Rpc, Error> {
    Ok(Rpc {
        selector,
        serial: Serial::new(0).ok_or(Error::Shape)?.next_initialization(),
    })
}

fn rpc_bearing(kind: Kind) -> bool {
    !matches!(
        kind,
        Kind::BoolProperty | Kind::FloatProperty | Kind::I32Property | Kind::StringProperty
    )
}
/// The RPC selector of `layout`'s serializer: its ordinal among RPC-bearing
/// serializers, as the client numbers them.
pub fn selector(layout: &Layout, field: usize) -> Result<u16, Error> {
    let kind = layout.fields.get(field).ok_or(Error::Bound)?.kind;
    if !rpc_bearing(kind) {
        return Err(Error::TypeMismatch);
    }
    let n = layout.fields[..field]
        .iter()
        .filter(|f| rpc_bearing(f.kind))
        .count();
    u16::try_from(n)
        .ok()
        .filter(|n| *n <= 511)
        .ok_or(Error::Bound)
}

impl Content {
    pub fn new(event: Event, category: String) -> Result<Self, Error> {
        if category.is_empty() || category.len() > 128 {
            return Err(Error::Shape);
        }
        if event
            .game_mode
            .standings_interval
            .is_none_or(|interval| !interval.is_finite() || !(0.01..=60.).contains(&interval))
        {
            return Err(Error::Bound);
        }
        if event.rewards.cash < 0
            || event.rewards.rep < 0
            || event.placement_rewards.is_some_and(|tiers| {
                tiers[0] != event.rewards || tiers.iter().any(|r| r.cash < 0 || r.rep < 0)
            })
        {
            return Err(Error::Shape);
        }
        let record_id = nfs_content::events::record_id(&event.game_mode_id);
        if let Some(destinations) = &event.restart {
            if destinations.is_empty()
                || destinations.len() > nfs_content::events::restart::MAX_LOCATIONS
            {
                return Err(Error::Shape);
            }
            for destination in destinations {
                if destination
                    .position
                    .iter()
                    .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
                {
                    return Err(Error::Shape);
                }
                nfs_world::replication::vehicle::orientation::quaternion_from_basis(
                    destination.basis,
                )?;
            }
            let arrivals: Vec<_> = event
                .sequences
                .iter()
                .filter(|s| s.layout.blueprint == request::RESTART_ARRIVAL)
                .collect();
            if arrivals.len() != 1
                || arrivals[0].layout.fields.len() != 1
                || arrivals[0].layout.fields[0].kind != Kind::SequenceRoot
            {
                return Err(Error::Unsupported);
            }
        }
        let mut layouts: Vec<&Layout> = vec![&event.game_mode.layout];
        layouts.extend(&event.start_logics);
        layouts.extend(&event.game_mode_logics);
        layouts.extend(event.sequences.iter().map(|s| &s.layout));
        layouts.extend(event.scene_logics.iter().map(|s| &s.layout));
        layouts.extend(&event.linked_logics);
        let mut catalogs: BTreeMap<u16, &Vec<(u32, u32)>> = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        for layout in &layouts {
            if let Some(old) = catalogs.insert(layout.asset.bundle, &layout.catalog)
                && old != &layout.catalog
            {
                return Err(Error::Shape);
            }
            let profile = layout.profile().map_err(bad)?;
            if let Some(old) = profiles.insert(layout.asset, profile.clone())
                && old != profile
            {
                return Err(Error::Shape);
            }
        }
        let catalogs = catalogs
            .into_iter()
            .map(|(bundle, entries)| {
                let entries = entries
                    .iter()
                    .map(|&(kind, count)| Ok((u16::try_from(kind)?, u16::try_from(count)?)))
                    .collect::<std::result::Result<Vec<_>, std::num::TryFromIntError>>()
                    .map_err(|_| Error::Bound)?;
                Catalog::new(bundle, &entries)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let creation = CreationContent::new(catalogs, profiles.into_iter().collect())?;
        let fields = &event.game_mode.layout.fields;
        let mut states = BTreeMap::from([(State::Root, 0)]);
        for (state, asset, class) in STATES {
            let mut found = fields
                .iter()
                .enumerate()
                .filter(|(_, f)| f.asset == asset && f.class == class);
            let (index, _) = found.next().ok_or(Error::Unsupported)?;
            if found.next().is_some() {
                return Err(Error::DuplicateObject);
            }
            selector(&event.game_mode.layout, index)?;
            states.insert(state, index);
        }
        let waiting: Vec<_> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.class == "WaitForAllAiToSpawnStateEntityData")
            .collect();
        match waiting.as_slice() {
            [(index, _)] => {
                selector(&event.game_mode.layout, *index)?;
                states.insert(State::WaitForAi, *index);
            }
            _ => return Err(Error::Unsupported),
        }
        if fields.first().map(|f| f.kind) != Some(Kind::GameModeRoot)
            || event.game_mode.checkpoints.is_empty()
            || event.start_logics.is_empty()
            || !(1..=15).contains(&event.game_mode.laps)
            || !event.dnf_timeout.is_finite()
            || event.dnf_timeout <= 0.
        {
            return Err(Error::Shape);
        }
        // Every category the results present mirrors a participant property.
        if event
            .game_mode_logics
            .iter()
            .flat_map(|l| &l.fields)
            .any(|f| presented(f) == Some(Presented::Category) && f.mirrors.is_none())
        {
            return Err(Error::Shape);
        }
        let content = Self {
            event,
            category,
            record_id,
            creation,
            states,
            ai: None,
            navigation: None,
            awards: None,
            discovery: BTreeSet::new(),
        };
        for name in [
            request::INTRO_SEQUENCE,
            request::COUNTDOWN_SEQUENCE,
            request::OUTRO_SEQUENCE,
        ] {
            content.sequence(name)?;
        }
        content.spawner(request::INTRO_SEQUENCE)?;
        content.spawner(request::OUTRO_SEQUENCE)?;
        for name in [request::IN_GAME_MODE, request::COOLDOWN] {
            content.scene_logic(name)?;
        }
        Ok(content)
    }
    pub fn event(&self) -> &Event {
        &self.event
    }
    /// Enable the normal installed roster on the map independently of playable
    /// mode support. Keep the active event's notifications under its lifecycle.
    pub fn with_roster(
        mut self,
        roster: &[nfs_content::events::discovery::Entry],
    ) -> Result<Self, Error> {
        use nfs_content::events::discovery::{Endpoint, MAX_EVENTS};
        if roster.len() > MAX_EVENTS {
            return Err(Error::Bound);
        }
        let mut identities = BTreeSet::new();
        let mut notifications = BTreeSet::new();
        for entry in roster {
            if entry.id.is_empty()
                || entry.id.len() > 512
                || !identities.insert(&entry.id)
                || entry.notifications.len() > 16
            {
                return Err(Error::Shape);
            }
            for endpoint in &entry.notifications {
                if endpoint.scene_key == 0 {
                    return Err(Error::Shape);
                }
                notifications.insert(*endpoint);
            }
        }
        for serializer in self
            .event
            .area_notifications
            .iter()
            .chain(&self.event.scene_notifications)
        {
            notifications.remove(&Endpoint {
                scene_key: self.event.scene_content_key,
                serializer: *serializer,
            });
        }
        self.discovery = notifications;
        Ok(self)
    }
    pub(crate) fn discovery_notifications(
        &self,
    ) -> &BTreeSet<nfs_content::events::discovery::Endpoint> {
        &self.discovery
    }
    pub fn with_awards(
        mut self,
        awards: nfs_content::events::awards::Content,
    ) -> Result<Self, Error> {
        let layout = self.logic(Owner::GameMode, request::AWARDS, 0)?;
        if awards.fields.len() != layout.fields.len()
            || awards
                .fields
                .iter()
                .zip(&layout.fields)
                .enumerate()
                .any(|(i, (a, b))| a.serializer != i || a.kind != b.kind)
        {
            return Err(Error::Shape);
        }
        awards.program.validate().map_err(bad)?;
        self.awards = Some(std::sync::Arc::new(awards));
        Ok(self)
    }
    pub fn evaluate_awards(
        &self,
        participant: u16,
        properties: &[(u64, BTreeMap<Property, i32>)],
    ) -> Result<Option<nfs_content::events::awards::ResultValues>, Error> {
        self.awards
            .as_ref()
            .map(|a| a.evaluate(u64::from(participant), properties).map_err(bad))
            .transpose()
    }
    /// Race-outro compares the one-based position against the running maximum
    /// participant count. Last place selects "other"; remaining positions clamp
    /// to the fourth tier. Departures must not reduce `participants`.
    pub fn rewards_for_finish(&self, finish: Finish) -> Result<Rewards, Error> {
        let position = usize::try_from(finish.position).map_err(|_| Error::Bound)?;
        if !(1..=nfs_world::race_progress::MAX_PARTICIPANTS).contains(&finish.participants)
            || position == 0
            || position > finish.participants
        {
            return Err(Error::Bound);
        }
        match self.event.placement_rewards {
            Some(tiers) => Ok(tiers[if position == finish.participants {
                3
            } else {
                position.min(4) - 1
            }]),
            None if position == 1 => Ok(self.event.rewards),
            None => Err(Error::Unsupported),
        }
    }
    /// Prepare every authored opponent preset off the socket worker. The same
    /// immutable vehicle definitions serve garages, roaming rivals and events.
    pub fn with_ai(
        mut self,
        vehicles: crate::vehicle_content::Content,
        items: &nfs_services::item_content::ItemContent,
    ) -> Result<Self, Error> {
        if !self.event.ai.spawns.is_empty() {
            self.race_goal()?;
            self.outro_goals()?;
            if self.event.game_mode.pacing.is_none() || self.event.placement_rewards.is_none() {
                return Err(Error::Unsupported);
            }
            self.ai = Some(std::sync::Arc::new(crate::event_ai::Content::new(
                self.event.ai.clone(),
                vehicles,
                items,
            )?));
        }
        Ok(self)
    }

    pub fn ai(&self) -> Option<&crate::event_ai::Content> {
        self.ai.as_deref()
    }

    pub fn with_navigation(
        mut self,
        network: std::sync::Arc<nfs_world::road_distance::Network>,
    ) -> Result<Self, Error> {
        self.navigation = Some(std::sync::Arc::new(
            crate::event_ai::navigation::Navigation::new(
                network,
                &self.event.game_mode.checkpoint_points,
                self.event.game_mode.laps,
                self.event.game_mode.pacing.clone(),
            )?,
        ));
        Ok(self)
    }

    pub fn navigation(&self) -> Option<&crate::event_ai::navigation::Navigation> {
        self.navigation.as_deref()
    }

    /// The authored simultaneous wander/speed commands during the results
    /// sequence. These differ from the director's subsequent free-roam wander.
    pub fn outro_goals(&self) -> Result<Vec<RecordInitial>, Error> {
        let data = self
            .sequence(request::OUTRO_SEQUENCE)?
            .wander
            .ok_or(Error::Unsupported)?;
        if !data.speed_limit.is_finite()
            || data.speed_limit <= 0.
            || !data.timeout.is_finite()
            || data.timeout <= 0.
        {
            return Err(Error::Shape);
        }
        // The observed normal-event interpreter enables the random-offset flag.
        // Other authored flag combinations require their own native comparison.
        if data.urgent || !data.prefer_straight {
            return Err(Error::Unsupported);
        }
        Ok(vec![
            RecordInitial::WanderGoal(wander::Initial {
                allocation_serial: 0,
                flags: [data.urgent, data.prefer_straight, true],
            }),
            RecordInitial::SpeedLimitGoal(speed_limit::Initial {
                selector: 0,
                serial: 1,
                allocation_serial: 0,
                limit: data.speed_limit.to_bits(),
            }),
        ])
    }

    /// Native waypoint-channel command: all authored destinations, lap count,
    /// DoDrift, 150 m reach radius (0x463dae3), and replicated [index, counter].
    /// Goal allocation serial is assigned by the world's shared allocator.
    pub fn race_goal(&self) -> Result<(RecordInitial, nfs_world::replication::Update), Error> {
        let mode = &self.event.game_mode;
        let route = route_goal(mode)?;
        Ok((
            RecordInitial::GotoWaypointGoal(route),
            nfs_world::replication::Update::GotoWaypointGoal(goto_waypoint::Update::Progress(
                Some([0, 0]),
            )),
        ))
    }
    /// The participant properties the results present as category REP.
    pub fn presented_properties(&self) -> Vec<&Property> {
        self.event
            .game_mode_logics
            .iter()
            .flat_map(|l| &l.fields)
            .filter(|f| presented(f) == Some(Presented::Category))
            .filter_map(|f| f.mirrors.as_ref())
            .collect()
    }
    pub fn category(&self) -> &str {
        &self.category
    }
    pub fn record_id(&self) -> u32 {
        self.record_id
    }
    pub fn creation_content(&self) -> &CreationContent {
        &self.creation
    }
    pub fn game_mode_layout(&self) -> &Layout {
        &self.event.game_mode.layout
    }
    /// The game-mode serializer index of a lifecycle state.
    pub fn state(&self, state: State) -> usize {
        self.states[&state]
    }
    pub fn logic(&self, owner: Owner, name: &str, occurrence: usize) -> Result<&Layout, Error> {
        let list = match owner {
            Owner::Scene => &self.event.start_logics,
            Owner::GameMode => &self.event.game_mode_logics,
            Owner::Linked => &self.event.linked_logics,
        };
        list.iter()
            .filter(|l| l.blueprint == name)
            .nth(occurrence)
            .ok_or(Error::Unsupported)
    }
    pub fn linked(&self) -> &[Layout] {
        &self.event.linked_logics
    }
    pub fn scene_logic(&self, name: &str) -> Result<&SceneLogic, Error> {
        self.event
            .scene_logics
            .iter()
            .find(|s| s.layout.blueprint == name)
            .ok_or(Error::Unsupported)
    }
    pub fn sequence(&self, name: &str) -> Result<&Sequence, Error> {
        self.event
            .sequences
            .iter()
            .find(|s| s.layout.blueprint == name)
            .ok_or(Error::Unsupported)
    }
    /// The game-mode state that spawns a sequence.
    pub fn spawner(&self, name: &str) -> Result<usize, Error> {
        let index = self.sequence(name)?.spawner.ok_or(Error::Unsupported)?;
        selector(&self.event.game_mode.layout, index)?;
        Ok(index)
    }

    fn bound(
        &self,
        world: &World,
        binding: Binding,
        game_mode: Option<u16>,
    ) -> Result<(u16, u16), Error> {
        match binding {
            Binding::Scene {
                content_key,
                serializer,
            } => world
                .scene_endpoint(content_key, serializer)
                .ok_or(Error::UnknownObject),
            Binding::GameMode { serializer } => Ok((
                game_mode.ok_or(Error::Shape)?,
                selector(&self.event.game_mode.layout, serializer)?,
            )),
            Binding::Participant { .. } => Err(Error::Unsupported),
        }
    }

    /// A participant logic's creation. RPC references follow the layout's bindings,
    /// a reference list starts empty and a synchronized value comes from `value`.
    fn logic_creation(
        &self,
        layout: &Layout,
        blueprint: u16,
        world: &World,
        participant: u16,
        game_mode: Option<u16>,
        value: &dyn Fn(usize, &Field) -> Option<Value>,
    ) -> Result<Creation, Error> {
        let creation = logic_creation(
            layout,
            blueprint,
            participant,
            |binding| self.bound(world, binding, game_mode),
            value,
        )?;
        creation.encode(&self.creation)?;
        Ok(creation)
    }

    /// A start-area or linked-area logic, owned by the event scene.
    pub fn area_logic(
        &self,
        layout: &Layout,
        world: &World,
        participant: u16,
    ) -> Result<Creation, Error> {
        let scene = world
            .scene(self.event.scene_content_key)
            .ok_or(Error::UnknownObject)?;
        let starting = layout.blueprint == request::SHOW_PARTICIPANTS;
        self.logic_creation(layout, scene, world, participant, None, &|_, field| {
            // The participants the start area starts with: this participant alone
            // (E994 solo start: 1).
            (starting && field.kind == Kind::I32Property).then_some(Value::Int(1))
        })
    }

    /// A game-mode logic, owned by the game mode. The results logic presents
    /// the finish, the event's rewards and the race's category totals; the
    /// driving-style logic's Boolean RPCs take the E994 creation's initial values
    /// (false, false, true) pending their source.
    pub fn game_mode_logic(
        &self,
        layout: &Layout,
        world: &World,
        participant: u16,
        game_mode: u16,
        results: Option<Results<'_>>,
    ) -> Result<Creation, Error> {
        let rewards = results
            .map(|r| self.rewards_for_finish(r.finish))
            .transpose()?;
        let total_rep = results.map(|r| self.result_rep(r)).transpose()?;
        let driving = layout.blueprint == request::DRIVING_STYLE;
        let outro = layout.blueprint == request::OUTRO;
        let awards = layout.blueprint == request::AWARDS;
        if results.and_then(|r| r.awards).is_some_and(|a| {
            a.rep < 0
                || a.fields.keys().any(|i| {
                    self.logic(Owner::GameMode, request::AWARDS, 0)
                        .ok()
                        .and_then(|l| l.fields.get(*i))
                        .is_none_or(|f| f.kind != Kind::I32Property)
                })
        }) {
            return Err(Error::Shape);
        }
        let bools: Vec<usize> = layout
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind == Kind::RpcBool)
            .map(|(i, _)| i)
            .collect();
        let value = |index: usize, field: &Field| -> Option<Value> {
            if driving && field.kind == Kind::RpcBool {
                return Some(Value::Bool(
                    bools.iter().position(|&i| i == index) == Some(2),
                ));
            }
            if !outro {
                return awards
                    .then_some(())
                    .and_then(|_| results?.awards?.fields.get(&index).copied())
                    .map(Value::Int);
            }
            let Results {
                finish,
                properties,
                awards: _,
            } = results?;
            let rewards = rewards?;
            let category = |field: &Field| {
                field
                    .mirrors
                    .as_ref()
                    .and_then(|p| properties.get(p))
                    .copied()
                    .unwrap_or(0)
            };
            let number = match presented(field)? {
                Presented::Category => category(field),
                // Driving categories were banked during the event; the final
                // credit contains placement REP plus the selected award bonus.
                Presented::TotalRep => total_rep?,
                Presented::Zero => 0,
                Presented::EventRep => rewards.rep,
                Presented::Cash => rewards.cash,
                Presented::RaceTimeMs => finish.race_ms,
                Presented::Position => finish.position,
                Presented::GroupSize => 1,
            };
            Some(match field.kind {
                Kind::FloatProperty => Value::Float(number as f32),
                _ => Value::Int(number),
            })
        };
        self.logic_creation(
            layout,
            game_mode,
            world,
            participant,
            Some(game_mode),
            &value,
        )
    }

    /// The same frozen total is presented in the outro and sent to Speedwall.
    /// Driving REP was already credited while racing; this includes it once.
    pub fn result_rep(&self, results: Results<'_>) -> Result<i32, Error> {
        let rewards = self.rewards_for_finish(results.finish)?;
        Ok(self
            .logic(Owner::GameMode, request::OUTRO, 0)?
            .fields
            .iter()
            .filter(|f| presented(f) == Some(Presented::Category))
            .map(|f| {
                f.mirrors
                    .as_ref()
                    .and_then(|p| results.properties.get(p))
                    .copied()
                    .unwrap_or(0)
            })
            .fold(
                rewards
                    .rep
                    .saturating_add(results.awards.map_or(0, |a| a.rep)),
                i32::saturating_add,
            ))
    }

    /// A logic spawned in another startup scene (the Events scene's participant
    /// sets), owned by that scene.
    pub fn scene_logic_creation(
        &self,
        name: &str,
        world: &World,
        participant: u16,
    ) -> Result<Creation, Error> {
        let logic = self.scene_logic(name)?;
        let scene = world
            .scene(logic.scene_content_key)
            .ok_or(Error::UnknownObject)?;
        self.logic_creation(&logic.layout, scene, world, participant, None, &|_, _| None)
    }

    /// The game mode, owned by the game-mode manager's scene. The root holds the
    /// start time on the host replication clock, an age of zero, the three Gameplay
    /// bindings and the participant; the intro spawner state starts with the
    /// participant; every other state and the outro provider start empty.
    pub fn game_mode(
        &self,
        world: &World,
        participant: u16,
        start_seconds: f64,
    ) -> Result<Creation, Error> {
        if !start_seconds.is_finite() || start_seconds < 0. {
            return Err(Error::Shape);
        }
        let layout = &self.event.game_mode.layout;
        let (manager_key, _) = self.event.game_mode.root_bindings[1];
        world.scene(manager_key).ok_or(Error::UnknownObject)?;
        let mut bindings = [(0, 0); 3];
        for (slot, &(key, serializer)) in self.event.game_mode.root_bindings.iter().enumerate() {
            bindings[slot] = world
                .scene_endpoint(key, serializer)
                .ok_or(Error::UnknownObject)?;
        }
        let mut waypoints = Vec::with_capacity(self.event.game_mode.checkpoints.len());
        for &(key, serializer) in &self.event.game_mode.checkpoints {
            waypoints.push(
                world
                    .scene_endpoint(key, serializer)
                    .ok_or(Error::UnknownObject)?,
            );
        }
        let shape = nfs_world::game_mode::authored::Blueprint::new(
            layout.asset,
            layout.sub_id,
            layout.bus_count,
            &layout.fields.iter().map(|f| f.kind).collect::<Vec<_>>(),
        )?;
        let creation = shape.creation(
            nfs_world::game_mode::authored::Instance {
                bindings,
                participant,
                start_seconds,
                age_seconds: 0.,
                members: BTreeSet::from([self.spawner(request::INTRO_SEQUENCE)?]),
                laps: self.event.game_mode.laps,
                waypoints,
            },
            |index| {
                let binding = layout.fields[index].binding.ok_or(Error::Unsupported)?;
                self.bound(world, binding, None)
            },
        )?;
        creation.encode(&self.creation)?;
        Ok(creation)
    }

    /// The sequence manager's endpoint in the live world.
    pub fn manager(&self, world: &World, name: &str) -> Result<(u16, u16), Error> {
        let sequence = self.sequence(name)?;
        world
            .scene_endpoint(sequence.manager.0, sequence.manager.1)
            .ok_or(Error::UnknownObject)
    }

    /// A participant sequence, owned by its manager's scene. Its number is the
    /// manager's live sequence count (E662: the manager vector ordinal).
    pub fn sequence_creation(
        &self,
        name: &str,
        world: &World,
        participant: u16,
        game_mode: u16,
    ) -> Result<Creation, Error> {
        let sequence = self.sequence(name)?;
        let layout = &sequence.layout;
        let (manager, manager_selector) = self.manager(world, name)?;
        let number = live_sequences(world, (manager, manager_selector));
        let blueprint = nfs_world::sequences::authored::Blueprint::new(
            layout.asset,
            layout.sub_id,
            layout.bus_count,
            &layout
                .fields
                .iter()
                .map(|field| field.kind)
                .collect::<Vec<_>>(),
        )?;
        let creation = blueprint.creation(
            nfs_world::sequences::authored::Instance {
                manager: (manager, manager_selector),
                number,
                participant,
            },
            |index| {
                let binding = layout.fields[index].binding.ok_or(Error::Unsupported)?;
                self.bound(world, binding, Some(game_mode))
            },
        )?;
        creation.encode(&self.creation)?;
        Ok(creation)
    }

    /// A call on the live game mode's lifecycle state.
    pub fn state_call(
        &self,
        world: &World,
        game_mode: u16,
        state: State,
        method: u32,
        reference: Option<u16>,
        arguments: Arguments,
    ) -> Result<Call, Error> {
        let selector = selector(&self.event.game_mode.layout, self.state(state))?;
        call(world, game_mode, selector, method, reference, arguments)
    }
    /// A call on a game-mode serializer by index (sequence spawner states).
    pub fn field_call(
        &self,
        world: &World,
        game_mode: u16,
        field: usize,
        method: u32,
        reference: Option<u16>,
    ) -> Result<Call, Error> {
        let selector = selector(&self.event.game_mode.layout, field)?;
        call(
            world,
            game_mode,
            selector,
            method,
            reference,
            Arguments::None,
        )
    }
    /// Publish the authored invitation condition for the supported normal-event
    /// participant set. A missing synchronized value does not initialize the
    /// client's visibility property; publish false explicitly even in solo play.
    pub fn invitation_update(
        &self,
        players: &mut nfs_world::replication::players::Players,
        eligible_humans: usize,
    ) -> Result<Option<nfs_world::replication::Record>, Error> {
        let binding = self.event.invitations;
        let Some(scene) = players.objects().scene(binding.scene_content_key) else {
            return Ok(None);
        };
        let object = players.objects().get(scene).ok_or(Error::UnknownObject)?;
        let RecordInitial::SubLevel {
            fields: initial, ..
        } = &object.initial
        else {
            return Err(Error::TypeMismatch);
        };
        let value = eligible_humans > binding.count_above;
        match initial.get(binding.serializer) {
            Some(sublevel::Initial::BoolProperty(Some(old))) if *old == value => return Ok(None),
            Some(sublevel::Initial::BoolProperty(_)) => {}
            _ => return Err(Error::TypeMismatch),
        }
        let nfs_world::replication::Update::SubLevel { profile, .. } = &object.current else {
            return Err(Error::TypeMismatch);
        };
        let profile = profile.clone();
        let mut fields = vec![None; profile.kinds().len()];
        fields[binding.serializer] = Some(sublevel::Update::BoolProperty(Some(value)));
        players.change(
            scene,
            nfs_world::replication::Update::SubLevel { profile, fields },
        )
    }

    /// A call on one of the in-event flags (E997): method 0 sets it, 1 clears it.
    pub fn flag_call(&self, world: &World, index: usize, method: u32) -> Result<Call, Error> {
        let &(key, serializer) = self
            .event
            .game_mode_notifications
            .get(index)
            .ok_or(Error::Bound)?;
        let (scene, selector) = world
            .scene_endpoint(key, serializer)
            .ok_or(Error::UnknownObject)?;
        call(world, scene, selector, method, None, Arguments::None)
    }
    /// The intro's AI goals for `participant`'s Actor (E997): a go-to-waypoint goal
    /// with the intro's waypoint and reach radius, and a speed-limit goal. The
    /// first value 0, second value 1, unset Booleans and selector 0 are the
    /// official intro goal's; the RPC serial is a new component's first one.
    pub fn intro_goals(&self) -> Result<Option<Vec<RecordInitial>>, Error> {
        let Some(goto) = self.sequence(request::INTRO_SEQUENCE)?.goto else {
            return Ok(None);
        };
        Ok(Some(goto_goals(goto)?))
    }
    /// A call on an event-scene serializer.
    pub fn scene_call(
        &self,
        world: &World,
        serializer: usize,
        method: u32,
        reference: Option<u16>,
    ) -> Result<Call, Error> {
        let (scene, selector) = world
            .scene_endpoint(self.event.scene_content_key, serializer)
            .ok_or(Error::UnknownObject)?;
        call(world, scene, selector, method, reference, Arguments::None)
    }
}

/// Native arrival goals shared by event and story sequences.
pub(crate) fn goto_goals(goto: nfs_content::events::Goto) -> Result<Vec<RecordInitial>, Error> {
    let serial = Serial::new(0)
        .ok_or(Error::Shape)?
        .next_initialization()
        .value();
    Ok(vec![
        RecordInitial::GotoWaypointGoal(goto_waypoint::Initial {
            selector: 0,
            serial,
            allocation_serial: 0,
            waypoint: 0,
            word: 1,
            points: vec![goto.point.map(f32::to_bits)],
            flags: [false, false],
            radius: goto.radius.to_bits(),
            progress_updates: false,
        }),
        RecordInitial::SpeedLimitGoal(speed_limit::Initial {
            selector: 0,
            serial,
            allocation_serial: 0,
            limit: goto.speed_limit.to_bits(),
        }),
    ])
}

/// Shared typed participant-logic constructor. The caller resolves live bindings
/// and validates the result with its own catalog before publishing it.
pub(crate) fn logic_creation(
    layout: &Layout,
    blueprint: u16,
    participant: u16,
    resolve: impl FnMut(Binding) -> Result<(u16, u16), Error>,
    value: &dyn Fn(usize, &Field) -> Option<Value>,
) -> Result<Creation, Error> {
    logic_creation_fields(
        layout.asset,
        (layout.sub_id, layout.bus_count),
        blueprint,
        participant,
        layout
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.kind, f.binding, value(i, f))),
        resolve,
    )
}

/// Shared serializer construction for installed event and scene participant logic.
pub(crate) fn logic_creation_fields(
    asset: Asset,
    buses: (u32, u16),
    blueprint: u16,
    participant: u16,
    fields: impl ExactSizeIterator<Item = (Kind, Option<Binding>, Option<Value>)>,
    mut resolve: impl FnMut(Binding) -> Result<(u16, u16), Error>,
) -> Result<Creation, Error> {
    let mut initial = Vec::with_capacity(fields.len());
    let mut updates = Vec::with_capacity(fields.len());
    let mut next = 0u16;
    for (index, (kind, binding, value)) in fields.enumerate() {
        let mut take = || -> Result<Rpc, Error> {
            let current = rpc(next)?;
            next = next.checked_add(1).ok_or(Error::Bound)?;
            Ok(current)
        };
        let (first, update) = match kind {
            Kind::Root if index == 0 => (
                Initial::Root {
                    value: buses.1,
                    rpc: take()?,
                    reference: participant,
                },
                Update::Noop,
            ),
            Kind::RpcReference => {
                let binding = binding.ok_or(Error::Unsupported)?;
                let (reference, target_selector) = resolve(binding)?;
                (
                    Initial::RpcReference {
                        rpc: take()?,
                        reference,
                        target_selector,
                    },
                    Update::Noop,
                )
            }
            Kind::Rpc => (Initial::Rpc(take()?), Update::Noop),
            Kind::RpcBool => {
                let initial = match value {
                    Some(Value::Bool(v)) => v,
                    None => false,
                    Some(_) => return Err(Error::TypeMismatch),
                };
                (
                    Initial::RpcBool {
                        rpc: take()?,
                        value: initial,
                    },
                    Update::Noop,
                )
            }
            Kind::RpcReferences => (
                Initial::RpcReferences {
                    rpc: take()?,
                    references: Vec::new(),
                },
                Update::Noop,
            ),
            Kind::BoolProperty => match value {
                Some(Value::Bool(v)) => {
                    (Initial::BoolProperty(Some(v)), Update::BoolProperty(None))
                }
                None => (Initial::BoolProperty(None), Update::BoolProperty(None)),
                Some(_) => return Err(Error::TypeMismatch),
            },
            Kind::I32Property => match value {
                Some(Value::Int(v)) => (Initial::I32Property(Some(v)), Update::I32Property(None)),
                None => (Initial::I32Property(None), Update::I32Property(None)),
                Some(_) => return Err(Error::TypeMismatch),
            },
            Kind::FloatProperty => match value {
                Some(Value::Float(v)) if v.is_finite() => (
                    Initial::FloatProperty(Some(v.to_bits())),
                    Update::FloatProperty(None),
                ),
                None => (Initial::FloatProperty(None), Update::FloatProperty(None)),
                Some(_) => return Err(Error::TypeMismatch),
            },
            Kind::StringProperty => (Initial::StringProperty(None), Update::StringProperty(None)),
            _ => return Err(Error::Unsupported),
        };
        initial.push(first);
        updates.push(Some(update));
    }
    let creation = Creation {
        prefix: Prefix {
            parent: None,
            blueprint,
            sub_id: buses.0,
            owner: None,
            asset,
        },
        body: Body {
            initial: Some(initial),
            updates,
        },
    };
    Ok(creation)
}

/// Live sequences that name the manager endpoint.
pub fn live_sequences(world: &World, manager: (u16, u16)) -> u32 {
    world
        .iter()
        .filter(|(_, object)| {
            matches!(&object.initial, RecordInitial::Entity { fields, .. }
                if matches!(fields.first(), Some(Initial::SequenceRoot { manager: m, manager_selector: s, .. })
                    if (*m, *s) == manager))
        })
        .count() as u32
}

/// A call on a live object's serializer with the serial it was created with.
pub fn call(
    world: &World,
    target: u16,
    selector: u16,
    method: u32,
    reference: Option<u16>,
    arguments: Arguments,
) -> Result<Call, Error> {
    let object = world.get(target).ok_or(Error::UnknownObject)?;
    let serial = match &object.initial {
        RecordInitial::Entity { fields, .. } => fields
            .iter()
            .filter_map(Initial::rpc)
            .find(|r| r.selector == selector)
            .map(|r| r.serial),
        RecordInitial::SubLevel { fields, .. } => fields
            .iter()
            .filter_map(sublevel::Initial::rpc)
            .find(|r| r.selector == selector)
            .map(|r| r.serial),
        _ => None,
    }
    .ok_or(Error::TypeMismatch)?;
    if reference.is_some_and(|id| world.get(id).is_none()) {
        return Err(Error::UnknownObject);
    }
    let call = Call {
        target,
        selector,
        serial,
        method,
        reference,
        arguments,
    };
    call.encode().map_err(|_| Error::Bound)?;
    Ok(call)
}

/// Whether a record is a live entity created from `asset`.
pub fn is_asset(world: &World, ghost: u16, asset: Asset) -> bool {
    matches!(world.get(ghost).map(|o| &o.initial),
        Some(RecordInitial::Entity { prefix, .. }) if prefix.asset == asset)
}

#[cfg(test)]
pub(crate) mod tests;
