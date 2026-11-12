use awep2p_core::{
    identity::{AweSecret, Identity, Username},
    network_control::{self, NetworkAction, NetworkControlPlane},
    network_topology::CentreLinkMode,
    user_registry::{self, UserRegistry, UserRole, UserStatus},
};
use std::{
    env, fs,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn request_raw(addr: &str, request: &str) -> String {
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
    String::from_utf8_lossy(&out).into_owned()
}

fn request(addr: &str, request: &str) -> String {
    request_raw(addr, request)
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

fn post_raw(addr: &str, path: &str, body: &str) -> String {
    request_raw(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn generated_join_token(response: &str, prefix: &str) -> String {
    let envelope: serde_json::Value =
        serde_json::from_str(response).expect("federation generation response JSON");
    assert_eq!(
        envelope.get("status").and_then(|value| value.as_str()),
        Some("generated"),
        "federation generation failed: {response}"
    );
    let encoded_config = envelope
        .get("content")
        .and_then(|value| value.as_str())
        .expect("generated federation config");
    let config: serde_json::Value =
        serde_json::from_str(encoded_config).expect("generated federation config JSON");
    let token = config
        .get("join_token")
        .and_then(|value| value.as_str())
        .expect("federation join token");
    assert!(
        token.starts_with(prefix),
        "unexpected token prefix: {token}"
    );
    assert_eq!(
        token.len(),
        prefix.len() + 64,
        "token must contain 256 random bits"
    );
    token.to_owned()
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

/// The combined stdout/stderr of one spawned node, kept bounded so a
/// long-running smoke test cannot grow it without limit. The panic
/// diagnostics of the consensus wait include these tails, because the
/// live transport failures that matter (rejected announces, rejected
/// reports, deferred deliveries) are printed by the nodes themselves.
type NodeLog = Arc<Mutex<String>>;

const NODE_LOG_LIMIT: usize = 64 * 1024;

fn spawn_node(bin: &PathBuf, data: &PathBuf, listen: u16, ui: u16) -> (Child, NodeLog) {
    let mut child = Command::new(bin)
        .env("AWE_DATA_DIR", data)
        .env("AWE_LISTEN_ADDR", format!("127.0.0.1:{listen}"))
        .env("AWE_UI_ADDR", format!("127.0.0.1:{ui}"))
        .env("AWE_NO_BROWSER", "1")
        .env("AWE_NO_NATIVE_UI", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn node");
    let log: NodeLog = Arc::new(Mutex::new(String::new()));
    let mut pipes: Vec<Box<dyn Read + Send>> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        pipes.push(Box::new(stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        pipes.push(Box::new(stderr));
    }
    for mut stream in pipes {
        let log = Arc::clone(&log);
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut log) = log.lock() {
                            log.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if log.len() > NODE_LOG_LIMIT {
                                let mut keep = log.len() - NODE_LOG_LIMIT / 2;
                                while keep < log.len() && !log.is_char_boundary(keep) {
                                    keep += 1;
                                }
                                *log = log[keep..].to_string();
                            }
                        }
                    }
                }
            }
        });
    }
    (child, log)
}

/// The last `chars` characters of one node's captured output.
fn node_log_tail(log: &NodeLog, chars: usize) -> String {
    log.lock()
        .map(|log| {
            log.chars()
                .rev()
                .take(chars)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// The verifier transport diagnostics one node printed (failed, deferred
/// or rejected announces and reports), capped so a noisy node cannot
/// drown the consensus panic. These lines pinpoint which side of the
/// live transport kept a verifier's report from being delivered.
fn node_log_verifier_lines(log: &NodeLog, max_lines: usize) -> String {
    log.lock()
        .map(|log| {
            let lines: Vec<&str> = log
                .lines()
                .filter(|line| line.contains("verifier"))
                .take(max_lines)
                .collect();
            if lines.is_empty() {
                "(no verifier transport diagnostics)".to_string()
            } else {
                lines.join("\n")
            }
        })
        .unwrap_or_default()
}

#[test]
fn three_node_product_smoke() {
    let bin = PathBuf::from(env::var_os("CARGO_BIN_EXE_awe-node").expect("binary path"));
    let root = env::temp_dir().join(format!("awep2p-smoke-{}", std::process::id()));
    let dirs = [root.join("n1"), root.join("n2"), root.join("n3")];
    for dir in &dirs {
        fs::create_dir_all(dir).expect("create data dir");
    }
    // Give node 1 a stable test founder key so the HTTP control-plane
    // mutation signatures can be generated by this integration test.
    let founder_identity = Identity::generate(Username::new("smoke-founder".to_string()).unwrap());
    let founder_secret = AweSecret::generate(&founder_identity);
    fs::write(
        dirs[0].join("node.awesecret"),
        founder_secret
            .to_bytes()
            .expect("serialize smoke founder secret"),
    )
    .expect("write smoke founder secret");

    let mut children = Vec::new();
    let mut node_logs = Vec::new();
    for (index, (listen, ui)) in [(46101u16, 46201u16), (46102, 46202), (46103, 46203)]
        .into_iter()
        .enumerate()
    {
        let (child, log) = spawn_node(&bin, &dirs[index], listen, ui);
        children.push(child);
        node_logs.push(log);
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait_for_health("127.0.0.1:46201");
        wait_for_health("127.0.0.1:46202");
        wait_for_health("127.0.0.1:46203");

        let same_origin = request_raw(
            "127.0.0.1:46201",
            "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:46201\r\nOrigin: http://127.0.0.1:46201\r\nConnection: close\r\n\r\n",
        );
        assert!(
            same_origin.starts_with("HTTP/1.1 200 OK"),
            "same-origin browser request should work: {same_origin}"
        );
        assert!(
            !same_origin
                .split("\r\n\r\n")
                .next()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains("access-control-allow-origin"),
            "local API must not emit wildcard CORS headers: {same_origin}"
        );

        let cross_origin = request_raw(
            "127.0.0.1:46201",
            "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:46201\r\nOrigin: https://evil.example\r\nConnection: close\r\n\r\n",
        );
        assert!(
            cross_origin.starts_with("HTTP/1.1 403 Forbidden"),
            "cross-origin browser request must be rejected: {cross_origin}"
        );

        let rebinding = request_raw(
            "127.0.0.1:46201",
            "GET /api/status HTTP/1.1\r\nHost: attacker.example\r\nOrigin: http://attacker.example\r\nConnection: close\r\n\r\n",
        );
        assert!(
            rebinding.starts_with("HTTP/1.1 403 Forbidden"),
            "DNS-rebinding host must be rejected: {rebinding}"
        );

        let node_invite = post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            r#"{"kind":"awenode","name":"E2E DC"}"#,
        );
        let node_token = generated_join_token(&node_invite, "node-");
        let repeated_node_invite = post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            r#"{"kind":"awenode","name":"E2E DC"}"#,
        );
        let repeated_node_token = generated_join_token(&repeated_node_invite, "node-");
        assert_ne!(
            node_token, repeated_node_token,
            "identical inputs must not reproduce a join token"
        );

        let dc_invite = post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            r#"{"kind":"awedc","name":"E2E DC","data_centre_id":"dc-e2e"}"#,
        );
        let _dc_token = generated_join_token(&dc_invite, "dc-");

        let dg_invite = post(
            "127.0.0.1:46201",
            "/api/federation/generate",
            r#"{"kind":"dgc","name":"E2E Group","owner_data_centre_id":"dc-e2e","data_centre_ids":["dc-e2e","dc-remote"]}"#,
        );
        let _dg_token = generated_join_token(&dg_invite, "dg-");

        // Malformed invitation JSON must be rejected without changing node state.
        let before_malformed_import: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46202", "/api/federation"))
                .expect("pre-malformed federation state");
        let malformed_response = request_raw(
            "127.0.0.1:46202",
            "POST /api/federation/import HTTP/1.1\r\nHost: 127.0.0.1:46202\r\nContent-Type: application/json\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{",
        );
        assert!(
            malformed_response.starts_with("HTTP/1.1 400 Bad Request"),
            "malformed federation JSON must return 400: {malformed_response}"
        );
        let after_malformed_import: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46202", "/api/federation"))
                .expect("post-malformed federation state");
        assert_eq!(
            before_malformed_import, after_malformed_import,
            "malformed federation JSON must not mutate state"
        );

        // Import a valid invitation on node 2 and verify that the committed
        // federation state is visible both through HTTP and on disk.
        let invitation_envelope: serde_json::Value =
            serde_json::from_str(&node_invite).expect("node invitation envelope");
        let invitation_content = invitation_envelope
            .get("content")
            .and_then(|value| value.as_str())
            .expect("node invitation content")
            .to_owned();
        let invitation_config: serde_json::Value =
            serde_json::from_str(&invitation_content).expect("node invitation config");
        let invited_dc_id = invitation_config
            .get("data_centre_id")
            .and_then(|value| value.as_str())
            .expect("invited data-centre ID")
            .to_owned();
        let import_body = serde_json::json!({
            "kind": "awenode",
            "content": invitation_content
        })
        .to_string();
        let import_response = request_raw(
            "127.0.0.1:46202",
            &format!(
                "POST /api/federation/import HTTP/1.1\r\nHost: 127.0.0.1:46202\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                import_body.len(),
                import_body
            ),
        );
        assert!(
            import_response.starts_with("HTTP/1.1 200 OK"),
            "valid federation invitation should import: {import_response}"
        );
        let imported_state: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46202", "/api/federation"))
                .expect("imported federation state");
        assert!(
            imported_state
                .get("joined_data_centres")
                .and_then(|value| value.as_array())
                .map(|items| items
                    .iter()
                    .any(|value| value.as_str() == Some(&invited_dc_id)))
                .unwrap_or(false),
            "successful import should publish the joined data centre: {imported_state}"
        );
        let persisted_state: serde_json::Value = serde_json::from_slice(
            &fs::read(dirs[1].join("awenet.json")).expect("read persisted federation state"),
        )
        .expect("persisted federation JSON");
        assert!(
            persisted_state
                .get("joined_data_centres")
                .and_then(|value| value.as_array())
                .map(|items| items
                    .iter()
                    .any(|value| value.as_str() == Some(&invited_dc_id)))
                .unwrap_or(false),
            "successful import should persist the joined data centre: {persisted_state}"
        );

        // Force the next node's federation-state publication to fail. The API
        // must report 500, keep the old in-memory state, and clean its temp file.
        let before_failed_import: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46203", "/api/federation"))
                .expect("pre-failure federation state");
        let blocked_path = dirs[2].join("awenet.json");
        fs::remove_file(&blocked_path).expect("remove federation state file");
        fs::create_dir(&blocked_path).expect("block federation state destination");
        let failed_import_response = request_raw(
            "127.0.0.1:46203",
            &format!(
                "POST /api/federation/import HTTP/1.1\r\nHost: 127.0.0.1:46203\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                import_body.len(),
                import_body
            ),
        );
        assert!(
            failed_import_response.starts_with("HTTP/1.1 500 Internal Server Error"),
            "failed federation persistence must return 500: {failed_import_response}"
        );
        let after_failed_import: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46203", "/api/federation"))
                .expect("post-failure federation state");
        assert_eq!(
            before_failed_import, after_failed_import,
            "failed persistence must not mutate in-memory federation state"
        );
        let leaked_temp = fs::read_dir(&dirs[2])
            .expect("read node data directory")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .any(|name| name.starts_with(".awenet.json.") && name.ends_with(".tmp"));
        assert!(
            !leaked_temp,
            "failed federation publication leaked a temp file"
        );

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

        // AWEID account lifecycle: self-registration proves key possession;
        // role/status mutations require a founder signature and durable snapshot.
        let founder: Identity =
            AweSecret::from_bytes(&fs::read(dirs[0].join("node.awesecret")).unwrap())
                .expect("founder node secret")
                .authenticate()
                .expect("founder identity");
        let initial_users: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/users"))
                .expect("initial user registry JSON");
        let founder_id = founder.public.awe_id.to_hex();
        assert_eq!(
            initial_users
                .get("founder_awe_id")
                .and_then(|value| value.as_str()),
            Some(founder_id.as_str()),
            "the node identity must be the seeded founder account"
        );
        assert_eq!(
            initial_users
                .get("users")
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(1),
            "fresh node should start with only its founder account"
        );

        let new_user = Identity::generate(Username::new("phase2-user".to_string()).unwrap());
        let registration_message =
            UserRegistry::registration_message("phase2-user", &new_user.public.public_key);
        let registration_signature = new_user.sign(&registration_message);
        let registration = post(
            "127.0.0.1:46201",
            "/api/users/register",
            &serde_json::json!({
                "username": "phase2-user",
                "public_key": hex::encode(new_user.public.public_key),
                "signature": hex::encode(registration_signature)
            })
            .to_string(),
        );
        assert!(
            registration.contains(r#""status":"created""#),
            "signed AWEID registration failed: {registration}"
        );
        let new_user_id = new_user.public.awe_id.to_hex();
        let mut expected_registry = UserRegistry::new(&founder, 1);
        expected_registry
            .register(
                "phase2-user",
                &new_user.public.public_key,
                &registration_signature,
                2,
            )
            .expect("build expected user registry sequence");

        let server_time = initial_users
            .get("server_time_unix")
            .and_then(|value| value.as_u64())
            .expect("user registry server time");
        let role_at = server_time + 60;
        let role_message = expected_registry
            .role_change_message(&new_user_id, UserRole::Admin, role_at)
            .expect("founder role-change message");
        let impostor = Identity::generate(Username::new("not-founder".to_string()).unwrap());
        let rejected_role = post(
            "127.0.0.1:46201",
            "/api/admin/users/role",
            &serde_json::json!({
                "awe_id": new_user_id,
                "role": "admin",
                "at_unix": role_at,
                "signature": hex::encode(impostor.sign(&role_message))
            })
            .to_string(),
        );
        assert!(
            rejected_role.contains("identity proof signature is invalid"),
            "non-founder role change must be rejected: {rejected_role}"
        );

        let role_signature = founder.sign(&role_message);
        let promoted = post(
            "127.0.0.1:46201",
            "/api/admin/users/role",
            &serde_json::json!({
                "awe_id": new_user_id,
                "role": "admin",
                "at_unix": role_at,
                "signature": hex::encode(role_signature)
            })
            .to_string(),
        );
        assert!(
            promoted.contains(r#""status":"updated""#) && promoted.contains(r#""role":"admin""#),
            "founder-signed promotion failed: {promoted}"
        );
        expected_registry
            .set_role(&new_user_id, UserRole::Admin, role_at, &role_signature)
            .expect("advance expected registry after promotion");

        let status_at = role_at + 1;
        let status_message = expected_registry
            .status_change_message(&new_user_id, UserStatus::Suspended, status_at)
            .expect("founder status-change message");
        let status_signature = founder.sign(&status_message);
        let suspended = post(
            "127.0.0.1:46201",
            "/api/admin/users/status",
            &serde_json::json!({
                "awe_id": new_user_id,
                "status": "suspended",
                "at_unix": status_at,
                "signature": hex::encode(status_signature)
            })
            .to_string(),
        );
        assert!(
            suspended.contains(r#""status":"updated""#)
                && suspended.contains(r#""status":"suspended""#),
            "founder-signed suspension failed: {suspended}"
        );
        let user_detail: serde_json::Value = serde_json::from_str(&get(
            "127.0.0.1:46201",
            &format!("/api/users/{new_user_id}"),
        ))
        .expect("user detail JSON");
        assert_eq!(
            user_detail
                .pointer("/user/role")
                .and_then(|value| value.as_str()),
            Some("admin")
        );
        assert_eq!(
            user_detail
                .pointer("/user/status")
                .and_then(|value| value.as_str()),
            Some("suspended")
        );
        let durable_users = user_registry::load(&dirs[0].join("users.json"), &founder)
            .expect("founder-signed durable user registry");
        assert_eq!(
            durable_users.get(&new_user_id).map(|account| account.role),
            Some(UserRole::Admin)
        );
        assert_eq!(
            durable_users
                .get(&new_user_id)
                .map(|account| account.status),
            Some(UserStatus::Suspended)
        );

        // A failed disk publish must not make a newly registered account
        // visible in memory. Replace the target file with a directory so the
        // atomic rename fails, then restore the original signed snapshot.
        let unpersisted_user =
            Identity::generate(Username::new("disk-failure-user".to_string()).unwrap());
        let unpersisted_message = UserRegistry::registration_message(
            "disk-failure-user",
            &unpersisted_user.public.public_key,
        );
        let unpersisted_signature = unpersisted_user.sign(&unpersisted_message);
        let users_path = dirs[0].join("users.json");
        let backup_path = dirs[0].join("users.json.backup");
        fs::rename(&users_path, &backup_path).expect("move signed user registry for failure test");
        fs::create_dir(&users_path).expect("create blocking registry directory");
        let failed_registration = post_raw(
            "127.0.0.1:46201",
            "/api/users/register",
            &serde_json::json!({
                "username": "disk-failure-user",
                "public_key": hex::encode(unpersisted_user.public.public_key),
                "signature": hex::encode(unpersisted_signature)
            })
            .to_string(),
        );
        assert!(
            failed_registration.starts_with("HTTP/1.1 500 Internal Server Error"),
            "registry persistence failure must return HTTP 500: {failed_registration}"
        );
        let in_memory_users = get("127.0.0.1:46201", "/api/users");
        assert!(
            !in_memory_users.contains(&unpersisted_user.public.awe_id.to_hex()),
            "failed persistence must not publish the candidate registry: {in_memory_users}"
        );
        let leftover_temps = fs::read_dir(&dirs[0])
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".awenet-users-")
            })
            .count();
        assert_eq!(
            leftover_temps, 0,
            "failed registry save must clean its temporary file"
        );
        fs::remove_dir(&users_path).expect("remove blocking registry directory");
        fs::rename(&backup_path, &users_path).expect("restore signed user registry");
        user_registry::load(&users_path, &founder).expect("restored signed user registry");

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
        assert!(
            ui.contains("<title>AWENET"),
            "desktop UI title must identify AWENET: {ui}"
        );
        let i18n_hy = get("127.0.0.1:46201", "/api/i18n/messages?locale=hy");
        assert!(
            i18n_hy.contains(r#""status":"ok""#) && i18n_hy.contains("Հիմնադրի"),
            "Armenian economy catalog must be served: {i18n_hy}"
        );
        let i18n_en = get("127.0.0.1:46201", "/api/i18n/messages?locale=en");
        assert!(
            i18n_en.contains(r#""status":"ok""#) && i18n_en.contains("Founder-published tasks"),
            "English economy catalog must be served: {i18n_en}"
        );
        let i18n_bad = get("127.0.0.1:46201", "/api/i18n/messages?locale=fr");
        assert!(
            i18n_bad.contains(r#""status":"error""#)
                && i18n_bad.contains("locale must be en or hy"),
            "unknown locale must be rejected: {i18n_bad}"
        );
        let i18n_js = get("127.0.0.1:46201", "/i18n.js");
        assert!(
            i18n_js.contains("AWEI18N"),
            "i18n.js loader must be served by the node: {i18n_js}"
        );
        let store = get("127.0.0.1:46201", "/api/store/catalog");
        assert!(store.contains(r#""status":"ok""#), "store API: {store}");

        // Exercise the real HTTP network control plane with founder-signed
        // mutations. The test mirrors the signing sequence locally and checks
        // the persisted snapshot after all lifecycle operations.
        let initial_control: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/network/control"))
                .expect("initial network control JSON");
        assert_eq!(initial_control["status"], "ok");
        let expected_founder_id = founder_identity.public.awe_id.to_hex();
        assert_eq!(
            initial_control["founder_awe_id"].as_str(),
            Some(expected_founder_id.as_str()),
            "the control plane must be pinned to the node's installed founder identity"
        );
        let initial_time = initial_control["updated_at_unix"]
            .as_u64()
            .expect("initial network control timestamp");
        let mut signing_state = NetworkControlPlane::new(&founder_identity, initial_time);
        let mut actions = Vec::new();
        for index in 0..6 {
            actions.push(NetworkAction::CreateCentre {
                centre_id: format!("dc-{index}"),
            });
        }
        for index in 0..6 {
            actions.push(NetworkAction::AddNode {
                centre_id: format!("dc-{index}"),
                node_id: format!("node-{index}"),
            });
        }
        actions.push(NetworkAction::ConnectCentres {
            left: "dc-0".into(),
            right: "dc-1".into(),
            mode: CentreLinkMode::FullMesh,
        });
        actions.push(NetworkAction::CreateDataGroup {
            group_id: "dg-0".into(),
            centre_ids: vec!["dc-0".into(), "dc-1".into(), "dc-2".into()],
        });
        actions.push(NetworkAction::CreateDataGroup {
            group_id: "dg-1".into(),
            centre_ids: vec!["dc-3".into(), "dc-4".into(), "dc-5".into()],
        });
        actions.push(NetworkAction::CreateCentreGroup {
            group_id: "cg-main".into(),
            group_ids: vec!["dg-0".into(), "dg-1".into()],
        });
        actions.push(NetworkAction::SetNodeHealth {
            node_id: "node-0".into(),
            alive: false,
        });
        actions.push(NetworkAction::SetNodeHealth {
            node_id: "node-0".into(),
            alive: true,
        });

        let mut first_signed_request: Option<(NetworkAction, u64, [u8; 64])> = None;
        for action in actions {
            let at_unix = signing_state
                .updated_at_unix()
                .checked_add(1)
                .expect("network mutation timestamp");
            let message = signing_state
                .signing_message(&action, at_unix)
                .expect("build founder signing message");
            let signature = founder_identity.sign(&message);
            if first_signed_request.is_none() {
                first_signed_request = Some((action.clone(), at_unix, signature));
            }
            let body = serde_json::json!({
                "action": action.clone(),
                "at_unix": at_unix,
                "signature": hex::encode(signature)
            })
            .to_string();
            let raw = post_raw("127.0.0.1:46201", "/api/network/control", &body);
            assert!(
                raw.starts_with("HTTP/1.1 200 OK"),
                "network mutation failed: {raw}"
            );
            let applied: serde_json::Value =
                serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap_or(""))
                    .expect("network mutation response JSON");
            assert_eq!(applied["status"], "applied", "network mutation: {applied}");
            signing_state
                .apply_signed(&founder_identity, &action, at_unix, at_unix, &signature)
                .expect("mirror accepted network mutation");
        }

        let (replay_action, replay_time, replay_signature) =
            first_signed_request.expect("first signed network request");
        let replay_body = serde_json::json!({
            "action": replay_action,
            "at_unix": replay_time,
            "signature": hex::encode(replay_signature)
        })
        .to_string();
        let replay = post_raw("127.0.0.1:46201", "/api/network/control", &replay_body);
        assert!(
            replay.starts_with("HTTP/1.1 409 Conflict"),
            "stale replay was not rejected as a conflict: {replay}"
        );

        let forged_action = NetworkAction::CreateCentre {
            centre_id: "dc-forged".into(),
        };
        let forged_time = signing_state.updated_at_unix() + 1;
        let forged_message = signing_state
            .signing_message(&forged_action, forged_time)
            .expect("build forged action message");
        let impostor = Identity::generate(Username::new("smoke-impostor".to_string()).unwrap());
        let forged_signature = impostor.sign(&forged_message);
        let forged_body = serde_json::json!({
            "action": forged_action,
            "at_unix": forged_time,
            "signature": hex::encode(forged_signature)
        })
        .to_string();
        let forged = post_raw("127.0.0.1:46201", "/api/network/control", &forged_body);
        assert!(
            forged.starts_with("HTTP/1.1 403 Forbidden"),
            "forged mutation was not rejected: {forged}"
        );

        let remove_referenced_centre = NetworkAction::RemoveCentre {
            centre_id: "dc-0".into(),
        };
        let remove_time = signing_state.updated_at_unix() + 1;
        let remove_message = signing_state
            .signing_message(&remove_referenced_centre, remove_time)
            .expect("build dependency removal message");
        let remove_signature = founder_identity.sign(&remove_message);
        let remove_body = serde_json::json!({
            "action": remove_referenced_centre,
            "at_unix": remove_time,
            "signature": hex::encode(remove_signature)
        })
        .to_string();
        let blocked = post_raw("127.0.0.1:46201", "/api/network/control", &remove_body);
        assert!(
            blocked.starts_with("HTTP/1.1 409 Conflict"),
            "referenced centre removal was not blocked: {blocked}"
        );

        let control_status: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/network/topology"))
                .expect("network topology JSON");
        assert_eq!(
            control_status["sequence"].as_u64(),
            Some(signing_state.sequence())
        );
        assert_eq!(
            control_status["topology"]["centres"]
                .as_object()
                .unwrap()
                .len(),
            6
        );
        assert_eq!(
            control_status["topology"]["data_groups"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            control_status["topology"]["centre_groups"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(
            control_status["topology"]["centres"]["dc-0"]["nodes"]["node-0"]["peers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer == "node-1")
        );
        assert!(
            control_status["topology"]["centres"]["dc-1"]["nodes"]["node-1"]["peers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer == "node-0")
        );

        let persisted_control =
            network_control::load(&dirs[0].join("network-control.json"), &founder_identity)
                .expect("load founder-signed network snapshot");
        assert_eq!(persisted_control.sequence(), signing_state.sequence());
        assert_eq!(persisted_control, signing_state);

        // Live verifier transport between the three nodes: the founder
        // enrolls nodes 2 and 3 as usage verifiers by their public node
        // keys, opens a short attestation window for node 1's own
        // contribution, and the peers must observe the announcement,
        // deliver one signed report each over the verifier transport
        // stream and reach the consensus the resource economy settles
        // from.
        let status_json: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/status"))
                .expect("status JSON for verifier enrollment");
        let peer_keys: Vec<String> = status_json
            .get("peers")
            .and_then(|value| value.as_array())
            .expect("status peers")
            .iter()
            .filter_map(|peer| peer.get("public_key").and_then(|value| value.as_str()))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            peer_keys.len(),
            2,
            "node 1 must see both peers with public keys: {status_json}"
        );
        for key in &peer_keys {
            let enrolled = post(
                "127.0.0.1:46201",
                "/api/founder/verifier/enroll",
                &serde_json::json!({ "public_key": key }).to_string(),
            );
            assert!(
                enrolled.contains(r#""status":"enrolled""#),
                "verifier enrollment failed: {enrolled}"
            );
        }
        let window = post(
            "127.0.0.1:46201",
            "/api/founder/verifier/window",
            r#"{"period_secs":10}"#,
        );
        assert!(
            window.contains(r#""status":"open""#),
            "attestation window open failed: {window}"
        );
        let window_json: serde_json::Value = serde_json::from_str(&window).expect("window JSON");
        let window_start = window_json
            .get("period_start_unix")
            .and_then(|value| value.as_u64())
            .expect("window period start");
        let consensus_deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let verifier_net: serde_json::Value =
                serde_json::from_str(&get("127.0.0.1:46201", "/api/verifier/net"))
                    .expect("verifier net JSON");
            let observed = verifier_net
                .get("windows")
                .and_then(|value| value.as_array())
                .expect("verifier net windows")
                .iter()
                .find(|window| {
                    window
                        .get("period_start_unix")
                        .and_then(|value| value.as_u64())
                        == Some(window_start)
                })
                .expect("the opened attestation window");
            let reported = observed
                .get("reports")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            if observed
                .get("has_consensus")
                .and_then(|value| value.as_bool())
                == Some(true)
                && reported == 2
            {
                break;
            }
            if Instant::now() >= consensus_deadline {
                panic!(
                    "peer verifiers did not reach consensus: {verifier_net}\n\
                     --- node 1 verifier transport diagnostics ---\n{}\n\
                     --- node 2 verifier transport diagnostics ---\n{}\n\
                     --- node 3 verifier transport diagnostics ---\n{}\n\
                     --- node 1 output tail ---\n{}\n\
                     --- node 2 output tail ---\n{}\n\
                     --- node 3 output tail ---\n{}",
                    node_log_verifier_lines(&node_logs[0], 30),
                    node_log_verifier_lines(&node_logs[1], 30),
                    node_log_verifier_lines(&node_logs[2], 30),
                    node_log_tail(&node_logs[0], 600),
                    node_log_tail(&node_logs[1], 600),
                    node_log_tail(&node_logs[2], 600),
                );
            }
            thread::sleep(Duration::from_millis(500));
        }

        // Live marketplace catalog sync between the nodes: node 2
        // lists a product on its own storefront and the listing must
        // reach node 1's network catalog over the marketplace sync
        // stream, with the seller's node address attached; node 2 then
        // delists the product and the next announcement cycle must
        // remove it from node 1's catalog. The catalog is discovery
        // data: no cross-node order is executed here.
        let listed = post(
            "127.0.0.1:46202",
            "/api/storefront/products",
            &serde_json::json!({
                "title": "AWE Marketplace Sync Theme",
                "description": "Listed on node 2, discovered on node 1",
                "kind": "digital",
                "category": "themes",
                "price_atoms": 250,
            })
            .to_string(),
        );
        assert!(
            listed.contains(r#""status":"listed""#),
            "node 2 product listing failed: {listed}"
        );
        let product_id: u64 = serde_json::from_str::<serde_json::Value>(&listed)
            .expect("listing JSON")
            .get("product_id")
            .and_then(|value| value.as_u64())
            .expect("listing product id");
        let sync_deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let catalog: serde_json::Value =
                serde_json::from_str(&get("127.0.0.1:46201", "/api/storefront/network"))
                    .expect("network catalog JSON");
            let found = catalog
                .get("listings")
                .and_then(|value| value.as_array())
                .map(|list| {
                    list.iter().any(|listing| {
                        listing.get("id").and_then(|value| value.as_u64()) == Some(product_id)
                            && listing.get("seller_node_address").and_then(|value| value.as_str())
                                == Some("127.0.0.1:46202")
                    })
                })
                .unwrap_or(false);
            if found {
                break;
            }
            if Instant::now() >= sync_deadline {
                panic!(
                    "node 1 never saw node 2's listing in the network catalog: {catalog}\n\
                     --- node 1 output tail ---\n{}\n\
                     --- node 2 output tail ---\n{}",
                    node_log_tail(&node_logs[0], 600),
                    node_log_tail(&node_logs[1], 600),
                );
            }
            thread::sleep(Duration::from_millis(500));
        }
        // the honest notice keeps the network catalog from being
        // mistaken for an order route
        let catalog: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/storefront/network"))
                .expect("network catalog JSON");
        let notice = catalog
            .get("notice")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(
            notice.contains("not an order route"),
            "network catalog notice must state the discovery-only contract: {catalog}"
        );
        // node 2 delists the product and the next announcement cycle
        // must remove it from node 1's catalog
        let delisted = post(
            "127.0.0.1:46202",
            &format!("/api/storefront/products/{product_id}/deactivate"),
            "",
        );
        assert!(
            delisted.contains(r#""status":"deactivated""#),
            "node 2 product delisting failed: {delisted}"
        );
        let delist_deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let catalog: serde_json::Value =
                serde_json::from_str(&get("127.0.0.1:46201", "/api/storefront/network"))
                    .expect("network catalog JSON");
            let still_listed = catalog
                .get("listings")
                .and_then(|value| value.as_array())
                .map(|list| {
                    list.iter()
                        .any(|listing| listing.get("id").and_then(|value| value.as_u64()) == Some(product_id))
                })
                .unwrap_or(false);
            if !still_listed {
                break;
            }
            if Instant::now() >= delist_deadline {
                panic!(
                    "node 1 still sees the delisted product in the network catalog: {catalog}\n\
                     --- node 1 output tail ---\n{}\n\
                     --- node 2 output tail ---\n{}",
                    node_log_tail(&node_logs[0], 600),
                    node_log_tail(&node_logs[1], 600),
                );
            }
            thread::sleep(Duration::from_millis(500));
        }

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
        // A tiny object must be stored with the scaled 12-shard policy, not the
        // 1000-shard network ceiling. The API used to hardcode 1000 here, which
        // disagreed with the policy actually used to encode the shards.
        assert!(
            stored.contains(r#""shards":12"#),
            "small file must report 12 shards: {stored}"
        );
        assert!(
            !stored.contains(r#""shards":1000"#),
            "small file must not report the 1000-shard ceiling: {stored}"
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
        let _ = children[0].kill();
        let _ = children[0].wait();
        let (child, log) = spawn_node(&bin, &dirs[0], 46101, 46201);
        children[0] = child;
        node_logs[0] = log;
        wait_for_health("127.0.0.1:46201");
        let restarted_control: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/network/control"))
                .expect("restarted network control JSON");
        assert_eq!(
            restarted_control["sequence"].as_u64(),
            Some(signing_state.sequence())
        );
        assert_eq!(
            restarted_control["topology"]["centres"]
                .as_object()
                .unwrap()
                .len(),
            6
        );
        assert_eq!(
            restarted_control["topology"]["data_groups"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            restarted_control["topology"]["centre_groups"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
        assert!(
            restarted_control["topology"]["centres"]["dc-0"]["nodes"]["node-0"]["peers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer == "node-1")
        );
        assert!(
            restarted_control["topology"]["centres"]["dc-1"]["nodes"]["node-1"]["peers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|peer| peer == "node-0")
        );
        // the enrolled verifier roster is a founder-signed snapshot and
        // must survive the restart untouched
        let restarted_verifier_net: serde_json::Value =
            serde_json::from_str(&get("127.0.0.1:46201", "/api/verifier/net"))
                .expect("restarted verifier net JSON");
        assert_eq!(
            restarted_verifier_net
                .get("active_verifiers")
                .and_then(|value| value.as_u64()),
            Some(2),
            "the enrolled verifier roster must survive the restart: {restarted_verifier_net}"
        );
    }));

    for child in &mut children {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = fs::remove_dir_all(&root);
    result.expect("product smoke test failed");
}
