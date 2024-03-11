use bincode::config::Configuration;
use bincode::{Decode, Encode};
use futures_util::future::Ready;
use std::future::Future;
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};
use tower::{Layer, Service};

pub(crate) type StdError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub struct UdpPayLoadService<S, IN, OUT> {
    inner: S,
    req: PhantomData<IN>,
    resp: PhantomData<OUT>,
}

impl<S, IN, OUT> Service<(UdpPayload<IN, OUT>, SocketAddr)> for UdpPayLoadService<S, IN, OUT>
where
    S: Service<IN, Response = OUT, Error = StdError, Future = Ready<Result<OUT, StdError>>> + Send,
    IN: Send + 'static,
    OUT: Send + 'static,
{
    type Response = (UdpPayload<IN, OUT>, SocketAddr);
    type Error = StdError;
    type Future = Pin<
        Box<
            dyn Future<Output = Result<(UdpPayload<IN, OUT>, SocketAddr), StdError>>
                + Send
                + 'static,
        >,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: (UdpPayload<IN, OUT>, SocketAddr)) -> Self::Future {
        let addr = req.1;
        let req = req.0;
        let re_opt = match req {
            UdpPayload::Request(r) => Some(r),
            UdpPayload::Response(_) => None,
        };

        if re_opt.is_some() {
            let resp_c = self.inner.call(re_opt.unwrap());
            let f = async move {
                let resp = resp_c.await.unwrap();
                println!("called inner service");
                Ok((UdpPayload::Response(resp), addr))
            };
            return Box::pin(f);
        } else {
            let err = std::io::Error::new(ErrorKind::Unsupported, "not a request".to_string());
            let err_f = futures_util::future::err::<_, StdError>(StdError::try_from(err).unwrap());
            return Box::pin(err_f);
        }
    }
}

pub struct UdpPayLoadLayer<IN, OUT> {
    req: PhantomData<IN>,
    resp: PhantomData<OUT>,
}

impl<IN, OUT> UdpPayLoadLayer<IN, OUT> {
    pub fn new() -> Self {
        Self {
            req: Default::default(),
            resp: Default::default(),
        }
    }
}

impl<S, IN, OUT> Layer<S> for UdpPayLoadLayer<IN, OUT> {
    type Service = UdpPayLoadService<S, IN, OUT>;

    fn layer(&self, inner: S) -> Self::Service {
        UdpPayLoadService {
            inner,
            req: Default::default(),
            resp: Default::default(),
        }
    }
}

#[derive(Decode, Encode, Clone)]
pub enum UdpPayload<Request, Response> {
    Request(Request),
    Response(Response),
}

pub struct BincodeEncoderDecoder<T: Decode + Encode> {
    conf: Configuration,
    marker: PhantomData<T>,
}

impl<T: Decode + Encode> BincodeEncoderDecoder<T> {
    pub fn new(conf: Configuration) -> Self {
        BincodeEncoderDecoder {
            conf,
            marker: PhantomData,
        }
    }
}

impl<T: Decode + Encode> Decoder for BincodeEncoderDecoder<T> {
    type Item = T;
    type Error = StdError;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if buf.is_empty() {
            return Ok(None);
        }
        let ret = match bincode::decode_from_slice(&buf, self.conf) {
            Err(e) => {
                println!("decode err: {}", e);
                None
            }
            Ok((r, _)) => {
                buf.clear();
                Some(r)
            }
        };
        Ok(ret)
    }
}

impl<T: Encode + Decode> Encoder<T> for BincodeEncoderDecoder<T> {
    type Error = StdError;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(match bincode::encode_to_vec(&item, self.conf) {
            Err(e) => {
                println!("encode err: {}", e)
            }
            Ok(data) => {
                dst.reserve(data.len());
                dst.extend(data);
            }
        })
    }
}
