# Compatibility

Spaceman is macOS-only. The app plist declares macOS 14.0 as its minimum, but the declared minimum,
newer macOS releases, and supported CPU architectures have not yet been validated on clean machines.
Treat compatibility as experimental rather than guaranteed.

The source build requires:

- Xcode command-line tools with `codesign` and `security`;
- Rust 1.100.0 from `rust-toolchain.toml`;
- a `Developer ID Application` signing identity.

The documented numbered-workspace shortcuts assume an ANSI number row. Other keyboard layouts may
not produce every required physical shortcut.

`Launch at Login` uses macOS Service Management's `SMAppService` and therefore requires a properly
signed application bundle. macOS may require explicit approval under **System Settings > General >
Login Items & Extensions**.

Accessibility grants live in **System Settings > Privacy & Security**. The list is named
`Accessibility` through macOS 26, `Device Control and Data Access` on macOS 27.0 through 27.2, and
`System & App Protection Override` on macOS 27.3 and later. macOS 27.3 no longer accepts a dragged
app tile in that list, so the setup guide asks for a click on the Privacy & Security notification or
the list's `+` button instead.

Spaceman observes normal-level (`kCGWindowLayer = 0`) Accessibility elements with role `AXWindow`
and subrole `AXStandardWindow` that are on screen, not minimized, not in native fullscreen, centered
on an attached display, and expose a settable position. Standard windows with an enabled fullscreen
button (or borderless regular-app windows without close/fullscreen buttons, such as Ghostty with
`window-decoration = false`) and a settable size can tile. Modal windows, standard windows whose
fullscreen button is disabled or absent while a close button is present (such as file Open/Save
panels, settings windows, and splash screens), movable non-resizable windows, and windows matched by
a `float` rule retain their frame but park and reveal with their logical workspace. Always-on-top
overlays at non-zero window layers (`kCGWindowLayer != 0`, such as popped-out Slack Huddles and
Picture-in-Picture windows), buttonless accessory-application windows, utility panels, dialogs with
other subroles, and applications with incomplete or unusual Accessibility implementations remain
global and are ignored so Spaceman never fights their bounds.

An already tiled window keeps its place when Accessibility identifies the same window in native
fullscreen. Exiting fullscreen reapplies the workspace layout.

Some applications reposition their own windows. If a parked window reappears on screen while its
application is frontmost, the user summoned it: the stale recovery frame is discarded and the window —
tiled or floating — is re-homed to the workspace it reappeared on. If it reappears in the background —
Electron applications re-assert saved bounds on screen-parameter changes — it is quietly pushed back
off screen with its recovery frame intact. If an application undoes several re-parks in a row, the
reappearance is honored after all and the window is re-homed. A window whose reappearance is honored
twice within five minutes becomes parking-exempt for the rest of the session: it remains owned and
tiled on its workspace, but is left visible when another workspace is active rather than being fought
indefinitely. Parking-exempt windows appear in the `parking_exempt` list of `spaceman ctl debug`; an
`ignore` config rule instead opts an application out of management permanently. If a newly identified
frontmost window appears at the exact one-pixel-edge position produced by Spaceman parking, it is
revealed so normal workspace adoption can recover an app that relaunched with stale saved bounds.
Other windows an application keeps off screen itself are left alone rather than parked, and never
recorded as recovery frames.

Apps run as a bare executable rather than opened as an `.app` (for example a development build
started from a terminal) have no LaunchServices launch date; Spaceman tells their process
incarnations apart by kernel process start time instead and manages their windows like any other.

Ghostty native tabs are the primary stable-logical-window target. Replacement is conservative: when
the evidence is ambiguous, Spaceman preserves existing logical state rather than guessing. Other apps
with physical-window identity churn are not implicitly guaranteed to behave like Ghostty.

Unresolved Ghostty slots still count toward application workspace affinity, and pending replacement
candidates are reserved against adoption by another workspace while reconciliation settles.

Multiple displays are supported dynamically. `secondary` means the first non-main display reported by
macOS. Exact duplicate display names are ambiguous; prefer UUIDs. Missing assigned displays fall back
to main until they reconnect.

Parking checks the full window rectangle against all attached displays and chooses an exposed edge,
including when the window's assigned display has neighbors on both sides. A window counts as on
screen when more than its one-pixel parking strip shows on a display, so one wider than its tile that
overflows the display edge is still parked. When a display is added or removed, every hidden window
is re-parked against the new arrangement before focus follow or reappearance handling runs (a failed
write is retried each cycle), and a recovery frame whose display is gone is restored onto the main
display. Empty gaps between
staggered displays are not treated as a window reappearing on screen. Native clamping and application
behavior still require live qualification on a particular display arrangement.

Spaceman uses the private `_AXUIElementGetWindow` symbol. It requires no SIP change, but an OS update
could remove or alter that symbol. Mac App Store distribution is not supported.
