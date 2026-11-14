//! One installation-owned inbound SSH rule, scoped to the enrolled peer. This
//! never disables firewall profiles or edits an existing rule with the same name.
use anyhow::{Context, Result, ensure};
use std::{net::IpAddr, path::Path};
use windows::{
    Win32::{Foundation::VARIANT_TRUE, NetworkManagement::WindowsFirewall::*, System::Com::*},
    core::BSTR,
};
struct Com;
impl Com {
    fn open() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }
            .context("initialize Windows firewall COM")?;
        Ok(Self)
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

fn application_path(path: &Path) -> Result<String> {
    let path = crate::isolation::security::local_file_path(path)?;
    Ok(path
        .to_str()
        .context("invalid SSH firewall application path")?
        .to_owned())
}

fn configure(installation: &str, sshd: &Path, port: u16, peer: IpAddr) -> Result<INetFwRule> {
    let name = BSTR::from(crate::rpc::pipe_transport::service_name(installation)?);
    let application = BSTR::from(application_path(sshd)?);
    unsafe {
        let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)
            .context("create unregistered SSH firewall rule")?;
        rule.SetName(&name).context("set SSH firewall rule name")?;
        rule.SetDescription(&BSTR::from("Wukong owned offline worker SSH management"))
            .context("set SSH firewall description")?;
        rule.SetGrouping(&BSTR::from("Wukong Worker"))
            .context("set SSH firewall grouping")?;
        rule.SetApplicationName(&application)
            .context("set SSH firewall executable")?;
        rule.SetProtocol(6)
            .context("set SSH firewall TCP protocol")?;
        rule.SetLocalPorts(&BSTR::from(port.to_string()))
            .context("set SSH firewall port")?;
        rule.SetRemoteAddresses(&BSTR::from(peer.to_string()))
            .context("set SSH firewall peer")?;
        rule.SetDirection(NET_FW_RULE_DIR_IN)
            .context("set SSH firewall inbound direction")?;
        rule.SetAction(NET_FW_ACTION_ALLOW)
            .context("set SSH firewall allow action")?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)
            .context("set SSH firewall profiles")?;
        rule.SetEnabled(VARIANT_TRUE)
            .context("enable SSH firewall rule")?;
        Ok(rule)
    }
}

pub fn add(installation: &str, sshd: &Path, port: u16, peer: IpAddr) -> Result<()> {
    let _com = Com::open()?;
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
            .context("open Windows firewall policy")?;
        let rules = policy
            .Rules()
            .context("open Windows firewall rule collection")?;
        let name = BSTR::from(crate::rpc::pipe_transport::service_name(installation)?);
        match rules.Item(&name) {
            Ok(_) => anyhow::bail!(
                "an SSH firewall rule already uses this installation name; refusing adoption"
            ),
            Err(error) => ensure!(
                error.code().0 as u32 == 0x80070002,
                "cannot inspect existing firewall rules: {error}"
            ),
        }
        let rule = configure(installation, sshd, port, peer)?;
        let properties = format!(
            "application={}, protocol={}, port={}, peer={}, profiles={}, direction={}, action={}, interfaces={}",
            rule.ApplicationName()
                .context("read back SSH firewall executable")?,
            rule.Protocol().context("read back SSH firewall protocol")?,
            rule.LocalPorts().context("read back SSH firewall port")?,
            rule.RemoteAddresses()
                .context("read back SSH firewall peer")?,
            rule.Profiles().context("read back SSH firewall profiles")?,
            rule.Direction()
                .context("read back SSH firewall direction")?
                .0,
            rule.Action().context("read back SSH firewall action")?.0,
            rule.InterfaceTypes()
                .context("read back SSH firewall interfaces")?
        );
        rules
            .Add(&rule)
            .with_context(|| format!("register owned SSH firewall rule ({properties})"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregistered_rule_validates_native_properties_without_policy_changes() -> Result<()> {
        let _com = Com::open()?;
        let installation = hex::encode(crate::host::accounts::random_bytes::<16>()?);
        let executable = std::env::current_exe()?;
        let rule = configure(&installation, &executable, 61248, "192.0.2.7".parse()?)?;
        unsafe {
            assert_eq!(rule.Protocol()?, 6);
            assert_eq!(rule.LocalPorts()?.to_string(), "61248");
            assert_eq!(rule.Enabled()?, VARIANT_TRUE);
        }
        let mixed = executable
            .parent()
            .context("missing fixture parent")?
            .join(format!(
                "ignored/../{}",
                executable.file_name().unwrap().to_string_lossy()
            ));
        // Relative aliases are refused before canonicalization, not adopted.
        assert!(application_path(&mixed).is_err());
        let normal = application_path(&executable)?;
        assert!(!normal.starts_with(r"\\?\") && !normal.contains('/'));
        let mixed_separators = std::path::PathBuf::from(
            executable
                .to_str()
                .context("invalid fixture path")?
                .replace('\\', "/"),
        );
        let alternate = configure(
            &installation,
            &mixed_separators,
            61248,
            "192.0.2.7".parse()?,
        )?;
        assert_eq!(unsafe { alternate.ApplicationName()? }.to_string(), normal);
        assert_eq!(
            Path::new(&normal).canonicalize()?,
            executable.canonicalize()?
        );
        // Deliberately never call Rules.Add: the rule exists only in memory.
        Ok(())
    }
}
