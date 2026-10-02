//! Incremental organization placement. Existing geometry is user-owned; structural edits place
//! only new/moved blocks and enlarge their ancestor frames, moving siblings only on collision.
use crate::workflow::Definition;
use std::collections::HashSet;
const PAD: f64 = 24.;
const HEADER: f64 = 44.;
const CARD_WIDTH: f64 = 212.;
const CARD_HEIGHT: f64 = 52.;

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl Rect {
    fn right(self) -> f64 {
        self.x + self.width
    }
    fn bottom(self) -> f64 {
        self.y + self.height
    }
    fn overlaps(self, other: Self) -> bool {
        self.x < other.right() + PAD
            && self.right() + PAD > other.x
            && self.y < other.bottom() + PAD
            && self.bottom() + PAD > other.y
    }
}
fn block(graph: &Definition, id: &str) -> (Option<String>, Rect) {
    if let Some(d) = graph.departments.iter().find(|d| d.id == id) {
        (
            d.parent_id.clone(),
            Rect {
                x: d.x,
                y: d.y,
                width: d.width,
                height: d.height,
            },
        )
    } else {
        let n = graph.nodes.iter().find(|n| n.id == id).unwrap();
        (
            n.department_id.clone(),
            Rect {
                x: n.x,
                y: n.y,
                width: CARD_WIDTH,
                height: CARD_HEIGHT,
            },
        )
    }
}
fn siblings(graph: &Definition, parent: Option<&str>, excluded: &HashSet<String>) -> Vec<String> {
    graph
        .nodes
        .iter()
        .filter(|n| n.department_id.as_deref() == parent && !excluded.contains(&n.id))
        .map(|n| n.id.clone())
        .chain(
            graph
                .departments
                .iter()
                .filter(|d| d.parent_id.as_deref() == parent && !excluded.contains(&d.id))
                .map(|d| d.id.clone()),
        )
        .collect()
}
fn descendant<'a>(graph: &'a Definition, ancestor: &str, mut parent: Option<&'a str>) -> bool {
    let mut visited = HashSet::new();
    while let Some(id) = parent {
        if id == ancestor {
            return true;
        }
        if !visited.insert(id) {
            return false;
        }
        parent = graph
            .departments
            .iter()
            .find(|d| d.id == id)
            .and_then(|d| d.parent_id.as_deref());
    }
    false
}
fn translate(graph: &mut Definition, id: &str, x: f64, y: f64) {
    let (_, old) = block(graph, id);
    let (dx, dy) = (x - old.x, y - old.y);
    let descendants: HashSet<_> = graph
        .departments
        .iter()
        .filter(|d| d.id == id || descendant(graph, id, d.parent_id.as_deref()))
        .map(|d| d.id.clone())
        .collect();
    for node in &mut graph.nodes {
        if node.id == id
            || node
                .department_id
                .as_ref()
                .is_some_and(|d| descendants.contains(d))
        {
            node.x += dx;
            node.y += dy;
        }
    }
    for department in &mut graph.departments {
        if descendants.contains(&department.id) {
            department.x += dx;
            department.y += dy;
        }
    }
}
fn position(graph: &Definition, id: &str, pending: &HashSet<String>) -> (f64, f64) {
    let (parent, rect) = block(graph, id);
    let frame = parent.as_deref().map(|id| block(graph, id).1);
    let mut excluded = pending.clone();
    excluded.insert(id.into());
    let occupied: Vec<_> = siblings(graph, parent.as_deref(), &excluded)
        .into_iter()
        .map(|id| block(graph, &id).1)
        .collect();
    let x = frame
        .map(|r| r.x + PAD)
        .unwrap_or_else(|| occupied.iter().map(|r| r.x).reduce(f64::min).unwrap_or(PAD));
    let y = frame
        .map(|r| r.y + HEADER)
        .unwrap_or_else(|| occupied.iter().map(|r| r.y).reduce(f64::min).unwrap_or(PAD));
    let mut xs = vec![x];
    let mut ys = vec![y];
    for r in &occupied {
        xs.push(r.right() + PAD);
        ys.push(r.bottom() + PAD);
    }
    let mut best = None;
    for x in xs {
        for &y in &ys {
            let candidate = Rect { x, y, ..rect };
            if occupied.iter().any(|r| candidate.overlaps(*r)) {
                continue;
            }
            let right = occupied
                .iter()
                .map(|r| r.right())
                .fold(candidate.right(), f64::max);
            let bottom = occupied
                .iter()
                .map(|r| r.bottom())
                .fold(candidate.bottom(), f64::max);
            let score = if let Some(frame) = frame {
                // Prefer an existing gap in the frame before expanding it. Among placements that
                // grow the frame, minimize added area, then keep the block close to the top left.
                let area = (right + PAD - frame.x).max(frame.width)
                    * (bottom + PAD - frame.y).max(frame.height);
                (area - frame.width * frame.height, y - frame.y, x - frame.x)
            } else {
                // Only the newly inserted root block is placed; existing optimized rows stay put.
                ((right / 2.).max(bottom), y, x)
            };
            if best.as_ref().is_none_or(|(old, _)| score < *old) {
                best = Some((score, (x, y)));
            }
        }
    }
    best.expect("placing below all occupied blocks always succeeds")
        .1
}
fn fit_ancestors(
    graph: &mut Definition,
    id: &str,
    pending: &HashSet<String>,
) -> Result<(), String> {
    let (mut parent, mut child) = block(graph, id);
    let mut visited = HashSet::new();
    while let Some(parent_id) = parent {
        if !visited.insert(parent_id.clone()) {
            return Err("Invalid organization department hierarchy".into());
        }
        let frame = graph
            .departments
            .iter_mut()
            .find(|d| d.id == parent_id)
            .ok_or("Unknown organization department")?;
        let old = (frame.width, frame.height);
        frame.width = frame.width.max(child.right() + PAD - frame.x);
        frame.height = frame.height.max(child.bottom() + PAD - frame.y);
        let grew = old != (frame.width, frame.height);
        parent = frame.parent_id.clone();
        child = Rect {
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
        };
        if grew {
            let mut excluded = pending.clone();
            excluded.insert(parent_id.clone());
            let collisions: Vec<_> = siblings(graph, parent.as_deref(), &excluded)
                .into_iter()
                .filter(|id| child.overlaps(block(graph, id).1))
                .collect();
            for collision in collisions {
                let (x, y) = position(graph, &collision, pending);
                translate(graph, &collision, x, y);
            }
            // Shifted siblings can enlarge the common parent too, without moving its contents.
            if let Some(ancestor) = parent.as_deref() {
                let others = siblings(graph, Some(ancestor), pending);
                for other in others {
                    let r = block(graph, &other).1;
                    child.width = child.width.max(r.right() - child.x);
                    child.height = child.height.max(r.bottom() - child.y);
                }
            }
        }
    }
    Ok(())
}

pub(super) fn reconcile(before: &Definition, graph: &mut Definition) -> Result<(), String> {
    let mut moved: Vec<String> = graph
        .departments
        .iter()
        .filter(|d| {
            before
                .departments
                .iter()
                .find(|old| old.id == d.id)
                .is_none_or(|old| old.parent_id != d.parent_id)
        })
        .map(|d| d.id.clone())
        .collect();
    // Ancestors must be placed before descendants; moving a frame translates its entire subtree.
    moved.sort_by_key(|id| {
        let mut depth = 0;
        let mut parent = block(graph, id).0;
        let mut visited = HashSet::new();
        while let Some(id) = parent {
            if !visited.insert(id.clone()) {
                break;
            }
            depth += 1;
            parent = graph
                .departments
                .iter()
                .find(|d| d.id == id)
                .and_then(|d| d.parent_id.clone());
        }
        depth
    });
    moved.extend(
        graph
            .nodes
            .iter()
            .filter(|n| {
                before
                    .nodes
                    .iter()
                    .find(|old| old.id == n.id)
                    .is_none_or(|old| old.department_id != n.department_id)
            })
            .map(|n| n.id.clone()),
    );
    let mut pending: HashSet<_> = moved.iter().cloned().collect();
    for id in moved {
        let (x, y) = position(graph, &id, &pending);
        translate(graph, &id, x, y);
        pending.remove(&id);
        fit_ancestors(graph, &id, &pending)?;
    }
    if graph
        .nodes
        .iter()
        .any(|n| n.x.abs() > 100_000. || n.y.abs() > 100_000.)
        || graph.departments.iter().any(|d| {
            d.x.abs() > 100_000.
                || d.y.abs() > 100_000.
                || d.width > 100_000.
                || d.height > 100_000.
        })
    {
        return Err("Organization layout exceeds canvas bounds".into());
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn layout(graph: &mut Definition) -> Result<(), String> {
    let mut empty = graph.clone();
    empty.nodes.clear();
    empty.departments.clear();
    reconcile(&empty, graph)
}
