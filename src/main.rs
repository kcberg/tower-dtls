use std::env::args;
use std::net::SocketAddr;
use std::process::exit;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bincode::{Decode, Encode};
use futures_util::future::{poll_fn, ready, Ready};
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tokio_tower::pipeline::{Client, Server};
use tokio_util::udp::UdpFramed;
use tower::{Service, ServiceBuilder};

use crate::udp::{BincodeEncoderDecoder, StdError, UdpPayLoadLayer, UdpPayload};

mod udp;

#[tokio::main]
async fn main() {
    let args: Vec<String> = args().collect();
    let bincode_conf = bincode::config::standard();

    if args[1] == "client" {
        println!("client!");
        let addr = SocketAddr::from_str("127.0.0.1:9999").unwrap();
        let c_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let c_sock = Arc::new(c_sock);

        let codec =
            BincodeEncoderDecoder::<UdpPayload<EchoRequest, EchoResponse>>::new(bincode_conf);
        let framed = UdpFramed::new(c_sock.clone(), codec);
        let mut client = Client::<_, tokio_tower::Error<_, _>, _>::new(framed);

        for i in 0..100 {
            poll_fn(|cx| client.poll_ready(cx)).await.unwrap();
            let req = UdpPayload::Request(EchoRequest(format!("hey hey hey hey {}", i)));
            let ret = match client.call((req, addr)).await {
                Ok(e) => Some(e),
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
    } else if args[1] == "server" {
        let sock = UdpSocket::bind("0.0.0.0:9999".parse::<SocketAddr>().unwrap())
            .await
            .unwrap();
        let sock = Arc::new(sock);

        let codec =
            BincodeEncoderDecoder::<UdpPayload<EchoRequest, EchoResponse>>::new(bincode_conf);
        let framed = UdpFramed::new(sock.clone(), codec);

        let svc = ServiceBuilder::new()
            .layer(UdpPayLoadLayer::new())
            .service(EchoService);

        match Server::new(framed, svc).await {
            Ok(_) => {}
            Err(e) => {
                println!("Aww crap: {}", e)
            }
        }
    } else {
        println!("no");
        exit(1)
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
