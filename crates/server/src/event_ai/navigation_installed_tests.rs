// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Installed-content coverage of the race navigation query and the authored
//! story rosters (E1280 opponent-difficulty lane). Gated on
//! `AFTERHOURS_GAME_DIR`; without it every test prints "skipped" and passes
//! without asserting anything about the installation.
use super::*;
use nfs_content::{Options, assets::AssetIndex, events::GameMode};
use nfs_world::{race_pacing, race_progress, road_distance::Location};
use std::path::Path;

const STEP: f32 = 10.;

fn index() -> Option<AssetIndex> {
    let Some(dir) = std::env::var_os("AFTERHOURS_GAME_DIR") else {
        eprintln!("skipped: AFTERHOURS_GAME_DIR is not set");
        return None;
    };
    Some(AssetIndex::open(Path::new(&dir), &Options::default()).unwrap())
}

struct Course {
    id: String,
    mode: GameMode,
    /// The first opponent grid slot; None for a course without a roster.
    grid: Option<[f32; 3]>,
    roster: Option<nfs_content::events::ai::Roster>,
}

fn sprint(index: &mut AssetIndex, id: &str) -> Course {
    let availability = index.event_availability().unwrap();
    let entry = availability
        .entries
        .iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("{id} is not an available event"));
    let context = crate::event_logic::request::sprint(entry).unwrap();
    let resolved = index.event_request(&context).unwrap();
    let event = index.event(&resolved).unwrap();
    Course {
        id: id.into(),
        grid: event.ai.spawns.first().map(|s| s.position),
        roster: Some(event.ai.clone()),
        mode: event.game_mode,
    }
}

fn prologue(index: &mut AssetIndex) -> Course {
    let mode = nfs_content::events::intro::extract(
        index,
        "gameplay/progression/intro/introracegamemode",
        Some("gameplay/progression/intro/introracespikeandamy"),
    )
    .unwrap();
    Course {
        id: "gameplay/progression/intro/introracegamemode (spike and amy)".into(),
        mode: mode.game_mode,
        grid: None,
        roster: None,
    }
}

fn chord_samples(a: [f32; 3], b: [f32; 3]) -> Vec<[f32; 3]> {
    let d: f32 = (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f32>().sqrt();
    let n = (d / STEP).ceil().max(1.) as usize;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
        })
        .collect()
}

/// Every authored checkpoint, every road-path sample between them and every
/// straight-chord sample resolves to a road location; the distance to the next
/// checkpoint and the measured remaining distance fall monotonically along the
/// road path; `measure` reports no location miss anywhere on the course.
fn cover(network: &Arc<Network>, course: &Course) {
    let points = &course.mode.checkpoint_points;
    let nav = Navigation::new(
        network.clone(),
        points,
        course.mode.laps,
        course.mode.pacing.clone(),
    )
    .unwrap();
    let locations: Vec<Location> = points
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            network
                .locate(p)
                .unwrap()
                .unwrap_or_else(|| panic!("{}: checkpoint {i} {p:?} off road", course.id))
        })
        .collect();
    let start = course.grid.unwrap_or(points[0]);
    let route = nav.start(start).unwrap();
    let participant = 7;
    let mut progress =
        race_progress::Race::new(points.len(), course.mode.laps, &[participant], 0.).unwrap();
    let (mut chord_misses, mut road_samples, mut chord_count) = (0usize, 0usize, 0usize);
    let mut last_remaining = i32::MAX;
    let mut legs = Vec::new();
    for i in 0..points.len() {
        let from = if i == 0 {
            network.locate(start).unwrap().unwrap()
        } else {
            locations[i - 1]
        };
        let target = network.prepare_target(locations[i]).unwrap();
        let samples = network
            .path_samples(from, &target, STEP)
            .unwrap()
            .unwrap_or_else(|| panic!("{}: no road route into checkpoint {i}", course.id));
        let mut last_distance = f32::INFINITY;
        for (k, &sample) in samples.iter().enumerate() {
            let location = network.locate(sample).unwrap().unwrap_or_else(|| {
                panic!(
                    "{}: road sample {k} before checkpoint {i} off road",
                    course.id
                )
            });
            let distance = network.distance(location, &target).unwrap().unwrap();
            assert!(
                distance <= last_distance + 1.,
                "{}: distance to checkpoint {i} rose {last_distance} -> {distance} at sample {k}",
                course.id
            );
            last_distance = distance;
            if nav.config.is_some() {
                let measured = nav
                    .measure(
                        &route,
                        &progress,
                        &[Sample {
                            participant,
                            position: sample,
                            base_skill: 0.5,
                        }],
                        Some(participant),
                    )
                    .unwrap();
                assert!(
                    measured.misses.is_empty(),
                    "{}: measure missed at sample {k} before checkpoint {i}",
                    course.id
                );
                let remaining = measured.remaining[0].1;
                assert!(
                    remaining <= last_remaining.saturating_add(1_000),
                    "{}: remaining rose {last_remaining} -> {remaining} mm at sample {k} before checkpoint {i}",
                    course.id
                );
                last_remaining = remaining;
            }
        }
        road_samples += samples.len();
        legs.push((
            i,
            samples.len(),
            network.distance(from, &target).unwrap().unwrap(),
        ));
        if i > 0 {
            for sample in chord_samples(points[i - 1], points[i]) {
                chord_count += 1;
                if network.locate(sample).unwrap().is_none() {
                    chord_misses += 1;
                }
            }
        }
        progress.cross(participant, i, i as f64 + 1.).unwrap();
    }
    eprintln!(
        "{}: {} checkpoints, {} road samples, {} chord samples ({} chord misses), legs {:?}",
        course.id,
        points.len(),
        road_samples,
        chord_count,
        chord_misses,
        legs
    );
    assert_eq!(
        chord_misses, 0,
        "{}: straight-chord samples off road",
        course.id
    );
}

#[test]
fn installed_sprint_007_and_prologue_routes_locate_every_sample_and_measure_monotonically() {
    let Some(mut index) = index() else { return };
    let network = Arc::new(index.road_distance().unwrap());
    for course in [
        sprint(&mut index, "gameplay/gamemodes/sprint_race_007"),
        sprint(&mut index, "gameplay/gamemodes/sprint_race_007_nar"),
        prologue(&mut index),
    ] {
        cover(&network, &course);
    }
}

/// E1278 capture, first career race (mode 649 at 99996 ms): participant 636
/// (26-byte name, Actor words [2, 8]) plus four 27-byte generic racers with
/// words [9, 3], [14, 3], [12, 3], [6, 3]; second race (mode 3349 at 663296
/// ms): one opponent, Actor words [2, 8]. The authored rosters match that
/// capture without any condition on the priority spawn.
#[test]
fn installed_story_rosters_match_the_e1278_capture_amy_plus_four_then_amy_alone() {
    let Some(mut index) = index() else { return };
    let first = sprint(&mut index, "gameplay/gamemodes/sprint_race_007_nar");
    let roster = first.roster.unwrap();
    assert_eq!(roster.spawns.len(), 5);
    let amy = &roster.spawns[0];
    assert_eq!(amy.personas.len(), 1);
    let persona = &roster.personas[amy.personas[0]];
    assert_eq!(persona.name, "ID_NARRATIVE_AIPERSONA_AMY");
    assert_eq!(persona.name.len(), 26);
    assert_eq!((persona.provider_index, persona.personality_index), (2, 8));
    assert!(amy.ignore_availability);
    assert_eq!(roster.pacing_skills[amy.personas[0]], Some(0.5));
    for spawn in &roster.spawns[1..] {
        assert_eq!(spawn.personas.len(), 18);
        for &p in &spawn.personas {
            let persona = &roster.personas[p];
            assert_eq!(persona.name.len(), 27, "{}", persona.name);
            assert!(persona.name.starts_with("ID_AIPERSONA_EVENT_CAR_"));
            assert_eq!(persona.personality_index, 3);
            assert_eq!(roster.pacing_skills[p], Some(0.5));
        }
    }
    let second = sprint(&mut index, "gameplay/gamemodes/sprint_race_034_nar");
    let roster = second.roster.unwrap();
    assert_eq!(roster.spawns.len(), 1);
    let persona = &roster.personas[roster.spawns[0].personas[0]];
    assert_eq!(persona.name, "ID_NARRATIVE_AIPERSONA_AMY");
    assert_eq!((persona.provider_index, persona.personality_index), (2, 8));
}

/// Documented divergence, not a gate: our rank-1 creation value against the
/// official 0.5016 (race 1, bits 1056991450) and 0.5019 (ordinary replay,
/// 1056995952); ranks 2–5 must still equal the official creation values.
/// Prints the target difference in metres the authored schedule needs for
/// each official rank-1 value.
#[test]
fn installed_sprint_007_rank_one_creation_value_divergence_is_reported() {
    let Some(mut index) = index() else { return };
    let course = sprint(&mut index, "gameplay/gamemodes/sprint_race_007_nar");
    let config = course.mode.pacing.clone().unwrap();
    let roster = course.roster.unwrap();
    let network = Arc::new(index.road_distance().unwrap());
    let nav = Navigation::new(
        network.clone(),
        &course.mode.checkpoint_points,
        course.mode.laps,
        Some(config.clone()),
    )
    .unwrap();
    let route = nav.start(roster.spawns[0].position).unwrap();
    let total = route.total_distance();
    let distances: Vec<f32> = roster
        .spawns
        .iter()
        .map(|s| {
            let d = nav.distance(s.position, 0).unwrap().unwrap();
            route.progress(0, 1, d).unwrap()
        })
        .collect();
    // The human at the leader's slot, then the five grid slots in order.
    let members: Vec<race_pacing::Member> = (0..6)
        .map(|i| race_pacing::Member {
            participant: 100 + i as u16,
            lap: 1,
            passed: 0,
            crossing_seconds: 0.,
            remaining_lap: total,
            distance: if i == 0 {
                distances[0]
            } else {
                distances[i - 1]
            },
            base_skill: 0.5,
        })
        .collect();
    let slots = config
        .evaluate_members_detailed(&members, Some(100), total)
        .unwrap();
    eprintln!(
        "grid slot progress {distances:?}; values at the slots {:?}",
        slots
            .iter()
            .map(|e| (e.value, e.value.to_bits(), e.difference))
            .collect::<Vec<_>>()
    );
    // The official creation values arise with every car level with the human.
    let level: Vec<race_pacing::Member> = members
        .iter()
        .map(|m| race_pacing::Member { distance: 0., ..*m })
        .collect();
    let values = config
        .evaluate_members_detailed(&level, Some(100), total)
        .unwrap();
    eprintln!(
        "values with every car level {:?}",
        values
            .iter()
            .map(|e| (e.value, e.value.to_bits(), e.difference))
            .collect::<Vec<_>>()
    );
    for official in [1056991450u32, 1056995952] {
        let wanted = f32::from_bits(official);
        let (mut lo, mut hi) = (-250f32, 250f32);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.;
            let value = config
                .evaluate(
                    5,
                    1,
                    race_pacing::Progress {
                        distance: mid,
                        total_distance: total,
                        reference_distance: 0.,
                        base_skill: 0.5,
                    },
                )
                .unwrap();
            if value > wanted {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        eprintln!(
            "official rank-1 {wanted} (bits {official}) needs distance-minus-reference {:.2} m at creation; ours is {:.2} m giving {} (bits {})",
            (lo + hi) / 2.,
            0.,
            values[1].value,
            values[1].value.to_bits()
        );
    }
    assert_eq!(values[2].value.to_bits(), 1057030511);
    assert_eq!(values[3].value.to_bits(), 1057061679);
    assert_eq!(values[4].value.to_bits(), 1057070094);
    assert_eq!(values[5].value.to_bits(), 1057019889);
}
