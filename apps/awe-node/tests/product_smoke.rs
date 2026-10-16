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

fn post_without_session(addr: &str, path: &str, body: &str) -> String {
    request(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn post(addr: &str, path: &str, body: &str) -> String {
    let html = get(addr, "/");
    let token = html
        .split("name=\"awenet-api-token\" content=\"")
        .nth(1)
        .and_then(|value| value.split('"').next())
        .expect("session token in UI HTML");
    request(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: {addr}\r\nX-AWENET-Session: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
        .env("AWE_NO_NATIVE_UI", "1")
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

        let unauthorized = post_without_session(
            "127.0.0.1:46201",
            "/api/federation/generate",
            r#"{"kind":"awenode","name":"must-not-be-created"}"#,
        );
        assert!(
            unauthorized.contains("valid AWENET session token required"),
            "unauthenticated state change was not rejected: {unauthorized}"
        );

        let offers_before = get("127.0.0.1:46201", "/api/onebank/exchange/offers");
        let malformed_offer = post(
            "127.0.0.1:46201",
            "/api/onebank/exchange/offers",
            "{",
        );
        assert!(
            malformed_offer.contains("JSON request body must be a valid object"),
            "malformed JSON was not rejected: {malformed_offer}"
        );
        let offers_after = get("127.0.0.1:46201", "/api/onebank/exchange/offers");
        assert_eq!(
            offers_after, offers_before,
            "malformed JSON unexpectedly changed the offer book"
        );

        let dc_generated: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind": "awedc",
                "data_centre_id": "dc-smoke-owner-auth",
                "name": "Signed smoke data centre"
            })
            .to_string(),
        ))
        .expect("signed data-centre generation JSON");
        assert_eq!(
            dc_generated["status"], "generated",
            "DC generation: {dc_generated}"
        );
        let dc_content = dc_generated["content"]
            .as_str()
            .expect("signed data-centre content")
            .to_string();
        let dc_envelope: serde_json::Value =
            serde_json::from_str(&dc_content).expect("signed DC envelope");
        let dc_id = dc_envelope["payload"]["data_centre_id"]
            .as_str()
            .expect("signed DC ID")
            .to_string();

        let dc_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({"kind":"awedc","content":dc_content.clone()}).to_string(),
        ))
        .expect("signed data-centre import JSON");
        assert_eq!(dc_import["status"], "imported", "DC import: {dc_import}");

        let dc2_generated: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind": "awedc",
                "data_centre_id": "dc-smoke-owner-auth-2",
                "name": "Second signed smoke data centre"
            })
            .to_string(),
        ))
        .expect("second signed data-centre generation JSON");
        assert_eq!(dc2_generated["status"], "generated");
        let dc2_content = dc2_generated["content"]
            .as_str()
            .expect("second signed data-centre content")
            .to_string();
        let dc2_envelope: serde_json::Value =
            serde_json::from_str(&dc2_content).expect("second signed DC envelope");
        let dc2_id = dc2_envelope["payload"]["data_centre_id"]
            .as_str()
            .expect("second signed DC ID")
            .to_string();
        let dc2_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({"kind":"awedc","content":dc2_content}).to_string(),
        ))
        .expect("second signed data-centre import JSON");
        assert_eq!(dc2_import["status"], "imported");

        let untrusted_group: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"dgc",
                "owner_data_centre_id":dc_id.clone(),
                "data_centre_ids":[dc_id.clone(), "dc-not-trusted"],
                "name":"untrusted reference must be denied"
            })
            .to_string(),
        ))
        .expect("untrusted data-group generation JSON");
        assert_eq!(
            untrusted_group["status"], "error",
            "data group referenced an untrusted data centre: {untrusted_group}"
        );

        let mut tampered_dc: serde_json::Value =
            serde_json::from_str(&dc_content).expect("tamper DC envelope");
        tampered_dc["payload"]["name"] = serde_json::Value::String("forged data-centre".into());
        let tampered_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({
                "kind":"awedc",
                "content":serde_json::to_string(&tampered_dc).unwrap()
            })
            .to_string(),
        ))
        .expect("tampered import JSON");
        assert_eq!(
            tampered_import["status"], "error",
            "tampered federation invitation was accepted: {tampered_import}"
        );

        let denied_group: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"dgc",
                "owner_data_centre_id":dc_id.clone(),
                "data_centre_ids":[dc_id.clone(), dc2_id.clone()],
                "name":"should be denied"
            })
            .to_string(),
        ))
        .expect("non-owner group generation JSON");
        assert_eq!(
            denied_group["status"], "error",
            "non-owner created a data group: {denied_group}"
        );

        let group_generated: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"dgc",
                "owner_data_centre_id":dc_id.clone(),
                "data_centre_ids":[dc_id.clone(), dc2_id.clone()],
                "name":"Signed smoke data group"
            })
            .to_string(),
        ))
        .expect("signed data-group generation JSON");
        assert_eq!(
            group_generated["status"], "generated",
            "DG generation: {group_generated}"
        );
        let group_content = group_generated["content"]
            .as_str()
            .expect("signed data-group content")
            .to_string();
        let group_envelope: serde_json::Value =
            serde_json::from_str(&group_content).expect("signed DG envelope");
        let group_id = group_envelope["payload"]["data_group_id"]
            .as_str()
            .expect("signed DG ID")
            .to_string();

        let group_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({"kind":"dgc","content":group_content.clone()}).to_string(),
        ))
        .expect("signed data-group import JSON");
        assert_eq!(
            group_import["status"], "imported",
            "DG import: {group_import}"
        );

        let group2_generated: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"dgc",
                "owner_data_centre_id":dc_id.clone(),
                "data_centre_ids":[dc_id.clone(), dc2_id.clone()],
                "name":"Second signed smoke data group"
            })
            .to_string(),
        ))
        .expect("second signed data-group generation JSON");
        assert_eq!(group2_generated["status"], "generated");
        let group2_content = group2_generated["content"]
            .as_str()
            .expect("second signed data-group content")
            .to_string();
        let group2_envelope: serde_json::Value =
            serde_json::from_str(&group2_content).expect("second signed DG envelope");
        let group2_id = group2_envelope["payload"]["data_group_id"]
            .as_str()
            .expect("second signed DG ID")
            .to_string();
        let group2_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({"kind":"dgc","content":group2_content}).to_string(),
        ))
        .expect("second signed data-group import JSON");
        assert_eq!(group2_import["status"], "imported");

        let untrusted_centre_group: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"cgc",
                "owner_data_group_id":group_id.clone(),
                "data_group_ids":[group_id.clone(), "dg-not-trusted"],
                "name":"untrusted reference must be denied"
            })
            .to_string(),
        ))
        .expect("untrusted centre-group generation JSON");
        assert_eq!(
            untrusted_centre_group["status"], "error",
            "centre group referenced an untrusted data group: {untrusted_centre_group}"
        );

        let denied_centre_group: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"cgc",
                "owner_data_group_id":group_id.clone(),
                "data_group_ids":[group_id.clone(), group2_id.clone()],
                "name":"should be denied"
            })
            .to_string(),
        ))
        .expect("non-owner centre-group generation JSON");
        assert_eq!(
            denied_centre_group["status"], "error",
            "non-owner created a centre group: {denied_centre_group}"
        );

        let centre_group_generated: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            &serde_json::json!({
                "kind":"cgc",
                "owner_data_group_id":group_id.clone(),
                "data_group_ids":[group_id.clone(), group2_id.clone()],
                "name":"Signed smoke centre group"
            })
            .to_string(),
        ))
        .expect("signed centre-group generation JSON");
        assert_eq!(
            centre_group_generated["status"], "generated",
            "CG generation: {centre_group_generated}"
        );
        let centre_group_content = centre_group_generated["content"]
            .as_str()
            .expect("signed centre-group content");
        let centre_group_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({"kind":"cgc","content":centre_group_content}).to_string(),
        ))
        .expect("signed centre-group import JSON");
        assert_eq!(
            centre_group_import["status"], "imported",
            "CG import: {centre_group_import}"
        );

        let leave_dc: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/leave",
            &serde_json::json!({"kind":"dc","id":dc_id.clone()}).to_string(),
        ))
        .expect("leave data-centre JSON");
        assert_eq!(leave_dc["status"], "left", "DC leave: {leave_dc}");
        let after_leave: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46202", "/api/federation"))
                .expect("federation state after leave");
        assert!(
            after_leave["data_centre_owners"]
                .get(dc_id.as_str())
                .is_none(),
            "DC owner trust survived leave: {after_leave}"
        );
        assert!(
            !after_leave["joined_data_centres"]
                .as_array()
                .expect("joined DC array")
                .iter()
                .any(|value| value.as_str() == Some(dc_id.as_str())),
            "DC membership survived leave: {after_leave}"
        );
        let stale_group_import: serde_json::Value = serde_json::from_str(&post(
            "127.0.0.1:46202",
            "/api/federation/import",
            &serde_json::json!({"kind":"dgc","content":group_content.clone()}).to_string(),
        ))
        .expect("stale group import JSON");
        assert_eq!(
            stale_group_import["status"], "error",
            "group invitation remained trusted after leaving its DC: {stale_group_import}"
        );

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

        let node2_wallet_before: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46202", "/api/onebank/wallet"))
                .expect("node2 wallet before json");
        let node2_id = node2_wallet_before
            .get("awe_id")
            .and_then(|v| v.as_str())
            .expect("node2 AWEID")
            .to_string();
        let wallet_before: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/onebank/wallet"))
                .expect("wallet before json");
        let onecoin_send = post(
            "127.0.0.1:46201",
            "/api/onebank/wallet/send",
            &format!(r#"{{"recipient":"{node2_id}","amount_coins":1,"memo":"E2E fee test"}}"#),
        );
        assert!(
            onecoin_send.contains(r#""status":"accepted""#),
            "ONECOIN send: {onecoin_send}"
        );
        assert!(
            onecoin_send.contains(r#""fee_bps":100"#),
            "ONEBANK fee: {onecoin_send}"
        );
        let wallet_after: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/onebank/wallet"))
                .expect("wallet after json");
        let receiver_deadline = Instant::now() + Duration::from_secs(5);
        let node2_wallet_after: serde_json::Value = loop {
            let current: serde_json::Value =
                serde_json::from_str(&get("127.0.0.1:46202", "/api/onebank/wallet"))
                    .expect("node2 wallet after json");
            let before_balance = node2_wallet_before
                .get("balance_atoms")
                .and_then(|v| v.as_u64())
                .expect("receiver balance");
            let current_balance = current
                .get("balance_atoms")
                .and_then(|v| v.as_u64())
                .expect("receiver current balance");
            if current_balance == before_balance + 1_000_000_000_000_000_000u64 {
                break current;
            }
            if Instant::now() >= receiver_deadline {
                panic!("recipient wallet did not receive transfer: {current}");
            }
            thread::sleep(Duration::from_millis(100));
        };
        let before = wallet_before
            .get("balance_atoms")
            .and_then(|v| v.as_u64())
            .expect("sender balance");
        let after = wallet_after
            .get("balance_atoms")
            .and_then(|v| v.as_u64())
            .expect("sender after");
        let receiver_before = node2_wallet_before
            .get("balance_atoms")
            .and_then(|v| v.as_u64())
            .expect("receiver balance");
        let receiver_after = node2_wallet_after
            .get("balance_atoms")
            .and_then(|v| v.as_u64())
            .expect("receiver after");
        assert_eq!(
            before - after,
            1_010_000_000_000_000_000u64,
            "sender pays amount plus 1% fee"
        );
        assert_eq!(
            receiver_after - receiver_before,
            1_000_000_000_000_000_000u64,
            "receiver gets exact amount"
        );
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
            group_send.contains(r#""status":"sent""#)
                && group_send.contains(r#""delivered_members":1"#),
            "group send did not reach a member: {group_send}"
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
        assert!(
            subscribe.contains(r#""owner_notified":true"#),
            "owner notification was not acknowledged: {subscribe}"
        );
        let subscriber_channels = get("127.0.0.1:46202", "/api/channels");
        assert!(
            subscriber_channels.contains(&format!(r#""{node2_id}""#)),
            "subscriber state: {subscriber_channels}"
        );
        let owner_channels_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let owner_channels = get("127.0.0.1:46201", "/api/channels");
            if owner_channels.contains(&format!(r#""{node2_id}""#)) {
                break;
            }
            if Instant::now() >= owner_channels_deadline {
                panic!("owner did not receive subscription: {owner_channels}");
            }
            thread::sleep(Duration::from_millis(100));
        }
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
