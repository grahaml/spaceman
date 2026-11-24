use std::collections::HashSet;

use crate::{
    Model, NativeObservation, PhysicalWindowId, Placement, Reconciliation, ReconciliationEvidence,
    Settings, Slot, layout_tree::LayoutTree, reconcile::reconcile_retaining,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CycleTicket {
    pub cycle: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaleCycle {
    pub ticket: CycleTicket,
    pub current_revision: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Engine {
    model: Model,
    layout: LayoutTree,
    // Derived from four or five default-weight leaves. Membership changes still operate
    // on the requested split; editing a quadrant commits the displayed tree.
    default_grid: Option<LayoutTree>,
    // Only the requested layout above is persisted. The fit is shared by
    // placement, navigation and HUD previews until its inputs change.
    fit: Option<DisplayFit>,
    insertion_focus: Option<crate::SlotId>,
    evidence: ReconciliationEvidence,
    revision: u64,
    next_cycle: u64,
    pending: Option<CycleTicket>,
}

#[derive(Clone, Debug)]
struct DisplayFit {
    output: crate::Rect,
    minimums: std::collections::HashMap<crate::SlotId, (i32, i32)>,
    tree: Option<LayoutTree>,
}

impl Engine {
    pub fn invalidate(&mut self) {
        self.pending = None;
        self.revision = self.revision.saturating_add(1);
    }

    pub fn begin_cycle(&mut self) -> CycleTicket {
        self.next_cycle = self.next_cycle.saturating_add(1);
        let ticket = CycleTicket {
            cycle: self.next_cycle,
            revision: self.revision,
        };
        self.pending = Some(ticket);
        ticket
    }

    pub fn accept(
        &mut self,
        ticket: CycleTicket,
        observation: &NativeObservation,
        settings: &Settings,
    ) -> Result<Reconciliation, StaleCycle> {
        self.accept_retaining(ticket, observation, settings, &HashSet::new())
    }

    pub(crate) fn accept_retaining(
        &mut self,
        ticket: CycleTicket,
        observation: &NativeObservation,
        settings: &Settings,
        retained: &HashSet<PhysicalWindowId>,
    ) -> Result<Reconciliation, StaleCycle> {
        if self.pending != Some(ticket) || observation.cycle != ticket.cycle {
            return Err(StaleCycle {
                ticket,
                current_revision: self.revision,
            });
        }
        self.pending = None;

        let mut result =
            reconcile_retaining(&self.model, observation, &self.evidence, settings, retained);
        if self.layout.sync_near(
            result.model.workspace.axis,
            &result.model.workspace.slots,
            self.insertion_focus,
        ) {
            self.refresh_display_layout();
            self.insertion_focus = self
                .insertion_focus
                .filter(|&id| result.model.slot(id).is_some());
        }
        result.placements = placements_for(self.display_layout(), &result.model, settings);
        self.evidence.previous_placements = result
            .placements
            .iter()
            .map(|placement| (placement.slot, placement.frame))
            .collect();
        self.model = result.model.clone();
        Ok(result)
    }

    #[must_use]
    pub const fn model(&self) -> &Model {
        &self.model
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn bound_windows(&self) -> Vec<PhysicalWindowId> {
        self.layout
            .slots()
            .into_iter()
            .filter_map(|id| self.model.slot(id)?.bound_window().cloned())
            .collect()
    }

    #[must_use]
    pub fn placements(&self, settings: &Settings) -> Vec<Placement> {
        placements_for(self.display_layout(), &self.model, settings)
    }

    pub(crate) fn record_placement_target(&mut self, slot: crate::SlotId, frame: crate::Rect) {
        self.evidence.previous_placements.insert(slot, frame);
    }

    pub(crate) fn toggle_axis(&mut self) {
        self.layout.toggle_root_axis();
        self.commit_layout();
    }

    pub(crate) fn take_slot(&mut self, window: &PhysicalWindowId) -> Option<Slot> {
        let index = self
            .model
            .workspace
            .slots
            .iter()
            .position(|slot| slot.bound_window() == Some(window))?;
        let slot = self.model.workspace.slots.remove(index);
        self.layout
            .sync(self.model.workspace.axis, &self.model.workspace.slots);
        self.evidence.previous_placements.remove(&slot.id);
        self.invalidate_layout();
        Some(slot)
    }

    /// Remove every slot, bound or unresolved, whose identity belongs to `app`.
    pub(crate) fn forget_app(&mut self, app: &crate::AppInstanceId) -> bool {
        let before = self.model.workspace.slots.len();
        self.model
            .workspace
            .slots
            .retain(|slot| !slot.belongs_to(app));
        self.model.continuity.remove(app);
        if self.model.workspace.slots.len() == before {
            return false;
        }
        let slots = &self.model.workspace.slots;
        self.evidence
            .previous_placements
            .retain(|id, _| slots.iter().any(|slot| slot.id == *id));
        self.layout
            .sync(self.model.workspace.axis, &self.model.workspace.slots);
        self.invalidate_layout();
        true
    }

    pub(crate) fn insert_slot(&mut self, mut slot: Slot) {
        if self
            .model
            .workspace
            .slots
            .iter()
            .any(|existing| existing.id == slot.id)
        {
            slot.id = crate::SlotId(self.model.next_slot_id);
        }
        self.model.next_slot_id = self.model.next_slot_id.max(slot.id.0.saturating_add(1));
        self.model.workspace.slots.push(slot);
        self.layout
            .sync(self.model.workspace.axis, &self.model.workspace.slots);
        self.invalidate_layout();
    }

    pub(crate) fn swap_windows(
        &mut self,
        first: &PhysicalWindowId,
        second: &PhysicalWindowId,
    ) -> bool {
        let first = self.slot_for_window(first);
        let second = self.slot_for_window(second);
        let Some((first, second)) = first.zip(second).filter(|(first, second)| first != second)
        else {
            return false;
        };
        // Swap inside the requested layout so swapping two tiles while a
        // minimum-width fit temporarily stacks them does not save that stack.
        let mut layout = self.layout.clone();
        if !layout.swap(first, second) {
            return false;
        }
        self.layout = layout;
        self.commit_layout();
        true
    }

    /// Reads a window the user resized to `current` as the split boundaries it moved.
    pub(crate) fn resize_window(
        &mut self,
        window: &PhysicalWindowId,
        current: crate::Rect,
        settings: &Settings,
    ) -> bool {
        let canvas = settings.canvas();
        let current = settings.spacing.untile(current);
        self.edit_weights(window, |layout, slot| layout.resize(slot, current, canvas))
    }

    pub(crate) fn remember_insertion_focus(&mut self, window: &PhysicalWindowId) {
        self.insertion_focus = self.slot_for_window(window);
    }

    fn display_layout(&self) -> &LayoutTree {
        self.fit
            .as_ref()
            .and_then(|fit| fit.tree.as_ref())
            .unwrap_or_else(|| self.base_layout())
    }

    fn base_layout(&self) -> &LayoutTree {
        self.default_grid.as_ref().unwrap_or(&self.layout)
    }

    /// Re-evaluate even without a new refusal: a removed sibling or a larger
    /// display can make the original split usable again.
    pub(crate) fn fit_minimums(
        &mut self,
        settings: &Settings,
        minimums: &std::collections::HashMap<PhysicalWindowId, (i32, i32)>,
    ) -> bool {
        let output = settings.canvas();
        // Every cell gives one whole gap to the space around its window.
        let gap = settings.spacing.inner;
        if minimums.is_empty() {
            return self.fit.take().is_some_and(|fit| fit.tree.is_some());
        }
        let slots: std::collections::HashMap<_, _> = self
            .model
            .workspace
            .slots
            .iter()
            .filter_map(|slot| {
                let (width, height) = *minimums.get(slot.bound_window()?)?;
                Some((slot.id, (width + gap, height + gap)))
            })
            .collect();
        if slots.is_empty() {
            return self.fit.take().is_some_and(|fit| fit.tree.is_some());
        }
        if self
            .fit
            .as_ref()
            .is_some_and(|fit| fit.output == output && fit.minimums == slots)
        {
            return false;
        }
        let tree = self
            .layout
            .fitted_for_display(self.default_grid.as_ref(), output, &slots);
        let changed = self.fit.as_ref().and_then(|fit| fit.tree.as_ref()) != tree.as_ref();
        self.fit = Some(DisplayFit {
            output,
            minimums: slots,
            tree,
        });
        changed
    }

    pub(crate) fn resize_focused_tile(
        &mut self,
        window: &PhysicalWindowId,
        direction: crate::ResizeDirection,
        amount: i32,
        settings: &Settings,
    ) -> bool {
        let canvas = settings.canvas();
        self.edit_weights(window, |layout, slot| {
            layout.resize_by(slot, direction, amount, canvas)
        })
    }

    pub(crate) fn balance_focused_parent(
        &mut self,
        window: &PhysicalWindowId,
        settings: &Settings,
    ) -> bool {
        let canvas = settings.canvas();
        self.edit_weights(window, |layout, slot| layout.balance(slot, canvas))
    }

    pub(crate) fn create_implicit_split(
        &mut self,
        window: &PhysicalWindowId,
        direction: crate::Direction,
    ) -> bool {
        self.edit_slot(window, |layout, slot| {
            layout.create_implicit_split(slot, direction)
        })
    }

    pub(crate) fn move_window_relative(
        &mut self,
        moved: &PhysicalWindowId,
        target: &PhysicalWindowId,
        direction: crate::Direction,
    ) -> bool {
        let Some(target) = self.slot_for_window(target) else {
            return false;
        };
        self.edit_slot(moved, |layout, moved| {
            layout.move_relative(moved, target, direction)
        })
    }

    pub(crate) fn layout(&self) -> &LayoutTree {
        &self.layout
    }

    pub(crate) fn root_axis(&self) -> crate::Axis {
        self.display_layout().root_axis()
    }

    /// Each tile's slot, window and frame in `output`, in layout order.
    pub(crate) fn preview_placements(&self, output: crate::Rect) -> Vec<Placement> {
        placements_for(
            self.display_layout(),
            &self.model,
            &Settings::for_output(output),
        )
    }

    pub(crate) fn preview(&self, output: crate::Rect) -> Vec<crate::Rect> {
        self.display_layout()
            .placements(output)
            .into_iter()
            .map(|(_, frame)| frame)
            .collect()
    }

    pub(crate) fn restore_layout(&mut self, mut layout: LayoutTree) {
        layout.sync(self.model.workspace.axis, &self.model.workspace.slots);
        self.layout = layout;
        self.commit_layout();
    }

    pub(crate) fn slot_for_window(&self, window: &PhysicalWindowId) -> Option<crate::SlotId> {
        self.model
            .workspace
            .slots
            .iter()
            .find(|slot| slot.bound_window() == Some(window))
            .map(|slot| slot.id)
    }

    fn edit_slot(
        &mut self,
        window: &PhysicalWindowId,
        op: impl FnOnce(&mut LayoutTree, crate::SlotId) -> bool,
    ) -> bool {
        let Some(slot) = self.slot_for_window(window) else {
            return false;
        };
        let mut layout = self.display_layout().clone();
        if !op(&mut layout, slot) {
            return false;
        }
        self.layout = layout;
        self.commit_layout();
        true
    }

    /// Edits tile sizes. While the display shows an automatic grouping the
    /// requested layout lacks, the edit changes only that temporary fit:
    /// saving it would make the grouping permanent.
    fn edit_weights(
        &mut self,
        window: &PhysicalWindowId,
        op: impl FnOnce(&mut LayoutTree, crate::SlotId) -> bool,
    ) -> bool {
        let requested = self.default_grid.as_ref().unwrap_or(&self.layout);
        let Some(tree) = self
            .fit
            .as_mut()
            .and_then(|fit| fit.tree.as_mut())
            .filter(|tree| tree.regroups(requested))
        else {
            return self.edit_slot(window, op);
        };
        let Some(slot) = self
            .model
            .workspace
            .slots
            .iter()
            .find(|slot| slot.bound_window() == Some(window))
            .map(|slot| slot.id)
        else {
            return false;
        };
        let mut edited = tree.clone();
        if !op(&mut edited, slot) {
            return false;
        }
        *tree = edited;
        self.invalidate();
        true
    }

    fn refresh_display_layout(&mut self) {
        self.default_grid = self.layout.default_grid();
        self.fit = None;
    }

    fn invalidate_layout(&mut self) {
        self.refresh_display_layout();
        self.insertion_focus = self
            .insertion_focus
            .filter(|&id| self.model.slot(id).is_some());
        self.invalidate();
    }

    fn commit_layout(&mut self) {
        self.model.workspace.axis = self.layout.root_axis();
        for slot in &mut self.model.workspace.slots {
            if let Some(weight) = self.layout.weight(slot.id) {
                slot.weight = weight;
            }
        }
        self.invalidate_layout();
    }
}

fn placements_for(layout: &LayoutTree, model: &Model, settings: &Settings) -> Vec<Placement> {
    settings
        .spacing
        .place(settings.usable(), layout.placements(settings.canvas()))
        .into_iter()
        .filter_map(|(id, frame)| {
            let slot = model.slot(id)?;
            Some(Placement {
                slot: id,
                window: slot.bound_window().cloned(),
                frame,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{
        AppInstanceId, AppObservation, NativeObservation, ObservedApplication, ObservedWindow,
        PhysicalWindowId, Rect,
    };

    use super::*;

    fn slot(id: u64, weight: u32) -> Slot {
        Slot {
            id: crate::SlotId(id),
            weight,
            binding: crate::BindingState::Bound(PhysicalWindowId {
                app: AppInstanceId {
                    pid: 1,
                    launched_at: 1,
                },
                window_id: id as u32,
            }),
        }
    }

    fn observe_windows(engine: &mut Engine, ids: &[u32], settings: &Settings) -> Reconciliation {
        let ticket = engine.begin_cycle();
        let mut snapshot = observation(ticket.cycle, ids[0]);
        let AppObservation::Complete { windows, .. } = &mut snapshot.applications[0] else {
            unreachable!()
        };
        let template = windows[0].clone();
        *windows = ids
            .iter()
            .map(|&id| {
                let mut window = template.clone();
                window.id.window_id = id;
                window
            })
            .collect();
        engine.accept(ticket, &snapshot, settings).unwrap()
    }

    #[test]
    fn four_default_tiles_open_and_reopen_in_equal_quadrants() {
        let mut settings = Settings::for_output(Rect {
            x: -1008,
            y: 20,
            width: 1008,
            height: 808,
        });
        settings.spacing.inner = 8;
        let ids = [1, 2, 3, 4];
        let expected = [(-1008, 20), (-1008, 428), (-500, 20), (-500, 428)].map(|(x, y)| Rect {
            x,
            y,
            width: 500,
            height: 400,
        });
        for incremental in [false, true] {
            let mut engine = Engine::default();
            if incremental {
                for n in 1..4 {
                    observe_windows(&mut engine, &ids[..n], &settings);
                }
            }
            let grid = observe_windows(&mut engine, &ids, &settings).placements;
            assert_eq!(grid.iter().map(|p| p.frame).collect::<Vec<_>>(), expected);
            assert_eq!(
                grid.iter()
                    .map(|p| p.window.as_ref().unwrap().window_id)
                    .collect::<Vec<_>>(),
                ids
            );
            observe_windows(&mut engine, &[1, 2, 3, 4, 5], &settings);
            assert_eq!(
                observe_windows(&mut engine, &ids, &settings).placements,
                grid
            );
            observe_windows(&mut engine, &[1, 2, 3], &settings);
            let reopened = observe_windows(&mut engine, &[1, 2, 3, 6], &settings);
            assert_eq!(
                reopened
                    .placements
                    .iter()
                    .map(|p| p.frame)
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn five_tiles_use_two_rows_and_fit_the_observed_minimums() {
        let mut engine = Engine::default();
        let settings = Settings::for_output(Rect {
            x: -1200,
            y: 30,
            width: 1200,
            height: 800,
        });
        let grid = observe_windows(&mut engine, &[1, 2, 3, 4, 5], &settings).placements;
        let geometry = |p: &Placement| (p.frame.x, p.frame.y, p.frame.width, p.frame.height);
        assert_eq!(
            grid.iter().map(geometry).collect::<Vec<_>>(),
            [
                (-1200, 30, 400, 400),
                (-800, 30, 400, 400),
                (-400, 30, 400, 400),
                (-1200, 430, 600, 400),
                (-600, 430, 600, 400)
            ]
        );
        // Only a leading pair plus a trailing trio can fit these widths and heights.
        let minima = [(600, 380), (600, 380), (400, 380), (400, 380), (400, 380)];
        let limits = grid
            .iter()
            .zip(minima)
            .map(|(p, size)| (p.window.clone().unwrap(), size))
            .collect();
        assert!(engine.fit_minimums(&settings, &limits));
        let fitted = engine.placements(&settings);
        for ((tile, original), (width, height)) in fitted.iter().zip(&grid).zip(minima) {
            assert_eq!(tile.slot, original.slot);
            assert!(tile.frame.width >= width && tile.frame.height >= height);
        }
        assert_eq!((fitted[1].frame.x, fitted[2].frame.y), (-600, 430));
        let four = observe_windows(&mut engine, &[1, 2, 3, 4], &settings).placements;
        assert!(
            four.iter()
                .all(|p| p.frame.width == 600 && p.frame.height == 400)
        );
    }

    #[test]
    fn four_tile_default_respects_and_restores_layout_edits() {
        let settings = Settings::for_output(Rect {
            width: 1200,
            height: 800,
            ..Rect::default()
        });
        let mut engine = Engine::default();
        for i in 1..=4 {
            engine.insert_slot(slot(i, 1));
        }
        let windows = engine.bound_windows();
        engine.toggle_axis();
        let grid = engine.placements(&settings);
        assert_eq!(
            grid[1].frame,
            Rect {
                x: 600,
                y: 0,
                width: 600,
                height: 400
            }
        );
        assert!(engine.resize_focused_tile(
            &windows[0],
            crate::ResizeDirection::Grow,
            50,
            &settings
        ));
        let edited = engine.placements(&settings);
        assert_eq!((edited[0].frame.width, edited[1].frame.width), (650, 550));
        assert_eq!(edited[2..], grid[2..]);

        let mut restored = Engine::default();
        for slot in &engine.model.workspace.slots {
            restored.insert_slot(slot.clone());
        }
        restored.restore_layout(engine.layout.clone());
        assert_eq!(restored.placements(&settings), edited);
    }

    #[test]
    fn quadrant_pairs_turn_to_fit_app_minimums() {
        let settings = Settings::for_output(Rect {
            width: 1000,
            height: 800,
            ..Rect::default()
        });
        let mut engine = Engine::default();
        let grid = observe_windows(&mut engine, &[1, 2, 3, 4], &settings).placements;
        let minimums = grid
            .iter()
            .zip([500, 500, 250, 250])
            .map(|(p, height)| (p.window.clone().unwrap(), (450, height)))
            .collect();
        engine.fit_minimums(&settings, &minimums);
        let fitted = engine.placements(&settings);
        let expected = [(0, 0, 500), (500, 0, 500), (0, 500, 300), (500, 500, 300)];
        for ((tile, original), (x, y, height)) in fitted.iter().zip(&grid).zip(expected) {
            assert_eq!((tile.slot, &tile.window), (original.slot, &original.window));
            assert_eq!(
                tile.frame,
                Rect {
                    x,
                    y,
                    width: 500,
                    height
                }
            );
        }
        let larger = Settings::for_output(Rect {
            height: 1000,
            ..settings.output
        });
        assert!(engine.fit_minimums(&larger, &minimums));
        let placements = engine.placements(&larger);
        assert_eq!((placements[1].frame.x, placements[1].frame.y), (0, 500));
    }

    #[test]
    fn tall_windows_use_a_temporary_row_until_quadrants_fit() {
        let settings = Settings::for_output(Rect {
            width: 1000,
            height: 800,
            ..Rect::default()
        });
        let mut engine = Engine::default();
        let grid = observe_windows(&mut engine, &[1, 2, 3, 4], &settings).placements;
        let tall = grid
            .iter()
            .map(|p| (p.window.clone().unwrap(), (0, 500)))
            .collect();
        assert!(engine.fit_minimums(&settings, &tall));
        let row = engine.placements(&settings);
        assert!(
            row.iter()
                .all(|p| p.frame.height == 800 && p.frame.width == 250)
        );
        let larger = Settings::for_output(Rect {
            height: 1000,
            ..settings.output
        });
        assert!(engine.fit_minimums(&larger, &tall));
        assert!(
            engine
                .placements(&larger)
                .iter()
                .all(|p| p.frame.height == 500 && p.frame.width == 500)
        );
    }

    #[test]
    fn new_window_joins_the_focused_nested_split() {
        let mut engine = Engine::default();
        for i in 1..=3 {
            engine.insert_slot(slot(i, 1));
        }
        let windows = engine.bound_windows();
        assert!(engine.move_window_relative(&windows[0], &windows[1], crate::Direction::Up));
        engine.remember_insertion_focus(&windows[0]);
        let ticket = engine.begin_cycle();
        engine
            .accept(ticket, &observation(ticket.cycle, 4), &settings())
            .unwrap();
        let output = crate::Rect {
            width: 1200,
            height: 800,
            ..crate::Rect::default()
        };
        let tiles = engine.layout.placements(output);
        assert_eq!(
            tiles.iter().map(|(s, _)| s.0).collect::<Vec<_>>(),
            [1, 4, 2, 3]
        );
        assert_eq!(
            tiles[3].1,
            crate::Rect {
                x: 600,
                width: 600,
                ..output
            }
        );
    }

    #[test]
    fn structural_layout_edits_commit_axis_and_slot_weights() {
        let mut engine = Engine::default();
        let slots = [slot(1, 1), slot(2, 1), slot(3, 1)];
        let windows = slots
            .iter()
            .map(|slot| slot.bound_window().unwrap().clone())
            .collect::<Vec<_>>();
        for slot in slots {
            engine.insert_slot(slot);
        }
        let output = Rect {
            width: 1000,
            height: 800,
            ..Rect::default()
        };
        assert!(engine.resize_focused_tile(
            &windows[0],
            crate::ResizeDirection::Grow,
            50,
            &Settings::for_output(output)
        ));
        assert!(engine.create_implicit_split(&windows[1], crate::Direction::Up));
        assert_eq!(engine.model().workspace.axis, crate::Axis::Vertical);
        assert_eq!(engine.model().workspace.slots[1].weight, 1);

        assert!(engine.move_window_relative(&windows[2], &windows[0], crate::Direction::Right));
        assert_eq!(engine.model().workspace.slots[2].weight, 1);
    }

    fn observation(cycle: u64, window_id: u32) -> NativeObservation {
        let app = AppInstanceId {
            pid: 10,
            launched_at: 1,
        };
        let window = PhysicalWindowId {
            app: app.clone(),
            window_id,
        };
        NativeObservation {
            cycle,
            mouse_down: false,
            applications: vec![AppObservation::Complete {
                app: ObservedApplication {
                    id: app,
                    bundle_id: "com.example.Editor".into(),
                },
                focused: Some(window.clone()),
                windows: vec![ObservedWindow {
                    id: window,
                    frame: Rect {
                        width: 100,
                        height: 100,
                        ..Rect::default()
                    },
                    resizable: true,
                    title: None,
                }],
            }],
        }
    }

    fn settings() -> Settings {
        Settings {
            output: Rect {
                width: 100,
                height: 100,
                ..Rect::default()
            },
            frame_tolerance: 2,
            spacing: crate::spacing::Spacing::NONE,
        }
    }

    #[test]
    fn accepted_cycle_commits_model_and_evidence() {
        let mut engine = Engine::default();
        engine.invalidate();
        let ticket = engine.begin_cycle();
        let result = engine.accept(ticket, &observation(ticket.cycle, 1), &settings());

        assert_eq!(result.unwrap().model.workspace.slots.len(), 1);
        assert_eq!(engine.model().workspace.slots.len(), 1);
    }

    #[test]
    fn invalidation_rejects_an_in_flight_cycle_without_mutation() {
        let mut engine = Engine::default();
        engine.invalidate();
        let ticket = engine.begin_cycle();
        engine.invalidate();

        let error = engine
            .accept(ticket, &observation(ticket.cycle, 1), &settings())
            .unwrap_err();
        assert_eq!(error.current_revision, 2);
        assert!(engine.model().workspace.slots.is_empty());
    }

    #[test]
    fn mismatched_observation_cycle_is_rejected() {
        let mut engine = Engine::default();
        let ticket = engine.begin_cycle();

        assert!(
            engine
                .accept(ticket, &observation(ticket.cycle + 1, 1), &settings())
                .is_err()
        );
    }

    #[test]
    fn newer_cycle_supersedes_an_older_ticket_in_the_same_revision() {
        let mut engine = Engine::default();
        let older = engine.begin_cycle();
        let current = engine.begin_cycle();

        assert!(
            engine
                .accept(older, &observation(older.cycle, 1), &settings())
                .is_err()
        );
        assert!(
            engine
                .accept(current, &observation(current.cycle, 1), &settings())
                .is_ok()
        );
    }

    #[test]
    fn accepted_ticket_cannot_be_committed_twice() {
        let mut engine = Engine::default();
        let ticket = engine.begin_cycle();
        let observation = observation(ticket.cycle, 1);

        assert!(engine.accept(ticket, &observation, &settings()).is_ok());
        assert!(engine.accept(ticket, &observation, &settings()).is_err());
    }

    #[test]
    fn reordering_a_window_invalidates_pending_work() {
        let first = PhysicalWindowId {
            app: AppInstanceId {
                pid: 1,
                launched_at: 1,
            },
            window_id: 1,
        };
        let second = PhysicalWindowId {
            window_id: 2,
            ..first.clone()
        };
        let mut engine = Engine::default();
        engine.insert_slot(Slot {
            id: crate::SlotId(1),
            weight: 1,
            binding: crate::BindingState::Bound(first.clone()),
        });
        engine.insert_slot(Slot {
            id: crate::SlotId(2),
            weight: 1,
            binding: crate::BindingState::Bound(second.clone()),
        });
        let pending = engine.begin_cycle();

        assert!(engine.swap_windows(&second, &first));
        assert_eq!(engine.bound_windows(), vec![second, first]);
        assert!(
            engine
                .accept(pending, &observation(pending.cycle, 1), &settings())
                .is_err()
        );
    }

    #[test]
    fn taking_a_slot_invalidates_pending_work() {
        let window = PhysicalWindowId {
            app: AppInstanceId {
                pid: 1,
                launched_at: 1,
            },
            window_id: 1,
        };
        let mut engine = Engine::default();
        engine.insert_slot(Slot {
            id: crate::SlotId(1),
            weight: 1,
            binding: crate::BindingState::Bound(window.clone()),
        });
        let pending = engine.begin_cycle();

        assert!(engine.take_slot(&window).is_some());
        assert!(
            engine
                .accept(pending, &observation(pending.cycle, 1), &settings())
                .is_err()
        );
        assert!(engine.model().workspace.slots.is_empty());
    }

    #[test]
    fn inserting_a_slot_invalidates_pending_work() {
        let mut engine = Engine::default();
        let pending = engine.begin_cycle();
        engine.insert_slot(Slot {
            id: crate::SlotId(1),
            weight: 1,
            binding: crate::BindingState::Bound(PhysicalWindowId {
                app: AppInstanceId {
                    pid: 1,
                    launched_at: 1,
                },
                window_id: 1,
            }),
        });

        assert!(
            engine
                .accept(pending, &observation(pending.cycle, 1), &settings())
                .is_err()
        );
        assert_eq!(engine.model().workspace.slots.len(), 1);
    }
}
