use std::env::args;
use std::future::ready;
use std::io::Write;
use std::net::SocketAddr;
use std::process::exit;
use std::str::FromStr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio_tower::pipeline::Server;
use tower::ServiceBuilder;
use webrtc_dtls::config::Config;
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::Certificate;
use webrtc_dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use webrtc_util::Conn;

use crate::udp::{
    CodecStream, DtlsClient, DtlsConnStream, RequestHandlerService, StdError, UdpPayload,
    UdpPayloadEncoderDecoder,
};

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
    let addr: String = args[2].clone();

    if args[1] == "client" {
        run_udps_client(addr).await
    } else if args[1] == "server" {
        run_udps_server(addr).await
    } else {
        log::error!("oh no");
        exit(1)
    }
}

pub async fn run_udps_server(bind_addr: String) {
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
    let dtls_stream = DtlsConnStream::new_server(Box::new(dtls_listener));

    let bincode_conf = bincode::config::standard();
    let str_codec =
        UdpPayloadEncoderDecoder::<UdpPayload<String, String>>::new(bincode_conf.clone());
    let cs = CodecStream::new(str_codec, dtls_stream);

    let svc = ServiceBuilder::new()
        .layer_fn(|inner| RequestHandlerService::new(inner))
        .service_fn(|str: String| {
            log::info!("client says: {}", str);
            let resp = format!("resp: {}", str);
            ready(Ok::<_, StdError>(resp))
        });

    Server::new(cs, svc).await.unwrap();
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

    let dtls_stream = DtlsConnStream::new_client(Arc::new(dtls_conn), l_addr);
    let codec_stream = CodecStream::new(str_codec, dtls_stream);

    let mut client = DtlsClient::new(codec_stream, l_addr);

    for i in 0..1000 {
        let msg = format!("howdy {}", i);
        let d = client.send_recv(UdpPayload::Request(msg)).await.unwrap();

        log::info!("blah");

        let data = match d {
            UdpPayload::Request(_) => "".to_string(),
            UdpPayload::Response(d) => d,
        };
        log::info!("server resp[{}]: {}", l_addr, data);
    }
}
