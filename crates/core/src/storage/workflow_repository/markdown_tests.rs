use super::*;
use crate::storage::migrations::run_migrations;
use crate::workflow_markdown;

fn definition() -> Definition {
    serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/organization-hierarchy-v1.json"
    ))
    .unwrap()
}

fn request(connection: &mut Connection, request: Request) -> Result<Response, Error> {
    super::request(connection, request, &HashSet::new())
}

fn department_path(definition: &Definition, id: &str) -> String {
    let department = definition
        .departments
        .iter()
        .find(|department| department.id == id)
        .unwrap();
    match department.parent_id.as_deref() {
        Some(parent) => format!(
            "{} / {}",
            department_path(definition, parent),
            department.name
        ),
        None => department.name.clone(),
    }
}

/// IDs, geometry and array order are local authoring details, not portable Markdown data.
fn portable_semantics(definition: &Definition) -> serde_json::Value {
    let mut departments: Vec<_> = definition
        .departments
        .iter()
        .map(|department| department_path(definition, &department.id))
        .collect();
    departments.sort();
    let mut members: Vec<_> = definition
        .nodes
        .iter()
        .map(|node| {
            let crate::workflow::NodeConfig::Agent(agent) = &node.config;
            serde_json::json!({
                "name": node.name,
                "departmentPath": node.department_id.as_deref().map(|id| department_path(definition, id)),
                "rank": node.rank,
                "managementRole": node.management_role,
                "receives": agent.receives,
                "task": agent.task,
                "delivers": agent.delivers,
            })
        })
        .collect();
    members.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    serde_json::json!({
        "name": definition.name,
        "description": definition.description,
        "background": definition.background,
        "departments": departments,
        "members": members,
    })
}

#[test]
fn markdown_import_creates_only_fresh_templates_with_unbound_models() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut source = definition();
    for node in &mut source.nodes {
        node.agent_mut().model_config_id = Some("source-local-model".into());
        node.agent_mut().permission_mode = crate::workflow::WorkflowPermissionMode::Full;
    }
    source.background = "共同背景第一段。\n\n共同背景第二段。".into();
    source.nodes[0].agent_mut().task = "核验 **证据**。\n\n保留来源和结论。".into();
    let markdown = workflow_markdown::export(&source, "zh-CN").unwrap();
    assert!(markdown.starts_with("Language: 中文\n\n# 组织：Research organization\n"));
    assert!(markdown.contains("## 部门：Research / Review team\n"));
    for private_field in [
        "modelConfigId",
        "permissionMode",
        "viewport",
        "schemaVersion",
        "```yaml",
    ] {
        assert!(!markdown.contains(private_field));
    }
    let mut imported_ids: HashSet<_> = std::iter::once(source.id.clone())
        .chain(source.nodes.iter().map(|node| node.id.clone()))
        .chain(
            source
                .departments
                .iter()
                .map(|department| department.id.clone()),
        )
        .collect();
    for expected_count in 1..=2 {
        let response = request(
            &mut connection,
            Request::ImportTemplateMarkdown {
                markdown: markdown.clone(),
            },
        )
        .unwrap();
        let id = response.imported_template_id.unwrap();
        assert_ne!(id, source.id);
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert!(imported_ids.insert(id.clone()));
        assert_eq!(response.records.len(), expected_count);
        let imported = response
            .records
            .iter()
            .find(|record| record.definition.id == id)
            .unwrap();
        assert_eq!(
            portable_semantics(&imported.definition),
            portable_semantics(&source)
        );
        assert_eq!(
            workflow_markdown::export(&imported.definition, "zh-CN").unwrap(),
            markdown
        );
        assert_eq!(
            imported
                .definition
                .nodes
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["implement", "验收", "review"]
        );
        assert_eq!(
            imported
                .definition
                .departments
                .iter()
                .map(|department| department_path(&imported.definition, &department.id))
                .collect::<Vec<_>>(),
            ["Research", "Research / Review team"]
        );
        for node_id in imported.definition.nodes.iter().map(|node| &node.id).chain(
            imported
                .definition
                .departments
                .iter()
                .map(|department| &department.id),
        ) {
            assert!(uuid::Uuid::parse_str(node_id).is_ok());
            assert!(imported_ids.insert(node_id.clone()));
        }
        assert_eq!(imported.revision, 1);
        assert!(!imported.enabled);
        assert_eq!(imported.issues.len(), source.nodes.len());
        assert!(imported
            .issues
            .iter()
            .all(|issue| issue.code == "node_model"));
        assert!(response.instances.is_empty());
        assert!(response.drafts.is_empty());
        assert!(response.affected_conversation_ids.is_empty());
        for node in &imported.definition.nodes {
            let crate::workflow::NodeConfig::Agent(agent) = &node.config;
            assert!(agent.model_config_id.is_none());
            assert_eq!(
                agent.permission_mode,
                crate::workflow::WorkflowPermissionMode::Default
            );
        }
    }
    for table in [
        "conversations",
        "agent_nodes",
        "workflow_instances",
        "workflow_instance_bindings",
        "workflow_editing_drafts",
        "workflow_mail_runs",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[test]
fn markdown_import_accepts_handwritten_text_and_restores_department_paths() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let markdown = r#"Language: 中文

# 组织：研究团队

## 简介

收集并复核证据。

## 公共背景

共同背景 **文字**。

## 直属成员

### 成员：协调者

- 职级：9
- 管理身份：组织管理员

#### 接收内容

用户目标。

#### 职责

协调研究。

#### 交付要求

完整报告。

## 部门：研究部

### 成员：研究员

- 职级：2
- 管理身份：普通成员

#### 接收内容

研究任务。

#### 职责

整理原始资料。

#### 交付要求

证据清单。

## 部门：研究部 / 复核组

### 成员：复核员

- 职级：8
- 管理身份：部门管理员

#### 接收内容

证据清单。

#### 职责

核验来源。

#### 交付要求

复核结论。
"#;
    let response = request(
        &mut connection,
        Request::ImportTemplateMarkdown {
            markdown: markdown.into(),
        },
    )
    .unwrap();
    let imported = &response.records[0].definition;
    assert_eq!(imported.name, "研究团队");
    assert_eq!(imported.description, "收集并复核证据。");
    assert_eq!(imported.background, "共同背景 **文字**。");
    assert_eq!(
        imported
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        ["协调者", "研究员", "复核员"]
    );
    for (index, path, rank, role, receives, task, delivers) in [
        (
            0,
            None,
            9,
            crate::workflow::ManagementRole::OrganizationAdmin,
            "用户目标。",
            "协调研究。",
            "完整报告。",
        ),
        (
            1,
            Some("研究部"),
            2,
            crate::workflow::ManagementRole::Member,
            "研究任务。",
            "整理原始资料。",
            "证据清单。",
        ),
        (
            2,
            Some("研究部 / 复核组"),
            8,
            crate::workflow::ManagementRole::DepartmentAdmin,
            "证据清单。",
            "核验来源。",
            "复核结论。",
        ),
    ] {
        let node = &imported.nodes[index];
        assert_eq!(
            node.department_id
                .as_deref()
                .map(|id| department_path(imported, id))
                .as_deref(),
            path
        );
        assert_eq!(node.rank, rank);
        assert_eq!(node.management_role, role);
        let crate::workflow::NodeConfig::Agent(agent) = &node.config;
        assert_eq!(agent.receives, receives);
        assert_eq!(agent.task, task);
        assert_eq!(agent.delivers, delivers);
    }
    assert!(!response.records[0].enabled);
    assert!(response.instances.is_empty());
    assert!(response.drafts.is_empty());
}

#[test]
fn markdown_import_and_export_preserve_content_across_all_app_languages() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut source = definition();
    source.description = "Research 研究 — **shared evidence**.\n\nKeep both paragraphs.".into();
    source.background = "## Existing background heading\n\nA literal field example:\n```md\n#### 职责\n- Rank: 9\n```\n".into();
    source.nodes[0].agent_mut().receives = "Unchanged user content: α, 中文, 日本語.  \n".into();
    source.nodes[0].agent_mut().task = "Verify **evidence**.\n\nDo not translate this task.".into();
    source.nodes[0].agent_mut().delivers = "Reports at `results/report.md`.\n".into();
    let original = workflow_markdown::export(&source, "zh-CN").unwrap();
    let imported = request(
        &mut connection,
        Request::ImportTemplateMarkdown {
            markdown: original.clone(),
        },
    )
    .unwrap();
    let imported_id = imported.imported_template_id.unwrap();

    for (language, declared_language) in [
        ("zh-CN", "中文"),
        ("zh-TW", "繁體中文"),
        ("en-US", "English"),
        ("en-GB", "English (United Kingdom)"),
        ("ko-KR", "한국어"),
        ("ja-JP", "日本語"),
        ("fr-FR", "Français"),
        ("it-IT", "Italiano"),
        ("ru-RU", "Русский"),
    ] {
        let exported = request(
            &mut connection,
            Request::ExportTemplateMarkdown {
                id: imported_id.clone(),
                expected_revision: 1,
                language: language.into(),
            },
        )
        .unwrap()
        .exported_template
        .unwrap()
        .markdown;
        assert_eq!(
            exported.lines().next(),
            Some(format!("Language: {declared_language}").as_str()),
            "{language}"
        );
        let reimported = request(
            &mut connection,
            Request::ImportTemplateMarkdown {
                markdown: exported.clone(),
            },
        )
        .unwrap();
        let reimported_id = reimported.imported_template_id.unwrap();
        let reimported_definition = &reimported
            .records
            .iter()
            .find(|record| record.definition.id == reimported_id)
            .unwrap()
            .definition;
        assert_eq!(
            portable_semantics(reimported_definition),
            portable_semantics(&source),
            "{language}"
        );
        for (output_language, expected_markdown) in
            [(language, exported.as_str()), ("zh-CN", original.as_str())]
        {
            let actual = request(
                &mut connection,
                Request::ExportTemplateMarkdown {
                    id: reimported_id.clone(),
                    expected_revision: 1,
                    language: output_language.into(),
                },
            )
            .unwrap()
            .exported_template
            .unwrap()
            .markdown;
            assert_eq!(actual, expected_markdown, "{language} -> {output_language}");
        }
    }

    let before = serde_json::to_value(request(&mut connection, Request::List).unwrap()).unwrap();
    for language in ["", "unknown", "en", "en-US ", "中文"] {
        assert!(matches!(
            request(
                &mut connection,
                Request::ExportTemplateMarkdown {
                    id: imported_id.clone(),
                    expected_revision: 1,
                    language: language.into(),
                },
            ),
            Err(Error::Invalid(message)) if message == workflow_markdown::INVALID_FORMAT
        ));
    }
    assert_eq!(
        serde_json::to_value(request(&mut connection, Request::List).unwrap()).unwrap(),
        before
    );
}

#[test]
fn markdown_export_reads_published_revision_and_leaves_draft_and_database_unchanged() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let source = definition();
    request(
        &mut connection,
        Request::Save {
            definition: source.clone(),
            expected_revision: 0,
        },
    )
    .unwrap();
    let mut draft = source.clone();
    draft.background = "Unpublished draft-only content".into();
    request(&mut connection, serde_json::from_value(serde_json::json!({
        "operation":"saveDraft", "definition":draft, "expectedRevision":1, "expectedDraftRevision":0
    })).unwrap()).unwrap();
    let before = serde_json::to_value(request(&mut connection, Request::List).unwrap()).unwrap();
    let response = request(
        &mut connection,
        Request::ExportTemplateMarkdown {
            id: source.id.clone(),
            expected_revision: 1,
            language: "zh-CN".into(),
        },
    )
    .unwrap();
    let exported = response.exported_template.unwrap();
    assert_eq!(
        exported.markdown,
        workflow_markdown::export(&source, "zh-CN").unwrap()
    );
    assert!(!exported.markdown.contains("Unpublished draft-only content"));
    let restored = workflow_markdown::import(&exported.markdown).unwrap();
    assert_eq!(portable_semantics(&restored), portable_semantics(&source));
    assert_eq!(exported.suggested_file_name, "Research organization.md");
    assert_eq!(
        serde_json::to_value(request(&mut connection, Request::List).unwrap()).unwrap(),
        before
    );
    for (id, expected_revision) in [(&source.id, 0), (&source.id, 2), (&"missing".into(), 1)] {
        assert!(matches!(
            request(
                &mut connection,
                Request::ExportTemplateMarkdown {
                    id: id.clone(),
                    expected_revision,
                    language: "zh-CN".into(),
                }
            ),
            Err(Error::Conflict(_))
        ));
    }
}

#[test]
fn markdown_import_is_atomic_on_invalid_format_and_projection_failure() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let markdown = workflow_markdown::export(&definition(), "zh-CN").unwrap();
    for malformed in [
        markdown.replacen("Language: 中文\n\n", "", 1),
        markdown.replacen("Language: 中文", "Language: Unknown", 1),
        markdown.replacen("## 公共背景", "## 简介", 1),
        markdown.replacen("## 公共背景", "## 未知章节", 1),
        markdown.replacen("- 管理身份：组织管理员", "- 管理身份：超级管理员", 1),
        "# Organization\n\n```yaml\nkind: organization\nversion: 1\n```\n".into(),
    ] {
        assert!(matches!(
            request(
                &mut connection,
                Request::ImportTemplateMarkdown { markdown: malformed }
            ),
            Err(Error::Invalid(message)) if message == workflow_markdown::INVALID_FORMAT
        ));
    }
    let count = || {
        connection
            .query_row("SELECT COUNT(*) FROM workflow_definitions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    assert_eq!(count(), 0);
    // Force an error after the INSERT, when constructing the response. The transaction
    // must roll the template back as well, rather than reporting failure after a partial import.
    connection
        .execute_batch("DROP TABLE workflow_editing_drafts;")
        .unwrap();
    assert!(request(
        &mut connection,
        Request::ImportTemplateMarkdown { markdown }
    )
    .is_err());
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM workflow_definitions", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
}

#[test]
fn markdown_import_rolls_back_when_catalog_limit_is_reached() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let source = definition();
    let json = serde_json::to_string(&source).unwrap();
    connection.execute(
        "WITH RECURSIVE entries(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM entries WHERE n<1000) INSERT INTO workflow_definitions(workflow_id, definition_json, revision, updated_at) SELECT 'existing-' || n,json_set(?1, '$.id', 'existing-' || n),1,1 FROM entries",
        [json],
    ).unwrap();
    assert!(matches!(
        request(
            &mut connection,
            Request::ImportTemplateMarkdown {
                markdown: workflow_markdown::export(&source, "zh-CN").unwrap()
            }
        ),
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM workflow_definitions", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1000
    );
}

#[test]
fn markdown_export_avoids_unrelated_catalog_activity_and_model_tables() {
    use crate::storage::service::StorageService;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("isolated-export.sqlite");
    let storage = StorageService::open(&path).unwrap();
    let source = definition();
    storage
        .workflow_request(Request::Save {
            definition: source.clone(),
            expected_revision: 0,
        })
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute("INSERT INTO workflow_definitions(workflow_id, definition_json, revision, updated_at) VALUES('corrupt', '{\"id\":\"corrupt\",\"schemaVersion\":1}', 1, 1)", []).unwrap();
    connection
        .execute_batch(
            "DROP TABLE workflow_editing_drafts; DROP TABLE workflow_mail_runs; DROP TABLE models;",
        )
        .unwrap();
    let response = storage
        .workflow_request(Request::ExportTemplateMarkdown {
            id: source.id.clone(),
            expected_revision: 1,
            language: "zh-CN".into(),
        })
        .unwrap();
    assert_eq!(
        response.exported_template.unwrap().markdown,
        workflow_markdown::export(&source, "zh-CN").unwrap()
    );
    assert!(response.records.is_empty());
    assert!(response.instances.is_empty());
    assert!(response.drafts.is_empty());
}
