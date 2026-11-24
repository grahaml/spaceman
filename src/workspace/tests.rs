use crate::{AppInstanceId, BindingState, Slot, SlotId};

use super::*;

fn window(id: u32) -> PhysicalWindowId {
    PhysicalWindowId {
        app: AppInstanceId {
            pid: 1,
            launched_at: 1,
        },
        window_id: id,
    }
}

#[test]
fn unresolved_affinity_reserves_candidates_without_owning_them() {
    let old = window(10);
    let replacement = window(11);
    let other = window(20);
    let mut workspaces = Workspaces::default();
    workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Unresolved {
            previous: old.clone(),
            candidates: vec![replacement.clone()],
            since: 1,
        },
    });
    assert_eq!(
        workspaces.workspace_for_app(&old.app),
        Some(WorkspaceId::ONE)
    );
    assert_eq!(workspaces.workspace_of(&replacement), None);
    assert_eq!(
        workspaces
            .replacement_workspaces(&replacement)
            .collect::<Vec<_>>(),
        vec![WorkspaceId::ONE]
    );
    workspaces.engine_mut(WorkspaceId::SIX).insert_slot(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(other),
    });
    assert_eq!(workspaces.workspace_for_app(&old.app), None);
    assert!(workspaces.state().validate().is_ok());
}

#[test]
fn workspace_range_and_default_assignment_match_aerospace() {
    assert!(WorkspaceId::new(0).is_err());
    assert!(WorkspaceId::new(11).is_err());
    assert_eq!(
        WorkspaceId::parse("d").unwrap(),
        WorkspaceId::named('D').unwrap()
    );
    assert_eq!(
        Workspaces::default().assignment(WorkspaceId::new(5).unwrap()),
        &DisplaySelector::Main
    );
    assert_eq!(
        Workspaces::default().assignment(WorkspaceId::new(6).unwrap()),
        &DisplaySelector::Secondary
    );
    assert!(!Workspaces::default().is_enabled(WorkspaceId::named('D').unwrap()));
    assert_eq!(
        Workspaces::default().state().slots.len(),
        WorkspaceId::NUMBERED_COUNT
    );
}

#[test]
fn workspace_identity_serializes_numbers_compatibly_and_names_as_letters() {
    assert_eq!(
        serde_json::to_value(WorkspaceId::ONE).unwrap(),
        serde_json::json!(1)
    );
    assert_eq!(
        serde_json::to_value(WorkspaceId::named('D').unwrap()).unwrap(),
        serde_json::json!("D")
    );
    assert_eq!(
        serde_json::from_value::<WorkspaceId>(serde_json::json!("D")).unwrap(),
        WorkspaceId::named('D').unwrap()
    );
}

#[test]
fn switching_one_display_preserves_the_other() {
    let mut workspaces = Workspaces::default();
    workspaces.switch(WorkspaceId::new(3).unwrap());
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Main),
        WorkspaceId::new(3).ok()
    );
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::SIX)
    );
}

#[test]
fn switching_to_a_named_workspace_preserves_the_other_display() {
    let mut workspaces = Workspaces::default();
    let named = WorkspaceId::named('D').unwrap();
    workspaces.enable(named);

    workspaces.switch(named);

    assert_eq!(workspaces.active_for(&DisplaySelector::Main), Some(named));
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::SIX)
    );
}

#[test]
fn unconfigured_named_workspaces_are_not_display_fallbacks() {
    let mut workspaces = Workspaces::default();
    for workspace in WorkspaceId::numbered().take(5) {
        workspaces.set_assignment(workspace, DisplaySelector::Secondary);
    }

    assert_eq!(workspaces.active_for(&DisplaySelector::Main), None);
}

/// Each display keeps its own previous workspace for moving a workspace off
/// it and for the HUD; Option+Tab uses the runtime's global focus history.
#[test]
fn per_display_previous_history_is_independent_for_moves() {
    let mut workspaces = Workspaces::default();
    let main_target = WorkspaceId::new(3).unwrap();
    let secondary_target = WorkspaceId::new(8).unwrap();

    workspaces.switch(main_target);
    workspaces.switch(secondary_target);

    assert_eq!(
        workspaces.previous_for(&DisplaySelector::Main),
        Some(WorkspaceId::ONE)
    );
    assert_eq!(
        workspaces.previous_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::SIX)
    );
    workspaces.switch(WorkspaceId::ONE);
    assert_eq!(
        workspaces.previous_for(&DisplaySelector::Main),
        Some(main_target)
    );
    assert_eq!(
        workspaces.previous_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::SIX)
    );
}

#[test]
fn switching_to_the_active_workspace_preserves_history() {
    let mut workspaces = Workspaces::default();
    let target = WorkspaceId::new(3).unwrap();
    workspaces.switch(target);

    workspaces.switch(target);

    assert_eq!(
        workspaces.previous_for(&DisplaySelector::Main),
        Some(WorkspaceId::ONE)
    );
}

#[test]
fn persisted_state_restores_active_workspaces_and_tile_order() {
    let first = window(10);
    let second = window(20);
    let mut workspaces = Workspaces::default();
    workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(first.clone()),
    });
    workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
        id: SlotId(2),
        weight: 1,
        binding: BindingState::Bound(second.clone()),
    });
    workspaces.engine_mut(WorkspaceId::ONE).toggle_axis();
    let third = WorkspaceId::new(3).unwrap();
    workspaces.switch(third);

    let encoded = serde_json::to_vec(&workspaces.state()).unwrap();
    let state = serde_json::from_slice(&encoded).unwrap();
    let mut restored = Workspaces::default();
    restored.restore(state);

    assert_eq!(restored.active_for(&DisplaySelector::Main), Some(third));
    assert_eq!(
        restored.previous_for(&DisplaySelector::Main),
        Some(WorkspaceId::ONE)
    );
    assert_eq!(
        restored.engine(WorkspaceId::ONE).model().workspace.axis,
        crate::Axis::Vertical
    );
    assert_eq!(
        restored.engine(WorkspaceId::ONE).bound_windows(),
        vec![first.clone(), second]
    );
    assert_eq!(restored.workspace_of(&first), Some(WorkspaceId::ONE));
}

#[test]
fn saved_four_tile_splits_show_quadrants_only_at_default_weights() {
    let output = crate::Rect {
        width: 1000,
        height: 800,
        ..crate::Rect::default()
    };
    for (weights, expected_extents) in [
        ([1, 1, 1, 1], [(500, 400); 4]),
        (
            [4, 2, 2, 2],
            [(400, 800), (200, 800), (200, 800), (200, 800)],
        ),
    ] {
        let slots = (1..=4)
            .zip(weights)
            .map(|(id, weight)| Slot {
                id: SlotId(u64::from(id)),
                binding: BindingState::Bound(window(id)),
                weight,
            })
            .collect::<Vec<_>>();
        let layout = crate::layout_tree::LayoutTree::weighted_flat(crate::Axis::Horizontal, &slots);
        for layouts in [vec![layout.clone()], vec![]] {
            let state = WorkspaceState {
                slots: vec![slots.clone()],
                layouts,
                ..WorkspaceState::default()
            };
            state.validate().unwrap();
            let mut restored = Workspaces::default();
            restored.restore(state);
            let frames = restored.engine(WorkspaceId::ONE).preview(output);
            assert_eq!(
                frames
                    .iter()
                    .map(|r| (r.width, r.height))
                    .collect::<Vec<_>>(),
                expected_extents
            );
        }
    }
}

#[test]
fn persisted_state_restores_focus_and_fullscreen_layout() {
    let first = window(10);
    let second = window(20);
    let mut workspaces = Workspaces::default();
    for (id, window) in [(1, &first), (2, &second)] {
        workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
            id: SlotId(id),
            weight: 1,
            binding: BindingState::Bound(window.clone()),
        });
    }
    workspaces.remember_focus(WorkspaceId::ONE, &second);
    workspaces.set_fullscreen_layout(WorkspaceId::ONE, true);

    let encoded = serde_json::to_value(workspaces.state()).unwrap();
    assert_eq!(encoded["fullscreen_layout"], serde_json::json!([true]));
    assert!(encoded.get("maximized").is_none());
    let state: WorkspaceState = serde_json::from_value(encoded).unwrap();
    state.validate().unwrap();
    let mut restored = Workspaces::default();
    restored.restore(state);

    assert_eq!(
        restored.preferred_focus(WorkspaceId::ONE),
        Some(second.clone())
    );
    assert!(restored.uses_fullscreen_layout(WorkspaceId::ONE));
}

#[test]
fn persisted_empty_workspace_cannot_restore_fullscreen_layout() {
    let mut state = Workspaces::default().state();
    state.fullscreen_layout = vec![true];
    let mut restored = Workspaces::default();

    restored.restore(state);

    assert!(!restored.uses_fullscreen_layout(WorkspaceId::ONE));
}

#[test]
fn fullscreen_layout_survives_until_the_final_slot_is_removed() {
    let first = window(10);
    let second = window(20);
    let mut workspaces = Workspaces::default();
    for (id, window) in [(1, &first), (2, &second)] {
        workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
            id: SlotId(id),
            weight: 1,
            binding: BindingState::Bound(window.clone()),
        });
    }
    workspaces.set_fullscreen_layout(WorkspaceId::ONE, true);

    workspaces.forget_window(WorkspaceId::ONE, &first);
    assert!(workspaces.uses_fullscreen_layout(WorkspaceId::ONE));
    workspaces.forget_window(WorkspaceId::ONE, &second);

    assert!(!workspaces.uses_fullscreen_layout(WorkspaceId::ONE));
}

#[test]
fn legacy_maximize_intent_migrates_to_fullscreen_layout() {
    let first = window(10);
    let state = serde_json::json!({
        "active": [],
        "slots": [[
            {
                "id": 7,
                "weight": 1,
                "binding": {
                    "state": "bound",
                    "app": { "pid": 1, "launched_at": 1 },
                    "window_id": 10
                }
            },
            {
                "id": 8,
                "weight": 1,
                "binding": {
                    "state": "bound",
                    "app": { "pid": 1, "launched_at": 1 },
                    "window_id": 20
                }
            }
        ]],
        "last_focused": [8],
        "maximized": [7]
    });
    let mut workspaces = Workspaces::default();

    workspaces.restore(serde_json::from_value(state).unwrap());

    assert!(workspaces.uses_fullscreen_layout(WorkspaceId::ONE));
    assert_eq!(workspaces.preferred_focus(WorkspaceId::ONE), Some(first));
}

#[test]
fn persisted_state_round_trips_a_named_workspace() {
    let named = WorkspaceId::named('D').unwrap();
    let owned = window(40);
    let mut workspaces = Workspaces::default();
    workspaces.enable(named);
    workspaces.engine_mut(named).insert_slot(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(owned.clone()),
    });
    workspaces.switch(named);

    let encoded = serde_json::to_value(workspaces.state()).unwrap();
    let state: WorkspaceState = serde_json::from_value(encoded).unwrap();
    state.validate().unwrap();
    let mut restored = Workspaces::default();
    restored.restore(state);

    assert_eq!(restored.active_for(&DisplaySelector::Main), Some(named));
    assert_eq!(restored.workspace_of(&owned), Some(named));
    assert!(restored.is_enabled(named));
}

#[test]
fn persisted_state_restores_floating_window_ownership() {
    let floating = window(30);
    let workspace = WorkspaceId::new(3).unwrap();
    let mut workspaces = Workspaces::default();
    workspaces.own_floating(floating.clone(), workspace);

    let encoded = serde_json::to_vec(&workspaces.state()).unwrap();
    let state: WorkspaceState = serde_json::from_slice(&encoded).unwrap();
    state.validate().unwrap();
    let mut restored = Workspaces::default();
    restored.restore(state);

    assert_eq!(restored.floating_workspace_of(&floating), Some(workspace));
    assert_eq!(restored.workspace_of(&floating), None);
}

#[test]
fn nested_layout_round_trips_without_flattening() {
    let first = window(10);
    let second = window(20);
    let third = window(30);
    let mut workspaces = Workspaces::default();
    for (id, window) in [(1, &first), (2, &second), (3, &third)] {
        workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
            id: SlotId(id),
            weight: 1,
            binding: BindingState::Bound(window.clone()),
        });
    }
    assert!(
        workspaces
            .engine_mut(WorkspaceId::ONE)
            .create_implicit_split(&second, crate::Direction::Up)
    );

    let expected = workspaces.engine(WorkspaceId::ONE).preview(crate::Rect {
        x: 0,
        y: 0,
        width: 1200,
        height: 800,
    });
    let encoded = serde_json::to_value(workspaces.state()).unwrap();
    assert!(encoded.get("layouts").is_some());
    assert!(encoded.get("axes").is_none());
    let mut restored = Workspaces::default();
    restored.restore(serde_json::from_value(encoded).unwrap());

    assert_eq!(
        restored.engine(WorkspaceId::ONE).preview(crate::Rect {
            x: 0,
            y: 0,
            width: 1200,
            height: 800,
        }),
        expected
    );
}

#[test]
fn persisted_state_rejects_layout_with_wrong_slot_membership() {
    let mut state = Workspaces::default().state();
    state.slots[0].push(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(window(10)),
    });

    assert!(state.validate().is_err());
}

#[test]
fn persisted_state_rejects_zero_weight_slots() {
    let mut state = Workspaces::default().state();
    state.slots[0].push(Slot {
        id: SlotId(1),
        weight: 0,
        binding: BindingState::Bound(window(10)),
    });
    state.layouts[0] = crate::layout_tree::LayoutTree::flat(crate::Axis::Horizontal, [SlotId(1)]);

    assert!(state.validate().is_err());
}

#[test]
fn persisted_state_rejects_a_window_owned_by_multiple_workspaces() {
    let mut state = Workspaces::default().state();
    state.slots[0].push(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(window(10)),
    });
    state.slots[1].push(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Unresolved {
            previous: window(10),
            candidates: vec![window(20)],
            since: 1,
        },
    });
    for index in [0, 1] {
        state.layouts[index] =
            crate::layout_tree::LayoutTree::flat(crate::Axis::Horizontal, [SlotId(1)]);
    }

    assert_eq!(
        state.validate().unwrap_err(),
        "tiled window is owned by multiple workspaces"
    );
}

#[test]
fn restoring_unresolved_state_restarts_its_settling_window() {
    let mut state = Workspaces::default().state();
    state.slots[0].push(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Unresolved {
            previous: window(10),
            candidates: vec![window(20)],
            since: 10_000,
        },
    });
    state.layouts[0] = crate::layout_tree::LayoutTree::flat(crate::Axis::Horizontal, [SlotId(1)]);
    let mut workspaces = Workspaces::default();

    workspaces.restore(state);

    assert!(matches!(
        workspaces.engine(WorkspaceId::ONE).model().workspace.slots[0].binding,
        BindingState::Unresolved { since: 0, .. }
    ));
}

#[test]
fn persisted_state_rejects_intent_for_a_missing_slot() {
    let mut state = Workspaces::default().state();
    state.maximized = vec![Some(SlotId(99))];

    assert!(state.validate().is_err());
}

#[test]
fn legacy_flat_state_migrates_to_a_root_split() {
    let first = window(10);
    let state = serde_json::json!({
        "active": [],
        "axes": ["vertical"],
        "slots": [[{
            "id": 7,
            "weight": 3,
            "binding": {
                "state": "bound",
                "app": { "pid": 1, "launched_at": 1 },
                "window_id": 10
            }
        }]]
    });
    let mut workspaces = Workspaces::default();

    workspaces.restore(serde_json::from_value(state).unwrap());

    assert_eq!(
        workspaces.engine(WorkspaceId::ONE).root_axis(),
        crate::Axis::Vertical
    );
    assert_eq!(
        workspaces.engine(WorkspaceId::ONE).bound_windows(),
        vec![first]
    );
}

#[test]
fn preferred_monitor_creates_an_independent_active_lane() {
    let mut workspaces = Workspaces::default();
    let ninth = WorkspaceId::new(9).unwrap();

    workspaces.set_assignment(ninth, DisplaySelector::Preferred("third-id".into()));

    assert_eq!(
        workspaces.active_for(&DisplaySelector::Main),
        Some(WorkspaceId::ONE)
    );
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::SIX)
    );
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Preferred("third-id".into())),
        Some(ninth)
    );
}

#[test]
fn moving_preserves_the_logical_slot() {
    let mut workspaces = Workspaces::default();
    let window = window(10);
    let slot = Slot {
        id: SlotId(7),
        weight: 3,
        binding: BindingState::Bound(window.clone()),
    };
    workspaces.engine_mut(WorkspaceId::ONE).insert_slot(slot);
    workspaces.set_fullscreen_layout(WorkspaceId::ONE, true);

    assert_eq!(
        workspaces.move_window(&window, WorkspaceId::new(3).unwrap()),
        Some(WorkspaceId::ONE)
    );
    assert!(
        workspaces
            .engine(WorkspaceId::ONE)
            .model()
            .workspace
            .slots
            .is_empty()
    );
    assert!(!workspaces.uses_fullscreen_layout(WorkspaceId::ONE));
    let moved = &workspaces
        .engine(WorkspaceId::new(3).unwrap())
        .model()
        .workspace
        .slots[0];
    assert_eq!((moved.id, moved.weight), (SlotId(7), 3));
    assert_eq!(moved.bound_window(), Some(&window));
}

#[test]
fn move_to_display_activates_on_target_and_promotes_source_previous() {
    let mut workspaces = Workspaces::default();
    workspaces.switch(WorkspaceId::THREE);
    workspaces.switch(WorkspaceId::ONE);

    workspaces.move_to_display(
        WorkspaceId::ONE,
        DisplaySelector::Secondary,
        Some(WorkspaceId::THREE),
    );

    assert_eq!(
        workspaces.assignment(WorkspaceId::ONE),
        &DisplaySelector::Secondary
    );
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::ONE)
    );
    assert_eq!(
        workspaces.previous_for(&DisplaySelector::Secondary),
        Some(WorkspaceId::SIX)
    );
    assert_eq!(
        workspaces.active_for(&DisplaySelector::Main),
        Some(WorkspaceId::THREE)
    );
    assert_eq!(workspaces.previous_for(&DisplaySelector::Main), None);
    assert!(workspaces.state().validate().is_ok());
}

#[test]
fn moving_into_a_populated_workspace_avoids_slot_id_collisions() {
    let mut workspaces = Workspaces::default();
    let source = window(10);
    let existing = window(20);
    workspaces.engine_mut(WorkspaceId::ONE).insert_slot(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(source.clone()),
    });
    let target = WorkspaceId::new(2).unwrap();
    workspaces.engine_mut(target).insert_slot(Slot {
        id: SlotId(1),
        weight: 1,
        binding: BindingState::Bound(existing),
    });

    workspaces.move_window(&source, target).unwrap();
    let ids: std::collections::HashSet<_> = workspaces
        .engine(target)
        .model()
        .workspace
        .slots
        .iter()
        .map(|slot| slot.id)
        .collect();
    assert_eq!(ids.len(), 2);
    workspaces.engine(target).model().validate().unwrap();
}
