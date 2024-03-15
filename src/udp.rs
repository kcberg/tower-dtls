use std::collections::HashMap;
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bincode::config::Configuration;
use bincode::{Decode, Encode};
use bytes::BufMut;
use futures::Stream;
use futures_sink::Sink;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder};
use webrtc_dtls::conn::DTLSConn;
use webrtc_util::conn::Listener;
use webrtc_util::Conn;

pub(crate) type StdError = Box<dyn std::error::Error + Send + Sync + 'static>;

type DtlsListener = dyn Listener + Send + Sync;

pub struct DtlsConnStream {
    rx1: mpsc::UnboundedReceiver<Result<(Vec<u8>, SocketAddr), StdError>>,
    tx2: mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>,
}

impl DtlsConnStream {
    pub fn new_client(dtls_conn: Arc<DTLSConn>, l_addr: SocketAddr) -> Self {
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel::<(Vec<u8>, SocketAddr)>();

        let tx = tx1.clone();
        let dc2 = dtls_conn.clone();
        tokio::spawn(async move {
            loop {
                match rx2.recv().await {
                    None => {}
                    Some(x) => {
                        let _ = dc2.send(x.0.as_slice()).await.unwrap();
                    }
                }
            }
        });

        tokio::spawn(async move {
            loop {
                let l_addr = l_addr.clone();
                let tx = tx.clone();
                let mut buf = BytesMut::zeroed(1500);
                let s = dtls_conn.clone().recv(&mut buf).await.unwrap();
                tx.send(Ok((buf[0..s].to_vec(), l_addr))).unwrap();
            }
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
                tokio::spawn(async move {
                    loop {
                        let mut buf = BytesMut::zeroed(1500);
                        let s = r_conn.recv(&mut buf).await.unwrap();
                        tx.send(Ok((buf[0..s].to_vec(), addr))).unwrap()
                    }
                });
                let t_conn = conn.clone();
                let (tx3, mut rx3) = mpsc::unbounded_channel();

                router.insert(addr, tx3.clone());

                tokio::spawn(async move {
                    loop {
                        match rx3.recv().await {
                            Some(d) => {
                                t_conn.send(d.0.as_slice()).await.unwrap();
                            }
                            None => {}
                        }
                    }
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
                        conns.insert(k, v.clone());
                    }
                    ClientConnsMsg::Get(k, stx) => {
                        let x = conns.get(&k).unwrap();
                        stx.send(x.clone()).unwrap()
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
}

enum ClientConnsMsg {
    Insert(SocketAddr, mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>),
    Get(
        SocketAddr,
        oneshot::Sender<mpsc::UnboundedSender<(Vec<u8>, SocketAddr)>>,
    ),
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
        encode(self.conf, item, dst).unwrap();
        Ok(())
    }
}

fn decode<T: Decode>(conf: Configuration, buf: &mut BytesMut) -> Result<Option<T>, StdError> {
    if buf.is_empty() {
        return Ok(None);
    }
    let ret = match bincode::decode_from_slice(&buf, conf) {
        Err(_e) => {
            // println!("decode err: {}", e);
            None
        }
        Ok((r, _)) => {
            buf.clear();
            Some(r)
        }
    };
    Ok(ret)
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
