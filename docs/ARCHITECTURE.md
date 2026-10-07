# Architecture

`mihomo-server-desktop` is an optional Tauri 2 client for a local
[mihomo-server](https://github.com/oodzchen/mihomo-server) instance: a window
showing the service's own Web UI, a tray menu for mode/TUN/node/subscription
control and the service lifecycle, and detection, installation and start of the
local instance. The service never requires it. The service's side of the
contract (API, Web UI, installer, helper) is described in that repository's
`docs/ARCHITECTURE.md`.

## Relationship to the service repository

| Concern | Where it lives |
| --- | --- |
| Management API client (instance discovery, commands, typed readings, WebSocket feeds) | `management-client` crate in the service repository, used here as a git dependency pinned to a service release tag in `Cargo.toml`. Move the tag (and `Cargo.lock`) to pick up changes; the Nix source package fetches the locked revision. |
| Installer the client runs | `install.sh` from the service's latest release; the repository is baked in at build time through `MIHOMO_SERVER_REPOSITORY` (default `oodzchen/mihomo-server`). |
| Service lifecycle | The release's `mihomo-server-user` helper (`MIHOMO_SERVER_HELPER` overrides it). |
| Web UI integration | The service's Web settings page detects the client through `__MIHOMO_DESKTOP_VERSION__` and calls its start-at-login commands over Tauri IPC. |

The client is released from this repository on its own `v*` tags. Releases up
to v0.2.20 were published together with the service, in its repository.

## Client design

The client is a third client of the service API (after the Web UI and the
`mihomo-server` command line) and is installed and
versioned independently of the service. It manages only the invoking user's local
instance, found the same way as by the command line, and re-reads the token file
on every connection instead of storing it. Its tray polls `status` and
`proxy_access`, and re-reads proxies, subscriptions and multi-user facts only when
they change or once a minute. Of the WebSocket feeds it uses only
`/api/streams/preferences`, never `/api/events`, which carries every core log
line. The management window loads the service origin directly and
logs in through the URL fragment, so the browser policy (same origin, no CORS)
is unchanged. It navigates only within that origin; a `target="_blank"` link
to the same origin (the Service page's tokenized management address) goes to
the system browser through `xdg-open`, and no other new window opens. That window may call only the client's own start-at-login commands
(`client_autostart`, `set_client_autostart`, an XDG autostart entry that runs
`mihomo-server-desktop --hidden`), through a capability added at runtime for
exactly the service origin it opened. Only the bundled status page may call the
other commands (detect, install, start, open), which the capability ACL enforces
per window and origin. The Web settings page shows the client switch only when
the client identifies itself (`__MIHOMO_DESKTOP_VERSION__`); the service switch
enables or disables the service's own systemd unit (`set_service_autostart`)
without stopping it, and is unavailable for transient units or a directly
started service. Installation runs the published installer,
whose root step uses polkit (`MIHOMO_INSTALL_ELEVATE=pkexec`) instead of a
terminal sudo prompt. The status page's settings view (gear button) offers the
interface language and a temporary proxy for installation. The proxy is kept
in memory only and used only by the install task: for the installer download
and, through `http(s)_proxy`/`all_proxy`, for the installer's own downloads,
which all run as the user before the polkit step; its test fetches that same
installer URL through it. The page's output panel shows the installer's output
as it is produced: the client sets `MIHOMO_INSTALL_PROGRESS=1` so curl draws its
download bar without a terminal and updates that line in place on each carriage
return, and the installer streams the user activation step line by line instead of
printing it once it has finished.

Release builds report the release tag as their version: CI sets
`MIHOMO_DESKTOP_VERSION` to the tag without its `v` when building and checks
`--version` against it; the Nix source package passes its package version the
same way, and local builds fall back to the Cargo package version. The Web
settings page shows this value, which comes from the client's immutable build
value, never from the service.


The client keeps three things apart: itself (versioned on its own), the
systemd service (`mihomo-server`, started, stopped and restarted through the
`mihomo-server-user` helper like the command line does) and the Mihomo core
the service supervises. The tray manages the first two and proxy settings
through the API; it never starts or stops the core and never presents the
core's version or phase as the service's. The service version shown is read
from the running service binary (`/proc/<MainPID>/exe --version`), since the
API's `status` reports the core. The tray uses tray-icon's StatusNotifierItem
(`ksni`) backend instead of Tauri's default libappindicator, which reports no
clicks and shows no tooltip: a left click opens the management window (or the
status page), the menu is on right click, and its header is a single line of
service state. On Wayland that window is raised only with the XDG activation
token Plasma sends through `ProvideXdgActivationToken` just before the click;
ksni 0.3.6 lacks the method, so `vendor/ksni` is a patched copy
(`[patch.crates-io]`) that stores the token, and the client hands it to GDK
before showing or focusing the window. Its last two items restart and quit the client. Restarting returns
from the Tauri event loop after cleanup releases the single-instance name, then
uses Unix `exec` to keep the application unit's main PID. Nix packages restart
through the wrapper named by `MIHOMO_DESKTOP_LAUNCHER` on the current PATH.
The wrapper appends its own bin directory as a fallback for direct `nix run`.
AppImages use the image path, and ordinary installations use the executable on
disk. The same launcher is used for autostart, so Nix updates keep the current
package and its GTK environment. Details go to the tooltip, and failures are
also sent as desktop notifications (freedesktop D-Bus).

The client follows the instance's interface language: it reads the service's
preference when it connects, so its tray opens in the instance's language, then
follows the `/api/streams/preferences` feed, and keeps a copy in
`$XDG_CONFIG_HOME/mihomo-server-desktop/settings.json` for when no instance
runs. Its status page changes the preference with `set_language` while
connected; otherwise the choice is saved there and becomes the instance's
preference on the next connection if the instance has none. Without either it
uses the system locale.

## Packaging

CI builds deb, RPM and AppImage packages with the Tauri CLI on Ubuntu 24.04 (so
the packages need glibc 2.39 or newer), plus a raw tarball with the binary,
desktop entry and icons for the Nix flake and manual installs. Every asset has a
SHA-256 file, and the release workflow advances the `desktopRelease` pin in
`flake.nix` on main after publishing.

The flake's `desktop-bin` (default on x86_64 Linux) patches that tarball;
`desktop-source` builds the locked sources. Both write `nix-installation.json`
beside the executable, so the client treats the installation as Nix-owned: the
wrapper selects a packaged helper that starts the declared instance through the
service flake's `server-bin` and blocks the script installer. GTK environment
variables and ownership helpers share one wrapper with a fixed
`mihomo-server-desktop` argv[0], so Wayland can match the window to its
installed desktop entry and icon. `nixosModules.default` provides
`programs.mihomo-server-desktop`, to be imported next to the service flake's
module.
