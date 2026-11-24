use super::*;
use crate::{ObservedApplication, ObservedWindow};

fn weighted_slot(id: u64, weight: u32) -> Slot {
    Slot {
        id: SlotId(id),
        weight,
        binding: BindingState::Bound(PhysicalWindowId {
            app: AppInstanceId {
                pid: 1,
                launched_at: 1,
            },
            window_id: id as u32,
        }),
    }
}

#[test]
fn default_placements_partition_without_gaps_or_rounding_loss() {
    let placements = default_placements(
        &Settings::for_output(Rect {
            x: 10,
            y: 20,
            width: 101,
            height: 50,
        }),
        crate::Axis::Horizontal,
        &[
            weighted_slot(1, 1),
            weighted_slot(2, 1),
            weighted_slot(3, 1),
        ],
    );

    assert_eq!(placements[0].frame.width, 33);
    assert_eq!(placements[1].frame.x, 43);
    assert_eq!(placements[1].frame.width, 34);
    assert_eq!(placements[2].frame.x, 77);
    assert_eq!(placements[2].frame.width, 34);
    assert_eq!(placements[2].frame.x + placements[2].frame.width, 111);
}

#[test]
fn default_placements_reserve_unresolved_slots() {
    let mut unresolved = weighted_slot(2, 1);
    unresolved.binding = BindingState::Unresolved {
        previous: unresolved.bound_window().unwrap().clone(),
        candidates: Vec::new(),
        since: 4,
    };

    let placements = default_placements(
        &Settings::for_output(Rect {
            width: 100,
            height: 50,
            ..Rect::default()
        }),
        crate::Axis::Horizontal,
        &[weighted_slot(1, 1), unresolved],
    );
    assert_eq!(placements.len(), 2);
    assert_eq!(placements[0].frame.width, 50);
    assert_eq!(placements[1].frame.width, 50);
    assert_eq!(placements[1].window, None);
}

#[test]
fn default_placements_preserve_cumulative_weight_rounding() {
    let placements = default_placements(
        &Settings::for_output(Rect {
            x: 10,
            y: 20,
            width: 50,
            height: 101,
        }),
        crate::Axis::Vertical,
        &[
            weighted_slot(1, 2),
            weighted_slot(2, 3),
            weighted_slot(3, 5),
        ],
    );

    assert_eq!(placements[0].frame.height, 20);
    assert_eq!(placements[1].frame.y, 40);
    assert_eq!(placements[1].frame.height, 30);
    assert_eq!(placements[2].frame.y, 70);
    assert_eq!(placements[2].frame.height, 51);
    assert_eq!(placements[2].frame.y + placements[2].frame.height, 121);
}

const OUTPUT: Rect = Rect {
    x: 0,
    y: 0,
    width: 1000,
    height: 800,
};

fn app(pid: i32, launched_at: u64) -> AppInstanceId {
    AppInstanceId { pid, launched_at }
}

fn window(app: &AppInstanceId, window_id: u32, frame: Rect) -> ObservedWindow {
    ObservedWindow {
        id: PhysicalWindowId {
            app: app.clone(),
            window_id,
        },
        frame,
        resizable: true,
        title: None,
    }
}

fn complete(
    app: &AppInstanceId,
    bundle_id: &str,
    focused: Option<u32>,
    windows: Vec<ObservedWindow>,
) -> AppObservation {
    AppObservation::Complete {
        app: ObservedApplication {
            id: app.clone(),
            bundle_id: bundle_id.into(),
        },
        focused: focused.map(|window_id| PhysicalWindowId {
            app: app.clone(),
            window_id,
        }),
        windows,
    }
}

fn run(model: &Model, cycle: u64, applications: Vec<AppObservation>) -> Reconciliation {
    reconcile(
        model,
        &NativeObservation {
            cycle,
            mouse_down: false,
            applications,
        },
        &ReconciliationEvidence::default(),
        &Settings {
            output: OUTPUT,
            frame_tolerance: 2,
            spacing: crate::spacing::Spacing::NONE,
        },
    )
}

#[test]
fn initial_observation_inserts_windows_in_id_order() {
    let app = app(10, 1);
    let result = run(
        &Model::default(),
        1,
        vec![complete(
            &app,
            "com.example.Editor",
            Some(4),
            vec![window(&app, 4, OUTPUT), window(&app, 2, OUTPUT)],
        )],
    );

    let ids: Vec<_> = result
        .model
        .workspace
        .slots
        .iter()
        .map(|slot| slot.bound_window().unwrap().window_id)
        .collect();
    assert_eq!(ids, vec![2, 4]);
    assert_eq!(result.placements.len(), 2);
}

#[test]
fn ordinary_churn_preserves_existing_slot_and_appends_new_window() {
    let app = app(10, 1);
    let first = run(
        &Model::default(),
        1,
        vec![complete(
            &app,
            "com.example.Editor",
            Some(1),
            vec![window(&app, 1, OUTPUT), window(&app, 2, OUTPUT)],
        )],
    );
    let retained_slot = first.model.workspace.slots[1].id;
    let result = run(
        &first.model,
        2,
        vec![complete(
            &app,
            "com.example.Editor",
            Some(3),
            vec![window(&app, 2, OUTPUT), window(&app, 3, OUTPUT)],
        )],
    );

    assert_eq!(result.model.workspace.slots[0].id, retained_slot);
    assert_eq!(
        result.model.workspace.slots[0]
            .bound_window()
            .unwrap()
            .window_id,
        2
    );
    assert_eq!(
        result.model.workspace.slots[1]
            .bound_window()
            .unwrap()
            .window_id,
        3
    );
}

#[test]
fn unavailable_app_is_preserved_while_another_app_reconciles() {
    let unavailable = app(10, 1);
    let available = app(20, 1);
    let initial = run(
        &Model::default(),
        1,
        vec![
            complete(
                &unavailable,
                "a",
                Some(1),
                vec![window(&unavailable, 1, OUTPUT)],
            ),
            complete(
                &available,
                "b",
                Some(2),
                vec![window(&available, 2, OUTPUT)],
            ),
        ],
    );
    let result = run(
        &initial.model,
        2,
        vec![
            AppObservation::Unavailable {
                app: unavailable.clone(),
                error: "timeout".into(),
            },
            complete(
                &available,
                "b",
                Some(3),
                vec![window(&available, 3, OUTPUT)],
            ),
        ],
    );

    assert!(result.model.workspace.slots.iter().any(|slot| {
        slot.bound_window()
            .is_some_and(|window| window.app == unavailable)
    }));
    assert!(result.model.workspace.slots.iter().any(|slot| {
        slot.bound_window()
            .is_some_and(|window| window.window_id == 3)
    }));
}

#[test]
fn termination_only_removes_the_matching_process_incarnation() {
    let old = app(10, 1);
    let new = app(10, 2);
    let initial = run(
        &Model::default(),
        1,
        vec![
            complete(&old, "a", Some(1), vec![window(&old, 1, OUTPUT)]),
            complete(&new, "a", Some(2), vec![window(&new, 2, OUTPUT)]),
        ],
    );
    let result = run(
        &initial.model,
        2,
        vec![AppObservation::Terminated { app: old.clone() }],
    );

    assert_eq!(result.model.workspace.slots.len(), 1);
    assert_eq!(
        result.model.workspace.slots[0].bound_window().unwrap().app,
        new
    );
}

#[test]
fn valid_ghostty_replacement_preserves_slot() {
    let ghostty = app(10, 1);
    let first = run(
        &Model::default(),
        1,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(1),
            vec![window(&ghostty, 1, OUTPUT)],
        )],
    );
    let slot = first.model.workspace.slots[0].id;
    let evidence = ReconciliationEvidence {
        previous_placements: HashMap::from([(slot, OUTPUT)]),
    };
    let result = reconcile(
        &first.model,
        &NativeObservation {
            cycle: 2,
            mouse_down: false,
            applications: vec![complete(
                &ghostty,
                "com.mitchellh.ghostty",
                Some(2),
                vec![window(&ghostty, 2, OUTPUT)],
            )],
        },
        &evidence,
        &Settings {
            output: OUTPUT,
            frame_tolerance: 2,
            spacing: crate::spacing::Spacing::NONE,
        },
    );

    assert_eq!(result.model.workspace.slots.len(), 1);
    assert_eq!(result.model.workspace.slots[0].id, slot);
    assert_eq!(
        result.model.workspace.slots[0]
            .bound_window()
            .unwrap()
            .window_id,
        2
    );
    assert!(matches!(
        result.decisions[0],
        Decision::NativeTabReplaced { .. }
    ));
}

#[test]
fn ambiguous_ghostty_replacement_reserves_the_slot_without_a_write() {
    let ghostty = app(10, 1);
    let first = run(
        &Model::default(),
        1,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(1),
            vec![window(&ghostty, 1, OUTPUT)],
        )],
    );
    let result = run(
        &first.model,
        2,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(2),
            vec![window(
                &ghostty,
                2,
                Rect {
                    width: 500,
                    ..OUTPUT
                },
            )],
        )],
    );

    assert!(matches!(
        result.model.workspace.slots[0].binding,
        BindingState::Unresolved { .. }
    ));
    assert_eq!(result.placements.len(), 1);
    assert_eq!(result.placements[0].window, None);
    assert_eq!(result.placements[0].frame, OUTPUT);

    let retried = run(
        &result.model,
        3,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(2),
            vec![window(
                &ghostty,
                2,
                Rect {
                    width: 500,
                    ..OUTPUT
                },
            )],
        )],
    );
    assert_eq!(retried.model.workspace.slots.len(), 1);
    assert!(matches!(
        retried.model.workspace.slots[0].binding,
        BindingState::Unresolved { .. }
    ));
}

#[test]
fn unresolved_ghostty_slot_expires_after_bounded_settling() {
    let ghostty = app(10, 1);
    let first = run(
        &Model::default(),
        1,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(1),
            vec![window(&ghostty, 1, OUTPUT)],
        )],
    );
    let settling = run(
        &first.model,
        2,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(2),
            vec![window(
                &ghostty,
                2,
                Rect {
                    width: 500,
                    ..OUTPUT
                },
            )],
        )],
    );

    let still_settling = run(
        &settling.model,
        4,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            None,
            Vec::new(),
        )],
    );
    assert_eq!(still_settling.model.workspace.slots.len(), 1);

    let expired = run(
        &still_settling.model,
        5,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            None,
            Vec::new(),
        )],
    );
    assert!(expired.model.workspace.slots.is_empty());
    assert!(matches!(
        expired.decisions.as_slice(),
        [Decision::WindowRemoved { .. }]
    ));
}

#[test]
fn incomplete_observations_do_not_advance_ghostty_settling() {
    let ghostty = app(10, 1);
    let first = run(
        &Model::default(),
        1,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(1),
            vec![window(&ghostty, 1, OUTPUT)],
        )],
    );
    let ambiguous = run(
        &first.model,
        2,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(2),
            vec![window(
                &ghostty,
                2,
                Rect {
                    width: 500,
                    ..OUTPUT
                },
            )],
        )],
    );
    let mouse_down = reconcile(
        &ambiguous.model,
        &NativeObservation {
            cycle: 7,
            mouse_down: true,
            applications: Vec::new(),
        },
        &ReconciliationEvidence::default(),
        &Settings {
            output: OUTPUT,
            frame_tolerance: 2,
            spacing: crate::spacing::Spacing::NONE,
        },
    );
    let unavailable = run(
        &mouse_down.model,
        8,
        vec![AppObservation::Unavailable {
            app: ghostty.clone(),
            error: "timed out".into(),
        }],
    );
    let retried = run(
        &unavailable.model,
        9,
        vec![complete(
            &ghostty,
            "com.mitchellh.ghostty",
            Some(2),
            vec![window(
                &ghostty,
                2,
                Rect {
                    width: 500,
                    ..OUTPUT
                },
            )],
        )],
    );

    assert_eq!(retried.model.workspace.slots.len(), 1);
    assert!(matches!(
        retried.model.workspace.slots[0].binding,
        BindingState::Unresolved { .. }
    ));
}

#[test]
fn mouse_down_defers_all_model_changes() {
    let app = app(10, 1);
    let model = Model::default();
    let result = reconcile(
        &model,
        &NativeObservation {
            cycle: 1,
            mouse_down: true,
            applications: vec![complete(&app, "a", Some(1), vec![window(&app, 1, OUTPUT)])],
        },
        &ReconciliationEvidence::default(),
        &Settings {
            output: OUTPUT,
            frame_tolerance: 2,
            spacing: crate::spacing::Spacing::NONE,
        },
    );

    assert_eq!(result.model, model);
    assert_eq!(result.decisions, vec![Decision::MouseDownDeferred]);
}
