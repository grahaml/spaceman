mod fitting;
mod sizing;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{Axis, Direction, Rect, SlotId};

const MIN_TILE_EXTENT: i32 = 50;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LayoutTree {
    root: Node,
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::flat(Axis::Horizontal, [])
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Node {
    Leaf(SlotId),
    Split { axis: Axis, children: Vec<Child> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Child {
    weight: u32,
    node: Node,
}

impl LayoutTree {
    pub(crate) fn flat(axis: Axis, slots: impl IntoIterator<Item = SlotId>) -> Self {
        Self {
            root: Node::Split {
                axis,
                children: slots
                    .into_iter()
                    .map(|slot| Child {
                        weight: 1,
                        node: Node::Leaf(slot),
                    })
                    .collect(),
            },
        }
    }

    pub(crate) fn weighted_flat<'a>(
        axis: Axis,
        slots: impl IntoIterator<Item = &'a crate::Slot>,
    ) -> Self {
        Self {
            root: Node::Split {
                axis,
                children: slots
                    .into_iter()
                    .map(|slot| Child {
                        weight: slot.weight,
                        node: Node::Leaf(slot.id),
                    })
                    .collect(),
            },
        }
    }

    /// Show a default four-tile split as two equal pairs. Explicit nesting and
    /// resize weights carry user intent and take precedence over this default.
    pub(crate) fn quadrants(&self) -> Option<Self> {
        let Node::Split { axis, children } = &self.root else {
            return None;
        };
        let ([first_pair, second_pair], []) = children.as_chunks::<2>() else {
            return None;
        };
        if !children
            .iter()
            .all(|c| c.weight == 1 && matches!(c.node, Node::Leaf(_)))
        {
            return None;
        }
        let pair_axis = match axis {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        };
        let pairs = [first_pair, second_pair]
            .into_iter()
            .map(|pair| Child {
                weight: 1,
                node: Node::Split {
                    axis: pair_axis,
                    children: pair.to_vec(),
                },
            })
            .collect();
        Some(Self {
            root: Node::Split {
                axis: *axis,
                children: pairs,
            },
        })
    }

    pub(crate) const fn root_axis(&self) -> Axis {
        match &self.root {
            Node::Split { axis, .. } => *axis,
            Node::Leaf(_) => Axis::Horizontal,
        }
    }

    pub(crate) fn sync(&mut self, fallback_axis: Axis, slots: &[crate::Slot]) {
        self.sync_near(fallback_axis, slots, None);
    }

    pub(crate) fn sync_near(
        &mut self,
        fallback_axis: Axis,
        slots: &[crate::Slot],
        mut anchor: Option<SlotId>,
    ) -> bool {
        let retained = slots.iter().map(|slot| slot.id).collect::<HashSet<_>>();
        let existing = self.slots().into_iter().collect::<HashSet<_>>();
        if existing == retained && existing.len() == slots.len() {
            return false;
        }
        let root = std::mem::replace(
            &mut self.root,
            Node::Split {
                axis: fallback_axis,
                children: Vec::new(),
            },
        );
        self.root = match root {
            Node::Split { axis, children } => Node::Split {
                axis,
                children: retain_children(children, &retained),
            },
            root => retain(root, &retained).unwrap_or(Node::Split {
                axis: fallback_axis,
                children: Vec::new(),
            }),
        };
        for slot in slots.iter().filter(|slot| !existing.contains(&slot.id)) {
            if let Some(previous) = anchor
                && insert_sibling(&mut self.root, previous, slot.id)
            {
                anchor = Some(slot.id);
            } else {
                self.append(fallback_axis, slot.id, slot.weight);
            }
        }
        true
    }

    pub(crate) fn slots(&self) -> Vec<SlotId> {
        let mut slots = Vec::new();
        collect_slots(&self.root, &mut slots);
        slots
    }

    pub(crate) fn validate(&self, slots: &[crate::Slot]) -> Result<(), String> {
        let expected: HashSet<_> = slots.iter().map(|slot| slot.id).collect();
        if expected.len() != slots.len() {
            return Err("layout slots contain duplicate IDs".into());
        }
        let mut leaves = HashSet::new();
        validate_node(&self.root, true, &mut leaves)?;
        if leaves != expected {
            return Err("layout leaves do not exactly match workspace slots".into());
        }
        Ok(())
    }

    pub(crate) fn weight(&self, slot: SlotId) -> Option<u32> {
        leaf_weight(&self.root, slot)
    }

    pub(crate) fn placements(&self, output: Rect) -> Vec<(SlotId, Rect)> {
        let mut placements = Vec::new();
        place(&self.root, output, &mut placements);
        placements
    }

    pub(crate) fn swap(&mut self, first: SlotId, second: SlotId) -> bool {
        if first == second || !self.root.contains(first) || !self.root.contains(second) {
            return false;
        }
        swap_slots(&mut self.root, first, second);
        true
    }

    pub(crate) fn create_implicit_split(&mut self, slot: SlotId, direction: Direction) -> bool {
        let Some((remaining, moved)) = self.take_slot(slot) else {
            return false;
        };
        self.root = split_pair(direction, moved, remaining);
        true
    }

    pub(crate) fn move_relative(
        &mut self,
        moved: SlotId,
        target: SlotId,
        direction: Direction,
    ) -> bool {
        if moved == target || !self.root.contains(target) {
            return false;
        }
        let Some((mut remaining, extracted)) = self.take_slot(moved) else {
            return false;
        };
        if !insert_relative(&mut remaining, target, extracted, direction) {
            return false;
        }
        self.root = remaining;
        true
    }

    pub(crate) fn toggle_root_axis(&mut self) {
        if let Node::Split { axis, .. } = &mut self.root {
            *axis = match *axis {
                Axis::Horizontal => Axis::Vertical,
                Axis::Vertical => Axis::Horizontal,
            };
        }
    }

    fn take_slot(&self, slot: SlotId) -> Option<(Node, Node)> {
        match extract(self.root.clone(), slot) {
            (Some(remaining), Some(moved)) => Some((remaining, moved)),
            _ => None,
        }
    }

    fn append(&mut self, axis: Axis, slot: SlotId, weight: u32) {
        match &mut self.root {
            Node::Split { children, .. } => {
                let weight = if children.is_empty() {
                    weight.max(1)
                } else {
                    average_weight(children)
                };
                children.push(Child {
                    weight,
                    node: Node::Leaf(slot),
                });
            }
            root => {
                let previous = std::mem::replace(root, Node::Leaf(slot));
                *root = Node::Split {
                    axis,
                    children: vec![
                        Child {
                            weight: 1,
                            node: previous,
                        },
                        Child {
                            weight,
                            node: Node::Leaf(slot),
                        },
                    ],
                };
            }
        }
    }
}

/// Insert in the same container without changing its share of the parent.
fn insert_sibling(node: &mut Node, anchor: SlotId, slot: SlotId) -> bool {
    let Node::Split { children, .. } = node else {
        return false;
    };
    if let Some(i) = children.iter().position(|c| c.node == Node::Leaf(anchor)) {
        children.insert(
            i + 1,
            Child {
                weight: average_weight(children),
                node: Node::Leaf(slot),
            },
        );
        return true;
    }
    children
        .iter_mut()
        .any(|child| insert_sibling(&mut child.node, anchor, slot))
}

fn validate_node(node: &Node, root: bool, leaves: &mut HashSet<SlotId>) -> Result<(), String> {
    match node {
        Node::Leaf(slot) => {
            if !leaves.insert(*slot) {
                return Err(format!("layout contains duplicate leaf {}", slot.0));
            }
        }
        Node::Split { children, .. } => {
            if !root && children.is_empty() {
                return Err("layout contains an empty nested split".into());
            }
            for child in children {
                if child.weight == 0 {
                    return Err("layout contains a zero-weight child".into());
                }
                validate_node(&child.node, false, leaves)?;
            }
        }
    }
    Ok(())
}

const fn direction_axis(direction: Direction) -> Axis {
    match direction {
        Direction::Left | Direction::Right => Axis::Horizontal,
        Direction::Down | Direction::Up => Axis::Vertical,
    }
}

fn split_pair(direction: Direction, moved: Node, anchor: Node) -> Node {
    let moved = Child {
        weight: 1,
        node: moved,
    };
    let anchor = Child {
        weight: 1,
        node: anchor,
    };
    Node::Split {
        axis: direction_axis(direction),
        children: if matches!(direction, Direction::Left | Direction::Up) {
            vec![moved, anchor]
        } else {
            vec![anchor, moved]
        },
    }
}

fn retain(node: Node, retained: &HashSet<SlotId>) -> Option<Node> {
    match node {
        Node::Leaf(slot) => retained.contains(&slot).then_some(Node::Leaf(slot)),
        Node::Split { axis, children } => collapse(axis, retain_children(children, retained)),
    }
}

fn retain_children(children: Vec<Child>, retained: &HashSet<SlotId>) -> Vec<Child> {
    children
        .into_iter()
        .filter_map(|child| {
            retain(child.node, retained).map(|node| Child {
                weight: child.weight,
                node,
            })
        })
        .collect()
}

fn collapse(axis: Axis, mut children: Vec<Child>) -> Option<Node> {
    match children.len() {
        0 => None,
        1 => Some(children.remove(0).node),
        _ => Some(Node::Split { axis, children }),
    }
}

fn extract(node: Node, slot: SlotId) -> (Option<Node>, Option<Node>) {
    match node {
        Node::Leaf(existing) if existing == slot => (None, Some(Node::Leaf(existing))),
        Node::Leaf(existing) => (Some(Node::Leaf(existing)), None),
        Node::Split { axis, mut children } => {
            for index in 0..children.len() {
                let child = children.remove(index);
                let (remaining, moved) = extract(child.node, slot);
                if let Some(moved) = moved {
                    if let Some(remaining) = remaining {
                        children.insert(
                            index,
                            Child {
                                weight: child.weight,
                                node: remaining,
                            },
                        );
                    }
                    return (collapse(axis, children), Some(moved));
                }
                if let Some(remaining) = remaining {
                    children.insert(
                        index,
                        Child {
                            weight: child.weight,
                            node: remaining,
                        },
                    );
                }
            }
            (Some(Node::Split { axis, children }), None)
        }
    }
}

fn insert_relative(node: &mut Node, target: SlotId, moved: Node, direction: Direction) -> bool {
    let axis = direction_axis(direction);
    let before = matches!(direction, Direction::Left | Direction::Up);
    match node {
        Node::Leaf(existing) if *existing == target => {
            let target = std::mem::replace(node, Node::Leaf(target));
            *node = split_pair(direction, moved, target);
            true
        }
        Node::Leaf(_) => false,
        Node::Split {
            axis: parent_axis,
            children,
        } => {
            let Some(index) = children
                .iter()
                .position(|child| child.node.contains(target))
            else {
                return false;
            };
            if *parent_axis == axis
                && matches!(children[index].node, Node::Leaf(existing) if existing == target)
            {
                // Resize stores pixel-scale weights. A literal 1 here would
                // collapse the inserted tile beside those larger siblings.
                let weight = average_weight(children);
                children.insert(
                    index + usize::from(!before),
                    Child {
                        weight,
                        node: moved,
                    },
                );
                true
            } else {
                insert_relative(&mut children[index].node, target, moved, direction)
            }
        }
    }
}

fn average_weight(children: &[Child]) -> u32 {
    let total = children
        .iter()
        .map(|child| u64::from(child.weight))
        .sum::<u64>();
    u32::try_from(total / children.len().max(1) as u64)
        .unwrap_or(u32::MAX)
        .max(1)
}

fn collect_slots(node: &Node, slots: &mut Vec<SlotId>) {
    match node {
        Node::Leaf(slot) => slots.push(*slot),
        Node::Split { children, .. } => {
            for child in children {
                collect_slots(&child.node, slots);
            }
        }
    }
}

fn leaf_weight(node: &Node, slot: SlotId) -> Option<u32> {
    let Node::Split { children, .. } = node else {
        return None;
    };
    for child in children {
        if matches!(child.node, Node::Leaf(existing) if existing == slot) {
            return Some(child.weight);
        }
        if let Some(weight) = leaf_weight(&child.node, slot) {
            return Some(weight);
        }
    }
    None
}

fn place(node: &Node, frame: Rect, placements: &mut Vec<(SlotId, Rect)>) {
    match node {
        Node::Leaf(slot) => placements.push((*slot, frame)),
        Node::Split { axis, children } => {
            for (child, child_frame) in children
                .iter()
                .zip(partition_children(frame, *axis, children))
            {
                place(&child.node, child_frame, placements);
            }
        }
    }
}

fn partition_children(frame: Rect, axis: Axis, children: &[Child]) -> Vec<Rect> {
    let total = children
        .iter()
        .map(|child| u64::from(child.weight))
        .sum::<u64>();
    if total == 0 {
        return Vec::new();
    }
    let length = extent(frame, axis);
    let mut cursor = 0;
    let mut cumulative = 0_u64;
    children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            cumulative += u64::from(child.weight);
            let end = if index + 1 == children.len() {
                length
            } else {
                (i64::from(length) * cumulative as i64 / total as i64) as i32
            };
            let result = match axis {
                Axis::Horizontal => Rect {
                    x: frame.x + cursor,
                    width: end - cursor,
                    ..frame
                },
                Axis::Vertical => Rect {
                    y: frame.y + cursor,
                    height: end - cursor,
                    ..frame
                },
            };
            cursor = end;
            result
        })
        .collect()
}

const fn extent(frame: Rect, axis: Axis) -> i32 {
    match axis {
        Axis::Horizontal => frame.width,
        Axis::Vertical => frame.height,
    }
}

const fn leading(frame: Rect, axis: Axis) -> i32 {
    match axis {
        Axis::Horizontal => frame.x,
        Axis::Vertical => frame.y,
    }
}

const fn trailing(frame: Rect, axis: Axis) -> i32 {
    leading(frame, axis) + extent(frame, axis)
}

impl Node {
    fn contains(&self, slot: SlotId) -> bool {
        match self {
            Self::Leaf(existing) => *existing == slot,
            Self::Split { children, .. } => children.iter().any(|child| child.node.contains(slot)),
        }
    }
}

fn swap_slots(node: &mut Node, first: SlotId, second: SlotId) {
    match node {
        Node::Leaf(slot) if *slot == first => *slot = second,
        Node::Leaf(slot) if *slot == second => *slot = first,
        Node::Leaf(_) => {}
        Node::Split { children, .. } => {
            for child in children {
                swap_slots(&mut child.node, first, second);
            }
        }
    }
}
