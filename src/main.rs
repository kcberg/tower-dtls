use std::env::args;
use std::net::{SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::pin::Pin;
use std::process::exit;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use async_bincode::tokio::AsyncBincodeStream;
use futures_util::future::{poll_fn, Ready, ready};
use futures_util::SinkExt;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use tokio_tower::pipeline::{Client, Server};
use tower::{Layer, Service, ServiceBuilder};
use udp_stream::{UdpListener, UdpStream};


#[tokio::main]
async fn main() {
    let args: Vec<String> = args().collect();

    if args[1] == "client" {
        println!("client!");
        let addr = SocketAddr::from_str("127.0.0.1:9999").unwrap();
        // let c_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let us = UdpStream::connect(addr).await.unwrap();

        // connect
        //let tx = TcpStream::connect(&addr).await.unwrap();
        // let tx = c_sock.connect(addr).await;
        let abs: AsyncBincodeStream<_, EchoResponse, _, _> = AsyncBincodeStream::from(us).for_async();
        let mut tx: Client<_, StdError, _> = Client::new(abs);

        if let Ok(_) = poll_fn(|cx| {
            (&mut tx).poll_ready(cx)
        }).await {
            let f1 = tx.call(EchoRequest("hey hey".to_string()));
            if let Ok(resp) = f1.await {
                println!("s: {}", resp.0);
            } else {
                println!("no resp?")
            }
        } else {
            unreachable!()
        }

        // us.shutdown();
    } else if args[1] == "server" {
        // let rx = TcpListener::bind("127.0.0.1:9999").await.unwrap();
        let rx = UdpListener::bind("127.0.0.1:9999".parse().unwrap()).await.unwrap();
        loop {

            // accept
            // let (rx, _) = rx.recv_from().await.unwrap();
            println!("udp accept");
            let (rx, peer) = rx.accept().await.unwrap();
            println!("handle udp: {}", peer);
            // let rx = UdpStream::from_tokio(rx).await.unwrap();
            let rx = AsyncBincodeStream::from(rx).for_async();

            let svc = ServiceBuilder::new()
                // .layer(EchoLayer)
                .service(EchoService);
            let server = Server::new(rx, svc);

            tokio::spawn(async move {
                server.await.unwrap();
                println!("ended?")
            });
        }
    } else {
        println!("no");
        exit(1)
    }
}

type StdError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Serialize, Deserialize)]
struct EchoRequest(String);

#[derive(Serialize, Deserialize)]
struct EchoResponse(String);

#[derive(Copy, Clone)]
struct EchoService;

impl Service<EchoRequest> for EchoService {
    type Response = EchoResponse;
    type Error = StdError;
    type Future = Ready<Result<EchoResponse, StdError>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        println!("svc ready");
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: EchoRequest) -> Self::Future {
        println!("svc call: {}", req.0.clone());
        let ret_msg = format!("client says: {}", req.0);
        ready(Ok(EchoResponse(ret_msg)))
    }
}
/*
#[derive(Clone)]
struct EchoUdpSvc<S, R, W, D> {
    inner: AsyncBincodeStream<S, R, W, D>
}

#[derive(Copy, Clone)]
struct EchoLayer<S, R, W, D>;

impl <S, R, W, D> Layer<AsyncBincodeStream<S, R, W, D>> for EchoLayer<S, R, W, D> {
    type Service = EchoUdpSvc<S, R, W, D>;

    fn layer(&self, inner: S)-> Self::Service {
        EchoUdpSvc{ inner }
    }
}*/