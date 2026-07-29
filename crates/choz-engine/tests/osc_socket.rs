//! End-to-end check of the OSC listener: a real UDP packet must come out of the
//! channel as the matching event.

use choz_engine::input::{ControlMsg, InputEvent, InputSource};

#[test]
fn udp_packets_become_input_events() {
    let (tx, rx) = flume::unbounded();
    // Port 0 would be ideal but `listen` binds a fixed port; pick a high one.
    let port = 47_311;
    // The handle has to stay alive: dropping it stops the listener (that is how
    // the settings modal moves OSC to another port without restarting choz).
    let _osc = choz_engine::osc::listen(port, tx).expect("bind");

    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let note = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage {
        addr: "/note".into(),
        args: vec![rosc::OscType::Int(60), rosc::OscType::Int(90)],
    }))
    .unwrap();
    let gain = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage {
        addr: "/mix/1/gain".into(),
        args: vec![rosc::OscType::Float(0.25)],
    }))
    .unwrap();
    sock.send_to(&note, ("127.0.0.1", port)).unwrap();
    sock.send_to(&gain, ("127.0.0.1", port)).unwrap();

    let mut got = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while got.len() < 2 && std::time::Instant::now() < deadline {
        if let Ok(ev) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
            got.push(ev);
        }
    }
    assert_eq!(got.len(), 2, "both packets arrive, got {got:?}");
    assert!(matches!(got[0], InputEvent::Note(n) if n.note == 60 && n.source == InputSource::Osc));
    assert!(matches!(got[1], InputEvent::Control(ControlMsg::Gain { tab: 1, value }) if value == 0.25));

    // Stopping frees the port, so the same one can be bound again right away.
    _osc.stop();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let rebound = loop {
        match std::net::UdpSocket::bind(("0.0.0.0", port)) {
            Ok(_) => break true,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Err(_) => break false,
        }
    };
    assert!(rebound, "the listener released the port");
}
