# Change the game's menu Back key without a mod

Need for Speed (2015) can use **F9 instead of Escape** for its menu Back action
through its normal user-profile settings. No mod, game binary patch, AutoHotkey,
administrator access or change to Computer Use is needed. This is useful when
Escape also stops a desktop automation session.

**Reapply the override before each launch.** The supported game accepts it for
the running session but rewrites the hidden binding when saving settings. A
normal quit followed by a fresh launch restored Escape in the live test. Keep
the settings file writable so other preferences can still be saved.

The relevant game binding is `ConceptDeactivate`. It is hidden from the in-game
key-binding screen. `ConceptBack` is a different action, normally Backspace;
changing that action does not replace Escape in these menus.

## Set F9

Close the game first. In an ordinary PowerShell window, locate and back up the
profile before editing it:

```powershell
$documents = [Environment]::GetFolderPath('MyDocuments')
$profilePath = Join-Path $documents 'Need for Speed\settings\PROFILEOPTIONS_profile'
if (-not (Test-Path -LiteralPath $profilePath -PathType Leaf)) {
  throw 'Game profile not found. Run the game once or locate its settings folder.'
}
$backupPath = $profilePath + '.before-menu-key-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
Copy-Item -LiteralPath $profilePath -Destination $backupPath -ErrorAction Stop
notepad.exe $profilePath
```

Find all lines starting with `GstKeyBinding.default.ConceptDeactivate.` and
replace that group with these eight lines. Preserve every other setting:

```text
GstKeyBinding.default.ConceptDeactivate.0.axis 0
GstKeyBinding.default.ConceptDeactivate.0.button 67
GstKeyBinding.default.ConceptDeactivate.0.negate 0
GstKeyBinding.default.ConceptDeactivate.0.type 0
GstKeyBinding.default.ConceptDeactivate.1.axis 24
GstKeyBinding.default.ConceptDeactivate.1.button 38
GstKeyBinding.default.ConceptDeactivate.1.negate 0
GstKeyBinding.default.ConceptDeactivate.1.type 3
```

Save the file without changing its name and start the game. The menu footer
should show **F9 Back**. This changes the game's action binding; it does not
generate an Escape keypress or remap any operating-system/automation shortcut.

## Restore the original binding

Close the game. Replace the entire `ConceptDeactivate` group with its original
lines from the backup. This preserves settings
changed since the backup. Alternatively, restore the complete backup if you want
all profile settings reverted to that point.

## Validation

Validated on the supported PC build with `NFS16.exe` SHA-256
`92aa6ff4b5f8d0f033ca7e88cb86cc64b42b1d2412c4294905eb22950616e4df`:
the Play/AllDrive submenu displays F9 Back and F9 returns to the main menu.
F9 also returns from air-filter details to performance categories, then to
customization and the main garage. Computer Use remained active throughout.
Other game builds and arbitrary key assignments have not been validated.
