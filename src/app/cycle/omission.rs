use super::*;

/// One prompt retry while AX and the window server settle, without raising
/// the steady-state observation rate for a minimized or unresponsive app.
const CLOSE_RECHECK: Duration = Duration::from_millis(75);

pub(in crate::app) struct Omission {
    since: Instant,
    probe: Probe,
}

enum Probe {
    First,
    RetryAt(Instant),
    Done,
}

impl Omission {
    pub(in crate::app) fn new(since: Instant) -> Self {
        Self {
            since,
            probe: Probe::First,
        }
    }

    pub(super) fn is_settling(&self, now: Instant) -> bool {
        now.duration_since(self.since) < OMISSION_SETTLE
    }

    /// Next prompt recheck while an omitted window is still settling, bounded
    /// by the end of `OMISSION_SETTLE` so reflow happens as soon as macOS drops
    /// a closing window from the window server instead of waiting for the 1s
    /// fallback refresh.
    pub(super) fn next_recheck(&self, now: Instant) -> Instant {
        (now + CLOSE_RECHECK).min(self.since + OMISSION_SETTLE)
    }

    fn should_probe(&mut self, now: Instant) -> bool {
        match self.probe {
            Probe::First => self.probe = Probe::RetryAt(now + CLOSE_RECHECK),
            Probe::RetryAt(at) if now >= at => self.probe = Probe::Done,
            _ => return false,
        }
        true
    }
}

impl<B: WorkspaceBackend> WorkspaceRuntime<B> {
    /// A bound window the window server still shows on screen, but which the
    /// app's Accessibility snapshot omitted may still be transitioning. For
    /// omissions without individual fullscreen or stranded-window evidence, downgrade
    /// the app to `Unavailable` so reconciliation keeps its slots instead of
    /// retiring one and re-adopting the window as new a cycle later. The
    /// omission is believed once it outlasts [`OMISSION_SETTLE`].
    pub(super) fn guard_omitted_windows(
        &mut self,
        workspace: WorkspaceId,
        observation: &mut crate::NativeObservation,
        onscreen_frames: Option<&HashMap<u32, crate::Rect>>,
        close_recheck: &mut Option<Instant>,
        retained: &HashSet<crate::PhysicalWindowId>,
    ) {
        if observation.mouse_down {
            return;
        }
        let Some(onscreen_frames) = onscreen_frames else {
            return;
        };
        let bound = self.workspaces.engine(workspace).bound_windows();
        if bound.is_empty() && self.omitted_windows.is_empty() {
            return;
        }
        let observed: HashSet<_> = live::observed_frames(observation).into_keys().collect();
        self.omitted_windows
            .retain(|window, _| match self.workspaces.workspace_of(window) {
                Some(owner) => owner != workspace || !observed.contains(window),
                // An omitted float's settle is tracked by affinity freezing.
                None => self.workspaces.floating_workspace_of(window).is_some(),
            });
        let now = Instant::now();
        for app_observation in &mut observation.applications {
            let crate::AppObservation::Complete { app, .. } = app_observation else {
                continue;
            };
            let settling = bound
                .iter()
                .filter(|window| window.app == app.id && !observed.contains(*window) && !retained.contains(*window))
                .filter(|window| onscreen_frames.contains_key(&window.window_id))
                .filter(|window| {
                    let omission = self
                        .omitted_windows
                        .entry((*window).clone())
                        .or_insert_with(|| {
                            log_event!(
                                "observation omitted {:?}: window server shows it on screen; keeping its slot",
                                window
                            );
                            Omission::new(now)
                        });
                    if omission.is_settling(now) {
                        let deadline = omission.next_recheck(now);
                        *close_recheck =
                            Some(close_recheck.map_or(deadline, |other| other.min(deadline)));
                        true
                    } else {
                        false
                    }
                })
                .count();
            if settling == 0 {
                continue;
            }
            *app_observation = crate::AppObservation::Unavailable {
                app: app.id.clone(),
                error: format!(
                    "accessibility snapshot omitted {settling} on-screen bound window(s)"
                ),
            };
        }
    }

    /// A complete list of tileable windows can omit a live slot. AX fullscreen
    /// identifies a window on its own Space. CG can also show a tile still
    /// waiting for off-display recovery. Keep these individual bindings while
    /// letting the same app open and close other windows.
    pub(super) fn retained_omissions(
        &mut self,
        workspace: WorkspaceId,
        observation: &crate::NativeObservation,
        onscreen: Option<&HashMap<u32, crate::Rect>>,
        outputs: &macos::DisplayOutputs,
    ) -> HashSet<crate::PhysicalWindowId> {
        let bound = self.workspaces.engine(workspace).bound_windows();
        let mut retained = HashSet::new();
        for entry in &observation.applications {
            let crate::AppObservation::Complete { app, windows, .. } = entry else {
                continue;
            };
            for id in bound
                .iter()
                .filter(|id| id.app == app.id && !windows.iter().any(|w| w.id == **id))
            {
                let stranded = onscreen
                    .and_then(|cg| cg.get(&id.window_id))
                    .is_some_and(|frame| {
                        !outputs
                            .frames()
                            .any(|display| display.contains_center(*frame))
                    });
                if stranded
                    || matches!(
                        self.backend.native_fullscreen(id),
                        Ok(true) | Err(macos::WindowAccessError::Unavailable)
                    )
                {
                    retained.insert(id.clone());
                }
            }
        }
        retained
    }

    /// An owned float that a Complete Accessibility snapshot omitted while the
    /// window server still shows it on screen keeps its workspace until the
    /// omission outlasts [`OMISSION_SETTLE`], like a bound tile does (#179).
    /// `onscreen` caches the window-server read for the rest of the cycle.
    pub(super) fn omitted_float_is_settling(
        &mut self,
        window: &crate::PhysicalWindowId,
        onscreen: &mut Option<Option<HashMap<u32, crate::Rect>>>,
    ) -> bool {
        let frames = onscreen.get_or_insert_with(|| self.backend.onscreen_window_frames());
        if !frames
            .as_ref()
            .is_some_and(|frames| frames.contains_key(&window.window_id))
        {
            return false;
        }
        let now = Instant::now();
        self.omitted_windows
            .entry(window.clone())
            .or_insert_with(|| {
                log_event!(
                    "observation omitted floating {:?}: window server shows it on screen; keeping its workspace",
                    window
                );
                Omission::new(now)
            })
            .is_settling(now)
    }

    pub(super) fn ordered_out_window_is_settling(
        &mut self,
        workspace: WorkspaceId,
        window: &crate::PhysicalWindowId,
        sibling_visible_without_drag: bool,
        now: Instant,
        close_recheck: &mut Option<Instant>,
    ) -> bool {
        // A synthesized empty app snapshot retires every slot for that
        // app on this workspace. Limit early confirmation to one bound
        // tile, and leave native tab replacement to its settling policy.
        let slots = &self.workspaces.engine(workspace).model().workspace.slots;
        let mut owned = slots.iter().filter(|slot| slot.belongs_to(&window.app));
        let probe = sibling_visible_without_drag
            && !self.parked.contains_key(window)
            && owned.next().and_then(crate::Slot::bound_window) == Some(window)
            && owned.next().is_none()
            && self
                .bundle_id(&window.app)
                .is_some_and(|bundle| bundle != "com.mitchellh.ghostty");
        let omission = self
            .omitted_windows
            .entry(window.clone())
            .or_insert_with(|| {
                log_event!(
                    "observation omitted {:?}: window server lists it off screen; releasing its slot unless it returns",
                    window
                );
                Omission::new(now)
            });
        if !omission.is_settling(now) {
            return false;
        }
        if probe && omission.should_probe(now) && self.backend.confirms_no_windows(&window.app) {
            return false;
        }
        if probe && let Probe::RetryAt(deadline) = omission.probe {
            *close_recheck = Some(close_recheck.map_or(deadline, |other| other.min(deadline)));
        }
        true
    }
}
