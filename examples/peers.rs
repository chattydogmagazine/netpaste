//! Diagnostic: join the room and print the devices that answer.
//! Usage: cargo run --example peers -- <room key> [seconds]
use netpaste_app::{crypto, net::{Event, Net}};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key = args.first().cloned().unwrap_or_else(|| "demo".into());
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(12);

    let (_net, rx) = Net::start(
        crypto::random_hex(8),
        "netpaste-doctor".into(),
        &key,
        47400,
        45454,
        Box::new(|| {}),
    )
    .expect("could not open the network ports");

    println!("listening as room \"{}\" for {}s…\n", key, secs);
    let end = Instant::now() + Duration::from_secs(secs);
    let mut seen: Vec<String> = Vec::new();
    while Instant::now() < end {
        if let Ok(Event::Peers(list)) = rx.recv_timeout(Duration::from_millis(500)) {
            for p in list {
                let line = format!("{:<16} v{:<8} {:<30} {} [{:?}]", p.name, p.version, p.platform, p.addr, p.health);
                if !seen.contains(&line) {
                    println!("  found  {}", line);
                    seen.push(line);
                }
            }
        }
    }
    println!("\n{} device(s) visible with that room key.", seen.len());
}
