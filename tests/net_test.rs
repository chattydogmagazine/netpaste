//! End-to-end checks of the transport, with no window involved: two nodes in
//! one process, talking over the real network stack.

use netpaste_app::crypto;
use netpaste_app::net::{Event, Net};
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn start(name: &str, key: &str, tcp: u16, disc: u16) -> (Arc<Net>, Receiver<Event>) {
    Net::start(
        crypto::random_hex(8),
        name.to_string(),
        key,
        tcp,
        disc,
        Box::new(|| {}),
    )
    .expect("node should start")
}

/// Wait up to `secs` for an event matching `f`, keeping the rest.
fn wait_for<T>(rx: &Receiver<Event>, secs: u64, mut f: impl FnMut(&Event) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => {
                if let Some(v) = f(&ev) {
                    return Some(v);
                }
            }
            Err(_) => continue,
        }
    }
    None
}

#[test]
fn peers_find_each_other_and_exchange_text_and_files() {
    let key = "test-room-alpha";
    let (a, rx_a) = start("Alpha", key, 47101, 47100);
    let (b, rx_b) = start("Bravo", key, 47111, 47100);

    // 1. discovery
    let found = wait_for(&rx_b, 12, |ev| match ev {
        Event::Peers(list) => list.iter().find(|p| p.name == "Alpha").map(|p| p.addr),
        _ => None,
    });
    assert!(found.is_some(), "Bravo never saw Alpha on the network");
    assert!(
        wait_for(&rx_a, 12, |ev| match ev {
            Event::Peers(list) => list.iter().find(|p| p.name == "Bravo").map(|p| p.addr),
            _ => None,
        })
        .is_some(),
        "Alpha never saw Bravo"
    );

    // 2. text, including the long-form and unicode cases
    let body = format!("line one\nline two — naïve 日本語 🚀\n{}", "x".repeat(50_000));
    a.post(crypto::random_hex(8), body.clone(), vec![]);
    let got = wait_for(&rx_b, 10, |ev| match ev {
        Event::Incoming(i) => Some((i.from_name.clone(), i.body.clone(), i.origin)),
        _ => None,
    })
    .expect("Bravo never received the message");
    assert_eq!(got.0, "Alpha");
    assert_eq!(got.1, body, "text arrived altered");

    // 3. a file, pulled by the receiver
    let dir = std::env::temp_dir().join(format!("netpaste-test-{}", crypto::random_hex(4)));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("report.bin");
    let payload: Vec<u8> = (0..900_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::File::create(&src).unwrap().write_all(&payload).unwrap();

    let metas = a.register_files(&[src.clone()]);
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].size, payload.len() as u64);
    a.post(crypto::random_hex(8), String::new(), metas.clone());

    let offer = wait_for(&rx_b, 10, |ev| match ev {
        Event::Incoming(i) if !i.files.is_empty() => Some((i.files[0].clone(), i.origin)),
        _ => None,
    })
    .expect("Bravo never received the file offer");
    assert_eq!(offer.0.name, "report.bin");

    let dest = dir.join("saved.bin");
    b.fetch(offer.1, offer.0.clone(), dest.clone());

    let saved = wait_for(&rx_b, 30, |ev| match ev {
        Event::Saved { path, .. } => Some(path.clone()),
        Event::Failed { reason, .. } => panic!("transfer failed: {}", reason),
        _ => None,
    })
    .expect("file never arrived");
    assert_eq!(saved, dest);
    assert_eq!(std::fs::read(&dest).unwrap(), payload, "file contents differ");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_different_room_key_is_invisible_and_unreadable() {
    let (a, _rx_a) = start("Secret", "room-one", 47201, 47200);
    let (_b, rx_b) = start("Stranger", "room-two", 47211, 47200);

    // Announcements are sealed, so the stranger cannot even list the device.
    let seen = wait_for(&rx_b, 6, |ev| match ev {
        Event::Peers(list) if !list.is_empty() => Some(list.clone()),
        _ => None,
    });
    assert!(seen.is_none(), "a device with the wrong key showed up: {:?}", seen);

    // And a message sent into room-one is never delivered into room-two.
    a.post(crypto::random_hex(8), "classified".into(), vec![]);
    let leaked = wait_for(&rx_b, 4, |ev| match ev {
        Event::Incoming(i) => Some(i.body.clone()),
        _ => None,
    });
    assert!(leaked.is_none(), "message leaked across room keys");
}

#[test]
fn sealed_payloads_do_not_open_with_the_wrong_key() {
    let k1 = crypto::derive_key("alpha");
    let k2 = crypto::derive_key("alpha ");  // trimmed: same room
    let k3 = crypto::derive_key("beta");
    assert_eq!(k1, k2);
    let sealed = crypto::seal(&k1, b"hello");
    assert_eq!(crypto::unseal(&k1, &sealed).as_deref(), Some(&b"hello"[..]));
    assert!(crypto::unseal(&k3, &sealed).is_none());
    // ciphertext is not the plaintext
    assert!(!sealed.windows(5).any(|w| w == b"hello"));
}

#[test]
fn a_device_on_another_network_is_reached_by_address() {
    // Different discovery ports stand in for "no shared broadcast domain":
    // neither node can possibly hear the other's multicast or broadcast, the
    // way two machines on opposite ends of a VPN cannot.
    let key = "test-room-remote";
    let (a, rx_a) = start("Here", key, 47501, 47500);
    let (b, rx_b) = start("Far away", key, 47511, 47510);

    // Nothing should be found on its own.
    assert!(
        wait_for(&rx_a, 4, |ev| match ev {
            Event::Peers(list) if !list.is_empty() => Some(list.clone()),
            _ => None,
        })
        .is_none(),
        "the two nodes should not find each other without help"
    );

    // Only ONE side is told about the other, as in the settings screen.
    a.set_remote_peers(vec!["127.0.0.1:47510".to_string()]);

    let a_sees_b = wait_for(&rx_a, 12, |ev| match ev {
        Event::Peers(list) => list.iter().find(|p| p.name == "Far away").map(|p| p.addr),
        _ => None,
    });
    assert!(a_sees_b.is_some(), "the configured side never saw the other");

    // ...and the other side learns about us from the introduction, despite
    // having been given no address at all.
    let b_sees_a = wait_for(&rx_b, 12, |ev| match ev {
        Event::Peers(list) => list.iter().find(|p| p.name == "Here").map(|p| p.addr),
        _ => None,
    });
    assert!(b_sees_a.is_some(), "the unconfigured side was never introduced");

    // And the link actually carries a message.
    b.post(crypto::random_hex(8), "hello across the tunnel".into(), vec![]);
    let got = wait_for(&rx_a, 10, |ev| match ev {
        Event::Incoming(i) => Some(i.body.clone()),
        _ => None,
    });
    assert_eq!(got.as_deref(), Some("hello across the tunnel"));
}

#[test]
fn turning_the_feature_off_stops_the_traffic() {
    let key = "test-room-remote-off";
    let (a, _rx_a) = start("Here", key, 47521, 47520);
    let (_b, rx_b) = start("Far away", key, 47531, 47530);

    a.set_remote_peers(vec!["127.0.0.1:47530".to_string()]);
    assert!(
        wait_for(&rx_b, 12, |ev| match ev {
            Event::Peers(list) if !list.is_empty() => Some(()),
            _ => None,
        })
        .is_some(),
        "should be reachable while switched on"
    );

    // An empty list is what the app passes when the toggle goes off.
    a.set_remote_peers(vec![]);
    // Wait out the peer timeout, then confirm it stays gone.
    std::thread::sleep(Duration::from_secs(netpaste_app::net::PEER_TIMEOUT + 3));
    let _ = rx_b.try_iter().count();
    let still_there = wait_for(&rx_b, 6, |ev| match ev {
        Event::Peers(list) if !list.is_empty() => Some(()),
        _ => None,
    });
    assert!(still_there.is_none(), "nothing should be sent once the feature is off");
}
