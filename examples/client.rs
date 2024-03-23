use std::env::args;
use std::io::Write;
use std::net::SocketAddr;
use std::process::exit;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tower::ServiceBuilder;
use webrtc_dtls::config::Config;
use webrtc_dtls::conn::DTLSConn;
use webrtc_dtls::crypto::Certificate;
use webrtc_dtls::extension::extension_use_srtp::SrtpProtectionProfile;
use webrtc_util::Conn;

extern crate tower_dtls;

use crate::tower_dtls::{
    CodecStream, DtlsClient, DtlsConnStream, UdpPayload, UdpPayloadEncoderDecoder,
};

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

    let args = args();
    if args.len() == 2 {
        let args: Vec<String> = args.collect();
        let addr: String = args[1].clone();
        run_udps_client(addr).await;
    } else {
        log::error!("please provide the address ex: 0.0.0.0:9999");
        exit(1)
    }
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

    let svc = ServiceBuilder::new().service(codec_stream);

    let mut client = DtlsClient::new(svc, l_addr);

    for i in 0..1000 {
        let msg = format!("howdy {}", i);
        let data = client.send_recv(msg).await.unwrap();
        log::info!("server resp[{}]: {}", l_addr, data);
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}
