use std::collections::HashMap;
use std::future::{Future, Ready};
use std::io;
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bincode::config::Configuration;
use bincode::{Decode, Encode};
use bytes::BufMut;
use futures::Stream;
use futures_sink::Sink;
use futures_util::future::poll_fn;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};
use tower::Service;
use webrtc_dtls::conn::DTLSConn;
use webrtc_util::conn::Listener;
use webrtc_util::Conn;

pub(crate) type StdError = Box<dyn std::error::Error + Send + Sync + 'static>;

type DtlsListener = dyn Listener + Send + Sync;

pub struct DtlsConnStream {
    rx1: mpsc::UnboundedReceiver<Result<(Vec<u8>, SocketAddr), StdError>>,
    tx2: mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>,
}

const BUF_SIZE: usize = 64 * 1_024;

impl DtlsConnStream {
    pub fn new_client(dtls_conn: Arc<DTLSConn>, l_addr: SocketAddr) -> Self {
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel::<(Vec<u8>, SocketAddr)>();
        let (ktx, mut krx) = mpsc::unbounded_channel();

        let tx = tx1.clone();
        let dc2 = dtls_conn.clone();
        tokio::spawn(async move {
            loop {
                let r = tokio::select! {
                    r = rx2.recv() => r,
                    _ = krx.recv() => break
                };

                match r {
                    None => {}
                    Some(x) => {
                        let _ = match dc2.send(x.0.as_slice()).await {
                            Ok(_) => {}
                            Err(e) => {
                                log::error!("client_send: {}", e);
                                break;
                            }
                        };
                    }
                }
            }
            log::debug!("client write loop exiting {}", l_addr)
        });

        tokio::spawn(async move {
            loop {
                let l_addr = l_addr.clone();
                let tx = tx.clone();
                let mut buf = BytesMut::zeroed(BUF_SIZE);

                let timeout = Duration::from_millis(3_000);
                match dtls_conn.clone().read(&mut buf, Some(timeout)).await {
                    Ok(s) => match tx.send(Ok((buf[0..s].to_vec(), l_addr))) {
                        Ok(_) => {}
                        Err(e) => {
                            log::error!("client_read: {}", e);
                            break;
                        }
                    },
                    Err(e) => {
                        log::error!("client_read: {}", e);
                        break;
                    }
                }
            }
            match ktx.send(()) {
                Ok(_) => {}
                Err(e) => log::error!("cr {}", e),
            };
            log::debug!("client read loop exiting {}", l_addr)
        });

        Self { rx1, tx2 }
    }

    pub fn new_server(listener: Box<DtlsListener>) -> Self {
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, rx2) = mpsc::unbounded_channel::<(Vec<u8>, SocketAddr)>();

        let router = ClientConnRouter::new(rx2);

        tokio::spawn(async move {
            loop {
                let tx = tx1.clone();
                let (conn, addr) = listener.accept().await.unwrap();
                let r_conn = conn.clone();
                let r_conn_router = router.clone();
                let (ktx, mut krx) = mpsc::unbounded_channel();
                tokio::spawn(async move {
                    let timeout = Duration::from_millis(3_000);
                    loop {
                        let mut buf = BytesMut::zeroed(BUF_SIZE);
                        let timer = tokio::time::sleep(timeout);
                        tokio::pin!(timer);
                        let r = tokio::select! {
                            r = r_conn.recv(&mut buf) => r,
                            _ = timer.as_mut() => break
                        };

                        match r {
                            Ok(s) => match tx.send(Ok((buf[0..s].to_vec(), addr))) {
                                Ok(_) => {}
                                Err(e) => log::error!("server_read[{}]: {}", addr, e),
                            },
                            Err(e) => log::error!("server_read[{}]: {}", addr, e),
                        }
                    }
                    r_conn_router.remove(addr);
                    ktx.send(()).unwrap();
                    log::debug!("server_read loop for {} exiting", addr);
                });
                let t_conn = conn.clone();
                let (tx3, mut rx3) = mpsc::unbounded_channel();

                router.insert(addr, tx3.clone());

                tokio::spawn(async move {
                    loop {
                        let r = tokio::select! {
                            r = rx3.recv() => r,
                            _ = krx.recv() => break
                        };

                        match r {
                            Some(d) => {
                                t_conn.send(d.0.as_slice()).await.unwrap();
                            }
                            None => {}
                        }
                    }
                    log::debug!("server_write loop for {} exiting", addr);
                });
            }
        });

        Self { rx1, tx2 }
    }
}

#[derive(Clone)]
struct ClientConnRouter {
    ttx: mpsc::UnboundedSender<ClientConnsMsg>,
}

impl ClientConnRouter {
    fn new(mut rx2: mpsc::UnboundedReceiver<(Vec<u8>, SocketAddr)>) -> Self {
        let (rtx, mut rrx) = mpsc::unbounded_channel::<ClientConnsMsg>();
        let mut conns: HashMap<SocketAddr, mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>> =
            HashMap::new();

        tokio::spawn(async move {
            loop {
                let txx = rrx.recv().await.unwrap();
                match txx {
                    ClientConnsMsg::Insert(k, v) => {
                        log::debug!("tracking client[{}]: {}", conns.len(), k);
                        conns.insert(k, v.clone());
                    }
                    ClientConnsMsg::Get(k, stx) => {
                        let v = conns.get(&k).unwrap();
                        stx.send(v.clone()).unwrap()
                    }
                    ClientConnsMsg::Remove(k) => {
                        let _stx = conns.remove(&k).unwrap();
                        log::debug!("removed {}", k)
                    }
                }
            }
        });

        let rtx2 = rtx.clone();
        tokio::spawn(async move {
            loop {
                let pkt = rx2.recv().await.unwrap();
                let rtx2 = rtx2.clone();
                tokio::spawn(async move {
                    let (otx, orx) = oneshot::channel();
                    rtx2.send(ClientConnsMsg::Get(pkt.1, otx)).unwrap();
                    let stx = orx.await.unwrap();
                    stx.send(pkt).unwrap();
                });
            }
        });

        Self { ttx: rtx }
    }

    fn insert(&self, addr: SocketAddr, tx: mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>) {
        self.ttx.send(ClientConnsMsg::Insert(addr, tx)).unwrap()
    }

    fn remove(&self, addr: SocketAddr) {
        self.ttx.send(ClientConnsMsg::Remove(addr)).unwrap()
    }
}

enum ClientConnsMsg {
    Insert(SocketAddr, mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>),
    Get(
        SocketAddr,
        oneshot::Sender<mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>>,
    ),
    Remove(SocketAddr),
}

impl Stream for DtlsConnStream {
    type Item = Result<(Vec<u8>, SocketAddr), StdError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let pin = &mut self.get_mut();
        match pin.rx1.poll_recv(cx) {
            Poll::Ready(o) => match o {
                None => Poll::Ready(None),
                Some(y) => Poll::Ready(Some(y)),
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Sink<(Vec<u8>, SocketAddr)> for DtlsConnStream {
    type Error = StdError;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: (Vec<u8>, SocketAddr)) -> Result<(), Self::Error> {
        let x = self.tx2.send(item).unwrap();
        Ok(x)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

pub struct CodecStream<C> {
    codec: C,
    stream: DtlsConnStream,
}

impl<C> CodecStream<C> {
    pub fn new(codec: C, stream: DtlsConnStream) -> Self {
        CodecStream { codec, stream }
    }
}

impl<C> Stream for CodecStream<C>
where
    C: Decoder + Unpin,
{
    type Item = Result<(C::Item, SocketAddr), StdError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let pin = self.get_mut();

        let poll = pin.stream.poll_next_unpin(cx);
        if let Poll::Ready(Some(Ok((d, a)))) = poll {
            let mut buf = BytesMut::new();
            buf.put(d.as_slice());

            if let Ok(Some(frame)) = pin.codec.decode_eof(&mut buf) {
                Poll::Ready(Some(Ok((frame, a))))
            } else {
                Poll::Ready(None)
            }
        } else {
            Poll::Pending
        }
    }
}

impl<I, C> Sink<(I, SocketAddr)> for CodecStream<C>
where
    C: Encoder<I> + Unpin,
{
    type Error = StdError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let pin = self.get_mut();
        pin.stream.poll_ready_unpin(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: (I, SocketAddr)) -> Result<(), Self::Error> {
        let pin = self.get_mut();
        let mut buf = BytesMut::new();
        if let Ok(_) = pin.codec.encode(item.0, &mut buf) {
            pin.stream.start_send_unpin((buf.to_vec(), item.1))
        } else {
            Err(
                StdError::try_from(std::io::Error::new(ErrorKind::Other, "start send error"))
                    .unwrap(),
            )
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let pin = self.get_mut();
        pin.stream.poll_flush_unpin(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let pin = self.get_mut();
        pin.stream.poll_close_unpin(cx)
    }
}

#[derive(Decode, Encode, Clone)]
pub enum UdpPayload<Request, Response> {
    Request(Request),
    Response(Response),
}

pub struct UdpPayloadEncoderDecoder<T: Decode + Encode> {
    conf: Configuration,
    marker: PhantomData<T>,
}

impl<T: Decode + Encode> UdpPayloadEncoderDecoder<T> {
    pub fn new(conf: Configuration) -> Self {
        UdpPayloadEncoderDecoder {
            conf,
            marker: PhantomData,
        }
    }
}

impl<T: Decode + Encode> Decoder for UdpPayloadEncoderDecoder<T> {
    type Item = T;
    type Error = StdError;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        decode(self.conf, buf)
    }
}

impl<T: Encode + Decode> Encoder<T> for UdpPayloadEncoderDecoder<T> {
    type Error = StdError;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        encode(self.conf, item, dst)?;
        Ok(())
    }
}

fn decode<T: Decode>(conf: Configuration, buf: &mut BytesMut) -> Result<Option<T>, StdError> {
    if buf.is_empty() {
        return Ok(None);
    }
    let (r, _) = bincode::decode_from_slice(&buf, conf)?;
    buf.clear();
    Ok(Some(r))
}

fn encode<T: Encode>(conf: Configuration, item: T, dst: &mut BytesMut) -> Result<(), StdError> {
    match bincode::encode_to_vec(&item, conf) {
        Err(e) => Err(StdError::try_from(e).unwrap()),
        Ok(data) => {
            dst.reserve(data.len());
            dst.extend(data);
            Ok(())
        }
    }
}

pub struct RequestHandlerService<S, IN, OUT> {
    inner: S,
    req: PhantomData<IN>,
    resp: PhantomData<OUT>,
}

impl<S, IN, OUT> RequestHandlerService<S, IN, OUT> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            req: Default::default(),
            resp: Default::default(),
        }
    }
}

impl<S, IN, OUT> Service<(UdpPayload<IN, OUT>, SocketAddr)> for RequestHandlerService<S, IN, OUT>
where
    S: Service<IN, Response = OUT, Error = StdError, Future = Ready<Result<OUT, StdError>>> + Send,
    IN: Send + 'static,
    OUT: Send + 'static,
{
    type Response = (UdpPayload<IN, OUT>, SocketAddr);
    type Error = S::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<(UdpPayload<IN, OUT>, SocketAddr), StdError>>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: (UdpPayload<IN, OUT>, SocketAddr)) -> Self::Future {
        let p = req.0;
        let addr = req.1.clone();

        match p {
            UdpPayload::Request(ireq) => {
                let inner_f = self.inner.call(ireq);
                Box::pin(async move {
                    match inner_f.await {
                        Ok(x) => Ok((UdpPayload::Response(x), addr)),
                        Err(e) => Err(e),
                    }
                })
            }
            UdpPayload::Response(_) => {
                Box::pin(async move { Err("doesn't handle  UdpPayload::Response ".into()) })
            }
        }
    }
}

pub struct DtlsClient<S, T> {
    stream: S,
    addr: SocketAddr,
    m: PhantomData<T>,
}

impl<S, T> DtlsClient<S, T> {
    pub fn new(stream: S, addr: SocketAddr) -> Self {
        DtlsClient {
            stream,
            addr,
            m: Default::default(),
        }
    }
}

impl<S, T> DtlsClient<S, T>
where
    T: Encode + Decode + Unpin,
    S: Sink<(T, SocketAddr), Error = StdError> + Unpin,
    S: Stream<Item = Result<(T, SocketAddr), StdError>>,
{
    pub async fn send_recv(&mut self, req: T) -> Result<T, StdError> {
        self.stream.send((req, self.addr.clone())).await.unwrap();

        let d = poll_fn(|cx| match self.stream.poll_next_unpin(cx) {
            Poll::Ready(x) => match x {
                None => Poll::Pending,
                Some(r) => match r {
                    Ok(r) => Poll::Ready(Ok(r)),
                    Err(e) => Poll::Ready(Err(io::Error::new(ErrorKind::Other, e))),
                },
            },
            Poll::Pending => Poll::Pending,
        })
        .await
        .unwrap();
        Ok(d.0)
    }
}
