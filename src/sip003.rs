use anyhow::{bail, Result};
use std::collections::HashMap;
use std::env;

use crate::args::Args;

#[derive(Debug, Clone)]
pub struct Sip003Args {
    pub remote_host: String,
    pub remote_port: String,
    pub local_host: String,
    pub local_port: String,
    pub options: HashMap<String, String>,
}

impl Sip003Args {
    pub fn remote_addr(&self) -> String {
        join_host_port(&self.remote_host, &self.remote_port)
    }

    pub fn local_addr(&self) -> String {
        join_host_port(&self.local_host, &self.local_port)
    }
}

fn join_host_port(host: &str, port: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn get_sip003_args() -> Result<Option<Sip003Args>> {
    let remote_host = env::var("SS_REMOTE_HOST").ok();
    let remote_port = env::var("SS_REMOTE_PORT").ok();
    let local_host = env::var("SS_LOCAL_HOST").ok();
    let local_port = env::var("SS_LOCAL_PORT").ok();
    let plugin_options = env::var("SS_PLUGIN_OPTIONS").ok();

    if remote_host.is_none()
        && remote_port.is_none()
        && local_host.is_none()
        && local_port.is_none()
        && plugin_options.is_none()
    {
        return Ok(None);
    }

    let (Some(remote_host), Some(remote_port), Some(local_host), Some(local_port)) =
        (remote_host, remote_port, local_host, local_port)
    else {
        bail!("invalid SIP003 args");
    };

    Ok(Some(Sip003Args {
        remote_host,
        remote_port,
        local_host,
        local_port,
        options: parse_plugin_options(plugin_options.as_deref().unwrap_or_default()),
    }))
}

fn parse_plugin_options(options: &str) -> HashMap<String, String> {
    let mut parsed = HashMap::new();
    for option in options.split(';') {
        if option.is_empty() {
            continue;
        }
        match option.split_once('=') {
            Some((key, value)) => {
                parsed.insert(key.to_owned(), value.to_owned());
            }
            None => {
                parsed.insert(option.to_owned(), String::new());
            }
        }
    }
    parsed
}

pub fn apply(args: &mut Args) -> Result<bool> {
    let Some(sip) = get_sip003_args()? else {
        return Ok(false);
    };

    log::info!("simple-tls is running as a sip003 plugin");

    let options = &sip.options;

    let apply_bool = |value: &mut bool, key: &str| {
        if options.contains_key(key) {
            *value = true;
        }
    };
    let apply_string = |value: &mut String, key: &str| {
        if let Some(option) = options.get(key) {
            if !option.is_empty() {
                *value = option.clone();
            }
        }
    };
    let apply_int = |value: &mut i32, key: &str| -> Result<()> {
        if let Some(option) = options.get(key) {
            if !option.is_empty() {
                let parsed: i32 = option
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid numeric value of sip003 key {key}"))?;
                if parsed > 0 {
                    *value = parsed;
                }
            }
        }
        Ok(())
    };

    apply_bool(&mut args.vpn, "V");
    apply_bool(&mut args.vpn, "__android_vpn");
    if args.vpn {
        log::info!("running in a android vpn mode");
    }

    apply_string(&mut args.bind, "b");
    apply_string(&mut args.dst, "d");
    apply_bool(&mut args.grpc, "grpc");
    apply_string(&mut args.grpc_path, "grpc-path");

    apply_string(&mut args.server_name, "n");
    apply_string(&mut args.ca, "ca");
    apply_string(&mut args.cert_hash, "cert-hash");
    apply_bool(&mut args.no_verify, "no-verify");

    apply_bool(&mut args.is_server, "s");
    apply_string(&mut args.cert, "cert");
    apply_string(&mut args.key, "key");

    if let Some(option) = options.get("t") {
        if !option.is_empty() {
            let parsed: i64 = option
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid numeric value of sip003 key t"))?;
            if parsed > 0 {
                args.timeout = parsed;
            }
        }
    }
    if let Some(option) = options.get("cpu") {
        if !option.is_empty() {
            let parsed: usize = option
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid numeric value of sip003 key cpu"))?;
            if parsed > 0 {
                args.cpu = parsed;
            }
        }
    }
    apply_int(&mut args.outbound_buf, "outbound-buf")?;
    apply_int(&mut args.inbound_buf, "inbound-buf")?;

    if args.is_server {
        args.dst = sip.local_addr();
        args.bind = sip.remote_addr();
    } else {
        args.bind = sip.local_addr();
        args.dst = sip.remote_addr();
    }

    Ok(true)
}
