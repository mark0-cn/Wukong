//! Bound Docker control commands independently from long image pulls/builds.
use crate::i18n::Message;
use anyhow::{Context, Result, ensure};
use std::{
    io::{Read, Seek, SeekFrom},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

pub(super) fn capture(
    command: &mut Command,
    timeout: Duration,
    limit: u64,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Output> {
    super::cancellation::check(cancel)?;
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()
        .context("start Docker control command")?;
    let started = Instant::now();
    let result = (|| {
        loop {
            super::cancellation::check(cancel)?;
            ensure!(
                stdout
                    .metadata()?
                    .len()
                    .saturating_add(stderr.metadata()?.len())
                    <= limit,
                Message::new("system-docker-output-limit")
            );
            if let Some(status) = child.try_wait()? {
                // Check again after exit, before allocating any output buffers.
                ensure!(
                    stdout
                        .metadata()?
                        .len()
                        .saturating_add(stderr.metadata()?.len())
                        <= limit,
                    Message::new("system-docker-output-limit")
                );
                stdout.seek(SeekFrom::Start(0))?;
                stderr.seek(SeekFrom::Start(0))?;
                let mut out = Vec::new();
                let mut err = Vec::new();
                (&mut stdout).take(limit).read_to_end(&mut out)?;
                (&mut stderr)
                    .take(limit.saturating_sub(out.len() as u64))
                    .read_to_end(&mut err)?;
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            ensure!(
                started.elapsed() < timeout,
                Message::new("system-docker-command-timeout").arg("seconds", timeout.as_secs())
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shell(windows: &str, unix: &str) -> Command {
        let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
        if cfg!(windows) {
            command.args(["/D", "/C", windows]);
        } else {
            command.args(["-c", unix]);
        }
        command
    }
    #[test]
    fn cancellation_interrupts_a_running_control_command() {
        let token = tokio_util::sync::CancellationToken::new();
        let cancel = token.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            cancel.cancel();
        });
        let started = Instant::now();
        let result = capture(
            &mut shell("for /L %i in (1,0,2) do @rem wait", "while :; do :; done"),
            Duration::from_secs(30),
            4096,
            Some(&token),
        );
        trigger.join().unwrap();
        assert!(
            result
                .unwrap_err()
                .is::<super::super::cancellation::Cancelled>()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn hung_control_command_has_a_deadline() {
        let started = Instant::now();
        let error = capture(
            &mut shell("for /L %i in (1,0,2) do @rem wait", "while :; do :; done"),
            Duration::from_millis(80),
            4096,
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Message>().unwrap().code,
            "system-docker-command-timeout"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn output_is_limited_before_it_is_collected() {
        let error = capture(
            &mut shell(
                "for /L %i in (1,1,10000) do @echo abcdefghijklmnop",
                "while :; do echo abcdefghijklmnop; done",
            ),
            Duration::from_secs(3),
            512,
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Message>().unwrap().code,
            "system-docker-output-limit"
        );
    }
    #[test]
    fn captures_both_streams_and_exit_status() {
        let result = capture(
            &mut shell(
                "echo hello & echo failure 1>&2 & exit /b 7",
                "echo hello; echo failure >&2; exit 7",
            ),
            Duration::from_secs(3),
            4096,
            None,
        )
        .unwrap();
        assert_eq!(result.status.code(), Some(7));
        assert!(String::from_utf8_lossy(&result.stdout).contains("hello"));
        assert!(String::from_utf8_lossy(&result.stderr).contains("failure"));
    }
}
