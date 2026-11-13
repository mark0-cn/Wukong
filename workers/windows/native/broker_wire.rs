//! Generated messages and library-owned framing. Authentication belongs to the broker.
use tokio_util::codec::{Framed, LengthDelimitedCodec};

include!(concat!(env!("OUT_DIR"), "/wukong.broker.v2.rs"));

pub const SCHEMA: u32 = 2;
pub const FRAME_LIMIT: usize = 4096;

pub fn framed<T>(io: T) -> Framed<T, LengthDelimitedCodec> {
    Framed::new(
        io,
        LengthDelimitedCodec::builder()
            .max_frame_length(FRAME_LIMIT)
            .new_codec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use prost::Message;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn fragmented_and_consecutive_frames_preserve_message_boundaries() {
        let (mut sender, receiver) = tokio::io::duplex(64);
        let request = Request {
            schema: SCHEMA,
            operation: Some(request::Operation::Health(HealthRequest {})),
        };
        let bytes = request.encode_to_vec();
        let expected = request;
        let sending = tokio::spawn(async move {
            // Exercise a length prefix split across reads, then two messages.
            for _ in 0..2 {
                for byte in (bytes.len() as u32).to_be_bytes().iter().chain(&bytes) {
                    sender.write_all(&[*byte]).await.unwrap();
                    tokio::task::yield_now().await;
                }
            }
        });
        let mut receiver = framed(receiver);
        for _ in 0..2 {
            let frame = receiver.next().await.unwrap().unwrap();
            assert_eq!(Request::decode(frame).unwrap(), expected);
        }
        assert!(receiver.next().await.is_none());
        sending.await.unwrap();
    }

    #[tokio::test]
    async fn oversized_and_truncated_frames_fail_without_dispatch() {
        for bytes in [
            ((FRAME_LIMIT + 1) as u32).to_be_bytes().to_vec(),
            vec![0, 0],
            vec![0, 0, 0, 5, 8, 2],
        ] {
            let mut reader = framed(bytes.as_slice());
            assert!(reader.next().await.unwrap().is_err());
        }
        let mut sender = framed(tokio::io::sink());
        assert!(
            sender
                .send(prost::bytes::Bytes::from(vec![0; FRAME_LIMIT + 1]))
                .await
                .is_err()
        );
    }

    #[test]
    fn absent_unknown_and_malformed_operations_cannot_become_health() {
        for bytes in [vec![], vec![8, SCHEMA as u8, 26, 0]] {
            assert!(
                Request::decode(bytes.as_slice())
                    .unwrap()
                    .operation
                    .is_none()
            );
        }
        assert!(Request::decode([0xff].as_slice()).is_err());
    }
}
