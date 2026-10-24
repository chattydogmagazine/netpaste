//! Dev helper: acts as another device on the network so you can see a running
//! app receive something. Usage: cargo run --example send_probe -- <room key> [text] [file]
use netpaste_app::{crypto, net::Net};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let key = args.first().cloned().unwrap_or_else(|| "demo".into());
    let text = args.get(1).cloned().unwrap_or_else(|| "hello from the probe".into());
    let file = args.get(2).map(PathBuf::from);

    let (net, rx) = Net::start(
        crypto::random_hex(8),
        "Studio PC".into(),
        &key,
        47300,
        45454,
        Box::new(|| {}),
    )
    .expect("probe should start");

    std::thread::sleep(std::time::Duration::from_secs(5)); // let discovery settle
    let files = file.map(|f| net.register_files(&[f])).unwrap_or_default();
    net.post(crypto::random_hex(8), text, files);
    // report how many devices accepted it
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if let Ok(netpaste_app::net::Event::Delivered { ok, failed, .. }) =
            rx.recv_timeout(std::time::Duration::from_millis(300))
        {
            println!("delivered to {} device(s), {} failed", ok, failed);
            break;
        }
    }
    std::thread::sleep(std::time::Duration::from_secs(20)); // stay up to serve the file
}
