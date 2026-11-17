# Running lapd as a service

`lapd` is designed to run as a long-lived background service on the machine that
holds the photo library. The project ships **templates only** — no
`lapd install-service` subcommand (decision 2026-11-17, §3.8): each platform has
a mature service manager, so wrapping it ourselves would add platform branches
and tests for little gain.

Adjust the paths/arguments in each template to your install (where `lapd` lives,
and where the data directory should be) before installing.

## Common notes

- The data directory (`--data-dir`) holds the config, libraries and the lock
  file. It is the same directory the desktop app uses
  (`dirs::data_local_dir()/com.julyx10.lap` by default), so the service and the
  desktop share one library set — **do not run both at once** (the `lapd.lock`
  file enforces one writer; see plan §3.7).
- Keep `models/` and `ffmpeg/` **beside the `lapd` binary**: the resource root
  is the executable's own directory.
- The service account must be able to read the photo library folders.

## Linux — systemd user unit

```bash
mkdir -p ~/.config/systemd/user
cp scripts/service/lapd.service ~/.config/systemd/user/
# edit ExecStart / --data-dir, then:
systemctl --user daemon-reload
systemctl --user enable --now lapd
# start at boot without a login session:
loginctl enable-linger "$USER"
```

## macOS — LaunchAgent

```bash
cp scripts/service/com.julyx10.lapd.plist ~/Library/LaunchAgents/
# edit the program path and --data-dir, then:
launchctl load -w ~/Library/LaunchAgents/com.julyx10.lapd.plist
```

The first run needs Full Disk Access (System Settings → Privacy & Security →
Full Disk Access) or lapd cannot read the photo folders.

## Windows — WinSW

```powershell
# put winsw.exe beside the template as lapd-service.exe, then:
.\lapd-service.exe install
.\lapd-service.exe start
```

The service account needs read access to the library folders; **LocalSystem is
discouraged** (use a dedicated user or the login user). NSSM works the same way.
