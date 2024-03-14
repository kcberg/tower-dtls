use std::env::args;
use std::io::Write;
use std::net::SocketAddr;
use std::process::exit;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::{poll_fn, ready};
use tokio::net::UdpSocket;
use tokio_tower::pipeline::{Client, Server};
use tower::{Service, ServiceBuilder};
use webrtc_dtls::config::Config;
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::Certificate;
use webrtc_dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use webrtc_util::Conn;

use crate::udp::{CodecStream, DtlsConnStream, StdError, UdpPayload, UdpPayloadEncoderDecoder};

// mod echo;
mod udp;

#[tokio::main]
async fn main() {
    env_logger::builder()
        .format(|buf, record| {
            writeln!(
                buf,
                "{}:{} {} [{}] - {}",
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .init();

    let args: Vec<String> = args().collect();

    if args[1] == "client" {
        // run_echo_client("127.0.0.1:9999".to_string()).await
        run_udps_client("127.0.0.1:9999".to_string()).await
    } else if args[1] == "server" {
        // run_echo_server("0.0.0.0:9999".to_string()).await
        run_udps_server("0.0.0.0:9999".to_string()).await
    } else {
        println!("no");
        exit(1)
    }
}

pub async fn run_udps_server(bind_addr: String) {

    let bincode_conf = bincode::config::standard();

    let mut dtls_conf = Config {
        insecure_skip_verify: true,
        srtp_protection_profiles: vec![SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80],
        ..Default::default()
    };

    let client_cert = Certificate::generate_self_signed(vec!["localhost".to_owned()]).unwrap();
    dtls_conf.certificates = vec![client_cert];

    let dtls_listener = webrtc_dtls::listener::listen(bind_addr, dtls_conf)
        .await
        .unwrap();

    let str_codec =
        UdpPayloadEncoderDecoder::<UdpPayload<String, String>>::new(bincode_conf.clone());
    let dtls_stream = DtlsConnStream::new_server(Box::new(dtls_listener));
    let cs = CodecStream::new(str_codec, dtls_stream);

    let svc = ServiceBuilder::new().service_fn(|x: (UdpPayload<String, String>, SocketAddr)| {
        let str = match x.0 {
            UdpPayload::Request(s) => s,
            UdpPayload::Response(_) => "".to_string(),
        };
        println!("client {} sent: {}", x.1.clone(), str);
        let resp = UdpPayload::Response(format!("resp: {}", str));
        ready(Ok::<_, StdError>((resp, x.1)))
    });

    Server::new(cs, svc).await.unwrap();

    println!("yeah")
}

pub async fn run_udps_client(addr: String) {
    let bincode_conf = bincode::config::standard();
    let addr = SocketAddr::from_str(addr.as_str()).unwrap();
    let c_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    c_sock.connect(addr.clone()).await.unwrap();
    let c_sock = Arc::new(c_sock);

    let mut dtls_conf = Config {
        insecure_skip_verify: true,
        srtp_protection_profiles: vec![SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80],
        ..Default::default()
    };

    let client_cert = Certificate::generate_self_signed(vec!["localhost".to_owned()]).unwrap();
    dtls_conf.certificates = vec![client_cert];

    let str_codec =
        UdpPayloadEncoderDecoder::<UdpPayload<String, String>>::new(bincode_conf.clone());

    let dtls_conn = DTLSConn::new(c_sock, dtls_conf, true, None).await.unwrap();
    let l_addr = dtls_conn.local_addr().unwrap().clone();
    let dtls_stream = DtlsConnStream::new_client(dtls_conn);
    let cs = CodecStream::new(str_codec, dtls_stream);

    let client = Client::<_, tokio_tower::Error<_, _>, _>::new(cs);
    let mut client = ServiceBuilder::new().service(client);

    for i in 0..100 {
        poll_fn(|cx| client.poll_ready(cx)).await.unwrap();
        let data = format!("hello {}", i);

        let ret = client
            .call((UdpPayload::Request(data), l_addr))
            .await
            .unwrap();

        match ret.0 {
            UdpPayload::Response(resp) => {
                println!("server says: {}", resp);
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    println!("cool")
}
