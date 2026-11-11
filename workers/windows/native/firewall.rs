//! One installation-owned inbound SSH rule, scoped to the enrolled peer. This
//! never disables firewall profiles or edits an existing rule with the same name.
use anyhow::{Result, ensure};
use std::{net::IpAddr, path::Path};
use windows::{
    Win32::{Foundation::VARIANT_TRUE, NetworkManagement::WindowsFirewall::*, System::Com::*},
    core::BSTR,
};
pub fn add(installation: &str, sshd: &Path, port: u16, peer: IpAddr) -> Result<()> {
    let name = crate::pipe_transport::service_name(installation)?;
    let application = BSTR::from(
        sshd.to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid SSH firewall application path"))?,
    );
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _com = Com;
    unsafe {
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
        let rules = policy.Rules()?;
        let name = BSTR::from(name);
        match rules.Item(&name) {
            Ok(_) => anyhow::bail!(
                "an SSH firewall rule already uses this installation name; refusing adoption"
            ),
            Err(error) => ensure!(
                error.code().0 as u32 == 0x80070002,
                "cannot inspect existing firewall rules: {error}"
            ),
        }
        let rule: INetFwRule = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)?;
        rule.SetName(&name)?;
        rule.SetDescription(&BSTR::from("Wukong owned offline worker SSH management"))?;
        rule.SetGrouping(&BSTR::from("Wukong Worker"))?;
        rule.SetApplicationName(&application)?;
        rule.SetProtocol(6)?;
        rule.SetLocalPorts(&BSTR::from(port.to_string()))?;
        rule.SetRemoteAddresses(&BSTR::from(peer.to_string()))?;
        rule.SetDirection(NET_FW_RULE_DIR_IN)?;
        rule.SetAction(NET_FW_ACTION_ALLOW)?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
        rule.SetEnabled(VARIANT_TRUE)?;
        rules.Add(&rule)?;
    }
    Ok(())
}
