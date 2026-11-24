//! Derive a display layout from the user's split tree and known window sizes.
//! Never write these temporary groupings to workspace state.
use std::collections::HashMap;

use super::{Child, LayoutTree, Node, extent, perpendicular, sizing::required_extent};
use crate::{Axis, Rect, SlotId};

impl LayoutTree {
    /// Try both two-by-two arrangements before regrouping a default workspace.
    /// Explicit splits keep the orientation the user gave them.
    pub(crate) fn fitted_for_display(
        &self,
        quadrants: Option<&Self>,
        output: Rect,
        minimums: &HashMap<SlotId, (i32, i32)>,
    ) -> Option<Self> {
        let Some(grid) = quadrants else {
            return self.fitted(output, minimums);
        };
        if grid.can_fit(output, minimums) {
            return grid.fitted(output, minimums);
        }
        if let Some(mut alternate) = self.quadrants_on(perpendicular(grid.root_axis()))
            && alternate.can_fit(output, minimums)
        {
            alternate.fit_minimums(output, minimums);
            return Some(alternate);
        }
        let candidate = grid.fitted(output, minimums);
        if candidate.as_ref().unwrap_or(grid).can_fit(output, minimums) {
            return candidate;
        }
        let flat_fit = self
            .fitted(output, minimums)
            .unwrap_or_else(|| self.clone());
        if flat_fit.can_fit(output, minimums) {
            Some(flat_fit)
        } else {
            candidate
        }
    }

    fn can_fit(&self, output: Rect, minimums: &HashMap<SlotId, (i32, i32)>) -> bool {
        required_extent(&self.root, Axis::Horizontal, minimums) <= output.width
            && required_extent(&self.root, Axis::Vertical, minimums) <= output.height
    }

    pub(super) fn fitted(
        &self,
        output: Rect,
        minimums: &HashMap<SlotId, (i32, i32)>,
    ) -> Option<Self> {
        if minimums.is_empty() {
            return None;
        }
        let mut candidate = self.clone();
        group_if_needed(&mut candidate.root, output, minimums);
        candidate.fit_minimums(output, minimums);
        (candidate != *self).then_some(candidate)
    }

    /// Whether this fit groups tiles that `requested` keeps apart, so it
    /// differs in more than weights and an edit to it must never be saved.
    pub(crate) fn regroups(&self, requested: &Self) -> bool {
        !same_shape(&self.root, &requested.root)
    }
}

fn same_shape(left: &Node, right: &Node) -> bool {
    match (left, right) {
        (Node::Leaf(left), Node::Leaf(right)) => left == right,
        (
            Node::Split {
                axis: left_axis,
                children: left,
            },
            Node::Split {
                axis: right_axis,
                children: right,
            },
        ) => {
            left_axis == right_axis
                && left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_shape(&left.node, &right.node))
        }
        _ => false,
    }
}

/// Grouping neighbors on the other axis saves the smaller of their two
/// required extents, and costs the sum of their cross-axis minimums. Prefer
/// the trailing pair to keep the leading tile in place.
fn group_if_needed(node: &mut Node, output: Rect, minimums: &HashMap<SlotId, (i32, i32)>) {
    let Node::Split { children, axis } = node else {
        return;
    };
    let cross = perpendicular(*axis);
    let sizes: Vec<_> = children
        .iter()
        .map(|child| {
            (
                i64::from(required_extent(&child.node, *axis, minimums)),
                i64::from(required_extent(&child.node, cross, minimums)),
            )
        })
        .collect();
    let shortage = sizes.iter().map(|s| s.0).sum::<i64>() - i64::from(extent(output, *axis));
    let available = i64::from(extent(output, cross));
    if shortage <= 0 || sizes.iter().any(|s| s.1 > available) {
        return;
    }
    let Some(index) = sizes.windows(2).rposition(|pair| {
        pair[0].0.min(pair[1].0) >= shortage && pair[0].1 + pair[1].1 <= available
    }) else {
        return;
    };
    let mut right = children.remove(index + 1);
    let mut left = children.remove(index);
    let weight = left.weight.saturating_add(right.weight);
    left.weight = 1;
    right.weight = 1;
    let stack = Node::Split {
        axis: cross,
        children: vec![left, right],
    };
    if children.is_empty() {
        *node = stack;
    } else {
        children.insert(
            index,
            Child {
                weight,
                node: stack,
            },
        );
    }
}
