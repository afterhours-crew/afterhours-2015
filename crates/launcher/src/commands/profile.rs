// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `profile`: show one saved driver or list every saved driver, read-only.

use super::Context;
use crate::{
    cli::ProfileArgs,
    exit::{Failure, Result},
    profile::{self, CarLocation, Cell, Columns, Listing, Profile, content::Need, grouped},
};
use std::fmt::Write;

/// `profile`: one account's profile, or with `--list` every account.
pub fn profile(context: &Context, args: &ProfileArgs) -> Result<()> {
    let saves = profile::Saves::open(&context.data_dir)?;
    if args.list {
        let content = profile::content(saves.data(), args.content_dir.as_deref(), Need::Summary)?;
        let listing = saves.list(&content)?;
        return context.emit(&listing, || list_text(&listing));
    }
    let id = match &args.account_id {
        Some(id) => id.clone(),
        None => saves.active().map(str::to_owned).ok_or_else(|| {
            Failure::no_session(
                "No saved driver is selected in this data directory; run setup first, or pass --account-id.",
            )
        })?,
    };
    profile::parse_account(&id)?;
    let mut content = profile::content(saves.data(), args.content_dir.as_deref(), Need::Full)?;
    let (install_dir, locale) = crate::platform::game_registration();
    profile::localize(
        &mut content,
        saves.data(),
        args.game_dir.as_deref(),
        args.language.as_deref(),
        profile::Registration {
            install_dir,
            locale,
        },
    )?;
    let profile = saves.profile(&id, &content)?;
    context.emit(&profile, || text(&profile))
}

fn cell(cell: Option<Cell>) -> String {
    match cell {
        Some(Cell {
            value,
            stored: true,
        }) => grouped(i64::from(value)),
        Some(Cell {
            value,
            stored: false,
        }) => format!("{} (not saved)", grouped(i64::from(value))),
        None => "unavailable".into(),
    }
}

fn number(value: Option<impl Into<i64>>) -> String {
    value.map_or_else(|| "unavailable".into(), |v| grouped(v.into()))
}

fn columns(out: &mut String, table: &str, columns: &Columns) {
    let _ = writeln!(out, "  {table}");
    for (name, value) in &columns.0 {
        let _ = writeln!(out, "    {name:<24} {}", cell(*value));
    }
}

/// Plain-text profile. Stored integers are shown without units.
fn text(p: &Profile) -> String {
    let mut out = String::new();
    let name = p
        .identity
        .as_ref()
        .map_or("Unnamed driver", |d| d.display_name.as_str());
    let active = if p.active { " (selected)" } else { "" };
    let _ = writeln!(out, "{name}{active}");
    match &p.identity {
        Some(driver) => {
            let _ = writeln!(
                out,
                "  Persona {}, account {}",
                driver.persona_id, p.account_id
            );
        }
        None => {
            let _ = writeln!(out, "  Account {}", p.account_id);
        }
    }
    let bank = p
        .bank
        .as_ref()
        .map_or("unavailable", |b| b.formatted.as_str());
    let _ = writeln!(out, "  Bank {bank}");
    if let Some(r) = &p.reputation {
        let _ = writeln!(
            out,
            "  REP level {} of {}, score {}, next level at {}",
            number(r.level),
            number(r.maximum_level),
            cell(r.score),
            number(r.next_threshold)
        );
        let c = &r.categories;
        let _ = writeln!(
            out,
            "  Build {}, crew {}, outlaw {}, speed {}, style {}",
            cell(c.build),
            cell(c.crew),
            cell(c.outlaw),
            cell(c.speed),
            cell(c.style)
        );
    }
    if let Some(garage) = &p.garage {
        let _ = writeln!(out, "Garage");
        for slot in &garage.slots {
            let label = slot.item.as_ref().map_or_else(
                || "empty".to_owned(),
                |item| match (&item.name, &item.vehicle, item.class) {
                    (Some(name), _, _) => name.clone(),
                    (None, Some(vehicle), _) => vehicle.label.clone(),
                    (None, None, Some(class)) => format!("{class} {}", item.definition),
                    (None, None, None) => format!("item {}", item.definition),
                },
            );
            let primary = if slot.primary { " (primary)" } else { "" };
            let _ = writeln!(out, "  {}. {label}{primary}", slot.index + 1);
        }
    }
    if let (Some(cars), Some(summary)) = (&p.cars, &p.car_summary) {
        let _ = writeln!(
            out,
            "Cars: garage {} of {}, warehouse {} of {}, total sale value ${}",
            summary.garage_used,
            summary.garage_slots,
            summary.warehouse_used,
            summary.warehouse_limit,
            grouped(i64::try_from(summary.total_sale_value).unwrap_or(i64::MAX))
        );
        for car in cars {
            let place = match (car.location, car.slot) {
                (CarLocation::Active, _) => "active".to_owned(),
                (CarLocation::Garage, Some(slot)) => format!("garage {}", slot + 1),
                _ => "warehouse".to_owned(),
            };
            let name = car
                .name
                .clone()
                .or_else(|| car.vehicle.as_ref().map(|vehicle| vehicle.label.clone()))
                .unwrap_or_else(|| format!("car {}", car.definition));
            let _ = writeln!(
                out,
                "  {place:<10} {name}: {} installed, {} spare, sells for ${}",
                car.installed_parts,
                car.spare_parts,
                grouped(i64::from(car.sale_price))
            );
        }
    }
    if let Some(tables) = &p.tables {
        let _ = writeln!(out, "Saved stats (raw stored integers)");
        for (table, values) in tables {
            columns(&mut out, table, values);
        }
    }
    let n = &p.counts;
    let rows = |value: Option<usize>| value.map_or_else(|| "none".into(), |v| grouped(v as i64));
    let _ = writeln!(
        out,
        "Collection: {} items, {} liveries ({} bytes), {} collectibles, {} activities, {} places",
        grouped(n.items as i64),
        n.livery_records,
        grouped(n.livery_bytes as i64),
        rows(n.collectibles),
        rows(n.activities),
        rows(n.points_of_interest)
    );
    let s = &p.save;
    let _ = writeln!(
        out,
        "Save: schema {}, generation {}, last saved {}, {} bytes\n  {}",
        s.schema_version,
        s.generation,
        s.updated_at_utc.as_deref().unwrap_or("never"),
        grouped(s.file_bytes as i64),
        s.path.display()
    );
    for warning in &p.warnings {
        let _ = writeln!(out, "Note: {warning}");
    }
    out.trim_end().to_owned()
}

fn list_text(listing: &Listing) -> String {
    if listing.accounts.is_empty() {
        return "No saved drivers in this data directory.".into();
    }
    let mut out = String::new();
    for entry in &listing.accounts {
        let marker = if entry.active { "*" } else { " " };
        match &entry.error {
            Some(error) => {
                let _ = writeln!(out, "{marker} {}  {error}", entry.account_id);
            }
            None => {
                let _ = writeln!(
                    out,
                    "{marker} {}  {}  REP {}  {}  last saved {}",
                    entry.account_id,
                    entry.display_name.as_deref().unwrap_or("Unnamed driver"),
                    number(entry.level),
                    entry
                        .bank
                        .as_ref()
                        .map_or("unavailable", |b| b.formatted.as_str()),
                    entry.updated_at_utc.as_deref().unwrap_or("never"),
                );
            }
        }
    }
    if listing.truncated {
        let _ = writeln!(
            out,
            "Only the first {} saves are listed.",
            profile::MAX_ACCOUNTS
        );
    }
    for warning in &listing.warnings {
        let _ = writeln!(out, "Note: {warning}");
    }
    out.trim_end().to_owned()
}
