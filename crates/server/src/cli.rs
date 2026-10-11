// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_server::{Failure, content, deployment::Deployment, net, record};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Args {
    root: PathBuf,
    world_mac_template: Option<PathBuf>,
    output: PathBuf,
    redirector_port: u16,
    idle_seconds: u64,
    qos_seconds: u64,
    stop_file: Option<PathBuf>,
    world_content: Option<PathBuf>,
    item_content: Option<PathBuf>,
    persistent_content: Option<PathBuf>,
    progression_content: Option<PathBuf>,
    vehicle_content: Option<PathBuf>,
    garage_layout: Option<PathBuf>,
    sequence_content: Option<PathBuf>,
    garage_logic: Option<PathBuf>,
    control_catalogs: Option<PathBuf>,
    entitlement_state: Option<PathBuf>,
    user_settings: Option<PathBuf>,
    kickback_state: Option<PathBuf>,
    speedwall_state: Option<PathBuf>,
    item_licenses: Option<PathBuf>,
    stat_definitions: Option<PathBuf>,
    group_policy: Option<PathBuf>,
    matchmaking_admission: Option<PathBuf>,
    matchmaking_policy: Option<PathBuf>,
    world_policy: Option<PathBuf>,
    owned_menu_awards: bool,
    owned_local_social: bool,
    owned_world_readiness: bool,
    owned_world_attributes: bool,
    owned_world_connection: bool,
    owned_only: bool,
    challenge_content: Option<PathBuf>,
    bootstrap_config: Option<PathBuf>,
    auth_config: Option<PathBuf>,
    local_identity: Option<PathBuf>,
    state_directory: Option<PathBuf>,
    local_account: Option<nfs_storage::AccountId>,
}

fn parse() -> Result<Args, String> {
    parse_from(std::env::args_os().skip(1))
}
fn parse_from(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<Args, String> {
    let mut root = std::env::current_dir().map_err(|e| e.to_string())?;
    let (mut output, mut stop_file, mut world_content) = (None, None, None);
    let (mut redirector_port, mut idle_seconds, mut qos_seconds) = (0, 120, 900);
    let (mut item_content, mut state_directory, mut local_account) = (None, None, None);
    let mut persistent_content = None;
    let mut progression_content = None;
    let (mut vehicle_content, mut garage_layout) = (None, None);
    let mut sequence_content = None;
    let mut garage_logic = None;
    let mut control_catalogs = None;
    let mut entitlement_state = None;
    let (mut kickback_state, mut speedwall_state, mut item_licenses) = (None, None, None);
    let mut user_settings = None;
    let mut stat_definitions = None;
    let mut group_policy = None;
    let mut matchmaking_admission = None;
    let mut matchmaking_policy = None;
    let mut world_policy = None;
    let mut owned_menu_awards = false;
    let mut owned_local_social = false;
    let mut owned_world_readiness = false;
    let mut owned_world_attributes = false;
    let mut owned_world_connection = false;
    let mut owned_only = true;
    let mut world_mac_template = None;
    let mut challenge_content = None;
    let mut bootstrap_config = None;
    let (mut auth_config, mut local_identity) = (None, None);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing value for {flag:?}"))
        };
        match flag.to_str() {
            Some("--root") => root = value()?.into(),
            Some("--output") => output = Some(PathBuf::from(value()?)),
            Some("--stop-file") => stop_file = Some(PathBuf::from(value()?)),
            Some("--world-content") => world_content = Some(PathBuf::from(value()?)),
            Some("--item-content") => item_content = Some(PathBuf::from(value()?)),
            Some("--persistent-content") => persistent_content = Some(PathBuf::from(value()?)),
            Some("--progression-content") => progression_content = Some(PathBuf::from(value()?)),
            Some("--vehicle-content") => vehicle_content = Some(PathBuf::from(value()?)),
            Some("--garage-layout") => garage_layout = Some(PathBuf::from(value()?)),
            Some("--sequence-content") => sequence_content = Some(PathBuf::from(value()?)),
            Some("--garage-logic") => garage_logic = Some(PathBuf::from(value()?)),
            Some("--control-catalogs") => control_catalogs = Some(PathBuf::from(value()?)),
            Some("--entitlement-state") => entitlement_state = Some(PathBuf::from(value()?)),
            Some("--kickback-state") => kickback_state = Some(PathBuf::from(value()?)),
            Some("--speedwall-state") => speedwall_state = Some(PathBuf::from(value()?)),
            Some("--item-licenses") => item_licenses = Some(PathBuf::from(value()?)),
            Some("--user-settings") => user_settings = Some(PathBuf::from(value()?)),
            Some("--stat-definitions") => stat_definitions = Some(PathBuf::from(value()?)),
            Some("--world-policy") => world_policy = Some(PathBuf::from(value()?)),
            Some("--matchmaking-admission") => {
                matchmaking_admission = Some(PathBuf::from(value()?))
            }
            Some("--matchmaking-policy") => matchmaking_policy = Some(PathBuf::from(value()?)),
            Some("--group-policy") => group_policy = Some(PathBuf::from(value()?)),
            Some("--owned-world-connection") => owned_world_connection = true,
            Some("--owned-only") => owned_only = true,
            Some("--world-mac-template") => world_mac_template = Some(PathBuf::from(value()?)),
            Some("--owned-world-readiness") => owned_world_readiness = true,
            Some("--owned-world-attributes") => {
                owned_world_attributes = true;
                owned_world_readiness = true;
            }
            Some("--owned-menu-awards") => owned_menu_awards = true,
            Some("--owned-local-social") => owned_local_social = true,
            Some("--challenge-content") => challenge_content = Some(PathBuf::from(value()?)),
            Some("--auth-config") => auth_config = Some(PathBuf::from(value()?)),
            Some("--local-identity") => local_identity = Some(PathBuf::from(value()?)),
            Some("--bootstrap-config") => bootstrap_config = Some(PathBuf::from(value()?)),
            Some("--state-directory") => state_directory = Some(PathBuf::from(value()?)),
            Some("--local-account") => {
                let text = value()?;
                local_account = Some(account(text.to_str().ok_or("bad local account")?)?);
            }
            Some("--template-frames") => {
                return Err("runtime replay is unsupported; use modeled world content".into());
            }
            Some("--redirector-port") => {
                redirector_port = value()?
                    .to_str()
                    .and_then(|v| v.parse().ok())
                    .ok_or("bad port")?
            }
            Some("--idle-seconds") => {
                idle_seconds = value()?
                    .to_str()
                    .and_then(|v| v.parse().ok())
                    .ok_or("bad idle")?
            }
            Some("--qos-seconds") => {
                qos_seconds = value()?
                    .to_str()
                    .and_then(|v| v.parse().ok())
                    .ok_or("bad qos")?
            }
            _ => return Err(format!("unknown argument {flag:?}")),
        }
    }
    if !(1..=3600).contains(&idle_seconds) || !(1..=86_400).contains(&qos_seconds) {
        return Err("idle must be 1..3600 s and qos 1..86400 s".into());
    }
    if entitlement_state.is_some() && (local_account.is_none() || auth_config.is_none()) {
        return Err("--entitlement-state requires owned authentication and --local-account".into());
    }
    if (kickback_state.is_some() || speedwall_state.is_some())
        && (local_account.is_none() || auth_config.is_none())
    {
        return Err("--kickback-state and --speedwall-state require owned authentication and --local-account".into());
    }
    if owned_only
        && (auth_config.is_none()
            || bootstrap_config.is_none()
            || group_policy.is_none()
            || matchmaking_admission.is_none()
            || matchmaking_policy.is_none()
            || world_policy.is_none()
            || control_catalogs.is_none()
            || stat_definitions.is_none()
            || !owned_menu_awards
            || challenge_content.is_none()
            || !owned_local_social
            || item_licenses.is_none())
    {
        return Err(
            "--owned-only requires every owned service option and no --control-content".into(),
        );
    }
    let configured = [
        item_content.is_some(),
        state_directory.is_some(),
        local_account.is_some(),
    ];
    if configured.iter().any(|v| *v) && !configured.iter().all(|v| *v) {
        return Err(
            "Items requires --item-content, --state-directory and --local-account together".into(),
        );
    }
    if persistent_content.is_some() && item_content.is_none() {
        return Err("--persistent-content requires the owned Items configuration".into());
    }
    if progression_content.is_some() && persistent_content.is_none() {
        return Err("--progression-content requires --persistent-content".into());
    }
    if (vehicle_content.is_some() || garage_layout.is_some())
        && !(vehicle_content.is_some()
            && garage_layout.is_some()
            && progression_content.is_some()
            && world_content.is_some())
    {
        return Err("Vehicles require --vehicle-content, --garage-layout, --world-content and --progression-content".into());
    }
    if sequence_content.is_some() && vehicle_content.is_none() {
        return Err(
            "--sequence-content requires the owned vehicle/progression configuration".into(),
        );
    }
    if garage_logic.is_some() && sequence_content.is_none() {
        return Err("--garage-logic requires --sequence-content".into());
    }
    if (stat_definitions.is_some() || owned_menu_awards)
        && (persistent_content.is_none() || garage_logic.is_none())
    {
        return Err("--stat-definitions and --owned-menu-awards require --persistent-content and --garage-logic for current account state".into());
    }
    if challenge_content.is_some() && item_content.is_none() {
        return Err("--challenge-content requires owned --item-content and account storage".into());
    }
    if (auth_config.is_some() || local_identity.is_some())
        && !(auth_config.is_some() && item_content.is_some() && bootstrap_config.is_some())
    {
        return Err("--auth-config requires owned Items/account storage and --bootstrap-config; --local-identity is an optional import".into());
    }
    Ok(Args {
        root,
        world_mac_template,
        output: output.ok_or("--output is required")?,
        redirector_port,
        idle_seconds,
        qos_seconds,
        stop_file,
        world_content,
        item_content,
        persistent_content,
        progression_content,
        vehicle_content,
        garage_layout,
        sequence_content,
        garage_logic,
        control_catalogs,
        entitlement_state,
        kickback_state,
        speedwall_state,
        item_licenses,
        user_settings,
        stat_definitions,
        owned_menu_awards,
        owned_local_social,
        owned_world_readiness,
        owned_world_attributes,
        owned_world_connection,
        owned_only,
        challenge_content,
        bootstrap_config,
        auth_config,
        local_identity,
        group_policy,
        matchmaking_admission,
        matchmaking_policy,
        world_policy,
        state_directory,
        local_account,
    })
}

fn account(text: &str) -> Result<nfs_storage::AccountId, String> {
    if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("local account must be 32 nonzero hex digits from owned configuration".into());
    }
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| "bad local account")?;
    }
    nfs_storage::AccountId::from_owned_config(bytes).map_err(|_| "zero local account".into())
}

fn inventory_config(args: &Args) -> Result<Option<net::InventoryProfile>, Failure> {
    let (Some(content), Some(directory), Some(account)) = (
        &args.item_content,
        &args.state_directory,
        args.local_account,
    ) else {
        return Ok(None);
    };
    let mut components = directory.components();
    if components.next() != Some(std::path::Component::Normal("artifacts".as_ref()))
        || !components.all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return Err(Failure::ProfileConfig);
    }
    let artifacts = args
        .root
        .join("artifacts")
        .canonicalize()
        .map_err(|_| Failure::Output)?;
    let directory = args.root.join(directory);
    let ancestor = directory
        .ancestors()
        .find(|p| p.exists())
        .ok_or(Failure::Output)?;
    if !ancestor
        .canonicalize()
        .map_err(|_| Failure::Output)?
        .starts_with(&artifacts)
    {
        return Err(Failure::ProfileConfig);
    }
    let content = nfs_server::item_content::ItemContent::load(&args.root.join(content))
        .map_err(nfs_server::content_failure)?;
    let catalog = content.inventory().ok_or(Failure::ProfileConfig)?.clone();
    catalog
        .instantiate_initial(1)
        .map_err(|_| Failure::ProfileConfig)?;
    let persistent = args
        .persistent_content
        .as_ref()
        .map(|path| {
            nfs_server::persistent::Catalog::load(&args.root.join(path))
                .map_err(nfs_server::content_failure)
        })
        .transpose()?;
    let repository = nfs_storage::SqliteRepository::open_owned_directory(&directory)
        .map_err(|_| Failure::Output)?;
    let mut service = net::InventoryService::new(Arc::new(repository), Arc::new(catalog));
    if let Some(persistent) = persistent {
        service = service.with_persistent(Arc::new(persistent));
    }
    Ok(Some(service.bind(account)))
}

pub fn main() -> std::process::ExitCode {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "create-account")
    {
        return nfs_server::fresh_account::main(std::env::args_os().skip(2));
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--help" || a == "-h")
    {
        println!(
            "nfs-server: owned local NFS 2015 services\n\nUsage: nfs-server --root <data-directory> --output <new-recording-directory> [configuration options]\n\nCreate account: nfs-server create-account --name <name> --state-directory <new-directory>\n--account-policy <policy.json> --item-content <items.json> --persistent-content <tables.json>\n\nRequired control configuration: --bootstrap-config, --auth-config,\n--group-policy, --matchmaking-admission, --matchmaking-policy, --world-policy,\n--control-catalogs, --stat-definitions, --challenge-content, --item-licenses,\n--owned-local-social and --owned-menu-awards.\n\nGarage configuration: --world-content (version 4), --world-mac-template,\n--item-content, --state-directory, --local-account, --persistent-content,\n--progression-content, --vehicle-content, --garage-layout, --sequence-content,\n--garage-logic.\n\nAccount state is read from SQLite. Optional one-time imports: --local-identity,\n--entitlement-state, --kickback-state, --speedwall-state, --user-settings.\n\nOptional: --redirector-port, --idle-seconds, --qos-seconds, --stop-file.\nAll listeners are loopback. No manifest, captured reply store or external\nauthentication is used. See crates/server/README.md for schemas and limitations."
        );
        return std::process::ExitCode::SUCCESS;
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let args = match parse() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start runtime: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run(args));
    runtime.shutdown_timeout(Duration::from_secs(2));
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nfs-server failed: {error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), Failure> {
    let inventory = inventory_config(&args)?;
    let account = args.local_account.ok_or(Failure::ProfileConfig)?;
    let root = args.root.join(
        args.state_directory
            .as_ref()
            .ok_or(Failure::ProfileConfig)?,
    );
    let imports = nfs_services::account_state::Imports {
        identity: args.local_identity.as_ref().map(|p| args.root.join(p)),
        entitlements: args.entitlement_state.as_ref().map(|p| args.root.join(p)),
        kickback: args.kickback_state.as_ref().map(|p| args.root.join(p)),
        speedwall: args.speedwall_state.as_ref().map(|p| args.root.join(p)),
        settings: args.user_settings.as_ref().map(|p| args.root.join(p)),
    };
    let owned = tokio::task::spawn_blocking(move || {
        nfs_services::account_state::open(&root, account, &imports)
    })
    .await
    .map_err(|_| Failure::Output)?
    .map_err(|_| Failure::ProfileConfig)?;
    let auth_config = args.auth_config.as_ref().ok_or(Failure::ProfileConfig)?;
    let auth_config = nfs_services::authentication::Config::load(&args.root.join(auth_config))
        .map_err(nfs_server::content_failure)?;
    let mut pack = Deployment::new();
    if let Some(path) = &args.world_mac_template {
        let bytes = std::fs::read(args.root.join(path)).map_err(|_| Failure::Output)?;
        pack = pack.with_world_mac_template(&bytes)?;
    }
    pack = pack.with_authentication(auth_config, owned.identity)?;
    if let Some(path) = &args.bootstrap_config {
        pack = pack.with_bootstrap(
            nfs_services::bootstrap::Config::load(&args.root.join(path))
                .map_err(nfs_server::content_failure)?,
        );
    }
    if let Some(path) = &args.world_policy {
        pack = pack.with_world_policy(
            nfs_services::world_setup::Config::load(&args.root.join(path))
                .map_err(|_| Failure::ProfileConfig)?,
        );
    }
    if let Some(path) = &args.matchmaking_admission {
        pack = pack.with_matchmaking_admission(
            nfs_services::matchmaking::Config::load(&args.root.join(path))
                .map_err(|_| Failure::ProfileConfig)?,
        );
    }
    if let Some(path) = &args.matchmaking_policy {
        pack = pack.with_matchmaking_policy(
            nfs_services::matchmaking_status::Config::load(&args.root.join(path))
                .map_err(|_| Failure::ProfileConfig)?,
        );
    }
    if let Some(path) = &args.group_policy {
        pack = pack.with_group_policy(
            nfs_services::group::Config::load(&args.root.join(path))
                .map_err(nfs_server::content_failure)?,
        )?;
    }
    if args.owned_only {
        pack = pack.validate()?;
    }
    let pack = Arc::new(pack);
    let world_content = match &args.world_content {
        Some(dir) => Some(Arc::new(content::WorldContent::load(&args.root.join(dir))?)),
        None => None,
    };
    let progression = args
        .progression_content
        .as_ref()
        .map(|path| nfs_server::progression::Content::load(&args.root.join(path)))
        .transpose()?
        .map(Arc::new);
    let content_messages = world_content.as_ref().map_or(0, |c| c.batches.len() + 1);
    let vehicles = match (&args.vehicle_content, &args.garage_layout) {
        (Some(vehicles), Some(layout)) => {
            Some(Arc::new(nfs_server::vehicle_content::GarageContent {
                vehicles: nfs_server::vehicle_content::Content::load(
                    &args.root.join(vehicles),
                    inventory.as_ref().ok_or(Failure::ProfileConfig)?.bindings(),
                )?,
                layout: nfs_server::vehicle_content::layout::Layout::load(&args.root.join(layout))?,
            }))
        }
        _ => None,
    };
    let sequences = args
        .sequence_content
        .as_ref()
        .map(|path| nfs_server::sequence_content::SequenceContent::load(&args.root.join(path)))
        .transpose()?
        .map(Arc::new);
    let garage_logic = args
        .garage_logic
        .as_ref()
        .map(|path| nfs_server::garage_logic::GarageLogic::load(&args.root.join(path)))
        .transpose()?
        .map(Arc::new);
    let entitlements = Some((account, Arc::new(owned.entitlements)));
    let kickback = Some((account, Arc::new(owned.kickback)));
    let speedwall = Some((account, Arc::new(owned.speedwall)));
    let item_licenses = match &args.item_licenses {
        Some(path) => {
            let path = args.root.join(path);
            Some(Arc::new(
                tokio::task::spawn_blocking(move || {
                    nfs_services::item_licenses::Content::load(&path)
                })
                .await
                .map_err(|_| Failure::Output)?
                .map_err(|_| Failure::ProfileConfig)?,
            ))
        }
        None => None,
    };
    let catalogs = match &args.control_catalogs {
        Some(path) => {
            let path = args.root.join(path);
            Some(Arc::new(
                tokio::task::spawn_blocking(move || {
                    nfs_services::control_catalogs::Catalog::load(&path)
                })
                .await
                .map_err(|_| Failure::Output)?
                .map_err(|_| Failure::ProfileConfig)?,
            ))
        }
        None => None,
    };
    let user_settings = Some(Arc::new(owned.settings));
    let challenges = args
        .challenge_content
        .as_ref()
        .map(|path| {
            nfs_services::challenges::Catalog::load(&args.root.join(path))
                .map(Arc::new)
                .map_err(|_| Failure::ProfileConfig)
        })
        .transpose()?;
    let stats = args
        .stat_definitions
        .as_ref()
        .map(|path| {
            nfs_services::stats::Catalog::load(&args.root.join(path))
                .map(Arc::new)
                .map_err(|_| Failure::ProfileConfig)
        })
        .transpose()?;
    if (stats.is_some() || args.owned_menu_awards)
        && garage_logic
            .as_ref()
            .and_then(|g| g.reputation_thresholds())
            .is_none()
    {
        return Err(Failure::ProfileConfig);
    }
    if let Some(content) = &world_content {
        content.validate_runtime()?;
    }
    let listeners = net::Listeners::bind(args.redirector_port).await?;
    let a = listeners.addresses()?;
    let (recorder, thread) = record::start(&args.root, &args.output)?;
    let features = pack.features();
    let mut ready = serde_json::json!({"ready":true,"pid":std::process::id(),
            "redirector":a.redirector.to_string(),"blaze":a.blaze.to_string(),
            "auxiliary":"per-control-session","qos":a.qos.to_string(),"qos_udp":a.qos_udp.to_string(),
            "world_sources_present":false,
            "world_served":features.world_setup && features.world_mac_template,
            "world_content_messages":content_messages,
            "world_content_mode":"modeled","world_template_frames":0,
            "items_mode": if inventory.is_some() { "modeled" } else { "unsupported" },
            "persistent_tables_mode": if args.persistent_content.is_some() { "modeled" } else { "unsupported" },
            "progression_mode": if progression.is_some() { "modeled" } else { "unsupported" },
            "vehicles_mode": if vehicles.is_some() { "modeled" } else { "unsupported" },
            "waiting_sequence_mode": if sequences.is_some() { "modeled" } else { "unsupported" },
            "garage_logic_mode": if garage_logic.is_some() { "modeled" } else { "unsupported" },
            "local_social_mode": if args.owned_local_social { "single-account" } else { "unsupported" },
            "entitlements_mode": if entitlements.is_some() { "modeled" } else { "unsupported" },
            "kickback_mode": if kickback.is_some() { "modeled" } else { "unsupported" },
            "speedwall_mode": if speedwall.is_some() { "modeled" } else { "unsupported" },
            "item_licenses_mode": if item_licenses.is_some() { "modeled" } else { "unsupported" },
            "control_catalogs_mode": if catalogs.is_some() { "modeled" } else { "unsupported" },
            "stats_mode": if stats.is_some() { "modeled" } else { "unsupported" },
            "menu_awards_mode": if args.owned_menu_awards { "modeled" } else { "unsupported" },
            "challenges_mode": if challenges.is_some() { "modeled" } else { "unsupported" },
            "world_connection_mode":if args.owned_world_connection || args.world_policy.is_some(){"modeled"}else{"unsupported"},
            "world_attributes_mode":if args.owned_world_attributes || args.world_policy.is_some(){"modeled"}else{"unsupported"},
            "world_readiness_mode": if args.owned_world_readiness || args.owned_world_attributes || args.world_policy.is_some(){"modeled"}else{"unsupported"},
            "authentication_mode": if args.auth_config.is_some() { "modeled" } else { "unsupported" },
            "bootstrap_mode": if args.bootstrap_config.is_some() { "modeled" } else { "unsupported" },
            "stat_definition_groups": stats.as_ref().map_or(0, |s| s.len()),
            "world_setup_mode":if args.world_policy.is_some(){"modeled"}else{"unsupported"},
            "matchmaking_admission_mode":if args.matchmaking_admission.is_some(){"modeled"}else{"unsupported"},
            "matchmaking_status_mode":if args.matchmaking_policy.is_some(){"modeled"}else{"unsupported"},
            "group_mode": if args.group_policy.is_some() { "modeled" } else { "unsupported" },
            "user_settings_mode": if user_settings.is_some() { "modeled" } else { "unsupported" },
            "user_settings_persisted": user_settings.as_ref().is_some_and(|s| s.persisted())});
    ready["profile_mode"] = serde_json::json!(if args.owned_only {
        "owned-only"
    } else {
        "pack"
    });
    ready["manifest_mode"] = serde_json::json!("none");
    println!("{ready}");
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let config = net::Config {
        root: args.root,
        redirector_port: args.redirector_port,
        idle: Duration::from_secs(args.idle_seconds),
        qos_lifetime: Duration::from_secs(args.qos_seconds),
        world_content,
        inventory,
        progression,
        vehicles,
        sequences,
        garage_logic,
        catalogs,
        entitlements,
        user_settings,
        stats,
        owned_menu_awards: args.owned_menu_awards,
        owned_local_social: args.owned_local_social,
        challenges,
        kickback,
        speedwall,
        item_licenses,
    };
    let server = tokio::spawn(net::serve(listeners, pack, recorder, config, shutdown));
    let stop_file = args.stop_file.clone();
    let watch_file = async move {
        match stop_file {
            Some(path) => loop {
                if path.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            },
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = watch_file => {}
    }
    let _ = stop.send(true);
    let served = server.await.map_err(|_| Failure::Io)?;
    thread.finish()?;
    served
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> Vec<std::ffi::OsString> {
        let mut args = Vec::new();
        for flag in [
            "--output",
            "--bootstrap-config",
            "--auth-config",
            "--local-identity",
            "--group-policy",
            "--matchmaking-admission",
            "--matchmaking-policy",
            "--world-policy",
            "--control-catalogs",
            "--entitlement-state",
            "--user-settings",
            "--stat-definitions",
            "--challenge-content",
            "--kickback-state",
            "--speedwall-state",
            "--item-licenses",
            "--item-content",
            "--state-directory",
            "--persistent-content",
            "--progression-content",
            "--vehicle-content",
            "--garage-layout",
            "--sequence-content",
            "--garage-logic",
            "--world-content",
        ] {
            args.extend([flag.into(), "configured-input".into()]);
        }
        args.extend([
            "--local-account".into(),
            "01010101010101010101010101010101".into(),
            "--owned-menu-awards".into(),
            "--owned-local-social".into(),
        ]);
        args
    }

    #[test]
    fn deployment_requires_owned_configuration_and_rejects_replay_options() {
        assert!(parse_from(configured().into_iter()).unwrap().owned_only);
        for flag in [
            "--manifest",
            "--control-content",
            "--template-frames",
            "--unknown",
        ] {
            let mut args = configured();
            args.extend([flag.into(), "unused".into()]);
            assert!(parse_from(args.into_iter()).is_err());
        }
        for flag in [
            "--bootstrap-config",
            "--auth-config",
            "--group-policy",
            "--world-policy",
        ] {
            let mut args = configured();
            let index = args.iter().position(|s| s == flag).unwrap();
            args.drain(index..index + 2);
            assert!(parse_from(args.into_iter()).is_err());
        }
        for (flag, value) in [
            ("--idle-seconds", "0"),
            ("--idle-seconds", "3601"),
            ("--qos-seconds", "86401"),
            ("--redirector-port", "65536"),
            ("--local-account", "00000000000000000000000000000000"),
        ] {
            let mut args = configured();
            args.extend([flag.into(), value.into()]);
            assert!(parse_from(args.into_iter()).is_err());
        }
    }

    #[test]
    fn initialized_account_does_not_require_external_state_documents() {
        let mut args = configured();
        for flag in [
            "--local-identity",
            "--entitlement-state",
            "--kickback-state",
            "--speedwall-state",
            "--user-settings",
        ] {
            let index = args.iter().position(|s| s == flag).unwrap();
            args.drain(index..index + 2);
        }
        assert!(parse_from(args.into_iter()).is_ok());
        // Existence and schema of persisted state are checked before readiness.
    }
}
