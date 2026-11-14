//! Bounded local worker IPC. SSH authentication occurs before the fixed relay;
//! the pipe independently identifies the actual Windows caller and SCM server.
use crate::security::{self, Handle, SecurityDescriptor};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions},
};
use windows_sys::Win32::{
    Foundation::{ERROR_PIPE_BUSY, INVALID_HANDLE_VALUE},
    Security::{RevertToSelf, TOKEN_QUERY},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_READ_EA,
        FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, OPEN_EXISTING, READ_CONTROL,
        SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, SYNCHRONIZE,
    },
    System::{
        Pipes::{GetNamedPipeServerProcessId, ImpersonateNamedPipeClient},
        Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
            SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
            SERVICE_STATUS_PROCESS,
        },
        Threading::{GetCurrentThread, OpenThreadToken},
    },
};

pub const MAX_FRAME: usize = 48 * 1024 * 1024;
// Generic write also grants FILE_CREATE_PIPE_INSTANCE. Give clients individual
// data/attribute rights and no right to create a competing server instance.
const CLIENT_ACCESS: u32 = FILE_READ_DATA
    | FILE_WRITE_DATA
    | FILE_READ_EA
    | FILE_WRITE_EA
    | FILE_READ_ATTRIBUTES
    | FILE_WRITE_ATTRIBUTES
    | READ_CONTROL
    | SYNCHRONIZE;

pub fn service_name(installation: &str) -> Result<String> {
    ensure!(
        installation.len() == 32
            && installation
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid native installation ID"
    );
    Ok(format!("WukongWorker-{installation}"))
}
#[derive(Clone, Copy)]
pub(crate) enum Channel {
    Relay,
}
fn pipe_name(installation: &str, channel: Channel) -> Result<String> {
    let service = service_name(installation)?;
    Ok(match channel {
        Channel::Relay => format!(r"\\.\pipe\{service}"),
    })
}

pub fn server(installation: &str, management_sid: &str, first: bool) -> Result<NamedPipeServer> {
    security::require_system()?;
    security::validate_account_sid(management_sid)?;
    create_server(
        installation,
        Channel::Relay,
        &format!("D:P(A;;GA;;;SY)(A;;0x{CLIENT_ACCESS:08x};;;{management_sid})"),
        first,
    )
}
fn create_server(
    installation: &str,
    channel: Channel,
    sddl: &str,
    first: bool,
) -> Result<NamedPipeServer> {
    let descriptor = SecurityDescriptor::from_sddl(sddl)?;
    let mut attributes = descriptor.attributes();
    let result = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .max_instances(16)
            .create_with_security_attributes_raw(
                pipe_name(installation, channel)?,
                (&mut attributes as *mut windows_sys::Win32::Security::SECURITY_ATTRIBUTES).cast(),
            )
    }?;
    Ok(result)
}

/// Call synchronously immediately after reading a frame, before parsing or
/// dispatching it. Never await with an impersonated thread token.
pub fn authenticate_client(server: &NamedPipeServer, management_sid: &str) -> Result<()> {
    security::validate_account_sid(management_sid)?;
    let sid = client_sid(server)?;
    ensure!(
        sid == management_sid || sid == "S-1-5-18",
        "native pipe caller is not the enrolled management account"
    );
    Ok(())
}
pub(crate) fn client_sid(server: &NamedPipeServer) -> Result<String> {
    security::check(unsafe { ImpersonateNamedPipeClient(server.as_raw_handle()) })?;
    struct Revert;
    impl Drop for Revert {
        fn drop(&mut self) {
            if unsafe { RevertToSelf() } == 0 {
                std::process::abort();
            }
        }
    }
    let guard = Revert;
    let mut token = null_mut();
    security::check(unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) })?;
    let sid = security::token_user_sid(&Handle(token))?;
    drop(guard);
    Ok(sid)
}

fn open_client(installation: &str, channel: Channel) -> Result<NamedPipeClient> {
    let encoded = security::wide(OsStr::new(&pipe_name(installation, channel)?))?;
    let handle = unsafe {
        CreateFileW(
            encoded.as_ptr(),
            CLIENT_ACCESS,
            0,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error().into());
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    Ok(unsafe { NamedPipeClient::from_raw_handle(handle.into_raw_handle()) }?)
}

struct Service(windows_sys::Win32::System::Services::SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
pub fn running_service_pid(installation: &str) -> Result<u32> {
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    ensure!(
        !manager.is_null(),
        "cannot query worker service manager: {}",
        io::Error::last_os_error()
    );
    let manager = Service(manager);
    let name = security::wide(OsStr::new(&service_name(installation)?))?;
    let service = unsafe { OpenServiceW(manager.0, name.as_ptr(), SERVICE_QUERY_STATUS) };
    ensure!(
        !service.is_null(),
        "cannot query enrolled worker service: {}",
        io::Error::last_os_error()
    );
    let service = Service(service);
    let mut status: SERVICE_STATUS_PROCESS = unsafe { std::mem::zeroed() };
    let mut size = 0;
    security::check(unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut size,
        )
    })?;
    ensure!(
        status.dwCurrentState == SERVICE_RUNNING && status.dwProcessId > 0,
        "enrolled worker service is not running"
    );
    Ok(status.dwProcessId)
}
pub async fn connect(installation: &str) -> Result<NamedPipeClient> {
    let expected = running_service_pid(installation)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let client = loop {
        match open_client(installation, Channel::Relay) {
            Ok(client) => break client,
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .and_then(io::Error::raw_os_error)
                    == Some(ERROR_PIPE_BUSY as i32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(error) => return Err(error),
        }
    };
    let mut actual = 0;
    security::check(unsafe { GetNamedPipeServerProcessId(client.as_raw_handle(), &mut actual) })?;
    ensure!(
        actual == expected && actual == running_service_pid(installation)?,
        "native pipe server does not match the enrolled SCM service"
    );
    Ok(client)
}

pub async fn read_frame(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Option<Vec<u8>>> {
    read_bounded_frame(reader, MAX_FRAME).await
}
pub(crate) async fn read_bounded_frame(
    reader: &mut (impl AsyncBufRead + Unpin),
    maximum: usize,
) -> Result<Option<Vec<u8>>> {
    ensure!(
        maximum > 0 && maximum <= MAX_FRAME,
        "invalid native frame limit"
    );
    let mut bytes = Vec::new();
    let size = reader
        .take(maximum as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .await?;
    if size == 0 {
        return Ok(None);
    }
    ensure!(
        size <= maximum && bytes.last() == Some(&b'\n'),
        "native RPC frame is oversized or incomplete"
    );
    Ok(Some(bytes))
}
pub async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() <= MAX_FRAME
            && bytes.last() == Some(&b'\n')
            && !bytes[..bytes.len() - 1].contains(&b'\n'),
        "invalid native RPC output frame"
    );
    writer
        .write_all(bytes)
        .await
        .context("native RPC write failed")?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn owned_local_pipe_uses_real_caller_identity_and_bounded_frames() -> Result<()> {
        let (_, random) = wukong_worker_package::Signer::generate()?;
        let id = hex::encode(&sha2::Sha256::digest(random.as_ref())[..16]);
        let sid = security::current_user_sid()?;
        let server = create_server(
            &id,
            Channel::Relay,
            &format!("D:P(A;;GA;;;{sid})(A;;GA;;;SY)"),
            true,
        )?;
        let client = open_client(&id, Channel::Relay)?;
        server.connect().await?;
        let (read, mut write) = tokio::io::split(client);
        write_frame(&mut write, b"{\"fixture\":true}\n").await?;
        let (read_server, write_server) = tokio::io::split(server);
        let mut reader = tokio::io::BufReader::new(read_server);
        let frame = read_frame(&mut reader)
            .await?
            .context("missing fixture frame")?;
        let underlying = reader.into_inner().unsplit(write_server);
        authenticate_client(&underlying, &sid)?;
        let wrong_sid = format!(
            "{}-999999999",
            sid.rsplit_once('-').context("fixture SID has no RID")?.0
        );
        assert!(authenticate_client(&underlying, &wrong_sid).is_err());
        assert_eq!(security::current_user_sid()?, sid);
        let (read_server, mut write_server) = tokio::io::split(underlying);
        write_frame(&mut write_server, &frame).await?;
        let mut reader = tokio::io::BufReader::new(read);
        assert_eq!(read_frame(&mut reader).await?.unwrap(), frame);
        drop(read_server);
        let mut incomplete = tokio::io::BufReader::new(b"no-newline".as_slice());
        assert!(read_frame(&mut incomplete).await.is_err());
        assert!(write_frame(&mut write, b"two\nframes\n").await.is_err());
        Ok(())
    }
    use sha2::Digest;
}
