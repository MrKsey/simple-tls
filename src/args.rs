use std::fmt::Write as _;

pub const USAGE: &str = "\
Usage of simple-tls:
  Client listen address        Server listen address
       |                            |
|Client|-->|simple-tls client|--TLS1.3-->|simple-tls server|-->|Destination|

Common arguments:
  -b string
        [Host:Port] Listen address (required).
  -d string
        [Host:Port] Destination address (required).
  -grpc
        Use the gRPC transport. Client and server must match.
  -grpc-path string
        gRPC service name. Client and server must match.
  -outbound-buf int
        Outbound TCP socket buffer size.
  -inbound-buf int
        Inbound TCP socket buffer size.

Client arguments:
  -n string
        Server name. Used to verify the server certificate and as SNI.
  -no-verify
        Do not verify the server certificate chain and host name.
  -ca string
        CA certificate file for server verification (defaults to the system pool).
  -cert-hash string
        Server certificate hash (certificate pinning).

Server arguments:
  -s
        Run as a server (without this flag simple-tls runs as a client).
  -cert string
        Certificate path (PEM).
  -key string
        Key path (PEM).

Other arguments:
  -t int
        Connection idle timeout in seconds (default 300).
  -cpu int
        Number of worker threads.
  -v
        Print version and exit.
  -vv
        Verbose log.

Commands:
  -gen-cert
        Generate an ECC P-256 certificate in the current directory.
  -template string
        Template certificate for -gen-cert.
  -hash-cert string
        Print certificate hashes (for the client's -cert-hash).
";

#[derive(Debug, Clone)]
pub struct Args {
    pub bind: String,
    pub dst: String,
    pub grpc: bool,
    pub grpc_path: String,
    pub outbound_buf: i32,
    pub inbound_buf: i32,
    pub server_name: String,
    pub ca: String,
    pub cert_hash: String,
    pub no_verify: bool,
    pub vpn: bool,
    pub is_server: bool,
    pub cert: String,
    pub key: String,
    pub timeout: i64,
    pub cpu: usize,
    pub gen_cert: bool,
    pub template: String,
    pub show_version: bool,
    pub hash_cert: String,
    pub debug: bool,
    pub help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            bind: String::new(),
            dst: String::new(),
            grpc: false,
            grpc_path: String::new(),
            outbound_buf: 0,
            inbound_buf: 0,
            server_name: String::new(),
            ca: String::new(),
            cert_hash: String::new(),
            no_verify: false,
            vpn: false,
            is_server: false,
            cert: String::new(),
            key: String::new(),
            timeout: 300,
            cpu: 0,
            gen_cert: false,
            template: String::new(),
            show_version: false,
            hash_cert: String::new(),
            debug: false,
            help: false,
        }
    }
}

impl Args {
    pub fn parse<I>(argv: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = Self::default();
        let mut iter = argv.into_iter().peekable();

        while let Some(raw) = iter.next() {
            if raw == "-" || !raw.starts_with('-') {
                return Err(format!("unexpected argument: {raw}"));
            }

            let flag = raw.trim_start_matches('-');
            if flag.is_empty() {
                return Err("unexpected argument: -".to_owned());
            }

            let (name, inline_value) = match flag.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (flag, None),
            };

            let mut take_value = |name: &str| -> Result<String, String> {
                if let Some(value) = inline_value.clone() {
                    Ok(value)
                } else {
                    iter.next()
                        .ok_or_else(|| format!("flag needs an argument: -{name}"))
                }
            };

            match name {
                "b" => args.bind = take_value(name)?,
                "d" => args.dst = take_value(name)?,
                "grpc" => args.grpc = parse_bool(name, inline_value.as_deref())?,
                "grpc-path" => args.grpc_path = take_value(name)?,
                "outbound-buf" => {
                    args.outbound_buf = parse_number(name, &take_value(name)?)?;
                }
                "inbound-buf" => {
                    args.inbound_buf = parse_number(name, &take_value(name)?)?;
                }
                "n" => args.server_name = take_value(name)?,
                "ca" => args.ca = take_value(name)?,
                "cert-hash" => args.cert_hash = take_value(name)?,
                "no-verify" => args.no_verify = parse_bool(name, inline_value.as_deref())?,
                "V" => args.vpn = parse_bool(name, inline_value.as_deref())?,
                "s" => args.is_server = parse_bool(name, inline_value.as_deref())?,
                "cert" => args.cert = take_value(name)?,
                "key" => args.key = take_value(name)?,
                "t" => args.timeout = parse_number(name, &take_value(name)?)?,
                "cpu" => args.cpu = parse_number(name, &take_value(name)?)?,
                "gen-cert" => args.gen_cert = parse_bool(name, inline_value.as_deref())?,
                "template" => args.template = take_value(name)?,
                "v" => args.show_version = parse_bool(name, inline_value.as_deref())?,
                "hash-cert" => args.hash_cert = take_value(name)?,
                "vv" => args.debug = parse_bool(name, inline_value.as_deref())?,
                "h" | "help" => args.help = true,
                _ => return Err(format!("flag provided but not defined: -{name}")),
            }
        }

        Ok(args)
    }

    pub fn usage() -> String {
        let mut usage = String::from(USAGE);
        let _ = writeln!(usage);
        usage
    }
}

fn parse_bool(name: &str, value: Option<&str>) -> Result<bool, String> {
    match value {
        None => Ok(true),
        Some("1" | "t" | "T" | "true" | "TRUE" | "True") => Ok(true),
        Some("0" | "f" | "F" | "false" | "FALSE" | "False") => Ok(false),
        Some(value) => Err(format!("invalid boolean value {value:?} for -{name}")),
    }
}

fn parse_number<T>(name: &str, value: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("invalid value {value:?} for -{name}"))
}
