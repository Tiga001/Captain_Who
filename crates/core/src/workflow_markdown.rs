//! Organization templates are ordinary Markdown. Names and department paths provide the
//! human-readable structure; importing creates new identities and a fresh canvas layout.
use crate::workflow::{
    AgentConfig, Definition, Department, ManagementRole, Node, NodeConfig, Viewport,
    WorkflowPermissionMode,
};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

mod layout;
mod text;

pub const MAX_MARKDOWN_BYTES: usize = 4 * 1024 * 1024;
pub const INVALID_FORMAT: &str = "organization_template_invalid_format";
pub const TOO_LARGE: &str = "organization_template_too_large";

const ORGANIZATION: &str = "# 组织：";
const DESCRIPTION: &str = "## 简介";
const BACKGROUND: &str = "## 公共背景";
const DIRECT: &str = "## 直属成员";
const DEPARTMENT: &str = "## 部门：";
const MEMBER: &str = "### 成员：";
const RANK: &str = "- 职级：";
const ROLE: &str = "- 管理身份：";
const RECEIVES: &str = "#### 接收内容";
const TASK: &str = "#### 职责";
const DELIVERS: &str = "#### 交付要求";

/// Parse a complete readable document before returning a new template. Local identities,
/// credentials, model bindings, permissions and canvas geometry never come from the file.
pub fn import(markdown: &str) -> Result<Definition, String> {
    if markdown.len() > MAX_MARKDOWN_BYTES {
        return Err(TOO_LARGE.into());
    }
    let mut cursor = text::Cursor::new(markdown.strip_prefix('\u{feff}').unwrap_or(markdown));
    let name = cursor
        .next()
        .ok_or(INVALID_FORMAT)?
        .strip_prefix(ORGANIZATION)
        .ok_or(INVALID_FORMAT)?;
    let name = text::decode_name(name);
    cursor.expect(DESCRIPTION)?;
    let description = cursor.body();
    cursor.expect(BACKGROUND)?;
    let background = cursor.body();
    let mut definition = Definition {
        schema_version: 1,
        id: fresh_id(),
        name,
        description,
        background,
        nodes: vec![],
        departments: vec![],
        viewport: Viewport {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        },
    };
    let mut paths = HashMap::<Vec<String>, String>::new();
    let mut current_department = None;
    let mut group_seen = false;
    let mut direct_seen = false;
    while let Some(line) = cursor.next() {
        if line == DIRECT {
            if direct_seen || !definition.departments.is_empty() || !definition.nodes.is_empty() {
                return Err(INVALID_FORMAT.into());
            }
            direct_seen = true;
            group_seen = true;
            current_department = None;
        } else if let Some(path) = line.strip_prefix(DEPARTMENT) {
            let path: Vec<_> = path.split(" / ").map(text::decode_name).collect();
            if path
                .iter()
                .any(|name| name.trim().is_empty() || name.contains('/'))
                || paths.contains_key(&path)
            {
                return Err(INVALID_FORMAT.into());
            }
            let parent_id = if path.len() > 1 {
                Some(
                    paths
                        .get(&path[..path.len() - 1])
                        .ok_or(INVALID_FORMAT)?
                        .clone(),
                )
            } else {
                None
            };
            let id = fresh_id();
            definition.departments.push(Department {
                id: id.clone(),
                name: path.last().unwrap().clone(),
                parent_id,
                x: 0.0,
                y: 0.0,
                width: 80.0,
                height: 64.0,
            });
            if definition.departments.len() > 64 {
                return Err(INVALID_FORMAT.into());
            }
            paths.insert(path, id.clone());
            current_department = Some(id);
            group_seen = true;
        } else if let Some(name) = line.strip_prefix(MEMBER) {
            if !group_seen {
                return Err(INVALID_FORMAT.into());
            }
            let name = text::decode_name(name);
            let rank: u8 = cursor
                .next()
                .ok_or(INVALID_FORMAT)?
                .strip_prefix(RANK)
                .ok_or(INVALID_FORMAT)?
                .parse()
                .map_err(|_| INVALID_FORMAT)?;
            let role = cursor
                .next()
                .ok_or(INVALID_FORMAT)?
                .strip_prefix(ROLE)
                .ok_or(INVALID_FORMAT)?;
            let management_role = match role {
                "普通成员" => ManagementRole::Member,
                "组织管理员" => ManagementRole::OrganizationAdmin,
                "部门管理员" => ManagementRole::DepartmentAdmin,
                _ => return Err(INVALID_FORMAT.into()),
            };
            cursor.expect(RECEIVES)?;
            let receives = cursor.body();
            cursor.expect(TASK)?;
            let task = cursor.body();
            cursor.expect(DELIVERS)?;
            let delivers = cursor.body();
            definition.nodes.push(Node {
                id: fresh_id(),
                name,
                x: 0.0,
                y: 0.0,
                rank,
                management_role,
                department_id: current_department.clone(),
                config: NodeConfig::Agent(AgentConfig {
                    permission_mode: WorkflowPermissionMode::Default,
                    model_config_id: None,
                    receives,
                    task,
                    delivers,
                }),
            });
            if definition.nodes.len() > 128 {
                return Err(INVALID_FORMAT.into());
            }
        } else {
            return Err(INVALID_FORMAT.into());
        }
    }
    layout::apply(&mut definition);
    validate(&definition)?;
    Ok(definition)
}

fn fresh_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn validate(definition: &Definition) -> Result<(), String> {
    definition
        .validate(&HashSet::new())
        .map_err(|_| INVALID_FORMAT)?;
    definition
        .validate_unique_member_names()
        .map_err(|_| INVALID_FORMAT)?;
    definition
        .validate_department_names()
        .map_err(|_| INVALID_FORMAT)?;
    // A department's public path must be unambiguous without private identifiers.
    if definition
        .departments
        .iter()
        .any(|department| department.name.trim().is_empty())
    {
        return Err(INVALID_FORMAT.into());
    }
    Ok(())
}

/// Public information comes first, followed by direct members and a depth-first department
/// tree. Members are sorted by rank descending, administrators first on ties, then original
/// order. Identity, document metadata and local execution settings are never serialized.
pub fn export(definition: &Definition) -> Result<String, String> {
    validate(definition)?;
    let mut markdown = format!("{ORGANIZATION}{}\n\n", text::encode_name(&definition.name));
    text::write_body(&mut markdown, DESCRIPTION, &definition.description);
    text::write_body(&mut markdown, BACKGROUND, &definition.background);
    if definition
        .nodes
        .iter()
        .any(|node| node.department_id.is_none())
    {
        writeln!(markdown, "{DIRECT}\n").unwrap();
        export_members(&mut markdown, definition, None);
    }
    export_departments(&mut markdown, definition, None, &[]);
    if markdown.len() > MAX_MARKDOWN_BYTES {
        return Err(TOO_LARGE.into());
    }
    Ok(markdown)
}

fn export_departments(
    markdown: &mut String,
    definition: &Definition,
    parent_id: Option<&str>,
    parent_path: &[&str],
) {
    for department in definition
        .departments
        .iter()
        .filter(|department| department.parent_id.as_deref() == parent_id)
    {
        let mut path = parent_path.to_vec();
        path.push(&department.name);
        writeln!(
            markdown,
            "{DEPARTMENT}{}\n",
            path.iter()
                .map(|name| text::encode_name(name))
                .collect::<Vec<_>>()
                .join(" / ")
        )
        .unwrap();
        export_members(markdown, definition, Some(&department.id));
        export_departments(markdown, definition, Some(&department.id), &path);
    }
}

fn export_members(markdown: &mut String, definition: &Definition, department_id: Option<&str>) {
    let mut members: Vec<_> = definition
        .nodes
        .iter()
        .filter(|node| node.department_id.as_deref() == department_id)
        .collect();
    members.sort_by_key(|node| {
        (
            std::cmp::Reverse(node.rank),
            node.management_role == ManagementRole::Member,
        )
    });
    for node in members {
        let role = match node.management_role {
            ManagementRole::Member => "普通成员",
            ManagementRole::OrganizationAdmin => "组织管理员",
            ManagementRole::DepartmentAdmin => "部门管理员",
        };
        writeln!(
            markdown,
            "{MEMBER}{}\n\n{RANK}{}\n{ROLE}{role}\n",
            text::encode_name(&node.name),
            node.rank
        )
        .unwrap();
        let NodeConfig::Agent(agent) = &node.config;
        text::write_body(markdown, RECEIVES, &agent.receives);
        text::write_body(markdown, TASK, &agent.task);
        text::write_body(markdown, DELIVERS, &agent.delivers);
    }
}

pub fn suggested_file_name(name: &str) -> String {
    let mut bytes = 0;
    let safe: String = name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                ch
            }
        })
        .take(80)
        .take_while(|ch| {
            bytes += ch.len_utf8();
            bytes <= 200
        })
        .collect();
    let safe = safe.trim_matches([' ', '.']);
    let upper = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            });
    if reserved {
        format!("organization-{safe}.md")
    } else {
        format!(
            "{}.md",
            if safe.is_empty() {
                "organization-template"
            } else {
                safe
            }
        )
    }
}

#[cfg(test)]
mod tests;
