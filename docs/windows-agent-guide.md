# Windows setup and test guide (for an AI agent)

You are an AI coding agent on a Windows PC. Your job is to set up the
toolchain, build the `windows-port` branch of Space Hog Inspector, run its
tests, run the hands-on checks below, and write up what you find. Follow
the steps in order. Do not skip a step without saying so in the report.

## What this project is

Space Hog Inspector is a disk-usage treemap app written in Rust with
egui/eframe. It started as macOS-only. The `windows-port` branch moves all
OS-specific code into `src/platform/` and adds `src/platform/windows.rs`.
That Windows code compiles but **has never been run on Windows**. You are
the first to run it. Your results decide what gets fixed.

Repository: https://github.com/pfoty/space-hog-inspector (branch
`windows-port`).

## Rules

1. **Don't change source code.** Don't edit anything under `src/`, or
   `Cargo.toml` or `Cargo.lock`, and don't commit or push. Report problems
   instead; they will be fixed on the Mac. The only file you create in the
   repository is `windows-test-report.md` (step 6).
2. **Ask before installing anything** and before anything that needs
   administrator rights. Installers show a UAC prompt that only the user
   can approve. Say what you're about to install and wait for a yes.
3. **Only delete what you created.** The only folder you may delete is the
   test folder from step 4 (`%TEMP%\sh-test`). Never move anything else to
   the Recycle Bin, and never empty the Recycle Bin.
4. **Record evidence, not guesses.** For every check, write the command you
   ran and the relevant output, or what you saw on screen. If you couldn't
   do a check, write "not done" and why.
5. **Leave the UI to the user if you can't drive it.** Some checks need
   clicks, right-clicks and screenshots. If you can't control the screen,
   give the user the exact steps, wait for their answer, and record it.
6. **Leave secrets alone.** Don't read, print or copy tokens, keys or
   passwords, including anything under `%USERPROFILE%\.claude`,
   `%USERPROFILE%\.codex` or `%APPDATA%\npm`.

Use **PowerShell** for every command. When a step says "new PowerShell",
close the window and open a fresh one, so a newly installed tool is on the
PATH.

## Step 1: Check what's already installed

Run each of these and record the output. A "not recognized" error means
the tool is missing.

```powershell
$PSVersionTable.PSVersion
[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
git --version
rustup --version
cargo --version
winget --version
```

Record whether the PC is **X64** or **Arm64**. Rust picks the right
toolchain for either, but say which one in the report.

Check for the C++ build tools (the MSVC linker):

```powershell
& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property displayName
```

If that prints a product name, the build tools are installed. If it prints
nothing, or `vswhere.exe` doesn't exist, they're missing. On an Arm64 PC,
also accept `Microsoft.VisualStudio.Component.VC.Tools.ARM64`.

## Step 2: Install what's missing (ask first)

Install only what step 1 showed is missing. Ask the user before each one.

Git:

```powershell
winget install --id Git.Git -e
```

Rust (rustup):

```powershell
winget install --id Rustlang.Rustup -e
```

C++ build tools:

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

If winget can't find that package ID, search for the current one with
`winget search "Visual Studio Build Tools"` and use it with the same
`--override`. If that fails too, ask the user to open the Visual Studio
Installer and add the **Desktop development with C++** workload.

Then, in a **new PowerShell**:

```powershell
rustup default stable-msvc
rustc --version
cargo --version
```

Repeat the `vswhere.exe` check from step 1. Don't continue until `cargo`
works and the build tools show up.

## Step 3: Get the branch, test, lint, build

Pick a working folder the user agrees to. The default is
`%USERPROFILE%\Developer`.

```powershell
New-Item -ItemType Directory -Force "$env:USERPROFILE\Developer" | Out-Null
Set-Location "$env:USERPROFILE\Developer"
git clone https://github.com/pfoty/space-hog-inspector.git
Set-Location space-hog-inspector
git switch windows-port
git log --oneline -1
```

The last line should start with the commit the user expects (`7ec2595` when
this guide was written). If it's newer, record it.

Run each command and record the full output of any failure:

```powershell
cargo test
cargo clippy --all-targets
cargo build --release
```

**Expected:**

- `cargo test`: `test result: ok` with 0 failed. It should include
  `junctions_are_not_followed` and `hard_links_count_once`. Any failure is
  the most important thing to report: copy the test name and its panic
  message in full.
- `cargo clippy`: no warnings. List any that appear.
- `cargo build --release`: builds `target\release\space-hog-inspector.exe`.

If the build fails, stop here. Write the report (step 6) with the full
error and hand it back.

## Step 4: Make a test folder with known sizes

```powershell
$T = "$env:TEMP\sh-test"
Remove-Item -Recurse -Force $T -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$T\keep" | Out-Null
[IO.File]::WriteAllBytes("$T\keep\big.bin", (New-Object byte[] 3000000))
[IO.File]::WriteAllBytes("$T\other.bin", (New-Object byte[] 700000))
[IO.File]::WriteAllBytes("$T\trash-me.bin", (New-Object byte[] 500000))
New-Item -ItemType HardLink -Path "$T\hardlink.bin" -Target "$T\keep\big.bin" | Out-Null
New-Item -ItemType Junction -Path "$T\junction" -Target "$T\keep" | Out-Null
Get-ChildItem $T -Force | Format-Table Name, Length, LinkType, Target
```

That's a 3 MB file plus a hard link to it, a 0.7 MB file, a 0.5 MB file for
the Recycle Bin test, and a junction pointing at the `keep` folder.

## Step 5: Run the app and check it

Start it on the test folder:

```powershell
.\target\release\space-hog-inspector.exe "$env:TEMP\sh-test"
```

If you can't see or control the screen, ask the user to do each check and
tell you what they see. Take a screenshot (or ask for one) wherever a check
says **screenshot**.

### 5.1 Test folder (screenshot)

**Expected:**

- The total in the status bar is about **4.2 MB**.
- The 3 MB file is listed **once**, as `hardlink.bin` or inside `keep`. It
  must not be counted twice.
- `junction` shows as **0 B** and isn't a folder you can zoom into.
- No console window opened along with the app.

### 5.2 File actions on `trash-me.bin`

Right-click `trash-me.bin` in the side list and try each item in turn:

- **Reveal in Finder**: Explorer opens with the file selected.
- **Open**: Windows opens it or asks which app to use. Close whatever
  opens.
- **Copy Path**: then run `Get-Clipboard` and record the result.
- **Move to Trash…**: confirm. The file leaves the list and the total drops
  by about 0.5 MB.

Then check that the file reached the Recycle Bin:

```powershell
(New-Object -ComObject Shell.Application).NameSpace(10).Items() | Where-Object Name -like "trash-me*" | Select-Object Name, Path
```

Ask the user to restore it from the Recycle Bin, then confirm it's back:

```powershell
Test-Path "$env:TEMP\sh-test\trash-me.bin"
```

### 5.3 Start screen and Settings (screenshot)

Close the app and start it with no folder:

```powershell
.\target\release\space-hog-inspector.exe
```

**Expected:**

- The drive ring shows the system drive's used and free space.
- Labels read "Local Disk (C:)" (or the drive's own label) and NTFS.
- Shortcuts are written as **Ctrl+O** and **Ctrl+R**, not ⌘.
- There's **no** Full Disk Access banner.

Open Settings (the gear button or Ctrl+,). It should have no Full Disk
Access section and no "Skip folders…" switch. Change the treemap depth,
close the app, open it again, and check that the change stuck:

```powershell
Get-Content "$env:APPDATA\Space Hog Inspector\settings.json"
```

### 5.4 Real folders: accuracy and speed

1. Scan `%USERPROFILE%\Downloads` (or another folder of a few GB). Record
   the app's total and the scan time from the status bar. Then ask the user
   for the **Size on disk** value in Explorer (right-click the folder →
   Properties). The two should be within a few percent.
2. Scan the whole system drive with **Whole disk** on the start screen.
   Record the total, the time, the "skipped or unreadable" count, and a few
   example skipped paths.
3. Ask the user to start an **administrator** PowerShell and run the app
   from it:

   ```powershell
   & "$env:USERPROFILE\Developer\space-hog-inspector\target\release\space-hog-inspector.exe"
   ```

   Scan the whole drive again and record the same numbers.
4. In that administrator PowerShell, get Windows' own figure for WinSxS:

   ```powershell
   Dism /Online /Cleanup-Image /AnalyzeComponentStore
   ```

   Record "Actual Size of Component Store", and the app's size for
   `C:\Windows\WinSxS` from the admin scan. They should be close. If the
   app's figure is much larger, hard links are being counted twice.
5. Check that junctions weren't followed: the user folder has old junctions
   like `Application Data` and `My Documents`:

   ```powershell
   Get-ChildItem $env:USERPROFILE -Force -Attributes ReparsePoint | Format-Table Name, LinkType, Target
   ```

   In the app, those names should show as 0 B entries, not folders.

### 5.5 OneDrive (only if the PC uses it)

Find a few cloud-only files: they have the "Offline" attribute.

```powershell
Get-ChildItem $env:OneDrive -Recurse -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.Attributes -band [IO.FileAttributes]::Offline } | Select-Object -First 5 FullName, Length
```

Scan the OneDrive folder in the app, then run the same command again.

**Expected:**

- The OneDrive folder appears as a normal folder with its contents.
- Cloud-only files count as about 0 B, not their full size.
- The same files are **still** listed as Offline afterwards, meaning the
  scan didn't download anything.

### 5.6 Other drives

If there's a second drive or a USB stick, scan a folder that contains a
mounted drive, or the drive itself. Try Settings → **Include other mounted
volumes** off, then on, and record what changes.

### 5.7 AI assistant (only if Claude Code or Codex is installed)

```powershell
where.exe claude
where.exe codex
```

Record the paths. Note whether each is a `.exe` or a `.cmd`.

In the app, open Settings → AI Assistant. The paths shown should match.
Scan `%LOCALAPPDATA%` (**Ask AI…** only works on items inside the scanned
folder). Then choose each installed agent in turn and right-click a folder
in the results, such as `Temp` → **Ask AI…**.

**Expected:** an answer appears in the AI tab, and no black console window
flashes up. Ask about another folder and press **Cancel** while it's
running; the question should stop. Record any error message word for word,
especially for an agent installed as a `.cmd`.

### 5.8 Icon and display scaling (screenshot)

1. Close the app and double-click
   `target\release\space-hog-inspector.exe` in Explorer. The hex icon should
   show in the taskbar and in Alt+Tab, and no console window should open.
2. If the display isn't already at 150%, ask the user to set Settings →
   Display → Scale to 150%. Then check that text is sharp and nothing is
   cut off. Ask them to set it back afterwards.

## Step 6: Write the report

Create `windows-test-report.md` in the repository folder (don't commit
it) using this outline, and show it to the user:

```markdown
# Windows test report

- Date:
- Windows version (`winver`):
- Architecture (X64 / Arm64):
- Commit tested (`git log --oneline -1`):
- Rust version (`rustc --version`):

## Setup
What was installed, and any problems.

## cargo test / clippy / build
Pass or fail; full output of failures and warnings.

## Checks
| Check | Result | Evidence |
| --- | --- | --- |
| 5.1 Test folder | | |
| 5.2 File actions | | |
| 5.3 Start screen and Settings | | |
| 5.4 Accuracy and speed | | |
| 5.5 OneDrive | | |
| 5.6 Other drives | | |
| 5.7 AI assistant | | |
| 5.8 Icon and scaling | | |

## Numbers
- Downloads: app total / Explorer size on disk / scan time
- Whole drive (normal): total / time / skipped count
- Whole drive (admin): total / time / skipped count
- WinSxS: app size / DISM "Actual Size of Component Store"

## Problems found
One entry per problem: what you did, what you expected, what happened, and
the exact error text or screenshot name.
```

## Step 7: Clean up

```powershell
Remove-Item -Recurse -Force "$env:TEMP\sh-test"
```

Leave the cloned repository, the build output and the report in place.
Tell the user the report is ready and where it is.
