//! Discovery and transport. Nothing here knows about the UI: it emits events
//! and calls a wake closure so the app can repaint.

use crate::crypto::{derive_key, random_hex, SecretKey};
use crate::proto::*;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

pub const ANNOUNCE_EVERY: Duration = Duration::from_secs(2);
/// Local-network multicast group. Broadcast alone is unreliable: some
/// access points drop it, and on one machine it may reach only one app.
pub const MCAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
pub const PEER_TIMEOUT: u64 = 8;

/// How a device is getting on, for the dot beside its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Announcing normally.
    Online,
    /// Gone quiet - still listed, but about to drop off.
    Quiet,
    /// Announcing, but the last thing we sent it never arrived.
    Unreachable,
}

#[derive(Clone, Debug)]
pub struct PeerView {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub addr: SocketAddr,
    pub seen_ago: u64,
    pub version: String,
    pub health: Health,
}

#[derive(Clone, Debug)]
pub struct Incoming {
    pub msg_id: String,
    pub from_id: String,
    pub from_name: String,
    pub ts: u64,
    pub body: String,
    pub files: Vec<FileMeta>,
    pub origin: SocketAddr,
}

pub enum Event {
    Peers(Vec<PeerView>),
    Incoming(Incoming),
    Delivered { msg_id: String, ok: usize, failed: usize },
    Progress { file_id: String, done: u64, total: u64 },
    Saved { file_id: String, path: PathBuf },
    Failed { file_id: String, reason: String },
    /// A device in this room is running a newer build than we are.
    UpdateAvailable { from_name: String, version: String },
    UpdateProgress { done: u64, total: u64 },
    UpdateFailed(String),
    Note(String),
}

struct PeerRec {
    name: String,
    platform: String,
    addr: SocketAddr,
    last_seen: u64,
    version: String,
    exe_sha: String,
    platform_key: String,
    /// Set when a message to this device could not be delivered, cleared as
    /// soon as one gets through.
    unreachable: bool,
}

pub struct Net {
    pub device_id: String,
    name: RwLock<String>,
    key: RwLock<SecretKey>,
    peers: Mutex<HashMap<String, PeerRec>>,
    offered: Mutex<HashMap<String, PathBuf>>,
    tx: Sender<Event>,
    wake: Box<dyn Fn() + Send + Sync>,
    pub tcp_port: u16,
    pub disc_port: u16,
    udp: UdpSocket,
    /// Fingerprint of our own executable, advertised so peers can tell
    /// whether an update from us is the build they were promised.
    exe_sha: String,
    /// Set while an update is being pulled, so we only ever run one.
    updating: Mutex<bool>,
    /// Builds that were already tried and rejected. Without this, a bad
    /// build would be downloaded again every time we look.
    rejected: Mutex<std::collections::HashSet<String>>,
}

pub fn platform_label() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "Apple silicon / ARM",
        "x86_64" => "Intel / x86-64",
        other => other,
    };
    format!("{} · {}", os, arch)
}

fn bind_udp(port: u16) -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(unix)]
    s.set_reuse_port(true)?;
    s.bind(&SockAddr::from(SocketAddr::from(([0, 0, 0, 0], port))))?;
    s.set_broadcast(true)?;
    s.set_multicast_loop_v4(true)?;
    s.set_multicast_ttl_v4(1)?;
    // Not fatal: a machine with no multicast-capable interface still has
    // broadcast and direct peers.
    let _ = s.join_multicast_v4(&MCAST_GROUP, &Ipv4Addr::UNSPECIFIED);
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    Ok(s.into())
}

/// Take the first free port in a small range, so a second copy of the app on
/// the same machine still starts.
fn bind_tcp(first: u16) -> std::io::Result<TcpListener> {
    let mut last_err = None;
    for port in first..first + 16 {
        match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => return Ok(l),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap())
}

impl Net {
    pub fn start(
        device_id: String,
        device_name: String,
        room_key: &str,
        tcp_port: u16,
        disc_port: u16,
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<(Arc<Net>, Receiver<Event>)> {
        let listener = bind_tcp(tcp_port)?;
        let bound = listener.local_addr()?.port();
        let udp = bind_udp(disc_port)?;
        let (tx, rx) = channel();

        let net = Arc::new(Net {
            device_id,
            name: RwLock::new(device_name),
            key: RwLock::new(derive_key(room_key)),
            peers: Mutex::new(HashMap::new()),
            offered: Mutex::new(HashMap::new()),
            tx,
            wake,
            tcp_port: bound,
            disc_port,
            udp,
            exe_sha: crate::update::self_exe_sha(),
            updating: Mutex::new(false),
            rejected: Mutex::new(std::collections::HashSet::new()),
        });

        {
            let n = Arc::clone(&net);
            thread::spawn(move || n.accept_loop(listener));
        }
        {
            let n = Arc::clone(&net);
            thread::spawn(move || n.discovery_loop());
        }
        {
            let n = Arc::clone(&net);
            thread::spawn(move || n.announce_loop());
        }
        Ok((net, rx))
    }

    fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
        (self.wake)();
    }

    fn key(&self) -> SecretKey {
        *self.key.read().unwrap()
    }

    pub fn set_room_key(&self, room_key: &str) {
        *self.key.write().unwrap() = derive_key(room_key);
        self.peers.lock().unwrap().clear();
        self.publish_peers();
    }

    pub fn set_name(&self, name: &str) {
        *self.name.write().unwrap() = name.to_string();
    }

    pub fn name(&self) -> String {
        self.name.read().unwrap().clone()
    }

    // ---------------------------------------------------------- discovery

    fn announce_loop(self: Arc<Self>) {
        loop {
            let ann = Announce {
                id: self.device_id.clone(),
                name: self.name(),
                platform: platform_label(),
                port: self.tcp_port,
                ts: now_secs(),
                version: crate::update::VERSION.to_string(),
                exe_sha: self.exe_sha.clone(),
                platform_key: crate::update::platform_key(),
            };
            if let Ok(json) = serde_json::to_vec(&ann) {
                let sealed = crate::crypto::seal(&self.key(), &json);
                for ip in [MCAST_GROUP, Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST] {
                    let _ = self.udp.send_to(&sealed, SocketAddr::from((ip, self.disc_port)));
                }
            }
            self.prune_peers();
            thread::sleep(ANNOUNCE_EVERY);
        }
    }

    fn discovery_loop(self: Arc<Self>) {
        let mut buf = [0u8; 4096];
        loop {
            let (n, from) = match self.udp.recv_from(&mut buf) {
                Ok(v) => v,
                Err(_) => continue, // read timeout, keep going
            };
            let Some(plain) = crate::crypto::unseal(&self.key(), &buf[..n]) else {
                continue; // different room key, or noise
            };
            let Ok(ann) = serde_json::from_slice::<Announce>(&plain) else { continue };
            if ann.id == self.device_id {
                continue;
            }
            let addr = SocketAddr::new(from.ip(), ann.port);
            let mut peers = self.peers.lock().unwrap();
            let (changed, unreachable) = match peers.get(&ann.id) {
                Some(p) => (
                    p.name != ann.name || p.addr != addr || p.version != ann.version,
                    p.unreachable,
                ),
                None => (true, false),
            };
            let newer_build = crate::update::is_newer(&ann.version, crate::update::VERSION)
                && ann.platform_key == crate::update::platform_key()
                && !ann.exe_sha.is_empty();
            let offer = newer_build.then(|| (ann.name.clone(), ann.version.clone()));
            peers.insert(
                ann.id.clone(),
                PeerRec {
                    name: ann.name,
                    platform: ann.platform,
                    addr,
                    last_seen: now_secs(),
                    version: ann.version,
                    exe_sha: ann.exe_sha,
                    platform_key: ann.platform_key,
                    unreachable,
                },
            );
            drop(peers);
            if changed {
                self.publish_peers();
                if let Some((from_name, version)) = offer {
                    self.emit(Event::UpdateAvailable { from_name, version });
                }
            }
        }
    }

    fn prune_peers(&self) {
        let now = now_secs();
        let mut peers = self.peers.lock().unwrap();
        let before = peers.len();
        peers.retain(|_, p| now.saturating_sub(p.last_seen) <= PEER_TIMEOUT);
        let changed = peers.len() != before;
        drop(peers);
        // Always republish: "seen N seconds ago" needs to keep ticking.
        let _ = changed;
        self.publish_peers();
    }

    fn publish_peers(&self) {
        let now = now_secs();
        let mut list: Vec<PeerView> = self
            .peers
            .lock()
            .unwrap()
            .iter()
            .map(|(id, p)| {
                let seen_ago = now.saturating_sub(p.last_seen);
                PeerView {
                    id: id.clone(),
                    name: p.name.clone(),
                    platform: p.platform.clone(),
                    addr: p.addr,
                    seen_ago,
                    version: p.version.clone(),
                    health: if p.unreachable {
                        Health::Unreachable
                    } else if seen_ago >= ANNOUNCE_EVERY.as_secs() * 2 {
                        Health::Quiet
                    } else {
                        Health::Online
                    },
                }
            })
            .collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.emit(Event::Peers(list));
    }

    fn peer_targets(&self) -> Vec<(String, SocketAddr)> {
        self.peers.lock().unwrap().iter().map(|(id, p)| (id.clone(), p.addr)).collect()
    }

    pub fn peer_addrs(&self) -> Vec<SocketAddr> {
        self.peers.lock().unwrap().values().map(|p| p.addr).collect()
    }

    /// Note whether a device is actually taking what we send it.
    fn mark_reachable(&self, id: &str, reachable: bool) -> bool {
        let mut peers = self.peers.lock().unwrap();
        match peers.get_mut(id) {
            Some(p) if p.unreachable == reachable => {
                p.unreachable = !reachable;
                true
            }
            _ => false,
        }
    }

    /// The device offering a build newer than ours, if any.
    pub fn update_source(&self) -> Option<(String, String, String, SocketAddr)> {
        let mine = crate::update::VERSION;
        let me = crate::update::platform_key();
        let rejected = self.rejected.lock().unwrap().clone();
        self.peers
            .lock()
            .unwrap()
            .values()
            .filter(|p| {
                p.platform_key == me
                    && !p.exe_sha.is_empty()
                    && !rejected.contains(&p.exe_sha)
                    && crate::update::is_newer(&p.version, mine)
            })
            .max_by_key(|p| crate::update::parse_version(&p.version))
            .map(|p| (p.name.clone(), p.version.clone(), p.exe_sha.clone(), p.addr))
    }

    // ----------------------------------------------------------- incoming

    fn accept_loop(self: Arc<Self>, listener: TcpListener) {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let n = Arc::clone(&self);
            thread::spawn(move || {
                let _ = n.handle(stream);
            });
        }
    }

    fn handle(&self, mut stream: TcpStream) -> std::io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(60)))?;
        let key = self.key();
        let peer_ip = stream.peer_addr()?.ip();

        let Some(frame) = read_frame(&mut stream, &key)? else { return Ok(()) };
        match frame {
            Frame::Post { msg_id, from_id, from_name, from_port, ts, body, files } => {
                if from_id == self.device_id {
                    return Ok(());
                }
                self.emit(Event::Incoming(Incoming {
                    msg_id,
                    from_id,
                    from_name,
                    ts,
                    body,
                    files,
                    origin: SocketAddr::new(peer_ip, from_port),
                }));
            }
            Frame::Fetch { file_id } => {
                let path = self.offered.lock().unwrap().get(&file_id).cloned();
                match path {
                    None => write_frame(
                        &mut stream,
                        &key,
                        &Frame::FetchErr { reason: "no longer shared".into() },
                    )?,
                    Some(path) => self.serve_file(&mut stream, &key, &path)?,
                }
            }
            Frame::FetchUpdate => match crate::update::package_self() {
                Ok((payload, _)) => self.serve_file(&mut stream, &key, &payload)?,
                Err(e) => {
                    write_frame(&mut stream, &key, &Frame::FetchErr { reason: e.to_string() })?
                }
            },
            _ => {}
        }
        Ok(())
    }

    fn serve_file(&self, stream: &mut TcpStream, key: &SecretKey, path: &PathBuf) -> std::io::Result<()> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                return write_frame(stream, key, &Frame::FetchErr { reason: e.to_string() });
            }
        };
        let size = file.metadata()?.len();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        write_frame(stream, key, &Frame::FetchOk { name, size })?;

        let mut reader = BufReader::with_capacity(CHUNK, file);
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            write_blob(stream, key, &buf[..n])?;
        }
        stream.flush()
    }

    // ----------------------------------------------------------- outgoing

    /// Offer files to the network and return the metadata to show locally.
    pub fn register_files(&self, paths: &[PathBuf]) -> Vec<FileMeta> {
        let mut offered = self.offered.lock().unwrap();
        paths
            .iter()
            .filter_map(|p| {
                let size = std::fs::metadata(p).ok()?.len();
                let id = random_hex(8);
                let name = p.file_name()?.to_string_lossy().to_string();
                offered.insert(id.clone(), p.clone());
                Some(FileMeta { id, name, size })
            })
            .collect()
    }

    /// Stop offering files whose message has aged out of the history.
    pub fn unoffer(&self, ids: &[String]) {
        let mut offered = self.offered.lock().unwrap();
        for id in ids {
            offered.remove(id);
        }
    }

    pub fn post(self: &Arc<Self>, msg_id: String, body: String, files: Vec<FileMeta>) {
        let targets = self.peer_targets();
        let frame = Frame::Post {
            msg_id: msg_id.clone(),
            from_id: self.device_id.clone(),
            from_name: self.name(),
            from_port: self.tcp_port,
            ts: now_secs(),
            body,
            files,
        };
        let net = Arc::clone(self);
        thread::spawn(move || {
            let key = net.key();
            let (mut ok, mut failed) = (0, 0);
            let mut health_changed = false;
            for (id, addr) in targets {
                let delivered = send_frame_to(addr, &key, &frame).is_ok();
                if delivered {
                    ok += 1;
                } else {
                    failed += 1;
                }
                health_changed |= net.mark_reachable(&id, delivered);
            }
            net.emit(Event::Delivered { msg_id, ok, failed });
            if health_changed {
                net.publish_peers();
            }
        });
    }

    /// Pull a file a peer offered, straight to the path the user picked.
    pub fn fetch(self: &Arc<Self>, origin: SocketAddr, meta: FileMeta, dest: PathBuf) {
        let net = Arc::clone(self);
        thread::spawn(move || {
            let key = net.key();
            match pull(&net, origin, &key, &meta, &dest) {
                Ok(()) => net.emit(Event::Saved { file_id: meta.id, path: dest }),
                Err(e) => {
                    let _ = std::fs::remove_file(dest.with_extension("netpaste-part"));
                    net.emit(Event::Failed { file_id: meta.id, reason: e.to_string() })
                }
            }
        });
    }

    /// Pull a newer build from a device in this room and swap it in. The
    /// download is checked against the fingerprint that device announced
    /// before anything is replaced; if it doesn't match, it is discarded.
    pub fn pull_update(self: &Arc<Self>, origin: SocketAddr, expected_sha: String) {
        {
            let mut busy = self.updating.lock().unwrap();
            if *busy {
                return;
            }
            *busy = true;
        }
        let net = Arc::clone(self);
        thread::spawn(move || {
            crate::update::log(format!("pulling an update from {}", origin));
            let key = net.key();
            let dir = crate::update::staging_dir();
            let _ = std::fs::create_dir_all(&dir);
            let dest = dir.join("incoming");
            let meta = FileMeta { id: "update".into(), name: "NetPaste".into(), size: 0 };

            let result = pull_with(&net, origin, &key, Frame::FetchUpdate, &meta, &dest, true)
                .and_then(|()| crate::update::apply(&dest, &expected_sha));

            // apply() does not return on success - it restarts the app.
            let _ = std::fs::remove_file(&dest);
            *net.updating.lock().unwrap() = false;
            if let Err(e) = result {
                crate::update::log(format!("update failed: {}", e));
                // Don't go round again on a build that has already been
                // rejected; wait for a different one.
                net.rejected.lock().unwrap().insert(expected_sha);
                net.emit(Event::UpdateFailed(e.to_string()));
            }
        });
    }
}

fn connect(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let s = TcpStream::connect_timeout(&addr, Duration::from_secs(4))?;
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    s.set_write_timeout(Some(Duration::from_secs(60)))?;
    s.set_nodelay(true).ok();
    Ok(s)
}

fn send_frame_to(addr: SocketAddr, key: &SecretKey, frame: &Frame) -> std::io::Result<()> {
    let mut s = connect(addr)?;
    write_frame(&mut s, key, frame)?;
    s.flush()
}

fn pull(
    net: &Arc<Net>,
    origin: SocketAddr,
    key: &SecretKey,
    meta: &FileMeta,
    dest: &PathBuf,
) -> std::io::Result<()> {
    let request = Frame::Fetch { file_id: meta.id.clone() };
    pull_with(net, origin, key, request, meta, dest, false)
}

/// Stream one thing a peer is offering into `dest`.
fn pull_with(
    net: &Arc<Net>,
    origin: SocketAddr,
    key: &SecretKey,
    request: Frame,
    meta: &FileMeta,
    dest: &PathBuf,
    is_update: bool,
) -> std::io::Result<()> {
    let mut s = connect(origin)?;
    write_frame(&mut s, key, &request)?;

    let total = match read_frame(&mut s, key)? {
        Some(Frame::FetchOk { size, .. }) => size,
        Some(Frame::FetchErr { reason }) => {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, reason))
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected reply",
            ))
        }
    };

    let part = dest.with_extension("netpaste-part");
    let mut out = BufWriter::new(File::create(&part)?);
    let mut done: u64 = 0;
    let mut last_report = 0u64;
    while let Some(chunk) = read_blob(&mut s, key)? {
        out.write_all(&chunk)?;
        done += chunk.len() as u64;
        if done - last_report > CHUNK as u64 || done == total {
            last_report = done;
            if is_update {
                net.emit(Event::UpdateProgress { done, total });
            } else {
                net.emit(Event::Progress { file_id: meta.id.clone(), done, total });
            }
        }
    }
    out.flush()?;
    drop(out);

    if done != total {
        let _ = std::fs::remove_file(&part);
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("transfer cut short at {} of {} bytes", done, total),
        ));
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

/// Best guess at this machine's LAN address, for the status line.
pub fn local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    s.connect(("8.8.8.8", 53)).ok()?;
    s.local_addr().ok().map(|a| a.ip())
}
