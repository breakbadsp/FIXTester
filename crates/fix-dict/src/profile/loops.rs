//! The loop search of profile groups. A loop is a strongly connected
//! component with more than one group, or a group that nests itself.

use super::groups::Planned;
use std::collections::VecDeque;

struct Tarjan<'p> {
    planned: &'p Planned,
    order: Vec<Option<usize>>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next: usize,
    /// The component of each group.
    component: Vec<usize>,
    components: usize,
}

impl Tarjan<'_> {
    fn visit(&mut self, node: usize) {
        self.order[node] = Some(self.next);
        self.low[node] = self.next;
        self.next += 1;
        self.stack.push(node);
        self.on_stack[node] = true;
        for (nested, _) in self.planned.nested(node) {
            match self.order[nested] {
                None => {
                    self.visit(nested);
                    self.low[node] = self.low[node].min(self.low[nested]);
                }
                Some(order) if self.on_stack[nested] => {
                    self.low[node] = self.low[node].min(order);
                }
                Some(_) => {}
            }
        }
        if Some(self.low[node]) == self.order[node] {
            while let Some(member) = self.stack.pop() {
                self.on_stack[member] = false;
                self.component[member] = self.components;
                if member == node {
                    break;
                }
            }
            self.components += 1;
        }
    }
}

/// The shortest cycle through `start` inside its component. Returns the
/// place of its first member and the group names, first and last the same.
fn cycle_through(
    planned: &Planned,
    component: &[usize],
    start: usize,
) -> Option<(usize, Vec<String>)> {
    let mut parent: Vec<Option<usize>> = vec![None; planned.len()];
    let mut place: Vec<usize> = vec![0; planned.len()];
    let mut seen = vec![false; planned.len()];
    let mut queue = VecDeque::new();
    let in_component = |node: usize| component[node] == component[start];
    for (nested, at) in planned.nested(start) {
        if nested == start {
            let name = planned.name(start).to_owned();
            return Some((at, vec![name.clone(), name]));
        }
        if in_component(nested) && !seen[nested] {
            seen[nested] = true;
            place[nested] = at;
            queue.push_back(nested);
        }
    }
    while let Some(node) = queue.pop_front() {
        for (nested, _) in planned.nested(node) {
            if nested == start {
                let mut path = vec![node];
                while let Some(before) = parent[*path.last()?] {
                    path.push(before);
                }
                path.push(start);
                path.reverse();
                path.push(start);
                let names = path.iter().map(|i| planned.name(*i).to_owned()).collect();
                return Some((place[node], names));
            }
            if in_component(nested) && !seen[nested] {
                seen[nested] = true;
                parent[nested] = Some(node);
                place[nested] = place[node];
                queue.push_back(nested);
            }
        }
    }
    None
}

/// Finds every group that is in a cycle. Each gives one entry: the place of
/// the member that starts its shortest cycle, and the cycle.
pub(super) fn find(planned: &Planned) -> Vec<(usize, Vec<String>)> {
    let count = planned.len();
    let mut tarjan = Tarjan {
        planned,
        order: vec![None; count],
        low: vec![0; count],
        on_stack: vec![false; count],
        stack: Vec::new(),
        next: 0,
        component: vec![0; count],
        components: 0,
    };
    for node in 0..count {
        if tarjan.order[node].is_none() {
            tarjan.visit(node);
        }
    }
    let component = tarjan.component;
    let mut found = Vec::new();
    for node in 0..count {
        let alone = component.iter().filter(|c| **c == component[node]).count() == 1;
        let own_edge = planned.nested(node).any(|(nested, _)| nested == node);
        if (!alone || own_edge)
            && let Some(cycle) = cycle_through(planned, &component, node)
        {
            found.push(cycle);
        }
    }
    found
}
