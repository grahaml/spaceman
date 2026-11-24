# A keyboard-first Spaceman desktop

Start with the defaults a Mac user encounters every day. Opening a terminal, editor, browser,
and fourth work window should produce a useful arrangement. The keyboard should be enough to
move between them, and a restart should put them back. This takes its cue from Omarchy’s emphasis
on a usable desktop out of the box. [Omarchy](https://omarchy.org/)

### 1. Give four windows equal quadrants

Open four tiled windows together and one at a time. Verify their frames, move focus around the
grid, close and reopen a tile, then add a fifth window and return to four. Repeat with saved
layouts and apps that enforce a minimum width or height. Automated checks cover trying both grid
orientations and restoring the preferred one on a larger display. The remaining acceptance check
is a four-window live run on a disposable workspace, including a pair of size-constrained apps.
[Tiling](features/tiling.md)

For five tiles, replay a mix of wide and tall apps before choosing a default. Compare the current
split with two rows of three and two tiles. Record each requested frame and the frame the app
actually accepts, check for overlaps, then close the fifth window and verify the four-tile layout.

### 2. Keep a browser in its tile through native fullscreen

Reproduce the documented loss of a browser’s slot after fullscreen video. Teach the observation
cycle to distinguish a fullscreen window from a closed one. Enter fullscreen for longer than the
omission grace period, exit, and verify that the same slot returns to its workspace and quadrant.
Closing the window afterwards should still release its tile. [Fullscreen Layout](features/fullscreen-layout.md)

### 3. Make the daily keyboard workflow predictable

Exercise directional focus, swap, workspace move, and return as one sequence. Include a second
display and a floating dialog. Verify the focused window after each shortcut and ownership after
a cross-display round trip. Run the sequence through the fake backend and with disposable live
windows. Fix and add a regression check for each reproducible divergence.

### 4. Make the shortcuts discoverable

Connect the first-run tour to the current bindings in Settings › Shortcuts. A first-time user
should be able to discover how to focus a neighbor, move a tile, switch workspaces, and undo a
move without opening the manual. Repeat with a remapped modifier and confirm that every
displayed chord matches the action it invokes. [Controls](controls.md)

### 5. Qualify recovery and upgrades

Exercise permission onboarding, pause, restart, upgrade, and uninstall on a clean supported Mac.
With windows parked on another workspace, verify that pause and quit restore them on-screen,
restart restores the arrangement, and a failed restore retains its recovery record. Record
the build, OS, display setup, and observed result for each run. Also exercise a visible workspace
with one window stranded off-screen. Check its actual frame after recovery and the parked record
after a failed write. [Recovery checks](features/restarts-and-recovery.md)
and [release checks](releasing.md)
