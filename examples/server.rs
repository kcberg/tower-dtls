use std::env::args;
use std::future::ready;
use std::io::Write;
use std::process::exit;

use tokio_tower::pipeline::Server;
use tower::ServiceBuilder;
use webrtc_dtls::config::Config;
use webrtc_dtls::crypto::Certificate;
use webrtc_dtls::extension::extension_use_srtp::SrtpProtectionProfile;

extern crate tower_dtls;

use crate::tower_dtls::{
    CodecStream, DtlsConnStream, RequestHandlerService, StdError, UdpPayload,
    UdpPayloadEncoderDecoder,
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
        run_udps_server(addr).await;
    } else {
        log::error!("please provide the address ex: 0.0.0.0:9999");
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
