# simple-tls

Simple and easy-to-use TCP connection forwarder. Adds a TLS 1.3 layer to raw
data streams. Supports the gRPC transport.

This is a Rust rewrite of the original Go implementation
([IrineSistiana/simple-tls](https://github.com/IrineSistiana/simple-tls)).
The command-line interface, TLS 1.3-only transport, certificate pinning and
SIP003 plugin protocol are compatible with the original.

## Features

- TLS 1.3 only.
- Raw TLS and gRPC transports.
- Self-signed ECDSA P-256 certificate generation (valid for 10 years).
- Certificate pinning by SHA-256 of the certificate TBS part.
- Custom CA, system CA store or bundled WebPKI roots.
- SIP003 plugin support (Shadowsocks).
- Static Linux builds (musl) for x86_64, aarch64, MIPS32 and MIPS32 LE.

## Prebuilt binaries

| Platform | File |
|----------|------|
| Linux x86_64 (static) | `simple-tls-linux-amd64` |
| Linux aarch64 (static) | `simple-tls-linux-arm64` |
| Linux MIPS32 big-endian (static, soft-float) | `simple-tls-linux-mips-softfloat` |
| Linux MIPS32 little-endian (static, hard-float) | `simple-tls-linux-mipsle-float` |
| Linux MIPS32 little-endian (static, soft-float) | `simple-tls-linux-mipsle-softfloat` |
| Windows x86_64 | `simple-tls-windows-amd64.exe` |
| Windows ARM64 | `simple-tls-windows-arm64.exe` |

The Linux binaries are statically linked and do not require a system libc or
OpenSSL. Every release archive contains a single binary: `simple-tls` on Linux
and `simple-tls.exe` on Windows, so install scripts can use the same path on
each platform.

## Arguments

```text
      Client listen address        Server listen address
           |                            |
|Client|-->|simple-tls client|--TLS1.3-->|simple-tls server|-->|Destination|
                                        |                     |
                                   Client destination   Server destination

# Common arguments
  -b string
      [Host:Port] (required) Listen address.
  -d string
      [Host:Port] (required) Destination address.
  -grpc
      Use gRPC protocol. Client and server must match.
  -grpc-path string
      (optional) gRPC service name. Client and server must match.

# Client arguments
# e.g. simple-tls -b 127.0.0.1:1080 -d your_server_ip:1080 -n your.server.name

  -n string
      Server name. Used to verify the server certificate and as SNI.
  -no-verify
      Client will not verify the server's certificate chain.
  -ca string
      CA certificate file for server verification. (defaults to system pool)
  -cert-hash string
      Server certificate hash (certificate pinning).
      Use -hash-cert to generate a certificate hash.

# Server arguments
# e.g. simple-tls -b :1080 -d 127.0.0.1:12345 -s -key /path/to/your/key -cert /path/to/your/cert
# Certificate format must be PEM (base64).
# -cert and -key can be left empty, a temporary certificate will be generated in memory.
# Certificate domain defaults to random, but can be taken from the -n parameter.
# e.g. simple-tls -b :1080 -d 127.0.0.1:12345 -s -n my.test.domain

  -s
      (required) Run as server.
  -cert string
      Certificate path.
  -key string
      Key path.

# Other common arguments

  -t int
      Connection idle timeout in seconds (default 300).
  -outbound-buf int
      Set outbound TCP socket buffer size.
  -inbound-buf int
      Set inbound TCP socket buffer size.
  -cpu int
      Number of worker threads (default: number of CPUs).

# Commands

  -gen-cert
      Generate an ECC certificate with a 256-bit key in the current directory.
      Certificate DNS name can be set with -n. Default is a random string.
      Can use -template to specify a template certificate. DNS names, IP SANs,
      common name and validity start are copied from the template.
      Can use -cert and -key to specify output paths.
      e.g. simple-tls -gen-cert -n my.domain
      Will generate my.domain.cert and my.domain.key files.
  -hash-cert
      Display certificate hashes (for the client's -cert-hash).
      e.g. simple-tls -hash-cert ./my.cert
  -v
      Display program version.
  -vv
      Verbose log.
```

## Quick start without a valid certificate

The server uses a temporary certificate, the client does not verify it. Use
this when the underlying connection already has security measures.

```shell
# Server: leave -cert and -key empty, generates a temporary certificate in memory.
simple-tls -b :1080 -d 127.0.0.1:12345 -s -n my.cert.domain
# Client: disable certificate chain verification.
simple-tls -b :1080 -d your.server.address:1080 -n my.cert.domain -no-verify
```

The server uses a fixed certificate, the client uses hash verification
(certificate pinning).

```shell
# Server: generate a certificate.
simple-tls -gen-cert -n my.cert.domain
# Then display the certificate hash. e.g. 8910fe28d2fb40398a...
simple-tls -hash-cert ./my.cert.domain.cert
# Start server with this certificate.
simple-tls -b :1080 -d 127.0.0.1:12345 -s -key ./my.cert.domain.key -cert ./my.cert.domain.cert
# Client: disable chain verification but enable hash verification.
simple-tls -b :1080 -d your.server.address:1080 -n my.cert.domain -no-verify -cert-hash 8910fe28d2fb40398a...
```

## Using as a SIP003 plugin

Supports the Shadowsocks [SIP003](https://shadowsocks.org/en/wiki/Plugin.html)
plugin protocol. The Shadowsocks main program automatically sets the listen
address `-b` and destination address `-d`.

Example with [shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust):

```shell
ssserver -c config.json --plugin simple-tls --plugin-opts "s;key=/path/to/your/key;cert=/path/to/your/cert"
sslocal -c config.json --plugin simple-tls --plugin-opts "n=your.server.certificates.dnsname"
```

## Building from source

A recent stable Rust toolchain is required.

```shell
cargo build --release
```

For static Linux binaries, cross-compile with
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) and Zig:

```shell
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo install cargo-zigbuild
cargo zigbuild --release --target x86_64-unknown-linux-musl
cargo zigbuild --release --target aarch64-unknown-linux-musl
```

The resulting binaries are `target/<target>/release/simple-tls`.

## Tests

```shell
cargo test
```

The integration tests start a local echo server and exercise raw TLS and gRPC
tunnels, including certificate pinning.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
