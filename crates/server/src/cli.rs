// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nfs_server::{Failure, content, deployment::Deployment, net, record};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Args {
    root: PathBuf,
    world_mac_template: Option<PathBuf>,
    output: PathBuf,
    record_protocol: bool,
    redirector_port: u16,
    /// The one address every listener binds and the server advertises.
    listen: std::net::Ipv4Addr,
    lsx_port: Option<u16>,
    idle_seconds: u64,
    qos_seconds: u64,
    stop_file: Option<PathBuf>,
    world_content: Option<PathBuf>,
    item_content: Option<PathBuf>,
    persistent_content: Option<PathBuf>,
    progression_content: Option<PathBuf>,
    vehicle_content: Option<PathBuf>,
    vehicle_parts: Option<PathBuf>,
    garage_layout: Option<PathBuf>,
    sequence_content: Option<PathBuf>,
    traffic_content: Option<PathBuf>,
    rival_content: Option<PathBuf>,
    garage_logic: Option<PathBuf>,
    reputation_thresholds: Option<PathBuf>,
    scene_catalog: Option<PathBuf>,
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
    /// Installation whose content replaces the item, table and template options.
    game_dir: Option<PathBuf>,
    content_cache: Option<PathBuf>,
    /// Cache entry summary once `game_dir` has been resolved.
    generated: Option<serde_json::Value>,
    /// Player-chosen traffic and roaming-rival amounts.
    ai: nfs_server::ai_settings::AiSettings,
    /// The player's settings file, watched for changes while the server
    /// runs; without it the amounts stay as started.
    settings_file: Option<PathBuf>,
    /// Print live positions for the launcher's map on standard output after
    /// the readiness line ([`nfs_server::map_feed`]).
    map_feed: bool,
}

fn parse_from(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<Args, String> {
    let mut root = std::env::current_dir().map_err(|e| e.to_string())?;
    let (mut output, mut stop_file, mut world_content) = (None, None, None);
    let mut record_protocol = false;
    let (mut redirector_port, mut idle_seconds, mut qos_seconds) = (0, 120, 10);
    let (mut item_content, mut state_directory, mut local_account) = (None, None, None);
    let mut lsx_port = None;
    let mut listen = std::net::Ipv4Addr::LOCALHOST;
    let mut persistent_content = None;
    let mut progression_content = None;
    let (mut vehicle_content, mut garage_layout) = (None, None);
    let mut vehicle_parts = None;
    let mut sequence_content = None;
    let mut traffic_content = None;
    let mut rival_content = None;
    let mut garage_logic = None;
    let mut reputation_thresholds = None;
    let mut scene_catalog = None;
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
    let (mut game_dir, mut content_cache) = (None, None);
    let (mut traffic_percent, mut rival_limit) = (100u16, None);
    let mut settings_file = None;
    let mut map_feed = false;
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("missing value for {flag:?}"))
        };
        match flag.to_str() {
            Some("--record-protocol") if !record_protocol => record_protocol = true,
            Some("--map-feed") if !map_feed => map_feed = true,
            Some("--root") => root = value()?.into(),
            Some("--output") => output = Some(PathBuf::from(value()?)),
            Some("--stop-file") => stop_file = Some(PathBuf::from(value()?)),
            Some("--settings-file") => settings_file = Some(PathBuf::from(value()?)),
            Some("--scene-catalog") => scene_catalog = Some(PathBuf::from(value()?)),
            Some("--world-content") => world_content = Some(PathBuf::from(value()?)),
            Some("--item-content") => item_content = Some(PathBuf::from(value()?)),
            Some("--persistent-content") => persistent_content = Some(PathBuf::from(value()?)),
            Some("--progression-content") => progression_content = Some(PathBuf::from(value()?)),
            Some("--vehicle-content") => vehicle_content = Some(PathBuf::from(value()?)),
            Some("--vehicle-parts") => vehicle_parts = Some(PathBuf::from(value()?)),
            Some("--garage-layout") => garage_layout = Some(PathBuf::from(value()?)),
            Some("--sequence-content") => sequence_content = Some(PathBuf::from(value()?)),
            Some("--traffic-content") => traffic_content = Some(PathBuf::from(value()?)),
            Some("--rival-content") => rival_content = Some(PathBuf::from(value()?)),
            Some("--garage-logic") => garage_logic = Some(PathBuf::from(value()?)),
            Some("--reputation-thresholds") => {
                reputation_thresholds = Some(PathBuf::from(value()?))
            }
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
            Some("--game-dir") => game_dir = Some(PathBuf::from(value()?)),
            Some("--content-cache") => content_cache = Some(PathBuf::from(value()?)),
            Some("--local-account") => {
                let text = value()?;
                local_account = Some(account(text.to_str().ok_or("bad local account")?)?);
            }
            Some("--template-frames") => {
                return Err("runtime replay is unsupported; use modeled world content".into());
            }
            Some("--lsx-port") => {
                lsx_port = Some(
                    value()?
                        .to_str()
                        .and_then(|v| v.parse().ok())
                        .ok_or("bad LSX port")?,
                );
            }
            Some("--listen") => {
                listen = value()?
                    .to_str()
                    .and_then(|v| v.parse::<std::net::Ipv4Addr>().ok())
                    .filter(|ip| !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast())
                    .ok_or("--listen takes one IPv4 address of this computer")?;
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
            Some("--traffic-percent") => {
                traffic_percent = value()?
                    .to_str()
                    .and_then(|v| v.parse().ok())
                    .ok_or("bad traffic percent")?
            }
            Some("--rival-limit") => {
                rival_limit = Some(
                    value()?
                        .to_str()
                        .and_then(|v| v.parse().ok())
                        .ok_or("bad rival limit")?,
                )
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
    let ai = nfs_server::ai_settings::AiSettings::new(traffic_percent, rival_limit)
        .map_err(|e| e.to_string())?;
    let generated = game_dir.is_some();
    if generated
        && (item_content.is_some() || persistent_content.is_some() || world_mac_template.is_some())
    {
        return Err("--game-dir generates --item-content, --persistent-content and --world-mac-template; pass either the installation or those files".into());
    }
    if content_cache.is_some() && !generated {
        return Err("--content-cache requires --game-dir".into());
    }
    let has_items = item_content.is_some() || generated;
    let has_persistent = persistent_content.is_some() || generated;
    let has_stats = stat_definitions.is_some() || generated;
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
            || (control_catalogs.is_none() && !generated)
            || !has_stats
            || !owned_menu_awards
            || challenge_content.is_none()
            || !owned_local_social
            || (item_licenses.is_none() && !generated))
    {
        return Err(
            "--owned-only requires every owned service option and no --control-content".into(),
        );
    }
    let configured = [
        has_items,
        state_directory.is_some(),
        local_account.is_some(),
    ];
    if configured.iter().any(|v| *v) && !configured.iter().all(|v| *v) {
        return Err(
            "Items requires --item-content, --state-directory and --local-account together".into(),
        );
    }
    if has_persistent && !has_items {
        return Err("--persistent-content requires the owned Items configuration".into());
    }
    if progression_content.is_some() && !has_persistent {
        return Err("--progression-content requires --persistent-content".into());
    }
    if (vehicle_content.is_some() || garage_layout.is_some() || generated)
        && !((vehicle_content.is_some() || generated)
            && (garage_layout.is_some() || generated)
            && (progression_content.is_some() || generated)
            && (world_content.is_some() || generated))
    {
        return Err("Vehicles require --vehicle-content, --garage-layout, --world-content and --progression-content".into());
    }
    if vehicle_parts.is_some() && vehicle_content.is_none() && !generated {
        return Err("--vehicle-parts requires --vehicle-content".into());
    }
    if sequence_content.is_some() && vehicle_content.is_none() && !generated {
        return Err(
            "--sequence-content requires the owned vehicle/progression configuration".into(),
        );
    }
    if traffic_content.is_some() && vehicle_content.is_none() && !generated {
        return Err(
            "--traffic-content requires the owned vehicle/progression configuration".into(),
        );
    }
    if rival_content.is_some() && vehicle_content.is_none() && !generated {
        return Err("--rival-content requires the owned vehicle/progression configuration".into());
    }
    if garage_logic.is_some() && sequence_content.is_none() && !generated {
        return Err("--garage-logic requires --sequence-content".into());
    }
    if (has_stats || owned_menu_awards) && (!has_persistent || garage_logic.is_none()) {
        return Err("--stat-definitions and --owned-menu-awards require --persistent-content and --garage-logic for current account state".into());
    }
    if challenge_content.is_some() && !has_items {
        return Err("--challenge-content requires owned --item-content and account storage".into());
    }
    if (auth_config.is_some() || local_identity.is_some())
        && !(auth_config.is_some() && has_items && bootstrap_config.is_some())
    {
        return Err("--auth-config requires owned Items/account storage and --bootstrap-config; --local-identity is an optional import".into());
    }
    Ok(Args {
        root,
        world_mac_template,
        output: output.ok_or("--output is required")?,
        record_protocol,
        redirector_port,
        listen,
        lsx_port,
        idle_seconds,
        qos_seconds,
        stop_file,
        world_content,
        item_content,
        persistent_content,
        progression_content,
        vehicle_content,
        vehicle_parts,
        garage_layout,
        sequence_content,
        traffic_content,
        rival_content,
        garage_logic,
        reputation_thresholds,
        scene_catalog,
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
        game_dir,
        content_cache,
        generated: None,
        ai,
        settings_file,
        map_feed,
    })
}

/// Replace the generated options with files built from `--game-dir`.
fn resolve_install_content(args: &mut Args, options: &nfs_content::Options) -> Result<(), String> {
    let Some(game_dir) = &args.game_dir else {
        return Ok(());
    };
    let generated = nfs_server::install_content::resolve(
        &args.root,
        game_dir,
        args.content_cache.as_deref(),
        options,
    )
    .map_err(|error| format!("cannot prepare content from the game installation: {error}"))?;
    args.generated = Some(generated.summary());
    args.item_content = Some(generated.item_content);
    args.persistent_content = Some(generated.persistent_content);
    args.stat_definitions
        .get_or_insert(generated.stat_definitions);
    args.item_licenses.get_or_insert(generated.item_licenses);
    args.control_catalogs
        .get_or_insert(generated.control_catalogs);
    args.reputation_thresholds
        .get_or_insert(generated.reputation_thresholds);
    args.scene_catalog.get_or_insert(generated.scene_catalog);
    args.world_content.get_or_insert(generated.world_content);
    args.garage_layout.get_or_insert(generated.garage_layout);
    args.vehicle_parts.get_or_insert(generated.vehicle_parts);
    args.vehicle_content
        .get_or_insert(generated.vehicle_profiles);
    args.progression_content
        .get_or_insert(generated.progression_content);
    args.sequence_content
        .get_or_insert(generated.sequence_content);
    args.traffic_content
        .get_or_insert(generated.traffic_content);
    args.rival_content.get_or_insert(generated.rival_content);
    args.world_mac_template = Some(generated.world_mac_template);
    Ok(())
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

fn inventory_config(
    args: &Args,
) -> Result<
    (
        Option<net::InventoryProfile>,
        Option<nfs_storage::sqlite::WriterLease>,
    ),
    Failure,
> {
    let (Some(content), Some(directory), Some(account)) = (
        &args.item_content,
        &args.state_directory,
        args.local_account,
    ) else {
        return Ok((None, None));
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
    let lease = repository
        .writer_lease(account)
        .map_err(|_| Failure::ProfileConfig)?;
    let records = nfs_storage::records::SqliteRecords(repository.clone());
    let mut service = net::InventoryService::new(Arc::new(repository), Arc::new(catalog))
        .with_records(Arc::new(records));
    if let Some(offers) = content.purchase_offers() {
        service = service.with_purchase_offers(offers.clone());
    }
    if let Some(offers) = content.car_offers() {
        service = service.with_car_offers(offers.clone());
    }
    if let Some(policy) = content.category_unlocks() {
        service = service.with_category_unlocks(policy.clone());
    }
    if let Some(persistent) = persistent {
        service = service.with_persistent(Arc::new(persistent));
    }
    Ok((Some(service.bind(account)), Some(lease)))
}

pub fn main() -> std::process::ExitCode {
    main_from(std::env::args_os().skip(1))
}

/// Server entry point shared by the standalone and terminal bundle binaries.
/// The iterator contains only server arguments (no executable or role flag).
pub fn main_from(args: impl Iterator<Item = std::ffi::OsString>) -> std::process::ExitCode {
    let mut args = args.peekable();
    match args.peek().and_then(|arg| arg.to_str()) {
        Some("story") => {
            args.next();
            return nfs_server::story_account::main(args);
        }
        Some("bank") => {
            args.next();
            return nfs_server::bank_account::main(args);
        }
        Some("reputation") => {
            args.next();
            return nfs_server::reputation_account::main(args);
        }
        Some("prepare-local") => {
            args.next();
            return nfs_server::local_setup::main(args);
        }
        Some("prepare-content") => {
            args.next();
            return nfs_server::prepare_content::main(args);
        }
        Some("create-account") => {
            args.next();
            return nfs_server::fresh_account::main(args);
        }
        _ => {}
    }
    if args.peek().is_some_and(|a| a == "--help" || a == "-h") {
        println!(
            "nfs-server: owned local NFS 2015 services\n\nUsage: nfs-server --root <data-directory> --output <new-recording-directory> [configuration options]\n\nPrepare complete local policy: nfs-server prepare-local --game-dir <installation> --root <data-directory>\n\nPrepare game content: nfs-server prepare-content --game-dir <installation>\n[--root <data-directory>] [--content-cache <directory>]\n\nCreate account: nfs-server create-account --name <name> --state-directory <new-directory>\n--account-policy <policy.json> --item-content <items.json> --persistent-content <tables.json>\n\nRequired control configuration: --bootstrap-config, --auth-config,\n--group-policy, --matchmaking-admission, --matchmaking-policy, --world-policy,\n--control-catalogs, --stat-definitions, --challenge-content, --item-licenses,\n--owned-local-social and --owned-menu-awards.\n\nGarage configuration: --world-content (version 4 or 5), --world-mac-template,\n--item-content, --state-directory, --local-account, --persistent-content,\n--progression-content, --vehicle-content, --garage-layout, --sequence-content,\n--garage-logic.\n\nGame content: --game-dir <installation> builds --item-content,\n--persistent-content and --world-mac-template on first start, supplies\n--stat-definitions, --item-licenses, --control-catalogs and\n--reputation-thresholds, --scene-catalog, --world-content, --garage-layout,\n--vehicle-content, --vehicle-parts, --progression-content and --sequence-content unless explicitly overridden, and caches the outputs\n(--content-cache, default artifacts/content under --root).\n\nAccount state is read from SQLite. Optional one-time imports: --local-identity,\n--entitlement-state, --kickback-state, --speedwall-state, --user-settings.\n\nOptional: --lsx-port (3216 for the game), --redirector-port, --listen, --idle-seconds, --qos-seconds, --stop-file, --record-protocol.\nRaw protocol recording is disabled by default and bounded when enabled.\nEvery listener binds 127.0.0.1 unless --listen names another IPv4 address\nof this computer, which is then advertised to the game and serves the network. No manifest, captured reply store or external\nauthentication is used. See crates/server/README.md for schemas and limitations."
        );
        return std::process::ExitCode::SUCCESS;
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let mut args = match parse_from(args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };
    // First start reads the installation (blocking); later starts reuse the cache.
    if let Err(message) = resolve_install_content(&mut args, &nfs_content::Options::default()) {
        eprintln!("{message}");
        return std::process::ExitCode::FAILURE;
    }
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
    let (inventory, _account_lease) = inventory_config(&args)?;
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
    let lsx_profile = nfs_server::lsx::profile(&owned.identity);
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
    let scene_catalog = args
        .scene_catalog
        .as_ref()
        .map(|path| {
            nfs_content::scenes::Catalog::load(&args.root.join(path))
                .map_err(|_| Failure::ProfileConfig)
        })
        .transpose()?;
    let world_content = match &args.world_content {
        Some(dir) => Some(Arc::new(content::WorldContent::load_with_scenes(
            &args.root.join(dir),
            scene_catalog.as_ref(),
        )?)),
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
                vehicles: nfs_server::vehicle_content::Content::load_with_parts(
                    &args.root.join(vehicles),
                    inventory.as_ref().ok_or(Failure::ProfileConfig)?.bindings(),
                    args.vehicle_parts
                        .as_ref()
                        .map(|p| args.root.join(p))
                        .as_deref(),
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
    let traffic = args
        .traffic_content
        .as_ref()
        .map(|path| {
            nfs_content::traffic::Content::load(&args.root.join(path))
                .map_err(|_| Failure::ProfileConfig)
        })
        .transpose()?
        .map(Arc::new);
    let rivals = if let Some(path) = &args.rival_content {
        let path = args.root.join(path);
        let item_path = args
            .root
            .join(args.item_content.as_ref().ok_or(Failure::ProfileConfig)?);
        let definitions = vehicles
            .as_ref()
            .ok_or(Failure::ProfileConfig)?
            .vehicles
            .clone();
        let game_dir = args.game_dir.clone();
        let (content, graph) = tokio::task::spawn_blocking(move || {
            let items = nfs_services::item_content::ItemContent::load(&item_path)
                .map_err(|_| Failure::ProfileConfig)?;
            let content = nfs_server::rival_content::Content::load(&path, definitions, &items)?;
            // The host simulates rivals on the installed road graph (E987).
            let graph = game_dir.ok_or(None).and_then(|dir| {
                nfs_content::assets::AssetIndex::open(&dir, &nfs_content::Options::default())
                    .and_then(|mut index| index.road_graph())
                    .map_err(Some)
            });
            Ok::<_, Failure>((content, graph))
        })
        .await
        .map_err(|_| Failure::Output)??;
        match graph {
            Ok(graph) => {
                tracing::info!(
                    nodes = graph.node_count(),
                    lanes = graph.lane_count(),
                    "road graph read from the installation for roaming rivals"
                );
                Some(Arc::new(content.with_graph(Arc::new(graph))))
            }
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "road graph unavailable (needs --game-dir); roaming rivals stay off"
                );
                None
            }
        }
    } else {
        None
    };
    let default_reputation = args
        .reputation_thresholds
        .as_ref()
        .map(|path| {
            nfs_services::reputation::Thresholds::load(&args.root.join(path))
                .map_err(|_| Failure::ProfileConfig)
        })
        .transpose()?;
    let garage_logic = args
        .garage_logic
        .as_ref()
        .map(|path| {
            nfs_server::garage_logic::GarageLogic::load_with_reputation(
                &args.root.join(path),
                default_reputation,
            )
        })
        .transpose()?
        .map(Arc::new);
    // Free-roam scoring compiles the installed driving participants directly
    // from the installation; without it (or without garage logic) scoring stays off.
    let mut events = None;
    let mut prologue = None;
    let scoring = match (&args.game_dir, garage_logic.is_some()) {
        (Some(game_dir), true) => {
            let game_dir = game_dir.clone();
            let event_vehicles = vehicles.as_ref().map(|v| v.vehicles.clone());
            let event_items = args.item_content.as_ref().map(|p| args.root.join(p));
            let intro_parent = progression
                .as_ref()
                .and_then(|p| {
                    p.construction
                        .blueprint(nfs_server::progression::Branch::Intro)
                })
                .map(|b| b.asset());
            let intro_items = inventory.as_ref().map(|i| i.item_catalog());
            let content = tokio::task::spawn_blocking(move || {
                let mut index = nfs_content::assets::AssetIndex::open(
                    &game_dir,
                    &nfs_content::Options::default(),
                )?;
                let scoring = index.scoring_content("levels/genesis01/gameplay")?;
                let road = index.road_network();
                let nitrous = index.nitrous_configurations()?;
                // The owned race event resolves from the same installation; its
                // failure leaves scoring on and the event off.
                let event = (|| -> Result<_, nfs_content::Error> {
                    let items = event_items
                        .as_ref()
                        .map(|path| {
                            nfs_services::item_content::ItemContent::load(path).map_err(|e| {
                                nfs_content::Error::Content(format!("event AI items: {e:?}"))
                            })
                        })
                        .transpose()?;
                    nfs_server::event_logic::catalog::prepare::sprints(
                        &mut index,
                        &scoring.categories,
                        event_vehicles.as_ref(),
                        items.as_ref(),
                    )
                })();
                let story = (|| {
                    let parent = intro_parent.ok_or_else(|| {
                        nfs_content::Error::Content("intro participant content missing".into())
                    })?;
                    let items = intro_items.ok_or_else(|| {
                        nfs_content::Error::Content("intro item catalog missing".into())
                    })?;
                    let prepared =
                        nfs_server::prologue::Content::prepare(&mut index, parent, &items)?;
                    let vehicles = event_vehicles.clone().ok_or_else(|| {
                        nfs_content::Error::Content("intro vehicle content missing".into())
                    })?;
                    let path = event_items.as_ref().ok_or_else(|| {
                        nfs_content::Error::Content("intro item content missing".into())
                    })?;
                    let ai_items =
                        nfs_services::item_content::ItemContent::load(path).map_err(|e| {
                            nfs_content::Error::Content(format!("intro AI items: {e:?}"))
                        })?;
                    prepared.with_opponents(&mut index, vehicles, &ai_items)
                })();
                Ok::<_, nfs_content::Error>((scoring, road, nitrous, event, story))
            })
            .await
            .map_err(|_| Failure::Output)?;
            match content {
                Ok((content, road, nitrous, event, story)) => {
                    match story {
                        Ok(content) => {
                            prologue = Some(Arc::new(content));
                            tracing::info!("installed prologue opening prepared");
                        }
                        Err(error) => tracing::warn!(%error,"prologue opening unavailable"),
                    }
                    let mut content = nfs_server::scoring::Content::new(Arc::new(content))
                        .map_err(|_| Failure::ProfileConfig)?
                        .with_items(
                            inventory
                                .as_ref()
                                .ok_or(Failure::ProfileConfig)?
                                .item_catalog(),
                        )
                        .with_nitrous(nitrous);
                    match event {
                        Ok(prepared) => {
                            let (catalog, attached) = prepared
                                .attach(content)
                                .map_err(|_| Failure::ProfileConfig)?;
                            content = attached;
                            tracing::info!(
                                playable = catalog.iter().count(),
                                "normal sprint catalog compiled; progression gating disabled"
                            );
                            events = Some(Arc::new(catalog));
                        }
                        Err(error) => {
                            tracing::warn!(%error,"sprint catalog unavailable; events stay off")
                        }
                    }
                    match road {
                        Ok(network) => {
                            tracing::info!(
                                segments = network.segments().len(),
                                "road network read from the installation"
                            );
                            content = content.with_road(Arc::new(network));
                        }
                        Err(error) => tracing::warn!(
                            ?error,
                            "road network unavailable; road-driven scoring stays off"
                        ),
                    }
                    tracing::info!(
                        acquisition_nodes = content.installed().acquisition.program.nodes.len(),
                        scoring_nodes = content.installed().scoring.program.nodes.len(),
                        unmodeled = content.installed().scoring.program.unmodeled.len(),
                        keepers = ?content.installed().categories,
                        bus_entities = ?content.installed().scoring.bus_entities,
                        "free-roam scoring programs compiled from the installation"
                    );
                    Some(Arc::new(content))
                }
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "free-roam scoring unavailable; participants stay disabled"
                    );
                    None
                }
            }
        }
        _ => None,
    };
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
    // An unfinished intro cannot be resumed mid-story, so settle it before
    // any session restores progression from the account.
    let resume = match (&inventory, &progression) {
        (Some(profile), Some(progression)) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            let now = nfs_storage::Timestamp(now);
            let start = prologue
                .as_deref()
                .map(|p: &nfs_server::prologue::Content| p.start.objective);
            match profile.resume_intro(progression.clone(), start, now).await {
                Ok(resume) => Some(resume),
                Err(error) => {
                    tracing::warn!(?error, "unfinished intro check failed; account unchanged");
                    None
                }
            }
        }
        _ => None,
    };
    match resume {
        Some(net::Resume::Stranded) if args.game_dir.is_some() => {
            eprintln!(
                "nfs-server: this account has not finished the prologue and owns no car, but the \
                 prologue could not be prepared from the game installation. Check the game \
                 directory and the server log for \"prologue opening unavailable\"."
            );
            return Err(Failure::ProfileConfig);
        }
        Some(net::Resume::Stranded) => {
            tracing::warn!("unfinished intro without a car and no prologue; garage will be empty")
        }
        Some(resume @ (net::Resume::Restarted | net::Resume::Finished)) => {
            tracing::warn!(
                resume = resume.label(),
                "interrupted intro settled before serving"
            )
        }
        _ => {}
    }
    let listeners = net::Listeners::bind(args.listen, args.redirector_port).await?;
    let a = listeners.addresses()?;
    let lsx_listener = match args.lsx_port {
        Some(port) => Some(nfs_server::lsx::bind(args.listen, port).await?),
        None => None,
    };
    let lsx_address = lsx_listener
        .as_ref()
        .map(|listener| listener.local_addr())
        .transpose()
        .map_err(|_| Failure::Io)?;
    let (recorder, thread) = record::start_with_options(
        &args.root,
        &args.output,
        record::Options {
            raw: args.record_protocol,
            ..record::Options::default()
        },
    )?;
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
    ready["lsx"] = serde_json::json!(lsx_address.map(|a| a.to_string()));
    ready["intro_resume"] = serde_json::json!(resume.map(net::Resume::label));
    ready["manifest_mode"] = serde_json::json!("none");
    ready["install_content"] = args.generated.clone().unwrap_or(serde_json::Value::Null);
    println!("{ready}");
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    // Saved changes reach running sessions through this handle (live
    // traffic and rival amounts); without a settings file it never changes.
    let (ai_sender, ai) = nfs_server::ai_settings::Live::channel(args.ai);
    let settings_watch = args.settings_file.as_ref().map(|path| {
        tokio::spawn(nfs_server::settings_file::watch(
            args.root.join(path),
            args.ai,
            ai_sender,
            shutdown.clone(),
            nfs_server::settings_file::POLL,
        ))
    });
    // Started after the readiness line, so every later line on standard
    // output is a map frame. Its own thread: a slow reader never blocks the
    // runtime, and it ends once the server drops the board.
    let map_feed = if args.map_feed {
        let (board, reader) = nfs_server::map_feed::Board::new();
        let interval = Duration::from_millis(nfs_server::map_feed::INTERVAL_MS);
        std::thread::Builder::new()
            .name("map-feed".into())
            .spawn(move || nfs_server::map_feed::write(reader, std::io::stdout(), interval))
            .map_err(|_| Failure::Io)?;
        Some(board)
    } else {
        None
    };
    let config = net::Config {
        root: args.root,
        redirector_port: args.redirector_port,
        idle: Duration::from_secs(args.idle_seconds),
        qos_lifetime: Duration::from_secs(args.qos_seconds),
        world_content,
        inventory,
        progression,
        prologue,
        vehicles,
        sequences,
        traffic,
        rivals,
        garage_logic,
        scoring,
        events,
        catalogs,
        entitlements,
        user_settings,
        stats,
        owned_menu_awards: args.owned_menu_awards,
        owned_local_social: args.owned_local_social,
        ai,
        map_feed,
        challenges,
        kickback,
        speedwall,
        item_licenses,
    };
    let mut servers = tokio::task::JoinSet::new();
    servers.spawn(net::serve(
        listeners,
        pack,
        recorder,
        config,
        shutdown.clone(),
    ));
    if let Some(listener) = lsx_listener {
        servers.spawn(nfs_server::lsx::serve(
            listener,
            lsx_profile,
            Duration::from_secs(args.idle_seconds),
            shutdown,
        ));
    }
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
    let early = tokio::select! {
        _ = tokio::signal::ctrl_c() => None,
        () = watch_file => None,
        result = servers.join_next() => result,
    };
    let _ = stop.send(true);
    let mut served = early
        .map(|r| r.unwrap_or(Err(Failure::Io)))
        .unwrap_or(Ok(()));
    while let Some(result) = servers.join_next().await {
        if let Err(error) = result.unwrap_or(Err(Failure::Io)) {
            served = Err(error);
        }
    }
    if let Some(task) = settings_watch {
        let _ = task.await;
    }
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
    fn protocol_capture_requires_an_explicit_single_opt_in() {
        assert!(
            !parse_from(configured().into_iter())
                .unwrap()
                .record_protocol
        );
        let mut args = configured();
        args.push("--record-protocol".into());
        assert!(
            parse_from(args.clone().into_iter())
                .unwrap()
                .record_protocol
        );
        args.push("--record-protocol".into());
        assert!(parse_from(args.into_iter()).is_err());
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
            ("--listen", "0.0.0.0"),
            ("--listen", "255.255.255.255"),
            ("--listen", "224.0.0.1"),
            ("--listen", "::1"),
            ("--listen", "localhost"),
            ("--listen", "192.168.1.5:80"),
        ] {
            let mut args = configured();
            args.extend([flag.into(), value.into()]);
            assert!(parse_from(args.into_iter()).is_err(), "{flag} {value}");
        }
    }

    #[test]
    fn listeners_use_loopback_unless_one_address_is_chosen() {
        let parsed = parse_from(configured().into_iter()).unwrap();
        assert_eq!(parsed.listen, std::net::Ipv4Addr::LOCALHOST);
        let mut args = configured();
        args.extend(["--listen".into(), "192.168.1.5".into()]);
        let parsed = parse_from(args.into_iter()).unwrap();
        assert_eq!(parsed.listen, std::net::Ipv4Addr::new(192, 168, 1, 5));
    }

    fn without(mut args: Vec<std::ffi::OsString>, flags: &[&str]) -> Vec<std::ffi::OsString> {
        for flag in flags {
            let index = args.iter().position(|s| s == flag).unwrap();
            args.drain(index..index + 2);
        }
        args
    }

    const GENERATED: [&str; 5] = [
        "--item-content",
        "--persistent-content",
        "--stat-definitions",
        "--item-licenses",
        "--control-catalogs",
    ];

    #[test]
    fn traffic_and_rival_amounts_default_to_the_original_and_are_bounded() {
        let parsed = parse_from(configured().into_iter()).unwrap();
        assert_eq!(parsed.ai, nfs_server::ai_settings::AiSettings::default());
        let mut args = configured();
        args.extend([
            "--traffic-percent".into(),
            "300".into(),
            "--rival-limit".into(),
            "15".into(),
            "--settings-file".into(),
            "settings.json".into(),
        ]);
        let parsed = parse_from(args.into_iter()).unwrap();
        assert_eq!(
            (parsed.ai.traffic_percent(), parsed.ai.rival_limit()),
            (300, Some(15))
        );
        assert_eq!(parsed.settings_file, Some(PathBuf::from("settings.json")));
        let parsed = parse_from(configured().into_iter()).unwrap();
        assert_eq!(parsed.settings_file, None, "amounts stay as started");
        assert!(!parsed.map_feed, "no map frames unless asked");
        for (flag, value) in [
            ("--traffic-percent", "301"),
            ("--traffic-percent", "-1"),
            ("--traffic-percent", "lots"),
            ("--rival-limit", "16"),
            ("--rival-limit", "none"),
        ] {
            let mut args = configured();
            args.extend([flag.into(), value.into()]);
            assert!(parse_from(args.into_iter()).is_err(), "{flag} {value}");
        }
    }

    #[test]
    fn map_feed_is_one_flag_without_a_value() {
        let mut args = configured();
        args.push("--map-feed".into());
        assert!(parse_from(args.clone().into_iter()).unwrap().map_feed);
        args.push("--map-feed".into());
        assert!(parse_from(args.into_iter()).is_err(), "given twice");
    }

    #[test]
    fn game_dir_replaces_generated_options_exclusively() {
        let mut args = without(configured(), &GENERATED);
        args.extend(["--game-dir".into(), "game".into()]);
        let parsed = parse_from(args.clone().into_iter()).unwrap();
        assert_eq!(parsed.game_dir, Some(PathBuf::from("game")));
        assert!(parsed.item_content.is_none() && parsed.generated.is_none());
        let mut cached = args.clone();
        cached.extend(["--content-cache".into(), "cache".into()]);
        assert!(parse_from(cached.into_iter()).is_ok());
        let missing_progression = without(
            args.clone(),
            &[
                "--vehicle-content",
                "--garage-layout",
                "--progression-content",
            ],
        );
        assert!(parse_from(missing_progression.into_iter()).is_ok());
        for flag in [
            "--item-content",
            "--persistent-content",
            "--world-mac-template",
        ] {
            let mut both = args.clone();
            both.extend([flag.into(), "explicit".into()]);
            assert!(parse_from(both.into_iter()).is_err(), "{flag}");
        }
        let mut orphan = configured();
        orphan.extend(["--content-cache".into(), "cache".into()]);
        assert!(parse_from(orphan.into_iter()).is_err());
        // Without either source the dependent options are rejected as before.
        assert!(parse_from(without(configured(), &GENERATED).into_iter()).is_err());
    }

    #[test]
    fn resolution_fills_generated_paths_and_reports_the_entry() {
        let dir = std::env::temp_dir().join(format!("nfs-server-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (options, _) =
            nfs_content::synthetic::write(&dir.join("game"), &nfs_content::synthetic::assets())
                .unwrap();
        let with_game = |game: &str| {
            let mut args = without(configured(), &GENERATED);
            args = without(
                args,
                &[
                    "--world-content",
                    "--garage-layout",
                    "--vehicle-content",
                    "--progression-content",
                    "--sequence-content",
                ],
            );
            args.extend([
                "--root".into(),
                dir.clone().into_os_string(),
                "--game-dir".into(),
                game.into(),
            ]);
            parse_from(args.into_iter()).unwrap()
        };
        let mut parsed = with_game("game");
        resolve_install_content(&mut parsed, &options).unwrap();
        let items = parsed.item_content.clone().unwrap();
        assert!(items.starts_with(dir.join("artifacts/content")));
        assert!(items.exists());
        assert!(parsed.persistent_content.as_ref().unwrap().exists());
        assert!(parsed.vehicle_parts.as_ref().unwrap().exists());
        assert!(parsed.vehicle_content.as_ref().unwrap().exists());
        nfs_content::traffic::Content::load(parsed.traffic_content.as_ref().unwrap()).unwrap();
        nfs_content::rivals::Content::load(parsed.rival_content.as_ref().unwrap()).unwrap();
        nfs_server::progression::Content::load(parsed.progression_content.as_ref().unwrap())
            .unwrap();
        nfs_server::sequence_content::SequenceContent::load(
            parsed.sequence_content.as_ref().unwrap(),
        )
        .unwrap()
        .arrivals()
        .unwrap()
        .unwrap();
        let stats = parsed.stat_definitions.as_ref().unwrap();
        nfs_services::stats::Catalog::load(stats).unwrap();
        nfs_services::item_licenses::Content::load(parsed.item_licenses.as_ref().unwrap()).unwrap();
        nfs_services::control_catalogs::Catalog::load(parsed.control_catalogs.as_ref().unwrap())
            .unwrap();
        nfs_services::reputation::Thresholds::load(parsed.reputation_thresholds.as_ref().unwrap())
            .unwrap();
        let scenes =
            nfs_content::scenes::Catalog::load(parsed.scene_catalog.as_ref().unwrap()).unwrap();
        content::WorldContent::load_with_scenes(
            parsed.world_content.as_ref().unwrap(),
            Some(&scenes),
        )
        .unwrap()
        .validate_runtime()
        .unwrap();
        crate::vehicle_content::layout::Layout::load(parsed.garage_layout.as_ref().unwrap())
            .unwrap();
        assert!(parsed.world_mac_template.as_ref().unwrap().exists());
        assert_eq!(parsed.generated.as_ref().unwrap()["built"], true);
        let mut again = with_game("game");
        resolve_install_content(&mut again, &options).unwrap();
        assert_eq!(again.generated.as_ref().unwrap()["built"], false);
        // Stats presentation is operator policy, so an explicit catalog wins.
        let mut explicit_args = without(configured(), &["--item-content", "--persistent-content"]);
        explicit_args.extend(["--scene-catalog".into(), "authored-scenes.json".into()]);
        explicit_args.extend(["--vehicle-parts".into(), "authored-parts.json".into()]);
        explicit_args.extend([
            "--root".into(),
            dir.clone().into_os_string(),
            "--game-dir".into(),
            "game".into(),
        ]);
        let mut explicit = parse_from(explicit_args.into_iter()).unwrap();
        resolve_install_content(&mut explicit, &options).unwrap();
        assert_eq!(
            explicit.stat_definitions,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.item_licenses,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.control_catalogs,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.scene_catalog,
            Some(PathBuf::from("authored-scenes.json"))
        );
        assert_eq!(
            explicit.vehicle_parts,
            Some(PathBuf::from("authored-parts.json"))
        );
        assert_eq!(
            explicit.vehicle_content,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.world_content,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.garage_layout,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.progression_content,
            Some(PathBuf::from("configured-input"))
        );
        assert_eq!(
            explicit.sequence_content,
            Some(PathBuf::from("configured-input"))
        );
        assert!(resolve_install_content(&mut with_game("missing"), &options).is_err());
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn lsx_port_is_optional_and_bounded() {
        assert!(
            parse_from(configured().into_iter())
                .unwrap()
                .lsx_port
                .is_none()
        );
        for port in ["0", "3216", "65535"] {
            let mut args = configured();
            args.extend(["--lsx-port".into(), port.into()]);
            assert_eq!(
                parse_from(args.into_iter()).unwrap().lsx_port,
                Some(port.parse().unwrap())
            );
        }
        let mut args = configured();
        args.extend(["--lsx-port".into(), "65536".into()]);
        assert!(parse_from(args.into_iter()).is_err());
    }
}
