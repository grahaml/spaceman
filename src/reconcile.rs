use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    AppInstanceId, AppObservation, BindingState, Model, NativeObservation, PhysicalWindowId, Rect,
    Slot, SlotId, layout_tree::LayoutTree,
};

const GHOSTTY_REPLACEMENT_SETTLE_CYCLES: u64 = 3;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReconciliationEvidence {
    #[serde(with = "crate::serde_map")]
    pub previous_placements: HashMap<SlotId, Rect>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Settings {
    pub output: Rect,
    #[serde(default = "default_frame_tolerance")]
    pub frame_tolerance: i32,
    /// Gaps, outer padding and the lone-window width cap applied to every tile.
    #[serde(default, skip_serializing_if = "crate::spacing::Spacing::is_none")]
    pub spacing: crate::spacing::Spacing,
}

pub const DEFAULT_FRAME_TOLERANCE: i32 = 2;

impl Settings {
    #[must_use]
    pub const fn for_output(output: Rect) -> Self {
        Self {
            output,
            frame_tolerance: DEFAULT_FRAME_TOLERANCE,
            spacing: crate::spacing::Spacing::NONE,
        }
    }

    #[must_use]
    pub const fn spaced(self, spacing: crate::spacing::Spacing) -> Self {
        Self { spacing, ..self }
    }

    /// The rect the layout partitions: the output less its outer padding.
    #[must_use]
    pub fn usable(&self) -> Rect {
        self.spacing.usable(self.output)
    }

    /// The rect the layout tree partitions: [`crate::spacing::Spacing::canvas`].
    #[must_use]
    pub fn canvas(&self) -> Rect {
        self.spacing.canvas(self.usable())
    }

    /// A lone tile's window frame: Fullscreen Layout's frame, and a single tiled window's.
    #[must_use]
    pub fn single(&self) -> Rect {
        self.spacing.single(self.usable())
    }
}

const fn default_frame_tolerance() -> i32 {
    DEFAULT_FRAME_TOLERANCE
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Placement {
    pub slot: SlotId,
    pub window: Option<PhysicalWindowId>,
    pub frame: Rect,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Decision {
    WindowInserted {
        slot: SlotId,
        window: PhysicalWindowId,
    },
    WindowRemoved {
        slot: SlotId,
        window: PhysicalWindowId,
    },
    NativeTabReplaced {
        slot: SlotId,
        previous: PhysicalWindowId,
        current: PhysicalWindowId,
    },
    NativeTabAmbiguous {
        app: AppInstanceId,
    },
    ObservationUnavailable {
        app: AppInstanceId,
        error: String,
    },
    ApplicationTerminated {
        app: AppInstanceId,
    },
    MouseDownDeferred,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Reconciliation {
    pub model: Model,
    pub placements: Vec<Placement>,
    pub decisions: Vec<Decision>,
}

#[must_use]
pub fn reconcile(
    model: &Model,
    observation: &NativeObservation,
    evidence: &ReconciliationEvidence,
    settings: &Settings,
) -> Reconciliation {
    reconcile_retaining(model, observation, evidence, settings, &HashSet::new())
}

pub(crate) fn reconcile_retaining(
    model: &Model,
    observation: &NativeObservation,
    evidence: &ReconciliationEvidence,
    settings: &Settings,
    retained: &HashSet<PhysicalWindowId>,
) -> Reconciliation {
    if observation.mouse_down {
        let mut model = model.clone();
        reset_settling(&mut model, observation.cycle, None);
        return Reconciliation {
            placements: default_placements(settings, model.workspace.axis, &model.workspace.slots),
            model,
            decisions: vec![Decision::MouseDownDeferred],
        };
    }

    let mut next = model.clone();
    let mut decisions = Vec::new();

    for app_observation in &observation.applications {
        match app_observation {
            AppObservation::Unavailable { app, error } => {
                reset_settling(&mut next, observation.cycle, Some(app));
                decisions.push(Decision::ObservationUnavailable {
                    app: app.clone(),
                    error: error.clone(),
                });
            }
            AppObservation::Terminated { app } => {
                remove_app(&mut next, app, &mut decisions);
                next.continuity.remove(app);
                decisions.push(Decision::ApplicationTerminated { app: app.clone() });
            }
            AppObservation::Complete {
                app,
                focused,
                windows,
            } => {
                let context = AppReconcileContext {
                    focused: focused.as_ref(),
                    windows,
                    cycle: observation.cycle,
                    evidence,
                    settings,
                };
                if !reconcile_unresolved(&mut next, &app.id, &context, &mut decisions) {
                    let current: HashSet<_> =
                        windows.iter().map(|window| window.id.clone()).collect();
                    let existing: HashSet<_> = next
                        .workspace
                        .slots
                        .iter()
                        .filter(|slot| slot.belongs_to(&app.id))
                        .filter_map(|slot| slot.bound_window().cloned())
                        .collect();
                    let stale: Vec<_> = existing
                        .difference(&current)
                        .filter(|id| !retained.contains(*id))
                        .cloned()
                        .collect();
                    let new: Vec<_> = current.difference(&existing).cloned().collect();

                    let replacement = app.bundle_id == "com.mitchellh.ghostty"
                        && replacement_candidate(&next, &stale, &new, &context);

                    if replacement {
                        let previous = &stale[0];
                        let current = &new[0];
                        if let Some(slot) = next
                            .workspace
                            .slots
                            .iter_mut()
                            .find(|slot| slot.bound_window() == Some(previous))
                        {
                            slot.binding = BindingState::Bound(current.clone());
                            decisions.push(Decision::NativeTabReplaced {
                                slot: slot.id,
                                previous: previous.clone(),
                                current: current.clone(),
                            });
                        }
                    } else if app.bundle_id == "com.mitchellh.ghostty"
                        && !stale.is_empty()
                        && !new.is_empty()
                    {
                        for slot in &mut next.workspace.slots {
                            let Some(previous) = slot
                                .bound_window()
                                .filter(|window| stale.contains(window))
                                .cloned()
                            else {
                                continue;
                            };
                            slot.binding = BindingState::Unresolved {
                                previous,
                                candidates: new.clone(),
                                since: observation.cycle,
                            };
                        }
                        decisions.push(Decision::NativeTabAmbiguous {
                            app: app.id.clone(),
                        });
                    } else {
                        remove_windows(&mut next, &stale, &mut decisions);
                        insert_windows(&mut next, &new, &mut decisions);
                    }
                }

                next.continuity
                    .entry(app.id.clone())
                    .or_default()
                    .previous_focused
                    .clone_from(focused);
            }
        }
    }

    let placements = default_placements(settings, next.workspace.axis, &next.workspace.slots);
    Reconciliation {
        model: next,
        placements,
        decisions,
    }
}

struct AppReconcileContext<'a> {
    focused: Option<&'a PhysicalWindowId>,
    windows: &'a [crate::ObservedWindow],
    cycle: u64,
    evidence: &'a ReconciliationEvidence,
    settings: &'a Settings,
}

fn reconcile_unresolved(
    model: &mut Model,
    app: &AppInstanceId,
    context: &AppReconcileContext<'_>,
    decisions: &mut Vec<Decision>,
) -> bool {
    let unresolved: Vec<_> = model
        .workspace
        .slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            matches!(slot.binding, BindingState::Unresolved { .. })
                .then_some(index)
                .filter(|_| slot.belongs_to(app))
        })
        .collect();
    if unresolved.is_empty() {
        return false;
    }

    let bound: HashSet<_> = model
        .workspace
        .slots
        .iter()
        .filter_map(|slot| slot.bound_window().cloned())
        .collect();
    let candidates: Vec<_> = context
        .windows
        .iter()
        .filter(|window| !bound.contains(&window.id))
        .map(|window| window.id.clone())
        .collect();

    if unresolved.len() == 1 && candidates.len() == 1 && context.focused == candidates.first() {
        let index = unresolved[0];
        let slot = &model.workspace.slots[index];
        if candidate_matches_target(slot.id, &candidates[0], context) {
            let slot_id = slot.id;
            let previous = match &slot.binding {
                BindingState::Unresolved { previous, .. } => previous.clone(),
                BindingState::Bound(_) => unreachable!(),
            };
            model.workspace.slots[index].binding = BindingState::Bound(candidates[0].clone());
            decisions.push(Decision::NativeTabReplaced {
                slot: slot_id,
                previous,
                current: candidates[0].clone(),
            });
            return true;
        }
    }

    let expired = unresolved.iter().all(|index| {
        let BindingState::Unresolved { since, .. } = &model.workspace.slots[*index].binding else {
            unreachable!()
        };
        context.cycle.saturating_sub(*since) >= GHOSTTY_REPLACEMENT_SETTLE_CYCLES
    });
    if expired {
        model.workspace.slots.retain(|slot| match &slot.binding {
            BindingState::Unresolved { previous, .. } if slot.belongs_to(app) => {
                decisions.push(Decision::WindowRemoved {
                    slot: slot.id,
                    window: previous.clone(),
                });
                false
            }
            _ => true,
        });
        insert_windows(model, &candidates, decisions);
        return true;
    }

    for index in unresolved {
        if let BindingState::Unresolved {
            candidates: existing,
            ..
        } = &mut model.workspace.slots[index].binding
        {
            *existing = candidates.clone();
        }
    }
    decisions.push(Decision::NativeTabAmbiguous { app: app.clone() });
    true
}

fn reset_settling(model: &mut Model, cycle: u64, app: Option<&AppInstanceId>) {
    for slot in model
        .workspace
        .slots
        .iter_mut()
        .filter(|slot| app.is_none_or(|app| slot.belongs_to(app)))
    {
        if let BindingState::Unresolved { since, .. } = &mut slot.binding {
            *since = cycle;
        }
    }
}

fn replacement_candidate(
    model: &Model,
    stale: &[PhysicalWindowId],
    new: &[PhysicalWindowId],
    context: &AppReconcileContext<'_>,
) -> bool {
    if stale.len() != 1 || new.len() != 1 || context.focused != new.first() {
        return false;
    }
    if model
        .continuity
        .get(&stale[0].app)
        .and_then(|value| value.previous_focused.as_ref())
        != stale.first()
    {
        return false;
    }
    let Some(slot) = model
        .workspace
        .slots
        .iter()
        .find(|slot| slot.bound_window() == stale.first())
    else {
        return false;
    };
    candidate_matches_target(slot.id, &new[0], context)
}

fn candidate_matches_target(
    slot: SlotId,
    candidate: &PhysicalWindowId,
    context: &AppReconcileContext<'_>,
) -> bool {
    context
        .evidence
        .previous_placements
        .get(&slot)
        .is_some_and(|target| {
            context
                .windows
                .iter()
                .find(|window| &window.id == candidate)
                .is_some_and(|window| {
                    window
                        .frame
                        .approximately_equals(*target, context.settings.frame_tolerance)
                })
        })
}

fn remove_app(model: &mut Model, app: &AppInstanceId, decisions: &mut Vec<Decision>) {
    model.workspace.slots.retain(|slot| {
        if !slot.belongs_to(app) {
            return true;
        }
        if let Some(window) = slot.bound_window().cloned() {
            decisions.push(Decision::WindowRemoved {
                slot: slot.id,
                window,
            });
        }
        false
    });
}

fn remove_windows(model: &mut Model, windows: &[PhysicalWindowId], decisions: &mut Vec<Decision>) {
    model.workspace.slots.retain(|slot| {
        let Some(window) = slot
            .bound_window()
            .filter(|window| windows.contains(window))
            .cloned()
        else {
            return true;
        };
        decisions.push(Decision::WindowRemoved {
            slot: slot.id,
            window,
        });
        false
    });
}

fn insert_windows(model: &mut Model, windows: &[PhysicalWindowId], decisions: &mut Vec<Decision>) {
    let mut windows = windows.to_vec();
    windows.sort_by_key(|window| window.window_id);
    for window in windows {
        let slot_id = SlotId(model.next_slot_id);
        model.next_slot_id += 1;
        model.workspace.slots.push(Slot {
            id: slot_id,
            weight: 1,
            binding: BindingState::Bound(window.clone()),
        });
        decisions.push(Decision::WindowInserted {
            slot: slot_id,
            window,
        });
    }
}

fn default_placements(settings: &Settings, axis: crate::Axis, slots: &[Slot]) -> Vec<Placement> {
    let layout = LayoutTree::weighted_flat(axis, slots.iter());
    let cells = layout
        .default_grid()
        .as_ref()
        .unwrap_or(&layout)
        .placements(settings.canvas());
    slots
        .iter()
        .zip(settings.spacing.place(settings.usable(), cells))
        .map(|(slot, (slot_id, frame))| Placement {
            slot: slot_id,
            window: slot.bound_window().cloned(),
            frame,
        })
        .collect()
}

#[cfg(test)]
mod tests;
