<a id="windows-worker-setup"></a>

# Windows Worker setup

English · [简体中文](windows-worker.zh-CN.md)

[Project home](../../README.md) · [User guide](usage.md) · [Online installation](online-install.md)

The Windows Worker card prepares the offline installer package for a disposable Windows analysis VM and registers the worker afterwards. It exposes exactly two primary actions: **Generate installer package** and **Import install result**. No VM address, port, user name or private key is entered in this card.

The card is shown on a Windows x64 controller only. Media generation runs inside the desktop application; it needs no Python installation and no source checkout. VMware auto-deploy is not included: you create, start and repair the VM yourself.

<a id="windows-worker-prerequisites"></a>

## Prerequisites

- The desktop release runs on a Windows x64 controller. The signed release template must match this controller version; a template built for another version is refused.
- The controller can reach the official signed release catalog to download the template, or you already have the version-matched template ZIP for a local upload. Template downloads are large (up to 8 GiB) and may take several minutes.
- A disposable Windows x64 VM reaches the controller through an IPv4 management network, usually a host-only or VMnet adapter. The card lists eligible controller adapters; only RFC 1918 addresses are recommended.
- The generated package embeds the official offline worker setup for exactly this controller. Do not replace its binaries or copy the media to an unrelated controller.

<a id="windows-worker-generate"></a>

## Generate the installer package

1. Open **Workspace settings → Environments** and find the **Windows Worker** card.
2. Select **Generate installer package**.
3. Optionally enter an environment name. Leave it empty to use the backend default, `Windows Worker` followed by the first eight characters of the installation ID.
4. Check the management network. A single eligible IPv4 network is selected automatically. When several networks exist, choose the adapter the VM can reach; the recommended entry is preselected.
5. Keep the signed release template, or choose **Already have a template ZIP?** and select a local, version-matched template file.
6. Choose the save location for the package and start generation.

Generation has three server-side stages: download the template, verify the signed size and SHA-256, then generate the sealed media. The card shows an indeterminate progress bar, the stage sequence and elapsed time; there is no fake percentage. Keep the window open until it finishes.

A generated installation reports its package path, SSH port, management network and installation steps on the card. The package contains no controller credentials, but it is bound to this controller and installation; keep it out of untrusted hands.

<a id="windows-worker-install"></a>

## Install in the Windows VM

1. Copy the generated ZIP archive into the disposable Windows VM and extract it.
2. Double-click `WukongWorkerSetup.exe` in the extracted media, review the installation summary and confirm the installation.
3. When the setup completes, save the exported `connection.json` install result. The same window shows the SSH host fingerprint; compare it with the value discussed below.
4. Return to the controller. Do not configure the VM address or port by hand: verification only uses the address candidates recorded in the install result.

<a id="windows-worker-import"></a>

## Import and verify the install result

1. Select **Import install result** and choose the saved `connection.json`.
2. The card shows the SSH host fingerprint contained in the file. Compare it with the fingerprint displayed in the VM setup window and enter the VM fingerprint into the confirmation field. The import button stays disabled until it matches exactly; the file alone is not an independent confirmation.
3. Optionally change the environment name. An empty name keeps the name used when the package was generated.
4. Select **Import and verify**. The controller verifies only the saved candidate addresses through the same managed SSH transport and strict host-key pin used for analysis, then registers the environment. The card turns available (green) and shows the registered environment and address.

The environment name can be changed later in **Workspace settings → Advanced**. Renaming does not change the installation identity, host key or media.

<a id="windows-worker-states"></a>

## Card states and recovery

| State | Meaning | Next action |
| --- | --- | --- |
| Installer package generated | The package exists on this controller but no install result was imported. | Install the media in the VM, then import the result. |
| Waiting for connection | The install result was imported, but no candidate address accepted a verified managed SSH connection yet. | Start the VM with its management network attached and select **Retry connection**. |
| Available | Verification succeeded and the environment is registered. | Use it like another registered environment. |

If the VM address changed, start the setup again in the VM without arguments. It does not reinstall the worker; it re-enumerates the current adapter addresses and saves a refreshed install result. Import that file again instead of resetting the worker. If the certificate-like host key of a previously registered worker changed, import fails and the existing connection is not replaced.

Other failures show the backend detail plus an actionable hint:

- Host-key mismatch or a receipt generated for another controller/installation: import is refused and nothing is overwritten. Recreate the media on this controller and install that media in the VM.
- The release catalog does not publish a media template: choose the local template ZIP built for this version.
- No eligible management network: connect the VM's host-only or management adapter and reload the network list.

<a id="windows-worker-limits"></a>

## Limits and trust boundaries

- The card does not deploy or restore VMware VMs, snapshots or guests. Prepare and clean up the disposable VM yourself.
- Verification is bounded to the address candidates in the install result; it never scans the management network. A stopped VM therefore stays pending rather than reaching a guessed address.
- A successful import proves the worker's identity and management reachability through the recorded host key. It does not prove hypervisor isolation or production readiness; see the [Windows component guide](../../workers/windows/README.md) for the guest controls and remaining limits.
- Generated media is controller-specific and credential-free for the controller, but it still authorizes a worker to this controller. Treat it as sensitive, and retire the VM rather than reusing it after an unreviewed install attempt.
