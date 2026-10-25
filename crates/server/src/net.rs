// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Loopback listeners, per-connection tasks, bounded queues and deadlines.
//! Blocking storage and compatibility TLS work run on bounded blocking workers.
//! The control edge persists staged settings before writing their acknowledgement.
use crate::{
    Failure,
    control::{ControlSession, Outcome, PhaseName},
    deployment::{Deployment, Endpoints},
    record::{Direction, Recorder, Route},
    seeds::{OsRandom, OsSeeds, fresh},
};
use crate::{world_handshake, world_readiness};
use nfs_server_support::{discovery, qos_tls};
use nfs_world::{
    application::{self, Policy},
    session::{Event, Host, Ids, StageName},
    transport::Codec,
};
use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{mpsc, watch},
    task::JoinSet,
    time::timeout,
};
use tracing::{debug, info, warn};

pub const READ_CHUNK: usize = 8192;
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_CONNECTIONS: usize = 32;
pub const WORLD_PROOF_TIMEOUT: Duration = Duration::from_secs(30);
pub const WORLD_TICK: Duration = Duration::from_millis(50);
pub const WORLD_IDLE: Duration = Duration::from_secs(120);
pub struct WorldPort {
    pub start: mpsc::Sender<world_handshake::Binding>,
    pub proof: watch::Receiver<Option<Option<world_readiness::Binding>>>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub root: std::path::PathBuf,
    pub redirector_port: u16,
    pub idle: Duration,
    /// Deadline for one QoS TLS request; idle listeners follow server shutdown.
    pub qos_lifetime: Duration,
    pub world_content: Option<Arc<crate::content::WorldContent>>,
    pub inventory: Option<InventoryProfile>,
    pub progression: Option<Arc<crate::progression::Content>>,
    pub prologue: Option<Arc<crate::prologue::Content>>,
    pub vehicles: Option<Arc<crate::vehicle_content::GarageContent>>,
    pub sequences: Option<Arc<crate::sequence_content::SequenceContent>>,
    pub traffic: Option<Arc<nfs_content::traffic::Content>>,
    pub rivals: Option<Arc<crate::rival_content::Content>>,
    pub garage_logic: Option<Arc<crate::garage_logic::GarageLogic>>,
    pub scoring: Option<Arc<crate::scoring::Content>>,
    /// The owned race event, compiled from the installation (with scoring).
    pub events: Option<Arc<crate::event_logic::catalog::Catalog>>,
    pub user_settings: Option<Arc<crate::user_settings::Store>>,
    pub catalogs: Option<Arc<nfs_services::control_catalogs::Catalog>>,
    pub entitlements: Option<(
        nfs_storage::AccountId,
        Arc<nfs_services::entitlements::State>,
    )>,
    pub stats: Option<Arc<nfs_services::stats::Catalog>>,
    pub owned_menu_awards: bool,
    pub owned_local_social: bool,
    /// Player-chosen traffic and roaming-rival amounts, updated while the
    /// server runs when it watches a settings file.
    pub ai: crate::ai_settings::Live,
    /// Where world sessions publish what the launcher's map shows
    /// (`--map-feed`); `None` builds no snapshots.
    pub map_feed: Option<crate::map_feed::Board>,
    pub challenges: Option<Arc<nfs_services::challenges::Catalog>>,
    pub kickback: Option<(nfs_storage::AccountId, Arc<nfs_services::kickback::State>)>,
    pub speedwall: Option<(nfs_storage::AccountId, Arc<nfs_services::speedwall::State>)>,
    pub item_licenses: Option<Arc<nfs_services::item_licenses::Content>>,
}
pub struct AccountServices {
    pub settings: Option<Arc<crate::user_settings::Store>>,
    pub stats: Option<(InventoryProfile, nfs_services::reputation::Thresholds)>,
    pub challenges: Option<InventoryProfile>,
    pub speedwall: Option<InventoryProfile>,
}
#[derive(Clone)]
pub struct WorldServe {
    pub mac_template: Option<[u8; 64]>,
    pub content: Option<Arc<crate::content::WorldContent>>,
    pub persona: i64,
    pub inventory: Option<InventoryProfile>,
    pub progression: Option<Arc<crate::progression::Content>>,
    pub prologue: Option<Arc<crate::prologue::Content>>,
    pub vehicles: Option<Arc<crate::vehicle_content::GarageContent>>,
    pub sequences: Option<Arc<crate::sequence_content::SequenceContent>>,
    pub traffic: Option<Arc<nfs_content::traffic::Content>>,
    pub rivals: Option<Arc<crate::rival_content::Content>>,
    pub garage_logic: Option<Arc<crate::garage_logic::GarageLogic>>,
    pub scoring: Option<Arc<crate::scoring::Content>>,
    pub events: Option<Arc<crate::event_logic::catalog::Catalog>>,
    pub ai: crate::ai_settings::Live,
    pub map_feed: Option<crate::map_feed::Board>,
}

mod items;
mod players;
mod qos;
#[cfg(feature = "network-test")]
mod test_loss;
pub use items::{Profile as InventoryProfile, Service as InventoryService, story::Resume};
const FILE_RECEIVE_MS: u64 = 60_000;
#[derive(Debug, Default)]
pub struct Accepting {
    pub frames: usize,
    pub accepted: usize,
    pub masks: std::collections::BTreeMap<u8, usize>,
    pub indices: std::collections::BTreeMap<u8, usize>,
    pub errors: usize,
    files: [nfs_world::files::Receiver; 2],
    file_started: [Option<u64>; 2],
    now: u64,
    pub file_records: usize,
}

impl Accepting {
    fn parse<'a>(
        &self,
        queued: application::Queued<'a>,
    ) -> Result<nfs_world::frame::Frame<'a>, nfs_world::frame::Error> {
        nfs_world::frame::parse_with_files(
            queued.data,
            nfs_world::frame::Direction::FromClient,
            self.files.each_ref().map(|f| f.size()),
        )
    }
    fn file_transition(
        &self,
        parsed: &nfs_world::frame::Frame<'_>,
    ) -> Result<Option<[nfs_world::files::Receiver; 2]>, nfs_world::files::Error> {
        if !parsed.complete && parsed.mask & 0b110000 != 0 {
            return Err(nfs_world::files::Error::Incomplete);
        }
        if parsed.files.is_empty() {
            return Ok(None);
        }
        let mut files = self.files.clone();
        for (handler, record) in &parsed.files {
            files[usize::from(handler - 4)].receive(record)?;
        }
        Ok(Some(files))
    }
    fn record(
        &mut self,
        parsed: &nfs_world::frame::Frame<'_>,
        files: Option<[nfs_world::files::Receiver; 2]>,
    ) -> bool {
        self.frames += 1;
        if let Some(files) = files {
            for (index, file) in files.iter().enumerate() {
                if self.file_started[index].is_none()
                    && (file.size().is_some() || file.completed().is_some())
                {
                    self.file_started[index] = Some(self.now);
                }
            }
            self.files = files;
        }
        self.file_records += parsed.files.len();
        *self.masks.entry(parsed.mask).or_default() += 1;
        if let Some(messages) = &parsed.messages {
            for group in &messages.groups {
                for m in &group.messages {
                    *self.indices.entry(m.index()).or_default() += 1;
                }
            }
        }
        self.accepted += 1;
        true
    }
    fn invalid(&mut self) -> bool {
        self.frames += 1;
        self.errors += 1;
        false
    }
    fn take_file(&mut self, index: usize) -> Option<nfs_world::files::Completed> {
        let result = self.files[index].take_completed()?;
        self.file_started[index] = None;
        Some(result)
    }
    fn files_alive(&mut self, now: u64) -> bool {
        if now < self.now {
            return false;
        }
        self.now = now;
        !self
            .file_started
            .iter()
            .flatten()
            .any(|start| now - start >= FILE_RECEIVE_MS)
    }
}

impl application::Listener for Accepting {
    fn frame(&mut self, queued: application::Queued<'_>) -> bool {
        let Ok(parsed) = self.parse(queued) else {
            return self.invalid();
        };
        let Ok(files) = self.file_transition(&parsed) else {
            return self.invalid();
        };
        self.record(&parsed, files)
    }
}
pub trait FrameHandler: Send {
    fn needs_speedwall_view(&self, _wire: &[u8]) -> bool {
        false
    }
    fn speedwall_view(&mut self, _current: nfs_services::speedwall::Current) {}
    fn needs_challenges_view(&self, _wire: &[u8]) -> bool {
        false
    }
    fn challenges_view(&mut self, _current: nfs_services::challenges::Current) {}
    fn needs_awards_view(&self, _wire: &[u8]) -> bool {
        false
    }
    fn awards_view(&mut self, _current: nfs_services::awards::Current) {}
    fn needs_account_view(&self, _wire: &[u8]) -> bool {
        false
    }
    fn account_view(&mut self, _current: nfs_services::stats::Current) {}
    fn on_frame(
        &mut self,
        wire: &[u8],
        unix_seconds: u32,
        unix_micros: i64,
    ) -> Result<Outcome, Failure>;
    fn committed(&mut self) -> Result<(), Failure>;
    fn write_failed(&mut self);
    fn phase(&self) -> PhaseName;
    fn world_proof(&mut self, _binding: world_readiness::Binding) {}
    fn resume_world(&mut self, _unix_micros: i64) -> Result<Outcome, Failure> {
        Ok(Outcome::Unsupported)
    }
    fn take_world_start(&mut self) -> Option<world_handshake::Binding> {
        None
    }
    fn needs_settings_view(&self, _wire: &[u8]) -> bool {
        false
    }
    fn settings_view(&mut self, _current: crate::user_settings::Settings) -> Result<(), Failure> {
        Ok(())
    }
    fn pending_settings_change(&self) -> Option<crate::user_settings::Change> {
        None
    }
}

impl FrameHandler for ControlSession<'_> {
    fn needs_speedwall_view(&self, wire: &[u8]) -> bool {
        ControlSession::needs_speedwall_view(self, wire)
    }
    fn speedwall_view(&mut self, current: nfs_services::speedwall::Current) {
        ControlSession::speedwall_view(self, current);
    }
    fn needs_challenges_view(&self, wire: &[u8]) -> bool {
        ControlSession::needs_challenges_view(self, wire)
    }
    fn challenges_view(&mut self, current: nfs_services::challenges::Current) {
        ControlSession::challenges_view(self, current);
    }
    fn needs_awards_view(&self, wire: &[u8]) -> bool {
        ControlSession::needs_awards_view(self, wire)
    }
    fn awards_view(&mut self, current: nfs_services::awards::Current) {
        ControlSession::awards_view(self, current);
    }
    fn needs_account_view(&self, wire: &[u8]) -> bool {
        ControlSession::needs_account_view(self, wire)
    }
    fn account_view(&mut self, current: nfs_services::stats::Current) {
        ControlSession::account_view(self, current);
    }
    fn on_frame(
        &mut self,
        wire: &[u8],
        unix_seconds: u32,
        unix_micros: i64,
    ) -> Result<Outcome, Failure> {
        ControlSession::on_frame(self, wire, unix_seconds, unix_micros)
    }
    fn committed(&mut self) -> Result<(), Failure> {
        ControlSession::committed(self)
    }
    fn write_failed(&mut self) {
        ControlSession::write_failed(self)
    }
    fn phase(&self) -> PhaseName {
        ControlSession::phase(self)
    }
    fn world_proof(&mut self, binding: world_readiness::Binding) {
        ControlSession::world_proof(self, binding)
    }
    fn resume_world(&mut self, unix_micros: i64) -> Result<Outcome, Failure> {
        ControlSession::resume_world(self, unix_micros)
    }
    fn take_world_start(&mut self) -> Option<world_handshake::Binding> {
        ControlSession::take_world_start(self)
    }
    fn needs_settings_view(&self, wire: &[u8]) -> bool {
        ControlSession::needs_settings_view(self, wire)
    }
    fn settings_view(&mut self, current: crate::user_settings::Settings) -> Result<(), Failure> {
        ControlSession::settings_view(self, current)
    }
    fn pending_settings_change(&self) -> Option<crate::user_settings::Change> {
        ControlSession::pending_settings_change(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    PeerClosed,
    Idle,
    InvalidFrame,
    WriteFailed,
    ReadFailed,
    Internal,
}

pub fn unix_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u32::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

fn unix_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_micros()).ok())
        .unwrap_or(1)
}
async fn wait_world_proof(port: &mut WorldPort) -> Option<Option<world_readiness::Binding>> {
    let wait = async {
        loop {
            let current = *port.proof.borrow();
            if let Some(binding) = current {
                return Some(binding);
            }
            if port.proof.changed().await.is_err() {
                return None;
            }
        }
    };
    timeout(WORLD_PROOF_TIMEOUT, wait).await.ok().flatten()
}
pub async fn run_control(
    stream: &mut TcpStream,
    handler: &mut dyn FrameHandler,
    connection: u32,
    recorder: &Recorder,
    idle: Duration,
    mut world: Option<WorldPort>,
    services: Option<AccountServices>,
) -> End {
    let limits = crate::frame_limits();
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0; READ_CHUNK];
    loop {
        loop {
            let consumed = match nfs_fire2::decode(&buffer, limits) {
                Ok(Some(decoded)) => decoded.consumed,
                Ok(None) => break,
                Err(_) => {
                    let _ = recorder
                        .chunk("blaze", connection, Direction::In, &buffer, Some("invalid"))
                        .await;
                    warn!(
                        connection,
                        bytes = buffer.len(),
                        "invalid Fire2 frame; closing"
                    );
                    return End::InvalidFrame;
                }
            };
            let frame: Vec<u8> = buffer.drain(..consumed).collect();
            let route = Route::of(&frame);
            let phase = handler.phase();
            if handler.needs_challenges_view(&frame) {
                let Some(profile) = services.as_ref().and_then(|s| s.challenges.as_ref()) else {
                    return End::Internal;
                };
                let now = nfs_storage::Timestamp(u64::try_from(unix_micros() / 1000).unwrap_or(0));
                match profile.current_challenges(now).await {
                    Ok(current) => {
                        info!(
                            connection,
                            ?route,
                            generation = current.generation(),
                            "current account challenges prepared"
                        );
                        handler.challenges_view(current);
                    }
                    Err(error) => {
                        warn!(connection, ?route, ?error, "account challenges unavailable");
                        return End::Internal;
                    }
                }
            } else if handler.needs_speedwall_view(&frame)
                && services.as_ref().is_some_and(|s| s.speedwall.is_some())
            {
                let Some(profile) = services.as_ref().and_then(|s| s.speedwall.as_ref()) else {
                    return End::Internal;
                };
                let now = nfs_storage::Timestamp(u64::try_from(unix_micros() / 1000).unwrap_or(0));
                match profile.current_speedwall(now).await {
                    Ok(current) => {
                        info!(
                            connection,
                            ?route,
                            generation = current.generation,
                            "current account Speedwall prepared"
                        );
                        handler.speedwall_view(current);
                    }
                    Err(error) => {
                        warn!(connection, ?route, ?error, "account Speedwall unavailable");
                        return End::Internal;
                    }
                }
            } else if handler.needs_awards_view(&frame) {
                let Some((profile, thresholds)) = services.as_ref().and_then(|s| s.stats.as_ref())
                else {
                    return End::Internal;
                };
                let now = nfs_storage::Timestamp(u64::try_from(unix_micros() / 1000).unwrap_or(0));
                match profile.current_awards(now, thresholds).await {
                    Ok(current) => {
                        info!(
                            connection,
                            ?route,
                            generation = current.generation(),
                            "current account menu awards prepared"
                        );
                        handler.awards_view(current);
                    }
                    Err(error) => {
                        warn!(
                            connection,
                            ?route,
                            ?error,
                            "account menu awards unavailable"
                        );
                        return End::Internal;
                    }
                }
            } else if handler.needs_account_view(&frame) {
                let Some((profile, thresholds)) = services.as_ref().and_then(|s| s.stats.as_ref())
                else {
                    return End::Internal;
                };
                let now = nfs_storage::Timestamp(u64::try_from(unix_micros() / 1000).unwrap_or(0));
                let current = profile.current_tables(now).await.and_then(|loaded| {
                    nfs_services::stats::Current::from_loaded(&loaded, thresholds)
                        .map_err(|_| Failure::ProfileConfig)
                });
                match current {
                    Ok(current) => {
                        info!(
                            connection,
                            ?route,
                            generation = current.generation,
                            "current account statistics prepared"
                        );
                        handler.account_view(current);
                    }
                    Err(error) => {
                        warn!(connection, ?route, ?error, "account statistics unavailable");
                        return End::Internal;
                    }
                }
            }
            if handler.needs_settings_view(&frame) {
                let Some(store) = services.as_ref().and_then(|s| s.settings.clone()) else {
                    handler.write_failed();
                    return End::Internal;
                };
                let refreshed = tokio::task::spawn_blocking(move || store.current())
                    .await
                    .map_err(|_| Failure::Output)
                    .and_then(|value| value.map_err(crate::user_settings::failure))
                    .and_then(|current| handler.settings_view(current));
                if refreshed.is_err() {
                    handler.write_failed();
                    warn!(connection, "user settings unavailable");
                    return End::Internal;
                }
            }
            let mut outcome = handler.on_frame(&frame, unix_seconds(), unix_micros());
            let note = match &outcome {
                Ok(Outcome::Reply(_)) => None,
                Ok(Outcome::Unsupported) => Some("unsupported"),
                Ok(Outcome::AwaitWorld) => Some("await-world"),
                Err(_) => Some("internal-error"),
            };
            if recorder
                .chunk("blaze", connection, Direction::In, &frame, note)
                .await
                .is_err()
            {
                return End::Internal;
            }
            loop {
                match outcome {
                    Ok(Outcome::Reply(frames)) => {
                        if let Some(change) = handler.pending_settings_change() {
                            let Some(store) = services.as_ref().and_then(|s| s.settings.clone())
                            else {
                                handler.write_failed();
                                return End::Internal;
                            };
                            let persisted =
                                tokio::task::spawn_blocking(move || store.apply(&change))
                                    .await
                                    .map_err(|_| Failure::Output)
                                    .and_then(|value| value.map_err(crate::user_settings::failure))
                                    .and_then(|current| handler.settings_view(current));
                            if persisted.is_err() {
                                handler.write_failed();
                                warn!(connection, "user settings save failed; no reply");
                                return End::Internal;
                            }
                            info!(connection, "user settings durably committed before reply");
                        }
                        for wire in &frames {
                            let written = timeout(WRITE_TIMEOUT, stream.write_all(wire)).await;
                            if !matches!(written, Ok(Ok(()))) {
                                handler.write_failed();
                                warn!(connection, "reply write failed");
                                return End::WriteFailed;
                            }
                            if recorder
                                .chunk("blaze", connection, Direction::Out, wire, None)
                                .await
                                .is_err()
                            {
                                return End::Internal;
                            }
                        }
                        if handler.committed().is_err() {
                            return End::Internal;
                        }
                        info!(connection, ?phase, route = ?route, frames = frames.len(), "reply");
                        if let Some(binding) = handler.take_world_start() {
                            match world.as_ref() {
                                Some(port) => {
                                    if port.start.send(binding).await.is_err() {
                                        warn!(connection, "world host is not running");
                                    } else {
                                        info!(connection, "world host identities handed over");
                                    }
                                }
                                None => {
                                    warn!(connection, "world setup written without a world host")
                                }
                            }
                        }
                        break;
                    }
                    Ok(Outcome::Unsupported) => {
                        // This path closes immediately, so at most one warning
                        // is emitted for this control connection.
                        warn!(service = "blaze", connection, ?phase, route = ?route, disposition = "closing", "unsupported operation received");
                        break;
                    }
                    Ok(Outcome::AwaitWorld) => {
                        let Some(port) = world.as_mut() else {
                            warn!(connection, route = ?route, "readiness needs a world host");
                            break;
                        };
                        info!(connection, route = ?route, "waiting for the world sync");
                        match wait_world_proof(port).await {
                            Some(Some(binding)) => {
                                handler.world_proof(binding);
                                outcome = handler.resume_world(unix_micros());
                                continue;
                            }
                            Some(None) => {
                                warn!(connection, route = ?route, "world synced without a readiness binding");
                                break;
                            }
                            None => {
                                warn!(connection, route = ?route, "world sync did not complete");
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        warn!(connection, ?phase, route = ?route, ?error, "handler failure; closing");
                        return End::Internal;
                    }
                }
            }
        }
        let read = match timeout(idle, stream.read(&mut chunk)).await {
            Err(_) => {
                if !buffer.is_empty() {
                    let _ = recorder
                        .chunk("blaze", connection, Direction::In, &buffer, Some("partial"))
                        .await;
                }
                return End::Idle;
            }
            Ok(Err(_)) => return End::ReadFailed,
            Ok(Ok(0)) => {
                if !buffer.is_empty() {
                    let _ = recorder
                        .chunk("blaze", connection, Direction::In, &buffer, Some("partial"))
                        .await;
                }
                return End::PeerClosed;
            }
            Ok(Ok(n)) => n,
        };
        if buffer.try_reserve(read).is_err() {
            return End::Internal;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}
pub struct Listeners {
    redirector: TcpListener,
    blaze: TcpListener,
    qos_tcp: std::net::TcpListener,
    qos_udp: std::net::UdpSocket,
}

#[derive(Clone, Copy, Debug)]
pub struct Addresses {
    pub redirector: SocketAddr,
    pub blaze: SocketAddr,
    pub qos: SocketAddr,
    pub qos_udp: SocketAddr,
}

/// Whether a listener on `local` serves `peer`. A loopback listener serves
/// only this computer; one on a network address (`--listen`, an explicit
/// opt-in) serves the network it was opened to.
pub fn serves(local: IpAddr, peer: IpAddr) -> bool {
    !local.is_loopback() || peer.is_loopback()
}

impl Listeners {
    /// Bind every listener to `listen`, the one address the server
    /// advertises; `127.0.0.1` unless the operator chose another.
    pub async fn bind(listen: Ipv4Addr, redirector_port: u16) -> Result<Self, Failure> {
        let local = |port| SocketAddr::from((listen, port));
        let tcp = |port| async move {
            TcpListener::bind(local(port))
                .await
                .map_err(|_| Failure::Io)
        };
        Ok(Self {
            redirector: tcp(redirector_port).await?,
            blaze: tcp(0).await?,
            qos_tcp: std::net::TcpListener::bind(local(0)).map_err(|_| Failure::Io)?,
            qos_udp: std::net::UdpSocket::bind(local(0)).map_err(|_| Failure::Io)?,
        })
    }

    pub fn addresses(&self) -> Result<Addresses, Failure> {
        let io = |_| Failure::Io;
        Ok(Addresses {
            redirector: self.redirector.local_addr().map_err(io)?,
            blaze: self.blaze.local_addr().map_err(io)?,
            qos: self.qos_tcp.local_addr().map_err(io)?,
            qos_udp: self.qos_udp.local_addr().map_err(io)?,
        })
    }
}

/// The QoS identity and the redirector reply for the bound listeners: the
/// certificate names the address the game reaches QoS at, and the reply
/// directs the game to Blaze.
fn advertise(addresses: &Addresses) -> Result<(qos_tls::Identity, Vec<u8>), Failure> {
    let IpAddr::V4(qos) = addresses.qos.ip() else {
        return Err(Failure::Io);
    };
    let identity = qos_tls::Identity::for_address(qos).map_err(|_| Failure::Reply)?;
    let reply = discovery::local_response_with_ca(addresses.blaze, Some(identity.ca_pem()))
        .map_err(crate::support_failure)?;
    Ok((identity, reply))
}

pub async fn serve(
    listeners: Listeners,
    pack: Arc<Deployment>,
    recorder: Recorder,
    config: Config,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), Failure> {
    if let Some(content) = &config.world_content {
        content.validate_runtime()?;
    }
    let addresses = listeners.addresses()?;
    let (identity, redirector_reply) = advertise(&addresses)?;
    let (identity, redirector_reply) = (Arc::new(identity), Arc::new(redirector_reply));
    let connections = Arc::new(AtomicU32::new(0));
    let mut tasks = JoinSet::new();

    let mut qos = tokio::spawn(qos::serve(
        listeners.qos_tcp,
        listeners.qos_udp,
        identity.clone(),
        config.qos_lifetime,
        shutdown.clone(),
    ));

    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            ended = &mut qos => {
                tasks.shutdown().await;
                if *shutdown.borrow() { return Ok(()); }
                warn!(?ended, "qos service ended before server shutdown");
                return Err(Failure::Io);
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            accepted = listeners.redirector.accept(), if tasks.len() < MAX_CONNECTIONS => {
                let Ok((stream, peer)) = accepted else { continue };
                let n = connections.fetch_add(1, Ordering::Relaxed);
                let reply = redirector_reply.clone();
                let recorder = recorder.clone();
                tasks.spawn(async move { redirector(stream, peer, n, &reply, &recorder).await });
            }
            accepted = listeners.blaze.accept(), if tasks.len() < MAX_CONNECTIONS => {
                let Ok((mut stream, peer)) = accepted else { continue };
                let n = connections.fetch_add(1, Ordering::Relaxed);
                let pack = pack.clone();
                let recorder = recorder.clone();
                let idle = config.idle;
                let world_content = config.world_content.clone();
                let inventory = config.inventory.clone();
                let progression = config.progression.clone();
                let vehicles = config.vehicles.clone();
                let sequences = config.sequences.clone();
                let traffic = config.traffic.clone();
                let rivals = config.rivals.clone();
                let ai = config.ai.clone();
                let map_feed = config.map_feed.clone();
                let garage_logic = config.garage_logic.clone();
                let scoring = config.scoring.clone();
                let events = config.events.clone();
                let prologue = config.prologue.clone();
                let catalogs = config.catalogs.clone();
                let entitlements = config.entitlements.clone();
                let user_settings = config.user_settings.clone();
                let stats = config.stats.clone();
                let owned_menu_awards = config.owned_menu_awards;
                let owned_local_social = config.owned_local_social;
                let challenges = config.challenges.clone();
                let kickback = config.kickback.clone();
                let speedwall = config.speedwall.clone();
                let item_licenses = config.item_licenses.clone();
                let connections = connections.clone();
                // The world and auxiliary endpoints share the advertised address.
                let listen = addresses.blaze.ip();
                let Ok(world_socket) = UdpSocket::bind((listen, 0)).await else {
                    warn!(connection = n, "cannot bind a world endpoint");
                    continue;
                };
                let Ok(world_address) = world_socket.local_addr() else { continue };
                let Ok(auxiliary) = TcpListener::bind((listen, 0)).await else { continue };
                let Ok(auxiliary_address) = auxiliary.local_addr() else { continue };
                let token_seed: [u8; crate::auxiliary::SEED_BYTES] = fresh(&mut OsSeeds)?;
                let endpoints = Endpoints {
                    auxiliary: auxiliary_address,
                    qos: addresses.qos,
                    world: world_address,
                };
                tasks.spawn(async move {
                    let profiles = match pack.session_profiles(endpoints, &mut OsSeeds, unix_micros()) {
                        Ok(p) => p,
                        Err(error) => {
                            warn!(connection = n, ?error, "cannot build session profiles");
                            return;
                        }
                    };
                    info!(connection = n, %peer, world = %world_address, auxiliary = %auxiliary_address, "control connection");
                    let (start, start_rx) = mpsc::channel(1);
                    let (proof_tx, proof) = watch::channel(None);
                    let mut worlds = JoinSet::new();
                    let account_services = AccountServices {
                        settings: user_settings.clone(),
                        challenges: challenges.as_ref().and(inventory.clone()),
                        speedwall: speedwall.as_ref().and(inventory.clone()),
                        stats: if stats.is_some() || owned_menu_awards {
                            inventory.clone().zip(garage_logic.as_ref().and_then(|l| l.reputation_thresholds()).cloned())
                        } else { None },
                    };
                    let records = match &inventory {
                        Some(profile) => profile.bind_records(profiles.records.clone()),
                        None => profiles.records.clone(),
                    };
                    worlds.spawn(crate::auxiliary::listener(
                        auxiliary, records, token_seed, connections, recorder.clone(),
                    ));
                    worlds.spawn(world_host(
                        world_socket,
                        start_rx,
                        proof_tx,
                        WorldServe {
                            mac_template: profiles.mac_template,
                            content: world_content.clone(),
                            persona: profiles.persona,
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
                            ai,
                            map_feed,
                        },
                        n,
                        recorder.clone(),
                    ));
                    let mut session = ControlSession::new(&profiles);
                    if let Some((account,state)) = &entitlements { session = session.with_entitlements(state,*account); }
                    if let Some((account, state)) = &kickback {
                        session = session.with_kickback(state, *account);
                    }
                    if let Some((account, state)) = &speedwall {
                        session = session.with_speedwall(state, *account);
                    }
                    if let Some(content) = item_licenses.as_deref() {
                        session = session.with_item_licenses(content);
                    }
                    if owned_local_social { session = session.with_owned_local_social(); }
                    if let Some(catalog) = catalogs.as_deref() {
                        session = session.with_control_catalogs(catalog);
                    }
                    if user_settings.is_some() {
                        session = session.with_user_settings(crate::user_settings::Settings::default());
                    }
                    if let Some(catalog) = stats.as_deref() {
                        session = session.with_stats(catalog);
                    }
                    if let Some(catalog) = challenges.as_deref() {
                        session = session.with_challenges(catalog);
                    }
                    if owned_menu_awards { session = session.with_owned_menu_awards(); }
                    let end = run_control(
                        &mut stream,
                        &mut session,
                        n,
                        &recorder,
                        idle,
                        Some(WorldPort { start, proof }),
                        Some(account_services),
                    )
                    .await;
                    info!(connection = n, ?end, phase = ?session.phase(), "control connection ended");
                    worlds.shutdown().await;
                });
            }
        }
    }
    tasks.shutdown().await;
    qos.await.map_err(|_| Failure::Io)??;
    Ok(())
}
pub async fn world_host(
    socket: UdpSocket,
    mut start: mpsc::Receiver<world_handshake::Binding>,
    proof: watch::Sender<Option<Option<world_readiness::Binding>>>,
    serve: WorldServe,
    connection: u32,
    recorder: Recorder,
) {
    let WorldServe {
        mac_template: template,
        content,
        persona,
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
        mut ai,
        map_feed,
    } = serve;
    let Some(binding) = start.recv().await else {
        return;
    };
    let Some(template) = template else {
        warn!(connection, "world host needs the MAC template; not serving");
        return;
    };
    let Ok(codec) = Codec::new(binding.key(), template) else {
        warn!(connection, "world host: bad session key");
        return;
    };
    let ids = Ids {
        client: binding.client(),
        host: binding.host(),
        host_index: 0,
    };
    let Some(mut host) = Host::new(codec, ids, Policy::default(), Box::new(OsRandom)) else {
        warn!(connection, "world host: unusable connection ids");
        return;
    };
    if let Some(content) = &content {
        let messages = match content.generate() {
            Ok(messages) => messages,
            Err(_) => {
                warn!(connection, "invalid local world content");
                return;
            }
        };
        let mut chunks = 0;
        for message in &messages {
            let target = message.target();
            let Ok(bytes) = message.encode() else {
                return;
            };
            match host.queue_content(target, &bytes) {
                Ok(n) => chunks += n,
                Err(error) => {
                    warn!(connection, target, ?error, "world content rejected");
                    return;
                }
            }
        }
        info!(
            connection,
            messages = messages.len(),
            chunks,
            "world content queued"
        );
    }
    let readiness = binding.readiness();
    let started = Instant::now();
    let now_ms = || started.elapsed().as_millis() as u64;
    let Ok(persona) = u64::try_from(persona) else {
        return;
    };
    // Leaves the map when this world host returns.
    let mut map_feed = map_feed.map(|board| board.publisher(connection, persona));
    let arrivals = match sequences.as_ref().and_then(|c| c.arrivals()).transpose() {
        Ok(arrivals) => arrivals,
        Err(error) => {
            warn!(connection, ?error, "arrive sequence configuration rejected");
            return;
        }
    };
    let returns = match sequences.as_ref().and_then(|c| c.returns()).transpose() {
        Ok(returns) => returns,
        Err(error) => {
            warn!(
                connection,
                ?error,
                "garage return sequence configuration rejected"
            );
            return;
        }
    };
    let crashes = match sequences.as_ref().and_then(|c| c.crashes()).transpose() {
        Ok(crashes) => crashes,
        Err(error) => {
            warn!(connection, ?error, "crash sequence configuration rejected");
            return;
        }
    };
    if crashes.is_some()
        && garage_logic
            .as_ref()
            .is_none_or(|logic| !logic.has_crash_recovery())
    {
        warn!(
            connection,
            "crash sequence requires paired crash/recovery presentation"
        );
        return;
    }
    let sequences = match sequences.as_ref().map(|c| c.instance()).transpose() {
        Ok(owner) => owner,
        Err(error) => {
            warn!(connection, ?error, "sequence configuration rejected");
            return;
        }
    };
    let mut listener = players::PlayerListener::new(persona)
        .with_connection(connection)
        .with_progression(progression)
        .with_prologue(prologue)
        .with_vehicles(vehicles)
        .with_sequences(sequences)
        .with_traffic(traffic)
        .with_rivals(rivals)
        // Marks the current value seen, so only later saves are reported.
        .with_ai_settings(ai.changed().unwrap_or_else(|| ai.current()))
        .with_arrivals(arrivals)
        .with_returns(returns)
        .with_crashes(crashes)
        .with_garage_logic(garage_logic)
        .with_scoring(scoring, unix_micros().max(0) as u64)
        .with_events(events);
    let mut items = items::Exchanges::new(inventory, connection);
    if let Some(content) = &content
        && let Err(error) = listener.initialize_world(content)
    {
        warn!(connection, ?error, "owned world initialization rejected");
        return;
    }
    let mut peer: Option<SocketAddr> = None;
    let mut buffer = vec![0; 2048];
    let mut proof_sent = false;
    let mut last_input = Instant::now();
    let mut tick = tokio::time::interval(WORLD_TICK);
    let mut delay_reported = None;
    #[cfg(feature = "network-test")]
    let mut test_loss = test_loss::Loss::new(recorder.loss_trigger(connection));
    info!(connection, "world host ready");
    loop {
        let advance_started = Instant::now();
        if let Err(error) = listener.advance_sequences(now_ms()) {
            warn!(connection, ?error, "sequence world clock rejected");
            return;
        }
        if let Err(error) = listener.advance_level_poll(now_ms()) {
            warn!(connection, ?error, "level poll rejected");
            return;
        }
        if let Err(error) = listener.advance_settled(now_ms()) {
            warn!(connection, ?error, "world car settlement rejected");
            return;
        }
        // Settings the player saved while this session runs.
        if let Some(settings) = ai.changed() {
            listener.set_ai_settings(settings);
            info!(
                connection,
                traffic_percent = settings.traffic_percent(),
                rival_limit = ?settings.rival_limit(),
                "traffic and rival settings applied to this session"
            );
        }
        listener.session_backlog = host.replication_progress().1;
        if let Err(error) = listener.advance_traffic(now_ms())
            && listener.disable_traffic()
        {
            warn!(
                connection,
                ?error,
                "owned traffic rejected; spawning disabled for this session"
            );
        }
        if let Err(error) = listener.advance_rivals(now_ms())
            && listener.disable_rivals()
        {
            warn!(
                connection,
                ?error,
                "owned rivals rejected; spawning disabled for this session"
            );
        }
        if let Err(error) = listener.advance_scoring(now_ms()) {
            // Scoring is best effort: a fault disables it for this session
            // and never closes the world host.
            warn!(
                connection,
                ?error,
                "owned scoring rejected; scoring disabled for this session"
            );
            listener.disable_scoring();
        }
        // Read-only: what the launcher's map shows, after this tick's AI.
        if let Some(feed) = &mut map_feed
            && feed.due(now_ms())
        {
            feed.publish(listener.map_snapshot(now_ms()));
        }
        listener.replication_origin_ms = host.application().and_then(|a| a.origin_ms());
        if let Err(error) = listener.advance_events(now_ms()) {
            // A first fault stages event teardown while retaining delegation.
            // If teardown itself fails, dropping this session owns all cleanup.
            if !listener.disable_events() {
                warn!(
                    connection,
                    ?error,
                    "owned event teardown rejected; closing world"
                );
                return;
            }
            warn!(
                connection,
                ?error,
                "owned event rejected; scheduling event teardown"
            );
        }
        if !listener.files_alive(now_ms()) {
            warn!(
                connection,
                "incoming File or MessageEX deadline; closing world"
            );
            return;
        }
        if let Err(error) = host.set_movement(listener.movement_objects()) {
            warn!(connection, ?error, "movement grants rejected");
            return;
        }
        let advance_ms = advance_started.elapsed().as_millis() as u64;
        let receive_started = Instant::now();
        let output = tokio::select! {
            success = items.completed(&mut listener), if items.working() => {
                if !success { return; }
                host.poll(now_ms())
            }
            received = socket.recv_from(&mut buffer) => {
                let (n, from) = match received {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                match peer {
                    None => {
                        peer = Some(from);
                        info!(connection, %from, "world client");
                    }
                    Some(p) if p != from => {
                        debug!(connection, %from, "datagram from another address ignored");
                        continue;
                    }
                    _ => {}
                }
                last_input = Instant::now();
                let raw = buffer[..n].to_vec();
                #[cfg(feature = "network-test")]
                let dropped = test_loss.drop_input(now_ms());
                #[cfg(not(feature = "network-test"))]
                let dropped = false;
                let note = dropped.then_some("diagnostic packet loss before transport");
                if recorder.chunk("world", connection, Direction::In, &raw, note).await.is_err() {
                    return;
                }
                if dropped { continue; }
                host.receive(&raw, now_ms(), &mut listener)
            }
            _ = tick.tick() => {
                #[cfg(feature = "network-test")]
                test_loss.poll(now_ms()).await;
                if last_input.elapsed() > WORLD_IDLE {
                    info!(connection, "world host idle; closing");
                    return;
                }
                host.poll(now_ms())
            }
        };
        let receive_or_poll_ms = receive_started.elapsed().as_millis() as u64;
        let queue_started = Instant::now();
        if !items.advance(&mut host, &output.events, now_ms()) {
            return;
        }
        items.start(
            &mut listener,
            now_ms(),
            nfs_storage::Timestamp(unix_micros().max(0) as u64 / 1_000),
        );
        while let Some(message) = listener.pending_content.front() {
            let Ok(bytes) = message.encode() else {
                warn!(connection, "owned content invalid");
                return;
            };
            match host.queue_content(message.target(), &bytes) {
                Ok(chunks) => {
                    info!(connection, chunks, "owned content queued");
                    listener.pending_content.pop_front();
                }
                Err(nfs_world::session::Error::Application(application::Error::Window)) => break,
                Err(error) => {
                    warn!(connection, ?error, "owned content rejected");
                    return;
                }
            }
        }
        while listener.pending_content.is_empty() {
            let Some(section) = listener.pending.front() else {
                break;
            };
            match host.queue_replication(section.clone()) {
                Ok(()) => {
                    info!(
                        connection,
                        records = section.records.len(),
                        "owned replication queued"
                    );
                    listener.pending.pop_front();
                }
                Err(nfs_world::session::Error::Application(application::Error::Window)) => break,
                Err(error) => {
                    warn!(connection, ?error, "owned replication rejected");
                    return;
                }
            }
        }
        while listener.pending_content.is_empty() && listener.pending.is_empty() {
            let Some(&reply) = listener.pending_rpcs.front() else {
                break;
            };
            match host.queue_rpc(reply) {
                Ok(()) => {
                    listener.pending_rpcs.pop_front();
                }
                Err(nfs_world::session::Error::Application(application::Error::Window)) => break,
                Err(error) => {
                    warn!(connection, ?error, "owned RPC notification rejected");
                    return;
                }
            }
        }
        while listener.pending_content.is_empty()
            && listener.pending.is_empty()
            && listener.pending_rpcs.is_empty()
        {
            let Some((section, before)) = listener.pending_entries.front() else {
                break;
            };
            match host.queue_replication_after(section.clone(), before.clone()) {
                Ok(()) => {
                    info!(
                        connection,
                        notifications = before.len(),
                        records = section.records.len(),
                        deleted = section.deleted.len(),
                        calls = ?before
                            .iter()
                            .map(players::describe_host_rpc)
                            .collect::<Vec<_>>(),
                        "owned entry notifications and replication queued together"
                    );
                    listener.pending_entries.pop_front();
                }
                Err(nfs_world::session::Error::Application(application::Error::Window)) => break,
                Err(error) => {
                    warn!(connection, ?error, "owned entry transition rejected");
                    return;
                }
            }
        }
        while let Some(command) = listener.due_teleport(now_ms()) {
            let bytes = match command.encode() {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(connection, ?error, "owned car teleport rejected");
                    return;
                }
            };
            match host.queue_content_after_replication(nfs_world::teleport::CONTENT_TARGET, &bytes)
            {
                Ok(chunks) => {
                    info!(
                        connection,
                        chunks,
                        participant = command.participant,
                        vehicle = command.vehicle,
                        "owned car teleport queued"
                    );
                    listener.teleport_sent();
                }
                Err(nfs_world::session::Error::Application(application::Error::Window)) => break,
                Err(error) => {
                    warn!(connection, ?error, "owned car teleport rejected");
                    return;
                }
            }
        }
        // A reliable boundary after result reporting prevents a later mode
        // deletion from overtaking its Speedwall handoff under queue pressure.
        while listener.pending_content.is_empty()
            && listener.pending.is_empty()
            && listener.pending_rpcs.is_empty()
            && listener.pending_entries.is_empty()
        {
            let Some(progress) = listener.pending_speedwalls.front() else {
                break;
            };
            let bytes = match progress.encode() {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn!(connection, ?error, "owned Speedwall progress rejected");
                    return;
                }
            };
            match host.queue_content_after_replication(nfs_world::speedwall::CONTENT_TARGET, &bytes)
            {
                Ok(chunks) => {
                    info!(
                        connection,
                        chunks,
                        event = progress.event_id,
                        "owned Speedwall progress queued"
                    );
                    listener.pending_speedwalls.pop_front();
                }
                Err(nfs_world::session::Error::Application(application::Error::Window)) => break,
                Err(error) => {
                    warn!(connection, ?error, "owned Speedwall handoff rejected");
                    return;
                }
            }
        }
        let queue_ms = queue_started.elapsed().as_millis() as u64;
        let send_started = Instant::now();
        for event in &output.events {
            match event {
                Event::Application(application::Event::UnknownControl(subtype)) => {
                    listener.unsupported_control(*subtype);
                }
                Event::Application(application::Event::Queued { .. })
                | Event::Application(application::Event::Duplicate) => {
                    debug!(connection, ?event, "world")
                }
                other => info!(connection, event = ?other, "world"),
            }
        }
        if let Some(p) = peer {
            for wire in &output.send {
                if socket.send_to(wire, p).await.is_err() {
                    warn!(connection, "world send failed");
                }
                if recorder
                    .chunk("world", connection, Direction::Out, wire, None)
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        let send_and_record_ms = send_started.elapsed().as_millis() as u64;
        if (advance_ms + queue_ms + send_and_record_ms >= 100 || receive_or_poll_ms >= 250)
            && delay_reported.is_none_or(|last| now_ms().saturating_sub(last) >= 1000)
        {
            delay_reported = Some(now_ms());
            warn!(connection, advance_ms, receive_or_poll_ms, queue_ms, send_and_record_ms,
                replication = ?host.replication_progress(), content = ?host.content_progress(),
                "world loop delay");
        }
        if !proof_sent && host.link().is_some_and(|l| l.running()) {
            proof_sent = true;
            let _ = proof.send(Some(readiness));
            info!(
                connection,
                readiness = readiness.is_some(),
                "world sync established"
            );
        }
        if host.stage() == StageName::Closed {
            let stats = host.application().map(|a| a.stats());
            info!(
                connection,
                ?stats,
                ?listener,
                content = ?host.content_progress(),
                registration = ?host.registration_progress(),
                replication = ?host.replication_progress(),
                "world host closed"
            );
            return;
        }
    }
}

fn into_blocking(stream: TcpStream) -> Option<std::net::TcpStream> {
    let stream = stream.into_std().ok()?;
    stream.set_nonblocking(false).ok()?;
    Some(stream)
}

async fn redirector(
    stream: TcpStream,
    peer: SocketAddr,
    n: u32,
    reply: &Arc<Vec<u8>>,
    recorder: &Recorder,
) {
    let Some(mut stream) = into_blocking(stream) else {
        return;
    };
    let reply = reply.clone();
    let result = tokio::task::spawn_blocking(move || {
        let capture =
            discovery::receive_socket(&mut stream, Instant::now() + Duration::from_secs(5));
        let accepted = discovery::eligible(&capture);
        let bytes: &[u8] = if accepted {
            &reply
        } else {
            discovery::ERROR_RESPONSE
        };
        let written = stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .and_then(|()| stream.write_all(bytes))
            .is_ok();
        (capture.wire, bytes.to_vec(), accepted, written)
    })
    .await;
    let Ok((request, response, accepted, written)) = result else {
        return;
    };
    let _ = recorder
        .chunk("redirector", n, Direction::In, &request, None)
        .await;
    let _ = recorder
        .chunk("redirector", n, Direction::Out, &response, None)
        .await;
    info!(connection = n, %peer, accepted, written, "redirector request");
}

#[cfg(test)]
mod listen_tests {
    use super::*;

    /// An IPv4 address of this computer other than loopback, if it has one:
    /// the source a UDP socket would use toward a documentation address
    /// (connecting a UDP socket sends nothing). None on Windows, where listening
    /// there asks the person at the computer about the firewall; Linux CI
    /// covers it.
    pub(crate) fn network_address() -> Option<Ipv4Addr> {
        if cfg!(windows) {
            return None;
        }
        let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
        match socket.local_addr().ok()?.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
            _ => None,
        }
    }

    #[test]
    fn startup_advertises_the_listen_address() {
        // Regression: on a network address the server stopped right after
        // its ready line, when this reply and identity required 127.0.0.1.
        for listen in [Ipv4Addr::LOCALHOST, Ipv4Addr::new(192, 168, 1, 32)] {
            let at = |port| SocketAddr::from((listen, port));
            let addresses = Addresses {
                redirector: at(42127),
                blaze: at(50001),
                qos: at(50002),
                qos_udp: at(50003),
            };
            let (_, reply) = advertise(&addresses).unwrap();
            let reply = String::from_utf8(reply).unwrap();
            let blaze = format!(
                "<hostname>{listen}</hostname><ip>{}</ip><port>50001</port>",
                u32::from(listen)
            );
            assert!(reply.contains(&blaze), "{reply}");
        }
        let at = |port| SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
        let unspecified = Addresses {
            redirector: at(42127),
            blaze: at(50001),
            qos: at(50002),
            qos_udp: at(50003),
        };
        assert!(advertise(&unspecified).is_err());
    }

    #[test]
    fn a_loopback_listener_serves_only_this_computer() {
        let me = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5));
        assert!(serves(me, me));
        assert!(!serves(me, lan));
        assert!(serves(lan, lan));
        assert!(serves(lan, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 9))));
        assert!(serves(lan, me));
    }

    #[tokio::test]
    async fn every_listener_binds_and_advertises_the_chosen_address() {
        let chosen: Vec<Ipv4Addr> = [Some(Ipv4Addr::LOCALHOST), network_address()]
            .into_iter()
            .flatten()
            .collect();
        for listen in chosen {
            let listeners = Listeners::bind(listen, 0).await.unwrap();
            let a = listeners.addresses().unwrap();
            for address in [a.redirector, a.blaze, a.qos, a.qos_udp] {
                assert_eq!(address.ip(), IpAddr::V4(listen), "{address}");
            }
            let lsx = crate::lsx::bind(listen, 0).await.unwrap();
            assert_eq!(lsx.local_addr().unwrap().ip(), IpAddr::V4(listen));
        }
    }

    #[tokio::test]
    async fn qos_serves_one_concrete_shared_address() {
        let (_stop, shutdown) = tokio::sync::watch::channel(false);
        let identity = Arc::new(nfs_server_support::qos_tls::Identity::new().unwrap());
        let start = |tcp: Ipv4Addr, udp: Ipv4Addr| {
            let tcp = std::net::TcpListener::bind((tcp, 0)).unwrap();
            let udp = std::net::UdpSocket::bind((udp, 0)).unwrap();
            qos::serve(
                tcp,
                udp,
                identity.clone(),
                Duration::from_secs(1),
                shutdown.clone(),
            )
        };
        // An unspecified address cannot be advertised.
        assert!(
            start(Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED)
                .await
                .is_err()
        );
        if let Some(lan) = network_address() {
            // Both on the chosen address: serves until shut down.
            let (stop, shutdown) = tokio::sync::watch::channel(false);
            let tcp = std::net::TcpListener::bind((lan, 0)).unwrap();
            let udp = std::net::UdpSocket::bind((lan, 0)).unwrap();
            let task = tokio::spawn(qos::serve(
                tcp,
                udp,
                identity.clone(),
                Duration::from_secs(1),
                shutdown,
            ));
            stop.send(true).unwrap();
            let served = tokio::time::timeout(Duration::from_secs(3), task).await;
            assert!(matches!(served, Ok(Ok(Ok(())))), "{served:?}");
            // Different addresses for TCP and UDP are refused.
            assert!(start(Ipv4Addr::LOCALHOST, lan).await.is_err());
        }
    }
}
