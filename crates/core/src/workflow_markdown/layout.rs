//! Reconstruct display-only geometry from the document's department tree and member order.
//! Dimensions follow the existing organization editor's automatic layout.
use crate::workflow::Definition;

const NODE_WIDTH: f64 = 212.0;
const NODE_HEIGHT: f64 = 52.0;
const PADDING: f64 = 24.0;
const HEADER: f64 = 40.0;

enum Target {
    Member(usize),
    Department(usize),
}

struct Block {
    target: Target,
    width: f64,
    height: f64,
    children: Vec<Positioned>,
}

struct Positioned {
    block: Block,
    x: f64,
    y: f64,
}

struct Packed {
    children: Vec<Positioned>,
    width: f64,
    height: f64,
}

fn pack(blocks: Vec<Block>, framed: bool) -> Packed {
    let left = if framed { PADDING } else { 0.0 };
    let top = if framed { HEADER } else { 0.0 };
    let gap = if blocks
        .iter()
        .any(|block| matches!(block.target, Target::Department(_)))
    {
        40.0
    } else {
        24.0
    };
    let mut best = (f64::INFINITY, 1, 0.0, 0.0);
    for columns in 1..=blocks.len().max(1) {
        let mut width: f64 = if framed {
            NODE_WIDTH + PADDING * 2.0
        } else {
            0.0
        };
        let mut y = top;
        for row in blocks.chunks(columns) {
            let row_width = row.iter().map(|block| block.width).sum::<f64>()
                + gap * (row.len() - 1) as f64
                + left * 2.0;
            width = width.max(row_width);
            y += row.iter().map(|block| block.height).fold(0.0, f64::max) + gap;
        }
        let height = if blocks.is_empty() {
            top + left
        } else {
            y - gap + left
        }
        .max(if framed {
            NODE_HEIGHT + HEADER + PADDING
        } else {
            0.0
        });
        let score = (width / 2.0).max(height) + (width * height).sqrt() * 0.1;
        if score < best.0 {
            best = (score, columns, width, height);
        }
    }
    let mut children = Vec::with_capacity(blocks.len());
    let mut x = left;
    let mut y = top;
    let mut row_height: f64 = 0.0;
    for (index, block) in blocks.into_iter().enumerate() {
        if index > 0 && index % best.1 == 0 {
            x = left;
            y += row_height + gap;
            row_height = 0.0;
        }
        row_height = row_height.max(block.height);
        let next_x = x + block.width + gap;
        children.push(Positioned { block, x, y });
        x = next_x;
    }
    Packed {
        children,
        width: best.2,
        height: best.3,
    }
}

fn children_of(definition: &Definition, parent_id: Option<&str>) -> Vec<Block> {
    let members = definition
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.department_id.as_deref() == parent_id)
        .map(|(index, _)| Block {
            target: Target::Member(index),
            width: NODE_WIDTH,
            height: NODE_HEIGHT,
            children: vec![],
        });
    let departments = definition
        .departments
        .iter()
        .enumerate()
        .filter(|(_, department)| department.parent_id.as_deref() == parent_id)
        .map(|(index, department)| {
            let packed = pack(children_of(definition, Some(&department.id)), true);
            Block {
                target: Target::Department(index),
                width: packed.width,
                height: packed.height,
                children: packed.children,
            }
        });
    members.chain(departments).collect()
}

fn place(definition: &mut Definition, children: Vec<Positioned>, origin_x: f64, origin_y: f64) {
    for Positioned { block, x, y } in children {
        let x = origin_x + x;
        let y = origin_y + y;
        match block.target {
            Target::Member(index) => {
                definition.nodes[index].x = x;
                definition.nodes[index].y = y;
            }
            Target::Department(index) => {
                let department = &mut definition.departments[index];
                department.x = x;
                department.y = y;
                department.width = block.width;
                department.height = block.height;
                place(definition, block.children, x, y);
            }
        }
    }
}

pub(super) fn apply(definition: &mut Definition) {
    let packed = pack(children_of(definition, None), false);
    place(definition, packed.children, 0.0, 0.0);
    definition.viewport.x = 0.0;
    definition.viewport.y = 0.0;
    definition.viewport.zoom = 1.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_disjoint(left: (f64, f64, f64, f64), right: (f64, f64, f64, f64)) {
        assert!(
            left.0 + left.2 <= right.0
                || right.0 + right.2 <= left.0
                || left.1 + left.3 <= right.1
                || right.1 + right.3 <= left.1,
            "overlapping layout rectangles: {left:?} and {right:?}"
        );
    }

    #[test]
    fn maximum_size_deep_and_wide_organizations_fit_without_sibling_overlap() {
        let source: Definition = serde_json::from_str(include_str!(
            "../../../../packages/protocol/fixtures/organization-hierarchy-v1.json"
        ))
        .unwrap();
        for (nested, distribute_members) in
            [(true, false), (true, true), (false, false), (false, true)]
        {
            let mut definition = source.clone();
            definition.departments = (0..64)
                .map(|index| {
                    let mut department = source.departments[0].clone();
                    department.id = format!("department-{index}");
                    department.name = format!("Department {index}");
                    department.parent_id =
                        (nested && index > 0).then(|| format!("department-{}", index - 1));
                    department
                })
                .collect();
            definition.nodes = (0..128)
                .map(|index| {
                    let mut node = source.nodes[0].clone();
                    node.id = format!("member-{index}");
                    node.name = format!("Member {index}");
                    node.department_id = if distribute_members {
                        Some(format!("department-{}", index / 2))
                    } else if nested {
                        Some("department-63".into())
                    } else {
                        None
                    };
                    node
                })
                .collect();
            apply(&mut definition);
            definition.validate(&Default::default()).unwrap();
            for parent_id in std::iter::once(None).chain(
                definition
                    .departments
                    .iter()
                    .map(|department| Some(department.id.as_str())),
            ) {
                let mut rectangles = Vec::new();
                for node in definition
                    .nodes
                    .iter()
                    .filter(|node| node.department_id.as_deref() == parent_id)
                {
                    rectangles.push((node.x, node.y, NODE_WIDTH, NODE_HEIGHT));
                }
                for department in definition
                    .departments
                    .iter()
                    .filter(|department| department.parent_id.as_deref() == parent_id)
                {
                    rectangles.push((
                        department.x,
                        department.y,
                        department.width,
                        department.height,
                    ));
                }
                if let Some(id) = parent_id {
                    let parent = definition
                        .departments
                        .iter()
                        .find(|department| department.id == id)
                        .unwrap();
                    for &(x, y, width, height) in &rectangles {
                        assert!(x >= parent.x + PADDING);
                        assert!(y >= parent.y + HEADER);
                        assert!(x + width <= parent.x + parent.width - PADDING);
                        assert!(y + height <= parent.y + parent.height - PADDING);
                    }
                }
                for (index, &rectangle) in rectangles.iter().enumerate() {
                    for &other in &rectangles[index + 1..] {
                        assert_disjoint(rectangle, other);
                    }
                }
            }
            for (index, node) in definition.nodes.iter().enumerate() {
                for other in &definition.nodes[index + 1..] {
                    assert_disjoint(
                        (node.x, node.y, NODE_WIDTH, NODE_HEIGHT),
                        (other.x, other.y, NODE_WIDTH, NODE_HEIGHT),
                    );
                }
            }
            let first = serde_json::to_value(&definition).unwrap();
            apply(&mut definition);
            assert_eq!(serde_json::to_value(&definition).unwrap(), first);
        }
    }

    #[test]
    fn nested_members_and_empty_departments_are_contained_and_do_not_overlap() {
        let mut definition: Definition = serde_json::from_str(include_str!(
            "../../../../packages/protocol/fixtures/organization-hierarchy-v1.json"
        ))
        .unwrap();
        let mut empty = definition.departments[1].clone();
        empty.id = "empty".into();
        empty.name = "Empty department".into();
        definition.departments.push(empty);
        let member = definition.nodes[1].clone();
        for index in 0..7 {
            let mut node = member.clone();
            node.id = format!("additional-{index}");
            node.name = format!("Member {index}");
            definition.nodes.push(node);
        }
        apply(&mut definition);
        definition.validate(&Default::default()).unwrap();
        for node in &definition.nodes {
            if let Some(id) = &node.department_id {
                let department = definition.departments.iter().find(|d| &d.id == id).unwrap();
                assert!(node.x >= department.x + PADDING);
                assert!(node.y >= department.y + HEADER);
                assert!(node.x + NODE_WIDTH <= department.x + department.width - PADDING);
                assert!(node.y + NODE_HEIGHT <= department.y + department.height - PADDING);
            }
        }
        for department in &definition.departments {
            if let Some(id) = &department.parent_id {
                let parent = definition.departments.iter().find(|d| &d.id == id).unwrap();
                assert!(department.x >= parent.x + PADDING);
                assert!(department.y >= parent.y + HEADER);
                assert!(department.x + department.width <= parent.x + parent.width - PADDING);
                assert!(department.y + department.height <= parent.y + parent.height - PADDING);
            }
        }
        for (index, node) in definition.nodes.iter().enumerate() {
            for other in &definition.nodes[index + 1..] {
                assert!(
                    node.x + NODE_WIDTH <= other.x
                        || other.x + NODE_WIDTH <= node.x
                        || node.y + NODE_HEIGHT <= other.y
                        || other.y + NODE_HEIGHT <= node.y
                );
            }
        }
        let first = serde_json::to_value(&definition).unwrap();
        apply(&mut definition);
        assert_eq!(serde_json::to_value(&definition).unwrap(), first);
    }
}
