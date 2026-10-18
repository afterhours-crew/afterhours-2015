# Afterhours terminal preview

1. Download `Afterhours-windows-x64.zip` from a GitHub release that includes it.
2. Extract the ZIP. Put `Afterhours.exe` beside your installed `NFS16.exe`.
3. Close the EA app and any running game. Double-click `Afterhours.exe` as an
   ordinary user. Keep its terminal open while playing.
4. Follow the terminal's steps. It checks the supported build, reads installed
   content, creates a separate local save, starts the local server and opens the
   game. Wait for **Ready to play** before pressing Space in the game.
5. Quit the game normally. Afterhours stops its server and preserves your save.
   The terminal stays open for you to read the result; press Enter to close it.

The first extraction can take several minutes. The terminal names each category
being extracted and reports completion. Later starts verify the cache and report
when extraction is skipped. Local service configuration is checked each time.
No percentage or estimated time is inferred from the amount of game loading.

Blue headings show the current stage; green **OK** means it completed. Yellow
**WAIT** means you need to respond to an activation window. Red **ERROR** explains
why a step stopped. Text labels remain readable without color. Redirected output
and `NO_COLOR=1` use plain text; JSON output never includes color codes.

New local tester accounts start at **REP level 69** (one below the installed cap
of 70) with **$10,000,000** for trying cars and upgrades. This is a one-time
starting balance, not money or REP earned by driving. Existing saves keep their
current level and balance; updates and repeat launches never top them up.

## Activation and installation requirements

Use an installed, previously activated Need for Speed (2015) PC game on 64-bit
Windows. Keep its existing license cache and EA registration. Afterhours does
not activate the game or remove its activation requirements. Fresh installations,
expired activation caches and missing EA registration are outside established
support. The supported `NFS16.exe` SHA-256 is
`92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df`.

If EA's activation dialog appears, the terminal says it is waiting. Complete the
dialog yourself in its own window; this may require connectivity. Afterhours
does not read or submit credentials. Local play is not ready while activation
is pending. Ctrl+C requests cancellation and cleanup; you can also run
`Afterhours.exe stop` from another terminal. If activation replaces or exits the
original game process, read the failure, finish activation and start again.

The executable includes the server and terminal launcher. It requires no Rust,
Python, PowerShell setup script, WebView2 or downloaded server. All gameplay
services bind to loopback. Installed activation remains a separate dependency.
Game files are read-only. The temporary routing change lasts only for the owned
game process. New local drivers start after the currently unsupported prologue;
this development preview does not implement the complete game.

## Saves, updates and feedback

Saves, generated content and logs live in `%LOCALAPPDATA%\Afterhours2015`.
Replacing `Afterhours.exe` does not replace that directory. Back it up while the
game and server are stopped before trying a new preview. Read each release's
save compatibility notes before downgrading. Startup does not check for updates.

The terminal prints the session log directory. For a report, include the release
name, `build-info.json`, the last named step, what you did, and expected/actual
results. Review logs for personal paths and identifiers before sharing. Avoid
uploading saves, databases, game assets, activation logs or raw protocol captures.
Default diagnostics contain metadata; raw protocol recording is off.

If launched from an existing terminal, explicit commands are also available:

```text
Afterhours.exe --version
Afterhours.exe check --game-dir "F:\EA Games\Need for Speed"
Afterhours.exe setup --game-dir "F:\EA Games\Need for Speed"
Afterhours.exe start --game-dir "F:\EA Games\Need for Speed"
Afterhours.exe status
Afterhours.exe stop
```

Use the same `--data-dir "D:\Afterhours saves"` on each command to choose another
dedicated local save directory. Keep it outside the installation. Explicit
commands return an exit code without waiting for Enter.

## Source and licenses

Source for this exact build is linked by `build-info.json` and the GitHub release.
Afterhours source is MPL-2.0; see `LICENSE`. Dependency notices accompany the
package in `THIRD-PARTY-NOTICES.txt`. No game content is included in this package.
