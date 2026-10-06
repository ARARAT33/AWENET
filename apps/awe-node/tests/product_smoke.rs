use std::{
    env, fs,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

fn request(addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    stream.write_all(request.as_bytes()).expect("write");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut out = Vec::with_capacity(65536);
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.len() >= 65536 {
                    break;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                if Instant::now() >= deadline {
                    panic!(
                        "read timed out for {}: {err}",
                        request.lines().next().unwrap_or("<request>")
                    );
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("read: {err}"),
        }
    }
    String::from_utf8_lossy(&out)
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .to_string()
}

fn get(addr: &str, path: &str) -> String {
    request(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    )
}

fn post(addr: &str, path: &str, body: &str) -> String {
    request(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn wait_for_health(addr: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if Instant::now() >= deadline {
            panic!("node at {addr} did not become healthy");
        }
        if let Ok(mut stream) = TcpStream::connect(addr) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.write_all(
                format!("GET /api/health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            );
            let mut body = String::new();
            if stream.read_to_string(&mut body).is_ok() && body.contains(r#""status":"healthy""#) {
                return;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_node(bin: &PathBuf, data: &PathBuf, listen: u16, ui: u16) -> Child {
    Command::new(bin)
        .env("AWE_DATA_DIR", data)
        .env("AWE_LISTEN_ADDR", format!("127.0.0.1:{listen}"))
        .env("AWE_UI_ADDR", format!("127.0.0.1:{ui}"))
        .env("AWE_NO_BROWSER", "1")
        .spawn()
        .expect("spawn node")
}

#[test]
fn three_node_product_smoke() {
    let bin = PathBuf::from(env::var_os("CARGO_BIN_EXE_awe-node").expect("binary path"));
    let root = env::temp_dir().join(format!("awep2p-smoke-{}", std::process::id()));
    let dirs = [root.join("n1"), root.join("n2"), root.join("n3")];
    for dir in &dirs {
        fs::create_dir_all(dir).expect("create data dir");
    }

    let mut children = vec![
        spawn_node(&bin, &dirs[0], 46101, 46201),
        spawn_node(&bin, &dirs[1], 46102, 46202),
        spawn_node(&bin, &dirs[2], 46103, 46203),
    ];

    let result = std::panic::catch_unwind(|| {
        wait_for_health("127.0.0.1:46201");
        wait_for_health("127.0.0.1:46202");
        wait_for_health("127.0.0.1:46203");

        for (port, ui) in [
            (46101u16, 46201u16),
            (46102u16, 46202u16),
            (46103u16, 46203u16),
        ] {
            let status = get(&format!("127.0.0.1:{ui}"), "/api/status");
            assert!(
                status.contains(r#""status":"online""#),
                "status {port}: {status}"
            );
            assert!(
                status.contains(&format!("127.0.0.1:{port}")),
                "address {port}: {status}"
            );
        }

        let storage = get("127.0.0.1:46201", "/api/storage");
        assert!(
            storage.contains(r#""capacity_bytes""#),
            "storage API: {storage}"
        );
        assert!(
            storage.contains(r#""replication_policy""#),
            "replication policy: {storage}"
        );

        let connect2 = post(
            "127.0.0.1:46201",
            "/api/connect?address=127.0.0.1%3A46102",
            "{}",
        );
        assert!(
            connect2.contains(r#""status":"connecting""#),
            "node2: {connect2}"
        );

        let connect3 = post(
            "127.0.0.1:46201",
            "/api/connect?address=127.0.0.1%3A46103",
            "{}",
        );
        assert!(
            connect3.contains(r#""status":"connecting""#),
            "node3: {connect3}"
        );

        thread::sleep(Duration::from_secs(2));

        let status = get("127.0.0.1:46201", "/api/status");
        assert!(
            status.contains(r#""active_connections":2"#),
            "connections: {status}"
        );

        let node2_status: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46202", "/api/status"))
                .expect("node2 status json");
        let node2_id = node2_status
            .get("node_id")
            .and_then(|v| v.as_str())
            .expect("node2 id");
        let message = post(
            "127.0.0.1:46201",
            "/api/messenger/send",
            &format!(r#"{{"recipient":"{node2_id}","text":"AWEP2P-E2E-MESSENGER"}}"#),
        );
        assert!(
            message.contains(r#""status":"sent""#),
            "messenger send: {message}"
        );
        let message_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let received = get("127.0.0.1:46202", "/api/messenger");
            if received.contains("AWEP2P-E2E-MESSENGER")
                && received.contains(r#""state":"delivered""#)
            {
                break;
            }
            if Instant::now() >= message_deadline {
                panic!("messenger delivery not observed: {received}");
            }
            thread::sleep(Duration::from_millis(100));
        }

        let group_create = post(
            "127.0.0.1:46201",
            "/api/groups/create",
            &format!(r#"{{"title":"E2E Group","members":["{node2_id}"]}}"#),
        );
        assert!(
            group_create.contains(r#""status":"created""#),
            "group create: {group_create}"
        );
        let group: serde_json::Value =
            serde_json::from_str(&group_create).expect("group create json");
        let group_id = group
            .get("group")
            .and_then(|g| g.get("id"))
            .and_then(|v| v.as_str())
            .expect("group id");
        let group_sync_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let groups = get("127.0.0.1:46202", "/api/groups");
            if groups.contains(group_id) && groups.contains("E2E Group") {
                break;
            }
            if Instant::now() >= group_sync_deadline {
                panic!("group sync not observed: {groups}");
            }
            thread::sleep(Duration::from_millis(100));
        }
        let group_send = post(
            "127.0.0.1:46201",
            "/api/groups/send",
            &format!(r#"{{"group_id":"{group_id}","text":"AWEP2P-E2E-GROUP"}}"#),
        );
        assert!(
            group_send.contains(r#""status":"sent""#),
            "group send: {group_send}"
        );
        let group_message_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let groups = get("127.0.0.1:46202", "/api/groups");
            if groups.contains("AWEP2P-E2E-GROUP") {
                break;
            }
            if Instant::now() >= group_message_deadline {
                panic!("group message not observed: {groups}");
            }
            thread::sleep(Duration::from_millis(100));
        }

        let channel_create = post(
            "127.0.0.1:46201",
            "/api/channels/create",
            r#"{"title":"E2E Channel"}"#,
        );
        assert!(
            channel_create.contains(r#""status":"created""#),
            "channel create: {channel_create}"
        );
        let channel: serde_json::Value =
            serde_json::from_str(&channel_create).expect("channel create json");
        let channel_id = channel
            .get("channel")
            .and_then(|g| g.get("id"))
            .and_then(|v| v.as_str())
            .expect("channel id");
        let channel_sync_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let channels = get("127.0.0.1:46202", "/api/channels");
            if channels.contains(channel_id) && channels.contains("E2E Channel") {
                break;
            }
            if Instant::now() >= channel_sync_deadline {
                panic!("channel sync not observed: {channels}");
            }
            thread::sleep(Duration::from_millis(100));
        }
        let subscribe = post(
            "127.0.0.1:46202",
            "/api/channels/subscribe",
            &format!(r#"{{"channel_id":"{channel_id}"}}"#),
        );
        assert!(
            subscribe.contains(r#""status":"subscribed""#),
            "subscribe: {subscribe}"
        );
        let subscriber_channels = get("127.0.0.1:46202", "/api/channels");
        assert!(
            subscriber_channels.contains(&format!(r#""{node2_id}""#)),
            "subscriber state: {subscriber_channels}"
        );
        let publish = post(
            "127.0.0.1:46201",
            "/api/channels/publish",
            &format!(r#"{{"channel_id":"{channel_id}","text":"AWEP2P-E2E-CHANNEL"}}"#),
        );
        assert!(
            publish.contains(r#""status":"published""#),
            "publish: {publish}"
        );
        let channel_message_deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let channels = get("127.0.0.1:46202", "/api/channels");
            if channels.contains("AWEP2P-E2E-CHANNEL") {
                break;
            }
            if Instant::now() >= channel_message_deadline {
                panic!("channel message not observed: {channels}");
            }
            thread::sleep(Duration::from_millis(100));
        }

        let ui = get("127.0.0.1:46201", "/");
        assert!(ui.contains("<title>AWENET</title>"), "desktop UI: {ui}");
        let store = get("127.0.0.1:46201", "/api/store/catalog");
        assert!(store.contains(r#""status":"ok""#), "store API: {store}");

        let payload = "AWEP2P-REAL-PRODUCT-SMOKE";
        let hex = payload
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let stored = post(
            "127.0.0.1:46201",
            "/api/storage/put",
            &format!(r#"{{"filename":"smoke.txt","data_hex":"{hex}"}}"#),
        );
        assert!(
            stored.contains(r#""status":"stored""#),
            "storage put: {stored}"
        );
        let file_id = stored
            .split(r#""file_id":""#)
            .nth(1)
            .and_then(|x| x.split('"').next())
            .expect("file id");
        let downloaded = get(
            "127.0.0.1:46201",
            &format!("/api/storage/get?file_id={file_id}"),
        );
        assert!(
            downloaded.contains(r#""status":"reconstructed""#)
                && downloaded.contains(&format!(r#""data_hex":"{hex}""#)),
            "storage get: {downloaded}"
        );
    });

    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = fs::remove_dir_all(&root);
    result.expect("product smoke test failed");
}
