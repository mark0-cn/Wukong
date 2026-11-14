//! Full-duplex pipe I/O for an owned interactive process. The standard pipe,
//! blocking adapter and Tokio drive I/O; this module only enforces its bounds
//! and retains evidence. The caller drains its Job on every error.
use crate::{session_artifacts::Transcript, session_wire, stream_io};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::{PipeReader, PipeWriter};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

impl session_wire::Write {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.data.len() <= 1024 * 1024,
            "interactive write exceeds 1 MiB"
        );
        ensure!(
            (1..=300).contains(&self.read_timeout_seconds),
            "interactive read timeout must be 1 through 300 seconds"
        );
        Ok(())
    }
}

pub(crate) struct Output {
    bytes: Vec<u8>,
    consumed: usize,
    limit: usize,
    pub eof: bool,
    transcript: Transcript,
}
impl Output {
    fn accept(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            self.eof = true;
            return Ok(());
        }
        self.transcript
            .append(&json!({"output_base64":STANDARD.encode(bytes)}))?;
        ensure!(
            self.bytes.len() + bytes.len() <= self.limit,
            "interactive output budget exceeded"
        );
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    pub(crate) fn snapshot(&mut self, sent: bool, timed_out: bool) -> Value {
        let available = self.bytes.len() - self.consumed;
        // UTF-8 replacement plus JSON escaping and base64 must fit the durable
        // journal's 64-KiB result bound, even for binary control bytes.
        let captured = &self.bytes[self.consumed.max(self.bytes.len().saturating_sub(4096))..];
        let result = json!({"captured":String::from_utf8_lossy(captured),
            "captured_base64":STANDARD.encode(captured),"captured_bytes":captured.len(),
            "truncated":available > captured.len(),"total_bytes":self.bytes.len(),
            "data_sent":sent,"timed_out":timed_out});
        self.consumed = self.bytes.len();
        result
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn event(&mut self, value: &Value) -> Result<()> {
        self.transcript.append(value)
    }
}

pub(crate) struct Pipes {
    input: stream_io::Stream<PipeWriter>,
    reader: stream_io::Stream<PipeReader>,
    pub output: Output,
}
impl Pipes {
    pub(crate) fn new(
        input: PipeWriter,
        reader: PipeReader,
        limit: usize,
        transcript: Transcript,
    ) -> Self {
        Self {
            input: stream_io::writer(input),
            reader: stream_io::reader(reader),
            output: Output {
                bytes: Vec::new(),
                consumed: 0,
                limit,
                eof: false,
                transcript,
            },
        }
    }
    pub(crate) async fn read(&mut self) -> Result<()> {
        let mut bytes = [0; 8192];
        let count = self.reader.read(&mut bytes).await?;
        self.output.accept(&bytes[..count])
    }
    /// A write timeout has an uncertain effect. Do not return a successful
    /// partial write or retry it. The caller cancels and reconciles the resource.
    pub(crate) async fn write(&mut self, request: &session_wire::Write) -> Result<bool> {
        request.validate()?;
        let deadline = Instant::now() + Duration::from_secs(request.read_timeout_seconds.into());
        self.output
            .event(&json!({"input_base64":STANDARD.encode(&request.data),"phase":"intent"}))?;
        let writing = async {
            self.input.write_all(&request.data).await?;
            self.input.flush().await
        };
        tokio::pin!(writing);
        let mut bytes = [0; 8192];
        let mut written = false;
        let mut quiet = Instant::now() + Duration::from_millis(250);
        loop {
            tokio::select! {
                result = &mut writing, if !written => {
                    result.context("interactive write outcome is uncertain")?;
                    written = true;
                    quiet = Instant::now() + Duration::from_millis(250);
                    self.output.event(&json!({"phase":"written","bytes":request.data.len()}))?;
                },
                result = self.reader.read(&mut bytes), if !self.output.eof => {
                    let count = result?;
                    self.output.accept(&bytes[..count])?;
                    quiet = Instant::now() + Duration::from_millis(250);
                },
                _ = tokio::time::sleep_until(quiet), if written => return Ok(false),
                _ = tokio::time::sleep_until(deadline) => {
                    ensure!(written, "interactive write timed out with an uncertain effect");
                    return Ok(true);
                }
            }
        }
    }
    pub(crate) async fn drain(&mut self) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !self.output.eof {
                self.read().await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("interactive pipes remained open after Job drain")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn output_before_input_does_not_deadlock_and_binary_evidence_is_exact() -> Result<()> {
        let (mut peer_input, input) = std::io::pipe()?;
        let (output, mut peer_output) = std::io::pipe()?;
        let transcript = tempfile::tempfile()?;
        let mut pipes = Pipes::new(input, output, 1024 * 1024, Transcript::new(transcript));
        let input = vec![0xff; 1024 * 1024];
        let expected = input.clone();
        let peer = std::thread::spawn(move || -> std::io::Result<()> {
            use std::io::{Read, Write};
            peer_output.write_all(&vec![0; 65536])?;
            let mut received = vec![0; expected.len()];
            peer_input.read_exact(&mut received)?;
            assert_eq!(received, expected);
            peer_output.write_all(b"\xff\0done")?;
            Ok(())
        });
        assert!(
            !pipes
                .write(&session_wire::Write {
                    data: input,
                    read_timeout_seconds: 5
                })
                .await?
        );
        peer.join().unwrap()?;
        pipes.drain().await?;
        assert_eq!(&pipes.output.bytes()[65536..], b"\xff\0done");
        let snapshot = pipes.output.snapshot(true, false);
        assert_eq!(snapshot["total_bytes"], 65542);
        assert_eq!(snapshot["truncated"], true);
        assert!(serde_json::to_vec(&snapshot)?.len() < 32768);
        let empty = pipes.output.snapshot(true, false);
        assert_eq!(empty["captured_bytes"], 0);
        Ok(())
    }
    #[tokio::test]
    async fn blocked_input_is_uncertain_and_empty_poll_keeps_stdin_open() -> Result<()> {
        let (mut peer_input, input) = std::io::pipe()?;
        let (output, peer_output) = std::io::pipe()?;
        let mut pipes = Pipes::new(input, output, 1024, Transcript::new(tempfile::tempfile()?));
        assert!(
            !pipes
                .write(&session_wire::Write {
                    data: vec![],
                    read_timeout_seconds: 1
                })
                .await?
        );
        assert!(
            pipes
                .write(&session_wire::Write {
                    data: vec![42; 1024 * 1024],
                    read_timeout_seconds: 1
                })
                .await
                .unwrap_err()
                .to_string()
                .contains("uncertain")
        );
        let mut first = [0];
        std::io::Read::read_exact(&mut peer_input, &mut first)?;
        assert_eq!(first, [42]);
        drop((peer_input, peer_output));
        pipes.drain().await?;
        Ok(())
    }
}
