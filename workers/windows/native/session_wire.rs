//! The resource host's private protocol uses generated protobuf messages and
//! Tokio framing. Only its inherited pipe handles reach this endpoint.
use anyhow::{Context, Result, ensure};
use futures_util::{SinkExt, StreamExt};
use prost::Message;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

include!(concat!(env!("OUT_DIR"), "/wukong.session.v1.rs"));
pub(crate) const SCHEMA: u32 = 1;
pub(crate) const FRAME_LIMIT: usize = 4 * 1024 * 1024;

pub(crate) struct Channel<R, W> {
    read: FramedRead<R, LengthDelimitedCodec>,
    write: FramedWrite<W, LengthDelimitedCodec>,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Channel<R, W> {
    pub(crate) fn new(read: R, write: W) -> Self {
        let codec = || {
            LengthDelimitedCodec::builder()
                .max_frame_length(FRAME_LIMIT)
                .new_codec()
        };
        Self {
            read: FramedRead::new(read, codec()),
            write: FramedWrite::new(write, codec()),
        }
    }
    pub(crate) async fn receive<T: Message + Default>(&mut self) -> Result<Option<T>> {
        self.read
            .next()
            .await
            .map(|frame| Ok(T::decode(frame?)?))
            .transpose()
    }
    pub(crate) async fn send(&mut self, message: &impl Message) -> Result<()> {
        ensure!(
            message.encoded_len() <= FRAME_LIMIT,
            "native session frame exceeds its bound"
        );
        self.write
            .send(prost::bytes::Bytes::from(message.encode_to_vec()))
            .await?;
        Ok(())
    }
    pub(crate) async fn exchange(
        &mut self,
        request: &Request,
        hash: &str,
    ) -> Result<serde_json::Value> {
        self.send(request).await?;
        let reply: Reply = self
            .receive()
            .await?
            .context("native session host closed without a reply")?;
        ensure!(
            reply.schema == SCHEMA
                && reply.resource == request.resource
                && reply.sequence == request.sequence
                && reply.request_sha256 == hash,
            "native session reply belongs to another request or host"
        );
        match reply.outcome.context("native session outcome is missing")? {
            reply::Outcome::ResultJson(bytes) => Ok(serde_json::from_slice(&bytes)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn framed_protobuf_round_trips_over_real_inherited_pipe_adapters() -> Result<()> {
        let (input, host_output) = std::io::pipe()?;
        let (host_input, output) = std::io::pipe()?;
        let mut client = Channel::new(
            crate::stream_io::reader(input),
            crate::stream_io::writer(output),
        );
        let mut host = Channel::new(
            crate::stream_io::reader(host_input),
            crate::stream_io::writer(host_output),
        );
        let request = Request {
            schema: SCHEMA,
            resource: "a".repeat(32),
            sequence: 1,
            operation: Some(request::Operation::Write(Write {
                data: vec![0, 255, 10],
                read_timeout_seconds: 10,
            })),
        };
        let hash = "b".repeat(64);
        let expected = serde_json::json!({"state":"running","text":"binary \u{0} and 中文"});
        let reply = async {
            let received: Request = host.receive().await?.context("pipe request absent")?;
            ensure!(received == request, "pipe request changed");
            host.send(&Reply {
                schema: SCHEMA,
                resource: request.resource.clone(),
                sequence: 1,
                request_sha256: hash.clone(),
                outcome: Some(reply::Outcome::ResultJson(serde_json::to_vec(&expected)?)),
            })
            .await
        };
        let (actual, sent) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(client.exchange(&request, &hash), reply)
        })
        .await?;
        sent?;
        assert_eq!(actual?, expected);
        Ok(())
    }
    #[tokio::test]
    async fn replies_bind_resource_sequence_and_admitted_request() -> Result<()> {
        for fault in 0..7 {
            let (left, right) = tokio::io::duplex(4096);
            let (read, write) = tokio::io::split(left);
            let (host_read, host_write) = tokio::io::split(right);
            let mut client = Channel::new(read, write);
            let mut host = Channel::new(host_read, host_write);
            let request = Request {
                schema: SCHEMA,
                resource: "a".repeat(32),
                sequence: 3,
                operation: Some(request::Operation::Close(Close {})),
            };
            let hash = "b".repeat(64);
            let reply = async {
                let received: Request = host.receive().await?.context("request absent")?;
                assert_eq!(received, request);
                let mut reply = Reply {
                    schema: SCHEMA,
                    resource: request.resource.clone(),
                    sequence: 3,
                    request_sha256: hash.clone(),
                    outcome: Some(reply::Outcome::ResultJson(
                        b"{\"state\":\"closed\"}".to_vec(),
                    )),
                };
                match fault {
                    1 => reply.schema += 1,
                    2 => reply.resource = "c".repeat(32),
                    3 => reply.sequence -= 1,
                    4 => reply.request_sha256 = "d".repeat(64),
                    5 => reply.outcome = None,
                    6 => reply.outcome = Some(reply::Outcome::ResultJson(b"{".to_vec())),
                    _ => {}
                }
                host.send(&reply).await
            };
            let (actual, sent) = tokio::join!(client.exchange(&request, &hash), reply);
            sent?;
            if fault == 0 {
                assert_eq!(actual?["state"], "closed");
            } else {
                assert!(actual.is_err(), "accepted fault {fault}");
            }
        }
        Ok(())
    }
}
