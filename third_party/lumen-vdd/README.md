# Lumen VDD 0.2.3 (prebuilt)

The virtual display driver Lumen uses for virtual monitors (the host's option in the installer).
These files are the signed package from the **Lumen VDD v0.2.3** release, unchanged:
https://github.com/aimbyLeet/lumen-vdd/releases/tag/v0.2.3 (`lumen-vdd-0.2.3.zip`, without `lumen-vdd-ctl.exe`,
a tool for checks by hand).

- Source, build and signing: https://github.com/aimbyLeet/lumen-vdd
- Checksums: `SHA256SUMS`; `deploy/build-installer.ps1` checks them before packing.
- The package is signed with the self-signed certificate `CN=Lumen VDD Dev` (`lumen-vdd.cer`); `lumen-vdd-setup.exe` puts it
  into the machine's trusted roots when it installs the driver.
- License: MIT (`LICENSE-MIT`).

To build the driver yourself, build the lumen-vdd repository and pass its package folder:
`deploy\build-installer.ps1 -VddPackage <lumen-vdd>\build\package` (the checksums are then not checked).
