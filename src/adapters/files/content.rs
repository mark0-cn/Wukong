//! Content digests shared by inspection, task identity and report bundles.
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    Ok(file_identity(path.as_ref())?.1)
}

pub(crate) fn file_identity(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hash.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_identity_counts_streamed_bytes_and_preserves_hashes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("artifact");
        for bytes in [vec![], b"abc".to_vec(), vec![0x5a; 2 * 65536 + 7]] {
            std::fs::write(&path, &bytes)?;
            assert_eq!(
                file_identity(&path)?,
                (bytes.len() as u64, sha256_bytes(&bytes))
            );
            assert_eq!(sha256_file(&path)?, sha256_bytes(&bytes));
        }
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(sha256_file(dir.path().join("missing")).is_err());
        Ok(())
    }
}
