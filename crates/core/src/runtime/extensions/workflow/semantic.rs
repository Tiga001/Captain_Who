//! Model names resolve against the immutable directory used for this sampling request.
//! Durable effects still use stable Host identities and recheck live authority.
use crate::organization_personnel::{Action as EditAction, Input as EditInput, OptionalId};
use crate::workflow::{member_name_key, Department};
use crate::workflow_awareness::{MailboxDirection, MailboxQuery, StateQuery, StateView};
use crate::workflow_execution::{ConversationSnapshot, SendOutput};
use crate::{AgentError, AgentResult, WorkflowRuntimeHost};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SendInput {
    pub messages: Vec<SendMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SendMessage {
    #[serde(default, deserialize_with = "present_string")]
    pub to: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    pub reply_to: Option<String>,
    pub message: String,
}

fn present_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn reference(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

impl SendInput {
    pub fn validate(&self) -> AgentResult<()> {
        if self.messages.is_empty() || self.messages.len() > 128 {
            return Err(AgentError::new("organization_send 需要 1 到 128 封邮件。"));
        }
        for message in &self.messages {
            if message.to.is_some() == message.reply_to.is_some()
                || !message
                    .to
                    .as_deref()
                    .or(message.reply_to.as_deref())
                    .is_some_and(reference)
                || message.message.trim().is_empty()
            {
                return Err(AgentError::new("每封邮件需要正文，并且只填写 to（成员姓名）或 replyTo（来信消息 ID）中的一项。"));
            }
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        snapshot: &ConversationSnapshot,
        host: &dyn WorkflowRuntimeHost,
    ) -> AgentResult<Vec<SendOutput>> {
        self.messages
            .iter()
            .map(|message| {
                let (target_node_id, reply_to_message_id) = if let Some(name) = &message.to {
                    (member_id(snapshot, name)?, None)
                } else {
                    let id = message
                        .reply_to
                        .as_deref()
                        .expect("validated send destination");
                    let mailbox = host.mailbox(MailboxQuery {
                        direction: MailboxDirection::Inbox,
                        message_id: Some(id.to_string()),
                        ..Default::default()
                    })?;
                    let letter = mailbox["messages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|letter| letter["messageId"].as_str() == Some(id))
                        .ok_or_else(|| {
                            AgentError::new("找不到这封来信。请使用自己收件箱中的 messageId 回复。")
                        })?;
                    let source = letter["sourceNodeId"]
                        .as_str()
                        .filter(|id| {
                            snapshot.members.iter().any(|member| {
                                member.node_id == *id
                                    && member.conversation_id.as_deref().is_some()
                                    && member.conversation_id.as_deref()
                                        == letter["sourceConversationId"].as_str()
                            })
                        })
                        .ok_or_else(|| {
                            AgentError::new(
                                "这封来信的发送者已不在当前成员目录中。请查看组织后再决定收件人。",
                            )
                        })?;
                    (source.to_owned(), Some(id.to_string()))
                };
                if target_node_id == snapshot.node_id {
                    return Err(AgentError::new("不能给自己发送组织邮件，请选择其他成员。"));
                }
                Ok(SendOutput {
                    target_node_id,
                    message: message.message.clone(),
                    reply_to_message_id,
                })
            })
            .collect()
    }
}

pub(super) fn member_id(snapshot: &ConversationSnapshot, name: &str) -> AgentResult<String> {
    let key = member_name_key(name);
    let mut matches = snapshot
        .members
        .iter()
        .map(|member| (member.node_id.as_str(), member.node_name.as_str()))
        .chain(std::iter::once((
            snapshot.node_id.as_str(),
            snapshot.node_name.as_str(),
        )))
        .filter(|(_, candidate)| member_name_key(candidate) == key)
        .map(|(id, _)| id)
        .collect::<BTreeSet<_>>();
    if matches.len() == 1 {
        return Ok(matches.pop_first().unwrap().to_owned());
    }
    Err(AgentError::new(if matches.is_empty() {
        format!("当前组织中没有名为‘{name}’的成员。请使用成员目录中的完整姓名。")
    } else {
        format!("成员姓名‘{name}’不唯一，暂时无法确定对象。请先为成员设置不同姓名。")
    }))
}

fn department_path(departments: &[Department], id: &str) -> Option<String> {
    let mut parts = Vec::new();
    let mut current = Some(id);
    let mut visited = BTreeSet::new();
    while let Some(id) = current {
        if !visited.insert(id) {
            return None;
        }
        let department = departments.iter().find(|department| department.id == id)?;
        parts.push(department.name.trim());
        current = department.parent_id.as_deref();
    }
    parts.reverse();
    Some(parts.join("/"))
}

fn department_key(path: &str) -> String {
    path.split('/')
        .map(member_name_key)
        .collect::<Vec<_>>()
        .join("/")
}

fn department_id(snapshot: &ConversationSnapshot, path: &str) -> AgentResult<String> {
    let key = department_key(path);
    let matches: Vec<_> = snapshot
        .departments
        .iter()
        .filter(|department| {
            department_path(&snapshot.departments, &department.id)
                .is_some_and(|candidate| department_key(&candidate) == key)
        })
        .collect();
    if matches.len() == 1 {
        return Ok(matches[0].id.clone());
    }
    Err(AgentError::new(format!("无法唯一确定部门‘{path}’。请使用当前部门目录中的完整路径，例如‘人事部/薪酬组’；新部门创建后需先获取更新后的目录。")))
}

/// Use the internal closed parser for the shared edit fields, but admit only semantic references.
/// This syntax check does not consult live state, so committed calls remain recoverable after a
/// member is renamed or removed. The original input is separately retained for receipt identity.
pub(super) fn parse_edit_input(mut args: Value) -> AgentResult<EditInput> {
    let changes = args
        .get_mut("changes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| AgentError::new("organization_edit 需要 changes 数组。"))?;
    for change in changes {
        let object = change
            .as_object_mut()
            .ok_or_else(|| AgentError::new("每个组织变更必须是一个对象。"))?;
        let action = object
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        for old in ["memberId", "departmentId", "parentId", "modelConfigId"] {
            if object.contains_key(old) {
                return Err(AgentError::new("组织编辑使用 member（姓名）、department/parent（完整部门路径）和 model（模型名称），不接受内部 ID。"));
            }
        }
        let mappings: &[(&str, &str)] = match action.as_str() {
            "add_member" => &[("department", "departmentId"), ("model", "modelConfigId")],
            "update_member" => &[
                ("member", "memberId"),
                ("department", "departmentId"),
                ("model", "modelConfigId"),
            ],
            "remove_member" => &[("member", "memberId")],
            "add_department" => &[("parent", "parentId")],
            "update_department" => &[("department", "departmentId"), ("parent", "parentId")],
            "remove_department" => &[("department", "departmentId")],
            _ => &[],
        };
        for (name, internal) in mappings {
            if let Some(mut value) = object.remove(*name) {
                if let Some(text) = value.as_str() {
                    value = Value::String(text.trim().to_owned());
                }
                object.insert((*internal).into(), value);
            }
        }
    }
    let input: EditInput = serde_json::from_value(args)
        .map_err(|_| AgentError::new("organization_edit 参数无效。请按工具定义填写 action、成员姓名或部门路径，以及需要修改的字段。"))?;
    validate_edit_shape(&input)?;
    Ok(input)
}

/// Full paths can be much longer than persisted IDs. Validate path syntax separately and use
/// short placeholders only in a disposable validation copy; the Host never receives these.
/// Shared field validation still runs before receipt lookup, then again on resolved real IDs.
fn validate_edit_shape(input: &EditInput) -> AgentResult<()> {
    let mut validation = input.clone();
    let mut paths = BTreeMap::<String, String>::new();
    let mut path = |value: &mut String| -> AgentResult<()> {
        let parts: Vec<_> = value.split('/').collect();
        if value.len() > 64 * 513 || parts.len() > 64 || parts.iter().any(|part| !reference(part)) {
            return Err(AgentError::new(
                "部门路径需要 1 到 64 个非空名称，每个名称不超过 512 字节，使用 / 分隔。",
            ));
        }
        let placeholder = format!("department-{}", paths.len());
        *value = paths
            .entry(department_key(value))
            .or_insert(placeholder)
            .clone();
        Ok(())
    };
    for action in &mut validation.changes {
        match action {
            EditAction::AddMember { department_id, .. }
            | EditAction::UpdateMember { department_id, .. } => {
                if let OptionalId::Value(Some(value)) = department_id {
                    path(value)?;
                }
            }
            EditAction::AddDepartment { parent_id, .. } => {
                if let OptionalId::Value(Some(value)) = parent_id {
                    path(value)?;
                }
            }
            EditAction::UpdateDepartment {
                department_id,
                parent_id,
                ..
            } => {
                path(department_id)?;
                if let OptionalId::Value(Some(value)) = parent_id {
                    path(value)?;
                }
            }
            EditAction::RemoveDepartment { department_id } => {
                path(department_id)?;
            }
            EditAction::RemoveMember { .. } => {}
        }
    }
    validation.validate().map_err(AgentError::new)
}

pub(super) fn resolve_edit_input(
    mut input: EditInput,
    snapshot: &ConversationSnapshot,
    host: &dyn WorkflowRuntimeHost,
) -> AgentResult<EditInput> {
    let needs_models = input.changes.iter().any(|action| {
        matches!(
            action,
            EditAction::AddMember {
                model_config_id: Some(_),
                ..
            } | EditAction::UpdateMember {
                model_config_id: Some(_),
                ..
            }
        )
    });
    let model_state = if needs_models {
        Some(host.state(StateQuery {
            view: StateView::Configuration,
            node_id: None,
        })?)
    } else {
        None
    };
    let placement = |value: &mut OptionalId| -> AgentResult<()> {
        if let OptionalId::Value(Some(path)) = value {
            *path = department_id(snapshot, path)?;
        }
        Ok(())
    };
    let model = |value: &mut Option<String>| -> AgentResult<()> {
        if let Some(name) = value {
            let state = model_state.as_ref().expect("model configuration requested");
            let matches = state["configuration"]["availableModels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|entry| {
                    entry["name"].as_str().is_some_and(|candidate| {
                        member_name_key(candidate) == member_name_key(name)
                    })
                })
                .filter_map(|entry| entry["modelConfigId"].as_str())
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(AgentError::new(format!("无法唯一确定可用模型‘{name}’。请查询 organization_get_state 的 view=configuration，并使用返回的模型名称。")));
            }
            *name = matches[0].to_string();
        }
        Ok(())
    };
    for action in &mut input.changes {
        match action {
            EditAction::AddMember {
                department_id,
                model_config_id,
                ..
            } => {
                placement(department_id)?;
                model(model_config_id)?;
            }
            EditAction::UpdateMember {
                member_id: member,
                department_id,
                model_config_id,
                ..
            } => {
                *member = member_id(snapshot, member)?;
                placement(department_id)?;
                model(model_config_id)?;
            }
            EditAction::RemoveMember { member_id: member } => {
                *member = member_id(snapshot, member)?;
            }
            EditAction::AddDepartment { parent_id, .. } => {
                placement(parent_id)?;
            }
            EditAction::UpdateDepartment {
                department_id: department,
                parent_id,
                ..
            } => {
                *department = department_id(snapshot, department)?;
                placement(parent_id)?;
            }
            EditAction::RemoveDepartment {
                department_id: department,
            } => {
                *department = department_id(snapshot, department)?;
            }
        }
    }
    input.validate().map_err(AgentError::new)?;
    Ok(input)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StateInput {
    #[serde(default)]
    pub view: StateView,
    #[serde(default, deserialize_with = "present_string")]
    pub member: Option<String>,
}
