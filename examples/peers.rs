//! Diagnostic: join the room and print the devices that answer.
//!
//!   cargo run --example peers -- <room key> [seconds] [options]
//!
//!     --disc <port>     listen on a different discovery port, which is a
//!                       quick way to prove a device was NOT found by local
//!                       multicast or broadcast
//!     --peer <address>  name a device directly, the way the settings screen
//!                       does for machines on another network
use netpaste_app::{
    crypto,
    net::{Event, Net},
};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key = args.first().cloned().unwrap_or_else(|| "demo".into());
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);

    let mut disc_port = 45454;
    let mut remote = Vec::new();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--disc" => {
                disc_port = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(45454);
                i += 2;
            }
            "--peer" => {
                if let Some(p) = args.get(i + 1) {
                    remote.push(p.clone());
                }
                i += 2;
            }
            _ => i += 1,
        }
    }

    let (net, rx) = Net::start(
        crypto::random_hex(8),
        "netpaste-doctor".into(),
        &key,
        47400,
        disc_port,
        Box::new(|| {}),
    )
    .expect("could not open the network ports");

    if !remote.is_empty() {
        println!("naming directly: {}", remote.join(", "));
        net.set_remote_peers(remote);
    }
    println!("listening as room \"{}\" on discovery port {} for {}s…\n", key, disc_port, secs);

    let end = Instant::now() + Duration::from_secs(secs);
    let mut seen: Vec<String> = Vec::new();
    while Instant::now() < end {
        if let Ok(Event::Peers(list)) = rx.recv_timeout(Duration::from_millis(500)) {
            for p in list {
                // Keyed on the device, not the whole line - health changes as
                // announcements come and go, and reprinting on every change
                // reads as if the same machine were two devices.
                if seen.contains(&p.id) {
                    continue;
                }
                println!(
                    "  found  {:<16} v{:<8} {:<30} {}",
                    p.name, p.version, p.platform, p.addr
                );
                seen.push(p.id);
            }
        }
    }
    println!("\n{} device(s) visible with that room key.", seen.len());
}
