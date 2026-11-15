//! Bounded stdio for native resources. Reuse std::io::pipe and the existing
//! suspended/Job-owned launcher; readers start after synchronous OS setup.
use crate::isolation::processes::OwnedProcess;
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{self, PipeReader, PipeWriter, Read, Write},
    os::windows::io::OwnedHandle,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct PreparedIo {
    input: File,
    output: File,
    error: File,
    input_writer: PipeWriter,
    output_reader: PipeReader,
    error_reader: PipeReader,
    limit: usize,
}
pub(crate) struct LaunchedIo {
    pub process: OwnedProcess,
    pub input: PipeWriter,
    pub output: PipeReader,
    pub error: PipeReader,
    pub limit: usize,
}
impl PreparedIo {
    pub fn new(limit: usize) -> Result<Self> {
        ensure!(
            (1024..=8 * 1024 * 1024).contains(&limit),
            "native output limit must be 1 KiB through 8 MiB"
        );
        let (input, input_writer) = io::pipe()?;
        let (output_reader, output) = io::pipe()?;
        let (error_reader, error) = io::pipe()?;
        Ok(Self {
            input: File::from(OwnedHandle::from(input)),
            output: File::from(OwnedHandle::from(output)),
            error: File::from(OwnedHandle::from(error)),
            input_writer,
            output_reader,
            error_reader,
            limit,
        })
    }
    pub fn input(&self) -> &File {
        &self.input
    }
    pub fn output(&self) -> &File {
        &self.output
    }
    pub fn error(&self) -> &File {
        &self.error
    }

    pub(crate) fn merged(limit: usize) -> Result<Self> {
        let mut io = Self::new(limit)?;
        io.error = io.output.try_clone()?;
        Ok(io)
    }

    /// Complete thread-affine station/profile setup before adapting any pipe.
    pub(crate) fn launch_pipes(
        self,
        launch: impl FnOnce(&Self) -> Result<OwnedProcess>,
    ) -> Result<LaunchedIo> {
        let process = launch(&self)?;
        let Self {
            input,
            output,
            error,
            input_writer,
            output_reader,
            error_reader,
            limit,
        } = self;
        drop((input, output, error));
        Ok(LaunchedIo {
            process,
            input: input_writer,
            output: output_reader,
            error: error_reader,
            limit,
        })
    }

    /// The callback performs private station/profile setup on this thread.
    /// Only after it returns do we create I/O threads and close parent copies.
    pub fn launch(
        self,
        input: Option<Box<dyn Read + Send>>,
        launch: impl FnOnce(&Self) -> Result<OwnedProcess>,
    ) -> Result<CapturedProcess> {
        let LaunchedIo {
            process,
            input: input_writer,
            output: output_reader,
            error: error_reader,
            limit,
        } = self.launch_pipes(launch)?;
        let budget = Arc::new(Budget {
            remaining: AtomicUsize::new(limit),
            exceeded: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        let io = (|| -> Result<_> {
            let output = reader("native-stdout", output_reader, budget.clone())?;
            let error = reader("native-stderr", error_reader, budget.clone())?;
            let input = thread::Builder::new()
                .name("native-stdin".into())
                .spawn(move || {
                    let mut sink = CountingWriter {
                        writer: input_writer,
                        written: 0,
                    };
                    let result = match input {
                        Some(mut source) => io::copy(&mut source, &mut sink).map(|_| ()),
                        None => Ok(()),
                    };
                    InputDelivery {
                        written: sink.written,
                        completed: result.is_ok(),
                        error: result.err().map(|error| error.to_string()),
                    }
                })?;
            Ok((output, error, input))
        })();
        match io {
            Ok((output, error, input)) => Ok(CapturedProcess {
                process,
                budget,
                output,
                error,
                input,
            }),
            Err(error) => {
                process
                    .stop(Duration::from_secs(10))
                    .context("drain native process after stdio setup failure")?;
                Err(error)
            }
        }
    }
}

struct Budget {
    remaining: AtomicUsize,
    exceeded: AtomicBool,
    failed: AtomicBool,
}
pub(crate) struct CaptureWatch(Arc<Budget>);
impl CaptureWatch {
    pub(crate) fn failed(&self) -> bool {
        self.0.failed.load(Ordering::Acquire) || self.0.exceeded.load(Ordering::Acquire)
    }
}
/// One diagnostic pipe, using the same bounded drain as target stdio.
pub(crate) struct CapturedStream {
    watch: CaptureWatch,
    reader: JoinHandle<io::Result<Vec<u8>>>,
}
impl CapturedStream {
    pub(crate) fn start(pipe: PipeReader, limit: usize) -> Result<Self> {
        ensure!(
            (1024..=8 * 1024 * 1024).contains(&limit),
            "invalid diagnostic budget"
        );
        let budget = Arc::new(Budget {
            remaining: AtomicUsize::new(limit),
            exceeded: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        Ok(Self {
            reader: reader("native-diagnostics", pipe, budget.clone())?,
            watch: CaptureWatch(budget),
        })
    }
    pub(crate) fn watch(&self) -> &CaptureWatch {
        &self.watch
    }
    /// Only join after the owning Job and all parent writers have closed.
    pub(crate) fn finish(self) -> Result<Vec<u8>> {
        self.reader
            .join()
            .map_err(|_| anyhow::anyhow!("native diagnostic reader panicked"))?
            .map_err(Into::into)
    }
}
fn reader(
    name: &str,
    mut pipe: PipeReader,
    budget: Arc<Budget>,
) -> Result<JoinHandle<io::Result<Vec<u8>>>> {
    Ok(thread::Builder::new().name(name.into()).spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = match pipe.read(&mut buffer) {
                Ok(0) => return Ok(output),
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    budget.failed.store(true, Ordering::Release);
                    return Err(error);
                }
            };
            let previous = budget
                .remaining
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    Some(remaining.saturating_sub(count))
                })
                .expect("bounded native output budget");
            let kept = count.min(previous);
            output.extend_from_slice(&buffer[..kept]);
            if kept < count {
                budget.exceeded.store(true, Ordering::Release);
            }
            // Continue draining until Job termination closes every inherited
            // writer; discarded bytes are never allocated or retained.
        }
    })?)
}
struct CountingWriter {
    writer: PipeWriter,
    written: u64,
}
impl Write for CountingWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = self.writer.write(data)?;
        self.written += count as u64;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[derive(Debug)]
pub struct InputDelivery {
    pub written: u64,
    pub completed: bool,
    pub error: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopReason {
    Exited,
    Deadline,
    OutputLimit,
    IoFailure,
    Cancelled,
}
pub struct CaptureResult {
    pub pid: u32,
    pub exit_code: u32,
    pub reason: StopReason,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub input: InputDelivery,
}
pub struct CapturedProcess {
    // Drop fallback drains the Job before dropping/detaching any I/O threads.
    process: OwnedProcess,
    budget: Arc<Budget>,
    output: JoinHandle<io::Result<Vec<u8>>>,
    error: JoinHandle<io::Result<Vec<u8>>>,
    input: JoinHandle<InputDelivery>,
}
impl CapturedProcess {
    pub(crate) fn process_mut(&mut self) -> &mut OwnedProcess {
        &mut self.process
    }
    pub(crate) fn watch(&self) -> CaptureWatch {
        CaptureWatch(self.budget.clone())
    }
    pub fn process(&self) -> &OwnedProcess {
        &self.process
    }
    pub fn wait(self, timeout: Duration, cancelled: &AtomicBool) -> Result<CaptureResult> {
        self.wait_with_cancel(timeout, || Ok(cancelled.load(Ordering::Acquire)))
    }
    pub(crate) fn wait_with_cancel(
        self,
        timeout: Duration,
        mut cancelled: impl FnMut() -> Result<bool>,
    ) -> Result<CaptureResult> {
        ensure!(
            timeout <= Duration::from_secs(86400),
            "invalid native capture deadline"
        );
        // Process creation may consume the remaining overall budget. Zero is
        // then an already expired deadline and must still drain the owned Job.
        let deadline = Instant::now() + timeout;
        let reason = loop {
            if cancelled()? {
                break StopReason::Cancelled;
            }
            if self.budget.failed.load(Ordering::Acquire) {
                break StopReason::IoFailure;
            }
            if self.budget.exceeded.load(Ordering::Acquire) {
                break StopReason::OutputLimit;
            }
            if self.process.exited() {
                break StopReason::Exited;
            }
            if Instant::now() >= deadline {
                break StopReason::Deadline;
            }
            thread::sleep(Duration::from_millis(10));
        };
        self.process.stop(Duration::from_secs(10))?;
        ensure!(
            self.process.job().active_processes()? == 0,
            "native stdio Job remains active"
        );
        let pid = self.process.pid();
        let exit_code = self
            .process
            .exit_code()?
            .context("native process lacks an exit after Job drain")?;
        let stdout = self
            .output
            .join()
            .map_err(|_| anyhow::anyhow!("native stdout reader panicked"))??;
        let stderr = self
            .error
            .join()
            .map_err(|_| anyhow::anyhow!("native stderr reader panicked"))??;
        let input = self
            .input
            .join()
            .map_err(|_| anyhow::anyhow!("native stdin writer panicked"))?;
        // A fast leader can exit before a reader observes its final overrun.
        let reason = if self.budget.exceeded.load(Ordering::Acquire) && reason == StopReason::Exited
        {
            StopReason::OutputLimit
        } else {
            reason
        };
        Ok(CaptureResult {
            pid,
            exit_code,
            reason,
            stdout,
            stderr,
            input,
        })
    }
}

#[cfg(test)]
#[path = "process_capture_tests.rs"]
mod tests;
