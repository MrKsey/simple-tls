# simple-tls

Simple and easy-to-use TCP connection forwarder. Adds a TLS layer to raw data streams. Supports gRPC transport.

**Optimized for MIPS/ARM routers (Keenetic, etc.) - see [MIPS_OPTIMIZATION.md](MIPS_OPTIMIZATION.md)**

---

## Available Builds (build/)

| Platform | File | Size |
|----------|------|------|
| **Linux ARM64** | `simple-tls-linux-arm64` | 10.44 MB |
| **Linux AMD64** | `simple-tls-linux-amd64` | 11.11 MB |
| **Linux MIPS LE** | `simple-tls-linux-mipsle-softfloat` | 12.06 MB |
| **Windows AMD64** | `simple-tls-windows-amd64.exe` | 11.44 MB |
| **Windows ARM64** | `simple-tls-windows-arm64.exe` | 10.51 MB |

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
      Server name. Used to verify server certificate and as SNI.
  -no-verify
      Client will not verify the server's certificate chain.
  -ca string
      CA certificate file for server verification. (defaults to system pool)
  -cert-hash string
      Server certificate hash (certificate pinning).
      tips: use -hash-cert command to generate certificate hash

# Server arguments
# e.g. simple-tls -b :1080 -d 127.0.0.1:12345 -s -key /path/to/your/key -cert /path/to/your/cert
# Certificate format must be PEM (base64).
# -cert and -key can be left empty, a temporary certificate will be generated in memory.
# Certificate domain defaults to random, but can be taken from `-n` parameter.
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
      Set outbound TCP rw socket buffer.
  -inbound-buf    
      Set inbound TCP rw socket buffer.

# Commands

  -gen-cert
      Generate an ECC certificate with 256-bit key to current directory.
      Certificate DNS name can be set with `-n`. Default is random string.
      Can use `-template` to specify a template certificate. All parameters except key will be copied from template.
      Can use `-cert` and `-key` to specify output paths. (default: current directory, filename is DNS name)
      e.g. simple-tls -gen-cert -n my.domain
      Will generate my.domain.cert and my.domain.key files in current directory.
  -hash-cert
      Display certificate hash. (for client's -cert-hash)
      e.g. simple-tls -hash-cert ./my.cert
  -v
      Display program version
```

## Quick Start Without Valid Certificate

Server uses a temporary certificate, client does not verify. Use this when the underlying connection has security measures.

```shell
# Server: leave -cert and -key empty, generates a temporary certificate in memory.
simple-tls -b :1080 -d 127.0.0.1:12345 -s -n my.cert.domain
# Client: disable certificate chain verification.
simple-tls -b :1080 -d your.server.address:1080 -n my.cert.domain -no-verify
```

Server uses a fixed certificate, client uses hash verification (certificate pinning).

```shell
# Server: generate a certificate.
simple-tls -gen-cert -n my.cert.domain
# Then display the certificate hash. e.g. 8910fe28d2fb40398a...
simple-tls -hash-cert ./my.cert.domain.cert
# Start server with this certificate
simple-tls -b :1080 -d 127.0.0.1:12345 -s -key ./my.cert.domain.key -cert ./my.cert.domain.cert
# Client: disable certificate chain verification but enable hash verification.
simple-tls -b :1080 -d your.server.address:1080 -n my.cert.domain -no-verify -cert-hash 8910fe28d2fb40398a...
```

## Using as SIP003 Plugin

Supports Shadowsocks [SIP003](https://shadowsocks.org/en/wiki/Plugin.html) plugin protocol. Shadowsocks main program automatically sets listen address `-b` and destination address `-d`.

Example with [shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust):

```shell
ssserver -c config.json --plugin simple-tls --plugin-opts "s;key=/path/to/your/key;cert=/path/to/your/cert"
sslocal -c config.json --plugin simple-tls --plugin-opts "n=your.server.certificates.dnsname"
```

### Android SIP003 Plugin

simple-tls-android is a GUI plugin for [shadowsocks-android](https://github.com/shadowsocks/shadowsocks-android). It is released together with simple-tls. You can download the universal APK from the release page.

simple-tls-android source code is [here](https://github.com/IrineSistiana/simple-tls-android).

### Beta Version

simple-tls does not guarantee compatibility between versions at this time.