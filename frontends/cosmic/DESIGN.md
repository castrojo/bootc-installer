# TunaOS COSMIC installer design

This repository contains the libcosmic frontend for the
[fisherman](https://github.com/tuna-os/fisherman) bootc installation backend.
The frontend gathers and validates the user's choices, writes the fisherman
recipe, and presents the backend's progress and result.

## Flow

The installer walks fifteen screens, in GNOME parity order. Conditional
screens are skipped rather than shown blank (see `page_visible`):

1. **Requirements** blocks on fisherman's unmet requirements (RAM, CPU,
   firmware) with per-gate cards. RAM and CPU offer "continue anyway";
   unsupported firmware offers only power off (plus a firmware-setup
   reboot). Shown only while a gate stands.
2. **Welcome** introduces the detected TunaOS product.
3. **Choose a system** offers the image catalog leaves plus a custom-URL
   row. Skipped on live media and with fewer than two leaves.
4. **Language**, 5. **Keyboard**, 6. **Time zone** apply the live
   session's locale settings choices via `localectl`/`timedatectl` on the
   host (best-effort; never blocking).
7. **Network** runs the connectivity check (`ghcr.io:443`, then
   `8.8.8.8:53`) with a Recheck button and holds Continue until it
   passes. Skipped on live media.
8. **Select a disk** lists the disks that `fisherman probe --json` offers and requires a
   selection before continuing.
9. **Options** collects the hostname, filesystem, and encryption settings.
10. **Account** creates the first user. Skipped when the selected image
    (or a custom URL) provides its own.
11. **Tour** shows the selected image's carousel slides, if it carries any.
12. **Use your phone** serves the LAN companion form: the phone's
    submission fills the account entries. Plain HTTP with a session
    token (see `flatpak/DEPENDENCIES.md` for why not TLS); the server
    stops when the page is left.
13. **Confirm** summarizes the choices and marks installation as a destructive
    action.
14. **Installing** writes a mode-0600 recipe under `XDG_RUNTIME_DIR`, starts
    fisherman, and streams its output.
15. **Finished** reports success or failure and prompts the user to restart.

Exiting mid-install (the header's Exit button, confirmed) signals the
install wrapper's process group before quitting, so no orphaned root
install survives the wizard.

The header bar shows the current step and overall progress. Navigation is
reversible through Confirm; starting installation is the boundary after which
the UI no longer offers Back or Continue.

## COSMIC integration

The app implements `cosmic::Application` and uses libcosmic widgets, spacing,
colors, and the user's active accent. Structural colors come from the COSMIC
theme rather than fixed RGB values so that light and dark themes remain
legible. The destructive action and status text use the corresponding COSMIC
semantic styles.

The layout is a centered, single-column wizard. Device choices use COSMIC list
items, configuration choices use settings sections, and the installation log
uses monospaced text. Copy is short, factual, and sentence case.

## Product and installation data

The displayed product name comes from the host's `PRETTY_NAME` in
`/etc/os-release` (or `/run/host/etc/os-release` in the Flatpak). The live ISO
path can leave the recipe's image field empty so fisherman installs the booted
bootc image without downloading it. Embedded OCI stores are passed through as
additional image stores.

## Capture mode

Setting `TUNA_CAPTURE_DIR` replaces disk and installation data with fixtures
and renders every screen for the
[GUI walkthrough](docs/gui-walkthrough.md). Capture mode refuses
`StartInstall`, so generating documentation cannot start fisherman or modify a
disk. The harness reads back the frame presented by wgpu and rejects blank or
nearly blank renders.

The CI capture runs under Xvfb with Mesa lavapipe. See the walkthrough and the
`capture` recipe in `justfile` for local prerequisites and commands.

## Accessibility and quality

Controls use explicit labels and disabled states to prevent incomplete input
from advancing. Theme-derived semantic colors preserve contrast across COSMIC
themes. Changes to the wizard should be checked with keyboard navigation and
by reviewing all generated screens in both the ordinary and variant-branded
capture artifacts.
