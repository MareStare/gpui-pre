use std::fmt::Debug;

use collections::FxHashMap;

use crate::{FocusHandle, FocusId};

/// Represents a collection of focus handles using the tab-index APIs.
///
/// Every focusable element is inserted on every frame, while the tab order is only needed
/// when the user moves focus with tab. So insertion just appends the node, and the nodes
/// are sorted in place the first time the order is needed after an insertion.
#[derive(Debug, Default)]
pub(crate) struct TabStopMap {
    current_path: TabStopPath,
    pub(crate) insertion_history: Vec<TabStopOperation>,
    /// Index into `nodes` of each focus handle's most recent insertion.
    by_id: FxHashMap<FocusId, usize>,
    nodes: Vec<TabStopNode>,
    sorted: bool,
    /// The wrapping group of each open group, innermost last. `None` for a
    /// group that does not wrap.
    open_groups: Vec<Option<WrapGroupId>>,
    /// The wrapping group that holds each wrapping group, if any.
    wrap_parents: FxHashMap<WrapGroupId, Option<WrapGroupId>>,
}

/// A group whose tab order wraps, by the index of its `Group` operation in
/// the insertion history.
type WrapGroupId = usize;

#[derive(Debug, Clone)]
pub enum TabStopOperation {
    Insert(FocusHandle),
    Group { tab_index: TabIndex, wraps: bool },
    GroupEnd,
}

impl TabStopOperation {
    fn focus_handle(&self) -> Option<&FocusHandle> {
        match self {
            TabStopOperation::Insert(focus_handle) => Some(focus_handle),
            _ => None,
        }
    }
}

type TabIndex = isize;

#[derive(Debug, Default, PartialEq, Eq, Clone, Ord, PartialOrd)]
struct TabStopPath(smallvec::SmallVec<[TabIndex; 6]>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TabStopNode {
    /// Path to access the node in the tree
    /// The final node in the list is a leaf node corresponding to an actual focus handle,
    /// all other nodes are group nodes
    path: TabStopPath,
    /// index into the backing array of nodes. Corresponds to insertion order
    node_insertion_index: usize,

    /// Whether this node is a tab stop
    tab_stop: bool,

    /// The innermost wrapping group that holds this node.
    wrap_group: Option<WrapGroupId>,
}

impl Ord for TabStopNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path
            .cmp(&other.path)
            .then(self.node_insertion_index.cmp(&other.node_insertion_index))
    }
}

impl PartialOrd for TabStopNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

impl TabStopMap {
    pub fn insert(&mut self, focus_handle: &FocusHandle) {
        self.insertion_history
            .push(TabStopOperation::Insert(focus_handle.clone()));
        let mut path = self.current_path.clone();
        path.0.push(focus_handle.tab_index);
        self.by_id.insert(focus_handle.id, self.nodes.len());
        self.sorted = false;
        self.nodes.push(TabStopNode {
            node_insertion_index: self.insertion_history.len() - 1,
            tab_stop: focus_handle.tab_stop,
            wrap_group: self.innermost_wrap_group(),
            path,
        });
    }

    /// Opens a group. When the group `wraps`, `next` and `prev` from a node
    /// in it stay in it and wrap at its ends.
    pub fn begin_group(&mut self, tab_index: isize, wraps: bool) {
        let id = self.insertion_history.len();
        self.insertion_history
            .push(TabStopOperation::Group { tab_index, wraps });
        self.current_path.0.push(tab_index);
        if wraps {
            self.wrap_parents.insert(id, self.innermost_wrap_group());
        }
        self.open_groups.push(wraps.then_some(id));
    }

    pub fn end_group(&mut self) {
        self.insertion_history.push(TabStopOperation::GroupEnd);
        self.current_path.0.pop();
        self.open_groups.pop();
    }

    fn innermost_wrap_group(&self) -> Option<WrapGroupId> {
        self.open_groups.iter().rev().find_map(|group| *group)
    }

    /// Whether a node of the wrapping group `group` is inside the wrapping
    /// group `scope`. With no scope, each node is inside.
    fn is_within(&self, mut group: Option<WrapGroupId>, scope: Option<WrapGroupId>) -> bool {
        let Some(scope) = scope else {
            return true;
        };
        while let Some(id) = group {
            if id == scope {
                return true;
            }
            group = self.wrap_parents.get(&id).copied().flatten();
        }
        false
    }

    /// The wrapping group of the focused node, which bounds a step from it.
    fn scope_of(&self, position: Option<usize>) -> Option<WrapGroupId> {
        position.and_then(|position| self.nodes[position].wrap_group)
    }

    pub fn clear(&mut self) {
        self.current_path.0.clear();
        self.insertion_history.clear();
        self.by_id.clear();
        self.nodes.clear();
        self.sorted = false;
        self.open_groups.clear();
        self.wrap_parents.clear();
    }

    pub fn next(&mut self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        self.ensure_sorted();
        let focused = focused_id.and_then(|id| self.by_id.get(id).copied());
        let scope = self.scope_of(focused);
        let start = focused.map_or(0, |position| position + 1);
        let node = self.nodes[start..]
            .iter()
            .chain(&self.nodes[..start])
            .find(|node| node.tab_stop && self.is_within(node.wrap_group, scope))?;
        self.focus_handle_for_order(node)
    }

    pub fn prev(&mut self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        self.ensure_sorted();
        let focused = focused_id.and_then(|id| self.by_id.get(id).copied());
        let scope = self.scope_of(focused);
        let end = focused.unwrap_or(self.nodes.len());
        let node = self.nodes[..end]
            .iter()
            .rev()
            .chain(self.nodes[end..].iter().rev())
            .find(|node| node.tab_stop && self.is_within(node.wrap_group, scope))?;
        self.focus_handle_for_order(node)
    }

    fn ensure_sorted(&mut self) {
        if self.sorted {
            return;
        }
        // Nodes never compare equal, since their insertion indices differ, so an unstable
        // sort gives the same order as a stable one without allocating a scratch buffer.
        self.nodes.sort_unstable();
        self.by_id.clear();
        for (position, node) in self.nodes.iter().enumerate() {
            let Some(focus_handle) = self
                .insertion_history
                .get(node.node_insertion_index)
                .and_then(TabStopOperation::focus_handle)
            else {
                continue;
            };
            let latest = self.by_id.entry(focus_handle.id).or_insert(position);
            if self.nodes[*latest].node_insertion_index < node.node_insertion_index {
                *latest = position;
            }
        }
        self.sorted = true;
    }

    pub fn replay(&mut self, nodes: &[TabStopOperation]) {
        for node in nodes {
            match node {
                TabStopOperation::Insert(focus_handle) => self.insert(focus_handle),
                TabStopOperation::Group { tab_index, wraps } => {
                    self.begin_group(*tab_index, *wraps)
                }
                TabStopOperation::GroupEnd => self.end_group(),
            }
        }
    }

    pub fn paint_index(&self) -> usize {
        self.insertion_history.len()
    }

    pub(crate) fn tab_stop_count(&self) -> usize {
        self.by_id
            .values()
            .filter(|index| self.nodes.get(**index).is_some_and(|node| node.tab_stop))
            .count()
    }

    fn focus_handle_for_order(&self, order: &TabStopNode) -> Option<FocusHandle> {
        let handle = self.insertion_history[order.node_insertion_index].focus_handle();
        debug_assert!(
            handle.is_some(),
            "The order node did not correspond to an element, this is a GPUI bug"
        );
        handle.cloned()
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools as _;
    use parking_lot::RwLock;

    use crate::{FocusHandle, FocusId, FocusMap, TabStopMap};
    use std::sync::Arc;

    #[test]
    fn test_tab_handles() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let mut tab_index_map = TabStopMap::default();

        let focus_handles = [
            FocusHandle::new(&focus_map).tab_stop(true).tab_index(0),
            FocusHandle::new(&focus_map).tab_stop(true).tab_index(1),
            FocusHandle::new(&focus_map).tab_stop(true).tab_index(1),
            FocusHandle::new(&focus_map),
            FocusHandle::new(&focus_map).tab_index(2),
            FocusHandle::new(&focus_map).tab_stop(true).tab_index(0),
            FocusHandle::new(&focus_map).tab_stop(true).tab_index(2),
        ];

        for handle in focus_handles.iter() {
            tab_index_map.insert(handle);
        }
        let expected = [
            focus_handles[0].clone(),
            focus_handles[5].clone(),
            focus_handles[1].clone(),
            focus_handles[2].clone(),
            focus_handles[6].clone(),
        ];

        let mut prev = None;
        let mut found = vec![];
        for _ in 0..expected.len() {
            let handle = tab_index_map.next(prev.as_ref()).unwrap();
            prev = Some(handle.id);
            found.push(handle.id);
        }

        assert_eq!(
            found,
            expected.iter().map(|handle| handle.id).collect::<Vec<_>>()
        );

        // Select first tab index if no handle is currently focused.
        assert_eq!(tab_index_map.next(None), Some(expected[0].clone()));
        // Select last tab index if no handle is currently focused.
        assert_eq!(tab_index_map.prev(None), expected.last().cloned(),);

        assert_eq!(
            tab_index_map.next(Some(&expected[0].id)),
            Some(expected[1].clone())
        );
        assert_eq!(
            tab_index_map.next(Some(&expected[1].id)),
            Some(expected[2].clone())
        );
        assert_eq!(
            tab_index_map.next(Some(&expected[2].id)),
            Some(expected[3].clone())
        );
        assert_eq!(
            tab_index_map.next(Some(&expected[3].id)),
            Some(expected[4].clone())
        );
        assert_eq!(
            tab_index_map.next(Some(&expected[4].id)),
            Some(expected[0].clone())
        );

        // prev
        assert_eq!(tab_index_map.prev(None), Some(expected[4].clone()));
        assert_eq!(
            tab_index_map.prev(Some(&expected[0].id)),
            Some(expected[4].clone())
        );
        assert_eq!(
            tab_index_map.prev(Some(&expected[1].id)),
            Some(expected[0].clone())
        );
        assert_eq!(
            tab_index_map.prev(Some(&expected[2].id)),
            Some(expected[1].clone())
        );
        assert_eq!(
            tab_index_map.prev(Some(&expected[3].id)),
            Some(expected[2].clone())
        );
        assert_eq!(
            tab_index_map.prev(Some(&expected[4].id)),
            Some(expected[3].clone())
        );
    }

    #[test]
    fn test_tab_non_stop_filtering() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let mut tab_index_map = TabStopMap::default();

        // Check that we can query next from a non-stop tab
        let tab_non_stop_1 = FocusHandle::new(&focus_map).tab_stop(false).tab_index(1);
        let tab_stop_2 = FocusHandle::new(&focus_map).tab_stop(true).tab_index(2);
        tab_index_map.insert(&tab_non_stop_1);
        tab_index_map.insert(&tab_stop_2);
        let result = tab_index_map.next(Some(&tab_non_stop_1.id)).unwrap();
        assert_eq!(result.id, tab_stop_2.id);

        // Check that we skip over non-stop tabs
        let tab_stop_0 = FocusHandle::new(&focus_map).tab_stop(true).tab_index(0);
        let tab_non_stop_0 = FocusHandle::new(&focus_map).tab_stop(false).tab_index(0);
        tab_index_map.insert(&tab_stop_0);
        tab_index_map.insert(&tab_non_stop_0);
        let result = tab_index_map.next(Some(&tab_stop_0.id)).unwrap();
        assert_eq!(result.id, tab_stop_2.id);
    }

    #[must_use]
    struct TabStopMapTest {
        tab_map: TabStopMap,
        focus_map: Arc<RwLock<FocusMap>>,
        expected: Vec<(usize, FocusId)>,
    }

    impl TabStopMapTest {
        #[must_use]
        fn new() -> Self {
            Self {
                tab_map: TabStopMap::default(),
                focus_map: Arc::new(RwLock::new(FocusMap::default())),
                expected: Vec::default(),
            }
        }

        #[must_use]
        fn tab_non_stop(mut self, index: isize) -> Self {
            let handle = FocusHandle::new(&self.focus_map)
                .tab_stop(false)
                .tab_index(index);
            self.tab_map.insert(&handle);
            self
        }

        #[must_use]
        fn tab_stop(mut self, index: isize, expected: usize) -> Self {
            let handle = FocusHandle::new(&self.focus_map)
                .tab_stop(true)
                .tab_index(index);
            self.tab_map.insert(&handle);
            self.expected.push((expected, handle.id));
            self.expected.sort_by_key(|(expected, _)| *expected);
            self
        }

        #[must_use]
        fn tab_group(mut self, tab_index: isize, children: impl FnOnce(Self) -> Self) -> Self {
            self.tab_map.begin_group(tab_index, false);
            self = children(self);
            self.tab_map.end_group();
            self
        }

        fn traverse_tab_map(
            &mut self,
            traverse: impl Fn(&mut TabStopMap, Option<&FocusId>) -> Option<FocusHandle>,
        ) -> Vec<FocusId> {
            let mut last_focus_id = None;
            let mut found = vec![];
            for _ in 0..self.expected.len() {
                let handle = traverse(&mut self.tab_map, last_focus_id.as_ref()).unwrap();
                last_focus_id = Some(handle.id);
                found.push(handle.id);
            }
            found
        }

        fn assert(mut self) {
            let mut expected = self.expected.iter().map(|(_, id)| *id).collect_vec();

            // Check next order
            let forward_found = self.traverse_tab_map(|tab_map, prev| tab_map.next(prev));
            assert_eq!(forward_found, expected);

            // Test overflow. Last to first
            assert_eq!(
                self.tab_map
                    .next(forward_found.last())
                    .map(|handle| handle.id),
                expected.first().cloned()
            );

            // Check previous order
            let reversed_found = self.traverse_tab_map(|tab_map, prev| tab_map.prev(prev));
            expected.reverse();
            assert_eq!(reversed_found, expected);

            // Test overflow. First to last
            assert_eq!(
                self.tab_map
                    .prev(reversed_found.last())
                    .map(|handle| handle.id),
                expected.first().cloned(),
            );
        }
    }

    #[test]
    fn test_insert_after_sorting() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let mut tab_index_map = TabStopMap::default();
        let second = FocusHandle::new(&focus_map).tab_stop(true).tab_index(1);
        let first = FocusHandle::new(&focus_map).tab_stop(true).tab_index(0);

        tab_index_map.insert(&second);
        assert_eq!(tab_index_map.next(None), Some(second.clone()));

        tab_index_map.insert(&first);
        assert_eq!(tab_index_map.next(None), Some(first.clone()));
        assert_eq!(tab_index_map.next(Some(&first.id)), Some(second.clone()));
        assert_eq!(tab_index_map.prev(Some(&first.id)), Some(second));
    }

    #[test]
    fn test_with_disabled_tab_stop() {
        TabStopMapTest::new()
            .tab_stop(0, 0)
            .tab_non_stop(1)
            .tab_stop(2, 1)
            .tab_stop(3, 2)
            .assert();
    }

    #[test]
    fn test_with_multiple_disabled_tab_stops() {
        TabStopMapTest::new()
            .tab_non_stop(0)
            .tab_stop(1, 0)
            .tab_non_stop(3)
            .tab_stop(3, 1)
            .tab_non_stop(4)
            .assert();
    }

    #[test]
    fn test_tab_group_functionality() {
        TabStopMapTest::new()
            .tab_stop(0, 0)
            .tab_stop(0, 1)
            .tab_group(2, |t| t.tab_stop(0, 2).tab_stop(1, 3))
            .tab_stop(3, 4)
            .tab_stop(4, 5)
            .assert()
    }

    #[test]
    fn test_sibling_groups() {
        TabStopMapTest::new()
            .tab_stop(0, 0)
            .tab_stop(1, 1)
            .tab_group(2, |test| test.tab_stop(0, 2).tab_stop(1, 3))
            .tab_stop(3, 4)
            .tab_stop(4, 5)
            .tab_group(6, |test| test.tab_stop(0, 6).tab_stop(1, 7))
            .tab_stop(7, 8)
            .tab_stop(8, 9)
            .assert();
    }

    #[test]
    fn test_nested_group() {
        TabStopMapTest::new()
            .tab_stop(0, 0)
            .tab_stop(1, 1)
            .tab_group(2, |t| {
                t.tab_group(0, |t| t.tab_stop(0, 2).tab_stop(1, 3))
                    .tab_stop(1, 4)
            })
            .tab_stop(3, 5)
            .tab_stop(4, 6)
            .assert();
    }

    #[test]
    fn test_sibling_nested_groups() {
        TabStopMapTest::new()
            .tab_stop(0, 0)
            .tab_stop(1, 1)
            .tab_group(2, |builder| {
                builder
                    .tab_stop(0, 2)
                    .tab_stop(2, 5)
                    .tab_group(1, |builder| builder.tab_stop(0, 3).tab_stop(1, 4))
                    .tab_group(3, |builder| builder.tab_stop(0, 6).tab_stop(1, 7))
            })
            .tab_stop(3, 8)
            .tab_stop(4, 9)
            .assert();
    }

    #[test]
    fn test_sibling_nested_groups_out_of_order() {
        TabStopMapTest::new()
            .tab_stop(9, 9)
            .tab_stop(8, 8)
            .tab_group(7, |builder| {
                builder
                    .tab_stop(0, 2)
                    .tab_stop(2, 5)
                    .tab_group(3, |builder| builder.tab_stop(1, 7).tab_stop(0, 6))
                    .tab_group(1, |builder| builder.tab_stop(0, 3).tab_stop(1, 4))
            })
            .tab_stop(3, 0)
            .tab_stop(4, 1)
            .assert();
    }

    /// Steps with `step` from `from` and returns the focused handle.
    fn step(
        map: &mut TabStopMap,
        step: fn(&mut TabStopMap, Option<&FocusId>) -> Option<FocusHandle>,
        from: &FocusHandle,
    ) -> Option<FocusId> {
        step(map, Some(&from.id)).map(|handle| handle.id)
    }

    #[test]
    fn test_wrapping_group_keeps_the_steps_inside() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let stop = || FocusHandle::new(&focus_map).tab_stop(true);
        let (before, after) = (stop(), stop());
        let container = FocusHandle::new(&focus_map);
        let (first, middle, last) = (stop(), stop(), stop());
        let mut map = TabStopMap::default();
        map.insert(&before);
        // The order is `before`, `after`, then the group: a group sorts
        // after the stops of its own tab index.
        map.begin_group(0, true);
        map.insert(&container);
        // Each stop in a plain group of its own, as a button has.
        for handle in [&first, &middle, &last] {
            map.begin_group(0, false);
            map.insert(handle);
            map.end_group();
        }
        map.end_group();
        map.insert(&after);

        let next = TabStopMap::next;
        let prev = TabStopMap::prev;
        assert_eq!(step(&mut map, next, &container), Some(first.id));
        assert_eq!(step(&mut map, next, &first), Some(middle.id));
        assert_eq!(step(&mut map, next, &middle), Some(last.id));
        assert_eq!(step(&mut map, next, &last), Some(first.id));
        assert_eq!(step(&mut map, prev, &first), Some(last.id));
        assert_eq!(step(&mut map, prev, &container), Some(last.id));

        // Outside the group, the steps walk the whole order.
        assert_eq!(step(&mut map, next, &before), Some(after.id));
        assert_eq!(step(&mut map, next, &after), Some(first.id));
        assert_eq!(step(&mut map, prev, &before), Some(last.id));
        assert_eq!(map.next(None).map(|handle| handle.id), Some(before.id));
    }

    #[test]
    fn test_nested_wrapping_groups() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let stop = || FocusHandle::new(&focus_map).tab_stop(true);
        let (outer_first, outer_last, inner_first, inner_last) = (stop(), stop(), stop(), stop());
        let mut map = TabStopMap::default();
        // The order is the outer stops, then the inner group.
        map.begin_group(0, true);
        map.insert(&outer_first);
        map.begin_group(0, true);
        map.insert(&inner_first);
        map.insert(&inner_last);
        map.end_group();
        map.insert(&outer_last);
        map.end_group();

        let next = TabStopMap::next;
        let prev = TabStopMap::prev;
        // The outer group reaches the stops of the inner one.
        assert_eq!(step(&mut map, next, &outer_first), Some(outer_last.id));
        assert_eq!(step(&mut map, next, &outer_last), Some(inner_first.id));
        assert_eq!(step(&mut map, prev, &outer_first), Some(inner_last.id));
        // The inner group keeps its own steps.
        assert_eq!(step(&mut map, next, &inner_last), Some(inner_first.id));
        assert_eq!(step(&mut map, prev, &inner_first), Some(inner_last.id));
    }

    #[test]
    fn test_wrapping_group_with_no_stop_moves_nothing() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let outside = FocusHandle::new(&focus_map).tab_stop(true);
        let container = FocusHandle::new(&focus_map);
        let mut map = TabStopMap::default();
        map.insert(&outside);
        map.begin_group(0, true);
        map.insert(&container);
        map.end_group();

        assert_eq!(step(&mut map, TabStopMap::next, &container), None);
        assert_eq!(step(&mut map, TabStopMap::prev, &container), None);
    }

    #[test]
    fn test_wrapping_group_survives_replay() {
        let focus_map = Arc::new(RwLock::new(FocusMap::default()));
        let stop = || FocusHandle::new(&focus_map).tab_stop(true);
        let (outside, first, last) = (stop(), stop(), stop());
        let mut map = TabStopMap::default();
        map.insert(&outside);
        map.begin_group(0, true);
        map.insert(&first);
        map.insert(&last);
        map.end_group();

        let mut replayed = TabStopMap::default();
        replayed.replay(&map.insertion_history);
        assert_eq!(step(&mut replayed, TabStopMap::next, &last), Some(first.id));
    }
}
