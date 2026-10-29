// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    data: Box<intro::Mode>,
    messages: Vec<(f32, u32)>,
    parent: Asset,
    sequences: BTreeMap<usize, Authored>,
    timing: BTreeMap<usize, f64>,
    reporters: Option<Arc<reporters::Source>>,
}
impl From<Mode> for Source {
    fn from(v: Mode) -> Self {
        Self {
            data: Box::new(v.data),
            messages: v.messages,
            parent: v.parent,
            sequences: v.sequences,
            timing: v.timing,
            reporters: v.reporter_source,
        }
    }
}
impl TryFrom<Source> for Mode {
    type Error = String;
    fn try_from(v: Source) -> Result<Self, Self::Error> {
        let invalid = || "prepared intro mode shape".to_owned();
        let spawners = v
            .data
            .sequences
            .iter()
            .map(|s| s.spawner.ok_or_else(invalid))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if spawners.len() != v.data.sequences.len()
            || spawners != v.sequences.keys().copied().collect()
            || v.timing.iter().any(|(id, t)| {
                !spawners.contains(id) || !t.is_finite() || !(0. ..=600.).contains(t)
            })
            || v.messages.len() != v.data.timed_messages.len()
            || v.messages
                .iter()
                .zip(&v.data.timed_messages)
                .any(|((seconds, _), message)| {
                    !seconds.is_finite()
                        || *seconds != message.seconds
                        || !(0. ..=600.).contains(seconds)
                })
            || v.data
                .timed_events
                .iter()
                .any(|e| !e.seconds.is_finite() || !(0. ..=600.).contains(&e.seconds))
        {
            return Err(invalid());
        }
        let mut mode = Mode::new(*v.data, v.parent, v.sequences)
            .map_err(|e| format!("prepared mode: {e:?}"))?;
        let source = v.reporters.ok_or_else(invalid)?;
        mode.reporters = Some(
            reporters::Reporters::from_source(&source, &mode.data, &mode.plan)
                .map_err(|e| format!("prepared reporters: {e}"))?,
        );
        mode.reporter_source = Some(source);
        mode.messages = v.messages;
        mode.timing = v.timing;
        Ok(mode)
    }
}
