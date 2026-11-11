//! Bounded observation of Docker's non-terminal layer events; no invented byte totals.
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    path::Path,
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize)]
pub struct LayerProgress {
    pub id: String,
    pub state: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PullProgress {
    pub layers: Vec<LayerProgress>,
    pub elapsed_ms: u64,
    pub quiet_ms: u64,
    pub finished: bool,
    pub cached: bool,
    pub truncated: bool,
}

#[derive(Default)]
struct Parser {
    progress: PullProgress,
    pending: Vec<u8>,
    oversized: bool,
    revision: u64,
}
impl Parser {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if matches!(byte, b'\n' | b'\r') {
                if !self.oversized {
                    let line = String::from_utf8_lossy(&self.pending).into_owned();
                    self.line(&line);
                }
                self.pending.clear();
                self.oversized = false;
            } else if self.pending.len() < 2048 {
                self.pending.push(byte);
            } else {
                self.oversized = true;
            }
        }
    }
    fn line(&mut self, line: &str) {
        let Some((id, text)) = line.split_once(": ") else {
            return;
        };
        if !(12..=64).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return;
        }
        let state = if text.starts_with("Pull complete") || text.starts_with("Already exists") {
            "ready"
        } else if text.starts_with("Download complete") {
            "downloaded"
        } else if text.starts_with("Extracting") {
            "extracting"
        } else if text.starts_with("Verifying Checksum") {
            "verifying"
        } else if text.starts_with("Downloading") {
            "downloading"
        } else if text.starts_with("Pulling fs layer") || text.starts_with("Waiting") {
            "waiting"
        } else {
            return;
        };
        if let Some(layer) = self.progress.layers.iter_mut().find(|layer| layer.id == id) {
            if layer.state != state {
                self.revision += 1;
            }
            layer.state = state;
        } else if self.progress.layers.len() < 256 {
            self.progress.layers.push(LayerProgress {
                id: id.into(),
                state,
            });
            self.revision += 1;
        } else {
            self.progress.truncated = true;
        }
    }
}

pub(super) fn monitor(
    child: &mut Child,
    log: &Path,
    report: &mut dyn FnMut(PullProgress),
) -> std::io::Result<ExitStatus> {
    let started = Instant::now();
    let mut file = File::open(log)?;
    let mut parser = Parser::default();
    let mut chunk = [0; 16384];
    let mut last_change = started;
    let mut revision = 0;
    report(parser.progress.clone());
    loop {
        let exit = child.try_wait()?;
        // Drain incrementally. No raw-output buffers or unbounded lines in the DTO.
        let mut drained = false;
        for _ in 0..64 {
            let count = file.read(&mut chunk)?;
            if count == 0 {
                drained = true;
                break;
            }
            parser.feed(&chunk[..count]);
        }
        parser.progress.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        if revision != parser.revision {
            revision = parser.revision;
            last_change = Instant::now();
        }
        parser.progress.quiet_ms = last_change.elapsed().as_millis().min(u64::MAX as u128) as u64;
        if let Some(status) = exit
            && drained
        {
            parser.feed(b"\n");
            parser.progress.finished = status.success();
            report(parser.progress);
            return Ok(status);
        }
        report(parser.progress.clone());
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_layer_events_distinguish_download_from_completion() {
        let mut parser = Parser::default();
        for chunk in b"abcdef123456: Pulling fs layer\r\nabcdef123456: Download complete\n123456abcdef: Already exists\nDigest: sha256:abcdef\n".chunks(7) { parser.feed(chunk); }
        assert_eq!(parser.progress.layers.len(), 2);
        assert_eq!(parser.progress.layers[0].state, "downloaded");
        assert_eq!(parser.progress.layers[1].state, "ready");
        assert!(!parser.progress.finished);
        let revision = parser.revision;
        parser.feed(b"abcdef123456: Download complete\n");
        assert_eq!(
            parser.revision, revision,
            "duplicate output is not new progress"
        );
        parser.feed(b"abcdef123456: Pull complete\n");
        assert_eq!(parser.progress.layers[0].state, "ready");
    }

    #[test]
    fn oversized_output_and_many_layers_remain_bounded() {
        let mut parser = Parser::default();
        parser.feed(&vec![b'x'; 100_000]);
        assert_eq!(parser.pending.len(), 2048);
        parser.feed(b"\n");
        for id in 0..300 {
            parser.feed(format!("{id:012x}: Pull complete\n").as_bytes());
        }
        assert_eq!(parser.progress.layers.len(), 256);
        assert!(parser.progress.truncated);
    }

    #[test]
    fn observes_a_real_child_without_treating_its_output_as_success() {
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("pull.log");
        let file = File::create(&log).unwrap();
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("cmd");
            c.args([
                "/D",
                "/C",
                "echo abcdef123456: Download complete & exit /b 7",
            ]);
            c
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut c = Command::new("sh");
            c.args(["-c", "echo 'abcdef123456: Download complete'; exit 7"]);
            c
        };
        let mut child = command.stdout(Stdio::from(file)).spawn().unwrap();
        let mut last = PullProgress::default();
        let status = monitor(&mut child, &log, &mut |value| last = value).unwrap();
        assert!(!status.success());
        assert!(!last.finished);
        assert_eq!(last.layers[0].state, "downloaded");
    }
}
