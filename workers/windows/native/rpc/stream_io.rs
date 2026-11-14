//! Adapt inherited synchronous pipes with the blocking I/O library. These are
//! not ordinary files: Tokio's filesystem adapter is not their I/O contract.
//! The resource owner drains its Job before discarding a pending pipe operation.
use blocking::Unblock;
use std::io::{Read, Write};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt};

pub(crate) type Stream<T> = Compat<Unblock<T>>;
const BUFFER: usize = 8192;

pub(crate) fn reader<T: Read + Send + 'static>(pipe: T) -> Stream<T> {
    Unblock::with_capacity(BUFFER, pipe).compat()
}
pub(crate) fn writer<T: Write + Send + 'static>(pipe: T) -> Stream<T> {
    Unblock::with_capacity(BUFFER, pipe).compat_write()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn cancelled_pipe_read_keeps_bytes_and_peer_closure_drains_it() -> anyhow::Result<()> {
        let (read, mut write) = std::io::pipe()?;
        let mut input = reader(read);
        let mut bytes = [0u8; 4];
        assert!(
            tokio::time::timeout(Duration::from_millis(25), input.read_exact(&mut bytes))
                .await
                .is_err()
        );
        std::io::Write::write_all(&mut write, b"\0\xffok")?;
        tokio::time::timeout(Duration::from_secs(2), input.read_exact(&mut bytes)).await??;
        assert_eq!(bytes, *b"\0\xffok");
        drop(write);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), input.read(&mut bytes)).await??,
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn blocked_pipe_write_settles_after_owned_peer_closes() -> anyhow::Result<()> {
        let (read, write) = std::io::pipe()?;
        let mut output = writer(write);
        let bytes = vec![0x7f; 1024 * 1024];
        assert!(
            tokio::time::timeout(Duration::from_millis(25), output.write_all(&bytes))
                .await
                .is_err()
        );
        drop(read);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), output.flush())
                .await?
                .is_err()
        );
        Ok(())
    }
}
