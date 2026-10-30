// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
use super::*;

/// Constructor inputs only. Codec profiles and active sequence state are rebuilt.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    data: Sequence,
    channels: BTreeMap<usize, ChannelBinding>,
    game_mode: Option<Asset>,
    parent: Asset,
    stop_signals: Vec<Binding>,
}
impl From<Authored> for Source {
    fn from(value: Authored) -> Self {
        Self {
            data: value.data,
            channels: value.channels,
            game_mode: value.game_mode,
            parent: value.parent,
            stop_signals: value.stop_signals,
        }
    }
}
impl TryFrom<Source> for Authored {
    type Error = String;
    fn try_from(value: Source) -> Result<Self, Self::Error> {
        Self::build(
            value.data,
            value.channels,
            value.parent,
            value.game_mode,
            value.stop_signals,
        )
        .map_err(|e| format!("prepared sequence: {e:?}"))
    }
}
