use super::*;

#[test]
fn diagnostic_stream_retains_only_its_budget_and_signals_overflow() -> Result<()> {
    let (read, mut write) = io::pipe()?;
    let captured = CapturedStream::start(read, 1024)?;
    let writer = thread::spawn(move || -> io::Result<()> { write.write_all(&vec![b'x'; 16384]) });
    writer.join().unwrap()?;
    for _ in 0..100 {
        if captured.watch().failed() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(captured.watch().failed());
    assert_eq!(captured.finish()?, vec![b'x'; 1024]);
    Ok(())
}
use crate::isolation::processes::{ResourceLimits, Spawn};
use std::{collections::BTreeMap, ffi::OsString, io::Cursor, path::Path};

#[test]
fn native_stdio_fixture_entry() -> Result<()> {
    let Some(mode) = std::env::var_os("WUKONG_NATIVE_STDIO_FIXTURE") else {
        return Ok(());
    };
    match mode.to_str().context("invalid inert fixture mode")? {
        "echo" => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes)?;
            std::io::stdout().write_all(b"stdout-marker:")?;
            std::io::stdout().write_all(&bytes)?;
            std::io::stderr().write_all(b"stderr-marker")?;
        }
        "flood" => loop {
            std::io::stdout().write_all(&[b'x'; 8192])?;
            std::io::stderr().write_all(&[b'y'; 8192])?;
        },
        "limits" => {
            let result = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "isolation::process_capture::tests::native_stdio_fixture_entry",
                    "--nocapture",
                ])
                .env("WUKONG_NATIVE_STDIO_FIXTURE", "leaf")
                .spawn();
            ensure!(
                result.is_err(),
                "resource process-count limit allowed another process"
            );
            let mut allocation = Vec::<u8>::new();
            ensure!(
                allocation.try_reserve_exact(128 * 1024 * 1024).is_err(),
                "resource memory limit allowed a 128 MiB allocation"
            );
            std::io::stdout().write_all(b"native-limits-enforced")?;
        }
        "descendant" => {
            let _child = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "isolation::process_capture::tests::native_stdio_fixture_entry",
                    "--nocapture",
                ])
                .env("WUKONG_NATIVE_STDIO_FIXTURE", "leaf")
                .spawn()?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !Path::new("leaf.pid").exists() {
                ensure!(Instant::now() < deadline, "owned descendant did not start");
                thread::sleep(Duration::from_millis(10));
            }
        }
        "leaf" => {
            std::fs::write("leaf.pending", std::process::id().to_string())?;
            std::fs::rename("leaf.pending", "leaf.pid")?;
            thread::sleep(Duration::from_secs(30));
        }
        "wait" => thread::sleep(Duration::from_secs(30)),
        _ => anyhow::bail!("unknown inert fixture mode"),
    }
    std::process::exit(0);
}

fn fixture(
    directory: &Path,
    mode: &str,
    input: Option<Box<dyn Read + Send>>,
    limit: usize,
    limits: ResourceLimits,
) -> Result<CapturedProcess> {
    let executable = std::env::current_exe()?;
    let mut environment =
        BTreeMap::from([("WUKONG_NATIVE_STDIO_FIXTURE".into(), OsString::from(mode))]);
    environment.insert(
        "SystemRoot".into(),
        std::env::var_os("SystemRoot").context("missing Windows root")?,
    );
    let arguments = [
        OsString::from("--exact"),
        OsString::from("isolation::process_capture::tests::native_stdio_fixture_entry"),
        OsString::from("--nocapture"),
    ];
    PreparedIo::new(limit)?.launch(input, |io| {
        Spawn {
            executable: &executable,
            arguments: &arguments,
            directory,
            environment: &environment,
            log: io.output(),
            input: io.input(),
        }
        .inert_streamed_fixture(io.error(), limits)
    })
}
fn limits() -> Result<ResourceLimits> {
    ResourceLimits::new(8, 256 * 1024 * 1024)
}
fn contains(bytes: &[u8], value: &[u8]) -> bool {
    bytes.windows(value.len()).any(|window| window == value)
}

#[test]
fn standard_pipes_keep_binary_stdin_stdout_and_stderr_separate() -> Result<()> {
    let root = tempfile::tempdir()?;
    let data = b"alpha\0beta".to_vec();
    let process = fixture(
        root.path(),
        "echo",
        Some(Box::new(Cursor::new(data.clone()))),
        65536,
        limits()?,
    )?;
    let result = process.wait(Duration::from_secs(5), &AtomicBool::new(false))?;
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.reason, StopReason::Exited);
    assert!(contains(&result.stdout, b"stdout-marker:alpha\0beta"));
    assert_eq!(result.stderr, b"stderr-marker");
    assert!(!contains(&result.stdout, b"stderr-marker"));
    assert_eq!(result.input.written, data.len() as u64);
    assert!(result.input.completed);
    assert!(result.input.error.is_none());
    Ok(())
}

#[test]
fn output_flood_is_bounded_across_both_streams_and_the_job_is_drained() -> Result<()> {
    let root = tempfile::tempdir()?;
    let result = fixture(root.path(), "flood", None, 1024, limits()?)?
        .wait(Duration::from_secs(5), &AtomicBool::new(false))?;
    assert_eq!(result.reason, StopReason::OutputLimit);
    assert!(result.stdout.len() + result.stderr.len() <= 1024);
    Ok(())
}

#[test]
fn sdk_limits_reject_another_process_and_excess_committed_memory() -> Result<()> {
    let root = tempfile::tempdir()?;
    let result = fixture(
        root.path(),
        "limits",
        None,
        65536,
        ResourceLimits::new(1, 64 * 1024 * 1024)?,
    )?
    .wait(Duration::from_secs(5), &AtomicBool::new(false))?;
    assert_eq!(result.reason, StopReason::Exited);
    assert_eq!(
        result.exit_code,
        0,
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(contains(&result.stdout, b"native-limits-enforced"));
    assert!(!root.path().join("leaf.pid").exists());
    Ok(())
}

#[test]
fn leader_exit_does_not_leave_a_descendant_holding_an_output_pipe() -> Result<()> {
    let root = tempfile::tempdir()?;
    let process = fixture(root.path(), "descendant", None, 65536, limits()?)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.path().join("leaf.pid").exists() {
        ensure!(Instant::now() < deadline, "owned leaf did not become ready");
        thread::sleep(Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(root.path().join("leaf.pid"))?.parse()?;
    use windows_sys::Win32::System::Threading::*;
    let handle = crate::isolation::security::Handle(unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | 0x100000, 0, pid)
    });
    ensure!(
        !handle.0.is_null(),
        "cannot hold the known fixture descendant before drain"
    );
    let started = Instant::now();
    let result = process.wait(Duration::from_secs(5), &AtomicBool::new(false))?;
    assert_eq!(result.reason, StopReason::Exited);
    assert_eq!(result.exit_code, 0);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        unsafe { WaitForSingleObject(handle.0, 1000) },
        windows_sys::Win32::Foundation::WAIT_OBJECT_0
    );
    Ok(())
}

#[test]
fn cancellation_and_deadline_stop_and_drain_the_same_owned_process() -> Result<()> {
    let root = tempfile::tempdir()?;
    for timeout in [Duration::ZERO, Duration::from_millis(100)] {
        let deadline = fixture(root.path(), "wait", None, 65536, limits()?)?
            .wait(timeout, &AtomicBool::new(false))?;
        assert_eq!(deadline.reason, StopReason::Deadline);
    }
    let cancelled = fixture(root.path(), "wait", None, 65536, limits()?)?
        .wait(Duration::from_secs(5), &AtomicBool::new(true))?;
    assert_eq!(cancelled.reason, StopReason::Cancelled);
    Ok(())
}
