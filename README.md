
## tower-dtls 

A [DTLS](https://en.wikipedia.org/wiki/Datagram_Transport_Layer_Security) client and server library 
compatible with [tower](https://github.com/tower-rs/tower) and its `async fn(Request) -> Result<Response, Error>` interfaces. 
The DTLS implementation is provided by the [webrtc](https://github.com/webrtc-rs/webrtc) 
project's [webrtc-dtls](https://crates.io/crates/webrtc-dtls) crate which uses [rustls](https://github.com/rustls/rustls).

See the [server](./examples/server.rs) and [client](./examples/client.rs) examples for usage details. 

### Build static library
```shell
cargo build --target=x86_64-unknown-linux-musl --release
```

### Run examples
```shell
cargo run --example server 0.0.0.0:9999
```

```shell
cargo run --example client 127.0.0.1:9999
```

### TODO's
1. Add `DtlsServer` wrapper around tokio-tower or perhaps make one 🤔
1. Documentation
1. Publish to crates.io
1. Add an HTTP/3 example