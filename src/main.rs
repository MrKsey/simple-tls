use anyhow::{Context, Result};
use simple_tls::{args::Args, cert, client, server, sip003};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            eprintln!("{}", Args::usage());
            std::process::exit(2);
        }
    };

    if args.help {
        print!("{}", Args::usage());
        return Ok(());
    }

    init_logger(args.debug);
    raise_fd_limit();

    if args.show_version {
        println!(
            "{}",
            option_env!("SIMPLE_TLS_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
        );
        return Ok(());
    }

    if args.gen_cert {
        generate_cert(&args)?;
        return Ok(());
    }

    if !args.hash_cert.is_empty() {
        cert::print_cert_hashes(Path::new(&args.hash_cert))?;
        return Ok(());
    }

    let mut args = args;
    if sip003::apply(&mut args)? {
        log::info!("simple-tls is running as a sip003 plugin");
    }

    if args.bind.is_empty() {
        anyhow::bail!("bind addr is required");
    }
    if args.dst.is_empty() {
        anyhow::bail!("destination addr is required");
    }

    log::info!(
        "simple-tls is starting, version: {}, os: {}, arch: {}",
        option_env!("SIMPLE_TLS_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.enable_all();
    if args.cpu > 0 {
        runtime.worker_threads(args.cpu);
    }
    let runtime = runtime.build().context("failed to create tokio runtime")?;

    runtime.block_on(async move {
        spawn_signal_handler();

        let idle_timeout = Duration::from_secs(args.timeout.max(0) as u64);

        if args.is_server {
            let listener = server::bind(&args.bind).await?;
            let opts = server::ServerOpts {
                dst: args.dst,
                grpc: args.grpc,
                grpc_path: args.grpc_path,
                cert: args.cert,
                key: args.key,
                server_name: args.server_name,
                idle_timeout,
                outbound_buf: args.outbound_buf,
                inbound_buf: args.inbound_buf,
            };
            server::serve(listener, opts).await?;
        } else {
            let listener = client::bind(&args.bind).await?;
            let opts = client::ClientOpts {
                dst: args.dst,
                grpc: args.grpc,
                grpc_path: args.grpc_path,
                server_name: args.server_name,
                ca: args.ca,
                cert_hash: args.cert_hash,
                no_verify: args.no_verify,
                idle_timeout,
                outbound_buf: args.outbound_buf,
                inbound_buf: args.inbound_buf,
            };
            client::serve(listener, opts).await?;
        }

        Ok::<(), anyhow::Error>(())
    })
}

fn init_logger(debug: bool) {
    let default_level = if debug { "debug" } else { "info" };
    let _ =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
            .format_timestamp_millis()
            .try_init();
}

/// Raises the soft open file limit up to the hard limit.
///
/// A proxy keeps one descriptor per tunnel. Many systems (and Docker by
/// default) start processes with a soft limit as low as 1024, which a busy
/// server reaches quickly, so use whatever the hard limit allows.
#[cfg(unix)]
fn raise_fd_limit() {
    const MAX_TARGET: libc::rlim_t = 1_048_576;

    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit and setrlimit only read and write the given struct.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        let target = limit.rlim_max.min(MAX_TARGET);
        if limit.rlim_cur >= target {
            return;
        }
        let raised = libc::rlimit {
            rlim_cur: target,
            rlim_max: limit.rlim_max,
        };
        if libc::setrlimit(libc::RLIMIT_NOFILE, &raised) == 0 {
            log::debug!(
                "raised the open file limit from {} to {}",
                limit.rlim_cur,
                target
            );
        }
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

fn generate_cert(args: &Args) -> Result<()> {
    log::info!("generating PEM encoded key and cert");

    let template = if args.template.is_empty() {
        None
    } else {
        Some(Path::new(&args.template))
    };
    let generated = cert::generate_cert(&args.server_name, template)?;

    let default_out_file_name = if !args.template.is_empty() {
        let template_path = Path::new(&args.template);
        format!(
            "gen_{}",
            template_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("certificate")
        )
    } else {
        generated.dns_name.trim_start_matches("*.").to_owned()
    };

    let key_path = if args.key.is_empty() {
        format!("{default_out_file_name}.key")
    } else {
        args.key.clone()
    };
    log::info!("generating PEM encoded key, file: {key_path}");
    fs::write(&key_path, &generated.key_pem)
        .with_context(|| format!("creating key file {key_path}"))?;

    let cert_path = if args.cert.is_empty() {
        format!("{default_out_file_name}.cert")
    } else {
        args.cert.clone()
    };
    log::info!("generating PEM encoded cert, file: {cert_path}");
    fs::write(&cert_path, &generated.cert_pem)
        .with_context(|| format!("creating cert file {cert_path}"))?;

    println!("Your new cert hash is:");
    println!("{}", hex::encode(generated.tbs_sha256));
    Ok(())
}

fn spawn_signal_handler() {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut terminate = match signal(SignalKind::terminate()) {
                Ok(terminate) => terminate,
                Err(error) => {
                    log::error!("failed to install SIGTERM handler: {error}");
                    let _ = tokio::signal::ctrl_c().await;
                    std::process::exit(0);
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }

        log::info!("exiting on signal");
        std::process::exit(0);
    });
}
