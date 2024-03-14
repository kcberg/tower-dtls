use crate::udp::{
    Base64Cryptor, CodecStream, DtlsConnStream, Noopryptor, StdError, StreamWrapper,
    UdpPayLoadLayer, UdpPayLoadService, UdpPayload, UdpPayloadEncoderDecoder,
};
use bincode::{Decode, Encode};
use bytes::BytesMut;
use futures_util::future::{poll_fn, ready, Ready};
use futures_util::{FutureExt, TryFutureExt};
use openssl::ssl::{Ssl, SslContextRef};
use std::collections::HashMap;
use std::fmt::{format, Write};
use std::future::Future;
use std::io::Read;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tokio_openssl::SslStream;
use tokio_tower::pipeline::{Client, Server};
use tokio_util::udp::UdpFramed;
use tower::{Layer, Service, ServiceBuilder};
use webrtc_dtls::config::Config;
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::Certificate;
use webrtc_dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use webrtc_dtls::state::State;
use webrtc_util::conn::conn_udp_listener::ListenConfig;
use webrtc_util::conn::Listener;
use webrtc_util::Conn;

pub async fn run_echo_client(addr: String) {
    println!("client!");

    let bincode_conf = bincode::config::standard();
    let addr = SocketAddr::from_str(addr.as_str()).unwrap();
    let c_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let c_sock = Arc::new(c_sock);

    // let x = c_sock
    //     .clone()
    //     .send_to("HANDSAKE".as_bytes(), addr.clone())
    //     .await
    //     .unwrap();

    let bc = Base64Cryptor::new();
    let codec =
        UdpPayloadEncoderDecoder::<UdpPayload<EchoRequest, EchoResponse>, Base64Cryptor>::new(
            bincode_conf,
            bc,
        );
    let framed = UdpFramed::new(c_sock.clone(), codec);
    let client = Client::<_, tokio_tower::Error<_, _>, _>::new(framed);
    let mut client = ServiceBuilder::new()
        // .layer(UdpPayLoadLayer::new())
        // .layer(CryptLayer::new())
        .service(client);

    for i in 0..100 {
        poll_fn(|cx| client.poll_ready(cx)).await.unwrap();
        let req = UdpPayload::Request(EchoRequest(format!("hey hey hey hey {}", i)));

        let ret = match client.call((req, addr)).await {
            Ok(p) => Some(p),
            Err(e) => {
                println!("err: {}", e);
                None
            }
        };

        if let Some(ret) = ret {
            match ret.0 {
                UdpPayload::Response(p) => {
                    println!("resp: {}", p.0);
                }
                _ => {}
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
}

pub async fn run_echo_server(bind_addr: String) {
    let bincode_conf = bincode::config::standard();
    let sock = UdpSocket::bind(bind_addr.parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let sock = Arc::new(sock);

    let bc = Base64Cryptor::new();
    let codec = UdpPayloadEncoderDecoder::new(bincode_conf, bc);
    let framed = UdpFramed::new(sock.clone(), codec);
    let framed = StreamWrapper::new(framed);

    let svc = ServiceBuilder::new()
        .layer(UdpPayLoadLayer::new())
        .layer(CryptLayer::new())
        .service(EchoService);

    match Server::new(framed, svc).await {
        Ok(_) => {}
        Err(e) => {
            println!("Aww crap: {}", e)
        }
    }
}


#[derive(Clone)]
pub struct CryptService<S, IN, OUT> {
    inner: S,
    req: PhantomData<IN>,
    resp: PhantomData<OUT>,
}

impl<S, IN, OUT> Service<IN> for CryptService<S, IN, OUT>
where
    S: Service<IN, Response = OUT, Error = StdError, Future = Ready<Result<OUT, StdError>>> + Send,
    IN: Send + 'static,
    OUT: Send + 'static,
{
    type Response = OUT;
    type Error = StdError;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: IN) -> Self::Future {
        self.inner.call(req)
    }
}

pub struct CryptLayer<IN, OUT> {
    req: PhantomData<IN>,
    resp: PhantomData<OUT>,
}

impl<IN, OUT> CryptLayer<IN, OUT> {
    pub fn new() -> Self {
        Self {
            req: Default::default(),
            resp: Default::default(),
        }
    }
}

impl<S, IN, OUT> Layer<S> for CryptLayer<IN, OUT> {
    type Service = CryptService<S, IN, OUT>;

    fn layer(&self, inner: S) -> Self::Service {
        CryptService {
            inner,
            req: Default::default(),
            resp: Default::default(),
        }
    }
}

#[derive(Clone, Decode, Encode)]
struct EchoRequest(String);

#[derive(Clone, Decode, Encode)]
struct EchoResponse(String);

#[derive(Clone)]
struct EchoService;

impl Service<EchoRequest> for EchoService {
    type Response = EchoResponse;
    type Error = StdError;
    type Future = Ready<Result<EchoResponse, StdError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: EchoRequest) -> Self::Future {
        println!("svc call: {}", req.0.clone());
        let ret_msg = format!("client says: {}", req.0);
        ready(Ok(EchoResponse(ret_msg)))
    }
}
