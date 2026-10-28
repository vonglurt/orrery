# Orrery and copal-tm as Mac Applications: Bundles, Icons and Ad-hoc Signatures

*Lab Report — IEEE Format · 28 October 2026*

Copyright (c) 2026 Paul Richeson. MIT licensed, as orrery and copal-tm.

The same report is kept in both projects, as `docs/mac-app-lab-report.md`
in each. Companions:
- orrery's `tools/mac-app.sh` and `tools/mac-icon.py`;
- copal-tm's `tools/mac-app.sh`, `tools/mac-icon.py` and its `Makefile`
  (`make install`);
- copal-tm's `docs/design-lab-report.md` §X-B, on the macOS back end that
  gives the second application something real to show.

---

## Abstract

orrery and copal-tm were installed on the Mac that coordinates the fleet as
command-line programs. Each was started from a terminal, and orrery with a
command line five options long. Both are now also Mac applications in
`~/Applications`, opened from Finder, Launchpad, Spotlight and the Dock.

| | Orrery.app | Copal TM.app |
| --- | --- | --- |
| What it is | orrery's native AppKit window, started with the grove console's options | a launcher: Terminal opens a window running copal-tm |
| How it is made | `tools/mac-app.sh [OPTIONS]` | `make install` (`APP=0` skips it) |
| Size | 1.4 MB | 968 KB |
| Signature | ad hoc, sealed (`codesign -v --deep` passes) | ad hoc, sealed |

Each bundle has:
- a copy of its program;
- a small launcher script;
- an `Info.plist`;
- an icon drawn by a short Python program in the project's own colours;
- an ad-hoc code signature.

Neither project gained a dependency: the tools used are `sips`,
`iconutil`, `codesign` and Python's `zlib`, all part of macOS.

Two faults were found and fixed on the way:
- on a Mac's case-insensitive disk, a launcher named `Orrery` overwrote the
  binary `orrery`;
- macOS refuses an unprivileged reader most of root's processes (in the
  macOS back end).

The signatures are **ad hoc**: they satisfy Apple silicon's rule that all
code is signed, and they let the bundles open on this Mac. Gatekeeper's own
assessment (`spctl -a`) **rejects** both, as it rejects every bundle without
a Developer ID. The bundles are signed applications for this Mac. They are
not yet ones to hand to another person's Mac (§VII).

## I. Objective

Paul's prompts:

> after make install. how do I get an application launcher like in
> Applications folder. Can I also get this install for Orrery.

> write a lab report about making orrery and copal-tm into a mac signed
> applications. Place documentation in both projects.

1. After `make install`, copal-tm should also be an application in the
   Applications folder.
2. The same for orrery, which has no Makefile.
3. Sign both, and record how, in both projects.

## II. Materials

| Item | Detail |
| --- | --- |
| Machine | the iMac that coordinates grove: Apple M1 (arm64, 4 Efficiency and 4 Performance cores), 16 GB, macOS 26.6.2, a case-insensitive APFS disk |
| orrery | `~/code/orrery`, no dependencies; `--gui` opens an AppKit window through the Objective-C runtime (`src/mac.rs`) |
| copal-tm | `~/code/copal-tm` 0.1.4, no dependencies, a terminal program; since commit 0a8730f it reads macOS natively |
| Tools | `sips` and `iconutil` (icons), `codesign` and `spctl` (signing and its check), `lsregister` (Launch Services), `open` |
| Precedent | treadcore's `scripts/install.sh`, which builds `~/Applications/Treadcore.app` |

## III. Why these choices

| Question | Answer | Because |
| --- | --- | --- |
| `/Applications` or `~/Applications`? | **`~/Applications`**, settable with `APPDIR` | Treadcore.app is already there; it needs no administrator; Finder, Launchpad and Spotlight index it the same |
| Should the bundle run the installed binary or carry its own? | **its own copy**, in `Contents/MacOS` | a bundle that carries its code can be signed and sealed whole; it does not break when `~/.local/bin` changes |
| How does a terminal program become an app? | `open -a Terminal FILE` on the copy inside the bundle | Terminal runs an executable file in a new window. AppleScript (`tell application "Terminal" to do script`) would ask for Automation permission on first use; `open` does not |
| Does the launcher app need a Dock icon? | **no** (`LSUIElement`) for Copal TM; **yes** for Orrery | Copal TM's launcher exits once Terminal has the window, so a Dock icon would only bounce. Orrery's launcher `exec`s the window itself, so the Dock icon is orrery's own |
| How does orrery get its options? | written into the launcher when the app is made: `tools/mac-app.sh --operator paul --fleet grove --fleet-cmd … --queue …` | an app opened from Finder has no command line; running the script again changes them |
| What PATH and output does it have from Finder? | `~/.local/bin`, Homebrew and `/usr/local/bin` put back; output appended to `~/Library/Logs/orrery.log` | Finder starts an app with a bare PATH, and orrery runs `copal fleet` and Python tools; with no terminal, the log is where it speaks |
| How are the icons made? | a Python drawing at 1,024 px, anti-aliased by signed distance, written as RGBA PNG with `zlib`; `sips` scales to the ten iconset sizes; `iconutil` makes the `.icns` | no image library to install; the drawing lives in the repository and can be regenerated; transparency for the corners (PPM, which `sips` also reads, has none) |
| Which signature? | **ad hoc** (`codesign --sign -`), the binary and then the bundle | Apple silicon runs only signed code. Ad hoc needs no certificate, and is what the Rust linker already applies. A Developer ID costs an Apple Developer Program membership and is Paul's decision (§VII) |
| How is a failed build kept from breaking the app? | built as `NAME.app.new`, then moved over the old | a failure leaves the working bundle in place |

## IV. The bundles

![orrery's icon](mac-app/orrery-icon.png) ![copal-tm's icon](mac-app/copal-tm-icon.png)

*Fig. 1. The icons at 256 px. orrery's (left): a brass sun and three tilted
orbits on the window's paper (`draw.rs`, light theme: base `#fce2ab`, line
`#c4a870`, accent `#87704f`). copal-tm's (right): the CPU history's green
trace over its grid and four ladder meters, in the program's own hues
(green `#7cf05a`, cyan `#4fe3e8`, orange `#f0a03a`, yellow `#f2e34a`).*

```
Orrery.app/Contents/
  Info.plist            org.copal.orrery, CFBundleExecutable = launch
  MacOS/launch          PATH, the log, then exec orrery --gui OPTIONS
  MacOS/orrery          the release binary
  Resources/orrery.icns 486,037 bytes
  _CodeSignature/

Copal TM.app/Contents/
  Info.plist            org.copal.copal-tm, CFBundleExecutable = launch, LSUIElement
  MacOS/launch          exec open -a Terminal "$(dirname "$0")/copal-tm"
  MacOS/copal-tm        the release binary
  Resources/copal-tm.icns 178,554 bytes
  _CodeSignature/
```

The launchers, as written:

```sh
#!/bin/sh
# Orrery.app's launcher, written by /Users/user/code/orrery/tools/mac-app.sh.
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
mkdir -p "$HOME/Library/Logs"
exec "$(dirname "$0")/orrery" --gui '--operator' 'paul' '--fleet' 'grove' '--fleet-cmd' '/Users/user/.local/bin/copal fleet' '--queue' '/Users/user/code/treadcore/build/queue/grove' >>"$HOME/Library/Logs/orrery.log" 2>&1
```

```sh
#!/bin/sh
# Copal TM.app's launcher, written by copal-tm's tools/mac-app.sh.
exec open -a Terminal "$(dirname "$0")/copal-tm"
```

## V. Method, to repeat

**orrery** (no Makefile; the script builds, installs and bundles):

```sh
cd ~/code/orrery
tools/mac-app.sh --operator paul --fleet grove \
    --fleet-cmd "$HOME/.local/bin/copal fleet" \
    --queue "$HOME/code/treadcore/build/queue/grove"
# also: tools/mac-app.sh            (the fleet alone, --gui added)
#       APPDIR=/Applications tools/mac-app.sh ...
#       tools/mac-app.sh --uninstall
```

It runs `cargo build --release` and installs `~/.local/bin/orrery`. It
writes the bundle, draws the icon, signs the binary and then the bundle,
swaps the bundle in, and registers it with Launch Services. It ends by
printing the command line the app will run.

**copal-tm:**

```sh
cd ~/code/copal-tm
make install            # ~/.local/bin/copal-tm and ~/Applications/Copal TM.app
make install APP=0      # the binary only
make uninstall          # both
```

**Checking a bundle:**

```sh
codesign -v --deep ~/Applications/Orrery.app      # silent and 0: sealed
codesign -dv ~/Applications/Orrery.app            # Signature=adhoc
spctl -a -vv ~/Applications/Orrery.app            # Gatekeeper's verdict
open ~/Applications/Orrery.app; tail ~/Library/Logs/orrery.log
```

## VI. Results

| Check | Orrery.app | Copal TM.app |
| --- | --- | --- |
| `codesign -v --deep` | passes | passes |
| `codesign -dv` | `Identifier=org.copal.orrery`, `Signature=adhoc`, `TeamIdentifier=not set`, sealed resources v2 | `Identifier=org.copal.copal-tm`, the same otherwise |
| `spctl -a -vv` (Gatekeeper) | **rejected** | **rejected** |
| Opened with `open` | the window opened; the log: `orrery: window open, backing scale 2  posture: operator`; the process is `Orrery.app/Contents/MacOS/orrery --gui --operator paul --fleet grove …` | a Terminal window running `Copal TM.app/Contents/MacOS/copal-tm` (pid 35522) |
| Icon drawing | 16.5 s for 1,024² pixels in Python | about the same |
| Build of the bundle after the binary | under a minute, mostly the icon | the same |

**The faults, in the order found:**

1. *The launcher overwrote the binary.* The first Orrery.app named its
   launcher `Orrery` and its binary `orrery`, in the same folder. On a
   case-insensitive disk those are one file: writing the launcher replaced
   the binary. `codesign` then refused the bundle ("code object is not
   signed at all … `Contents/MacOS/orrery`"). That message read at first
   like a missing signature, and the script's comment said so. Listing the
   folder showed a single 409-byte file; the error was the collision. Both
   launchers are now named `launch`, and the comment is corrected.
2. *An unprivileged user cannot read root's processes on macOS* (found the
   same day while writing copal-tm's macOS back end, LR in copal-tm
   §X-B):
   - `proc_pidinfo(PROC_PIDTBSDINFO)` refused 221 of 558 processes;
   - `sysctl KERN_PROC_ALL` lists them all, as `ps` reads them;
   - their CPU and memory stay unreadable (`/bin/ps` is setuid root to get
     them), and Copal TM draws a dash there, not a zero.

**Errors and limits.** The Gatekeeper verdict and the launches were checked
on this one Mac. Nothing was tried on a second Mac, so §VII's account of one
is Apple's documented behaviour, not a measurement.

## VII. Discussion: what "signed" means here

macOS has three levels, and these bundles are on the first:

| Level | What it takes | What it gives |
| --- | --- | --- |
| **Ad hoc** (these) | `codesign --sign -`; no certificate | Apple silicon runs the code; the bundle is sealed, so a changed file is detected (`codesign -v` fails). Opens on the Mac that built it, and on a Mac it is copied to without the quarantine flag (`scp`, `rsync`) |
| Developer ID | an Apple Developer Program membership; `codesign --sign "Developer ID Application: …" --options runtime` | a named, revocable signer; Gatekeeper still asks for notarization |
| Notarized | Developer ID, then `xcrun notarytool submit … --wait` and `xcrun stapler staple` | `spctl` accepts; opens with no warning after a download |

A bundle that is downloaded, mailed or AirDropped arrives quarantined.
Gatekeeper refuses an ad-hoc one: the person must Control-click, choose
Open, and confirm, or remove the flag (`xattr -dr com.apple.quarantine`).
For the fleet's console on its own Mac, ad hoc is what is wanted: nothing to
renew and nothing to pay. For handing orrery or Copal TM to another person,
the next step is a Developer ID. The scripts' two `codesign` lines take an
identity in place of `-` and are otherwise ready for it. That is Paul's
decision, and it costs a membership.

Two smaller points remain open:
- Copal TM uses Terminal. A person who lives in iTerm or Ghostty would want
  the launcher to name that program instead.
- Orrery's options are fixed when the app is made. A second fleet would
  want a second app, or a choice in the window.
