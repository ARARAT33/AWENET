    }
}

async fn run_product() -> Result<()> {
    let data_dir = if let Some(value) = env::var_os("AWE_DATA_DIR") {
        PathBuf::from(value)
    } else if let Some(home) = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME")) {
        PathBuf::from(home).join(".awep2p")
    } else {
        PathBuf::from(".awep2p")
    };
    fs::create_dir_all(&data_dir)?;
    let secret_path = data_dir.join("node.awesecret");
    let identity = if secret_path.exists() {
        AweSecret::from_bytes(&fs::read(&secret_path)?)
            .map_err(anyhow::Error::msg)?
            .authenticate()
            .map_err(anyhow::Error::msg)?
    } else {
        let identity =
            Identity::generate(Username::new("awe-node".to_string()).map_err(anyhow::Error::msg)?);
        let secret = AweSecret::generate(&identity);
        fs::write(&secret_path, secret.to_bytes()?)?;
        identity
    };
    let node_id = format_uid(identity.public.awe_id.as_bytes());
    let listen: SocketAddr = env::var("AWE_LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:41000".into())
        .parse()
        .context("invalid AWE_LISTEN_ADDR")?;
    let node = Node::new(identity, listen);
    let storage_root = data_dir.join("storage");
    let storage_quota = awep2p_core::node::get_available_disk_space(&storage_root).unwrap_or(0);