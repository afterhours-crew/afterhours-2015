// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `map-images`: decode the game's world map into the data folder.

use super::Context;
use crate::{
    cli::MapImagesArgs,
    exit::{Failure, Result},
    map, profile,
};
use std::fmt::Write;

/// `map-images`: decode the world map's tiles into the data folder, or
/// report the complete set already there.
pub fn map_images(context: &Context, args: &MapImagesArgs) -> Result<()> {
    let report = map_images_report(&context.data_dir, args.game_dir.as_deref())?;
    map_images_emit(context, &report)
}

/// The `map-images` record: decode the tile set, or reuse a complete one;
/// the session API's `POST /v1/map-images` answers with it.
pub(crate) fn map_images_report(
    data: &std::path::Path,
    game_dir: Option<&std::path::Path>,
) -> Result<map::MapImagesReport> {
    let data = crate::paths::checked_data_path(data, None)?;
    let source = profile::content::locate(&data, None)?.ok_or_else(|| {
        Failure::no_session("This data directory has no prepared content; run setup first.")
    })?;
    let fingerprint = source
        .directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    let prepared = match map::load(&data, &fingerprint) {
        Ok(Some(cached)) => map::Prepared {
            cached,
            built: false,
        },
        _ => {
            let (registered, _) = crate::platform::game_registration();
            let (game, _) = profile::installation(&data, game_dir, registered)?
                .ok_or_else(|| Failure::refused("No game installation found; pass --game-dir."))?;
            map::prepare(&data, &fingerprint, &game, nfs_frostbite::Limits::default())?
        }
    };
    Ok(map::MapImagesReport::new(&prepared, &fingerprint))
}

fn map_images_emit(context: &Context, report: &map::MapImagesReport) -> Result<()> {
    context.emit(report, || {
        let mut out = format!(
            "{} {} map tiles in\n  {}",
            if report.built { "Decoded" } else { "Reused" },
            report.files,
            report.directory.display()
        );
        for level in &report.levels {
            let _ = write!(
                out,
                "\n  Level {}: {} px, {}x{} tiles, {} decoded",
                level.level, level.size, level.tiles_per_side, level.tiles_per_side, level.tiles
            );
        }
        if let Some(world) = report.world {
            let _ = write!(
                out,
                "\n  World box: x {} to {}, z {} to {} m",
                world.min_x, world.max_x, world.min_z, world.max_z
            );
        }
        for missing in &report.missing {
            let _ = write!(
                out,
                "\n  No tile {}-{}-{}: {}",
                missing.level, missing.column, missing.row, missing.reason
            );
        }
        for warning in &report.warnings {
            let _ = write!(out, "\n  Note: {warning}");
        }
        out
    })
}
