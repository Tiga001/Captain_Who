use super::*;

fn definition() -> Definition {
    serde_json::from_str(include_str!(
        "../../../../packages/protocol/fixtures/organization-hierarchy-v1.json"
    ))
    .unwrap()
}

fn portable(definition: &Definition) -> serde_json::Value {
    let paths = definition
        .departments
        .iter()
        .map(|department| {
            let mut path = vec![department.name.clone()];
            let mut parent = department.parent_id.as_deref();
            while let Some(id) = parent {
                let department = definition
                    .departments
                    .iter()
                    .find(|department| department.id == id)
                    .unwrap();
                path.push(department.name.clone());
                parent = department.parent_id.as_deref();
            }
            path.reverse();
            (department.id.clone(), path)
        })
        .collect::<HashMap<_, _>>();
    let members = definition.nodes.iter().map(|node| {
        let NodeConfig::Agent(agent) = &node.config;
        (node.name.clone(), serde_json::json!({
            "rank":node.rank,"role":node.management_role,"department":node.department_id.as_ref().map(|id|&paths[id]),
            "receives":agent.receives,"task":agent.task,"delivers":agent.delivers,
        }))
    }).collect::<std::collections::BTreeMap<_, _>>();
    let departments = paths
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    serde_json::json!({"name":definition.name,"description":definition.description,"background":definition.background,"members":members,"departments":departments})
}

#[test]
fn markdown_is_readable_and_round_trips_public_content_without_local_identifiers() {
    let mut source = definition();
    source.nodes[0].agent_mut().model_config_id = Some("private-model-configuration".into());
    source.nodes[0].agent_mut().permission_mode = WorkflowPermissionMode::Full;
    source.nodes[1].agent_mut().permission_mode = WorkflowPermissionMode::Custom;
    source.description = "Description\n\nsecond paragraph\n".into();
    source.background =
        "**Bold** and [link](https://example.test)\n\n- An ordinary list item\n".into();
    let markdown = export(&source, "zh-CN").unwrap();
    assert!(markdown.starts_with("Language: 中文\n\n# 组织：Research organization\n\n## 简介\n\n"));
    assert!(markdown.contains("## 部门：Research / Review team\n"));
    for internal in [
        "private-model-configuration",
        "modelConfigId",
        "permissionMode",
        "schemaVersion",
        "viewport",
        "kind:",
        "order:",
        "```yaml",
        &source.id,
    ] {
        assert!(!markdown.contains(internal), "{internal}");
    }
    let actual = import(&markdown).unwrap();
    assert_ne!(actual.id, source.id);
    for id in std::iter::once(&actual.id)
        .chain(actual.nodes.iter().map(|node| &node.id))
        .chain(actual.departments.iter().map(|department| &department.id))
    {
        assert!(uuid::Uuid::parse_str(id).is_ok());
        assert!(!markdown.contains(id));
    }
    for node in &actual.nodes {
        let NodeConfig::Agent(agent) = &node.config;
        assert!(agent.model_config_id.is_none());
        assert_eq!(agent.permission_mode, WorkflowPermissionMode::Default);
    }
    assert_eq!(portable(&actual), portable(&source));
    assert_eq!(export(&actual, "zh-CN").unwrap(), markdown);
    assert!(!actual.validate(&HashSet::new()).unwrap().is_empty());
}

#[test]
fn markdown_order_is_hierarchical_then_rank_then_administrator_and_stable_for_ties() {
    let mut source = definition();
    source.departments.swap(0, 1);
    let base = source.nodes[0].clone();
    source.nodes = [
        ("member-first", Some("research"), 8, ManagementRole::Member),
        (
            "admin-first",
            Some("research"),
            8,
            ManagementRole::OrganizationAdmin,
        ),
        ("member-higher", Some("research"), 9, ManagementRole::Member),
        (
            "admin-second",
            Some("research"),
            8,
            ManagementRole::DepartmentAdmin,
        ),
        ("member-second", Some("research"), 8, ManagementRole::Member),
        ("child", Some("review-team"), 99, ManagementRole::Member),
        ("unassigned", None, 1, ManagementRole::Member),
    ]
    .into_iter()
    .map(|(id, department, rank, role)| {
        let mut node = base.clone();
        node.id = id.into();
        node.name = format!("Name {id}");
        node.department_id = department.map(str::to_string);
        node.rank = rank;
        node.management_role = role;
        node
    })
    .collect();
    let markdown = export(&source, "zh-CN").unwrap();
    let expected_order = [
        "unassigned",
        "member-higher",
        "admin-first",
        "admin-second",
        "member-first",
        "member-second",
        "child",
    ];
    let restored = import(&markdown).unwrap();
    assert_eq!(
        restored
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        expected_order
            .map(|id| format!("Name {id}"))
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        restored
            .departments
            .iter()
            .map(|department| department.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Research", "Review team"]
    );
    assert_eq!(portable(&source), portable(&restored));
    assert_eq!(export(&restored, "zh-CN").unwrap(), markdown);
}

#[test]
fn markdown_prompts_preserve_exact_whitespace_inline_markup_and_structural_lookalikes() {
    for value in [
        "", "plain", "\n", "\n\n", "hello\n", "hello\n\n", "\nleading\n\n", "   ",
        "first\n  indented\n\tTabbed\n", "\tinitial tab", "line\r\nCRLF\rbare", "bare\r",
        "**bold** _italic_ [link](https://example.test/a?b=c&d=e) ![image](local.png)",
        "# Prompt heading\n## Another heading\n### Detailed heading\n#### Subheading\n",
        "## 部门：A fake department\n### 成员：A fake member\n#### 职责\n- 职级：90\n- 管理身份：组织管理员",
        "\\## 公共背景\n\\\\#### 交付要求\n\\\\\\- 职级：90\n\\ordinary\n\\*literal*",
        "  ## Indented heading\n    ## Indented code\n#\n###\tTab heading",
        "中文🙂\u{0085}\u{2028}\u{2029}\0\u{001b}\u{007f}\u{ffff}",
    ] {
        let mut source = definition();
        source.description = value.into();
        source.background = value.into();
        for node in &mut source.nodes {
            node.agent_mut().receives = value.into();
            node.agent_mut().task = value.into();
            node.agent_mut().delivers = value.into();
        }
        let markdown = export(&source, "zh-CN").unwrap();
        let restored = import(&markdown).unwrap_or_else(|error|panic!("{value:?}: {error}\n{markdown}"));
        assert_eq!(portable(&source), portable(&restored), "{value:?}");
        assert_eq!(export(&restored, "zh-CN").unwrap(), markdown, "{value:?}");
    }
}

#[test]
fn markdown_fences_protect_embedded_sections_and_unclosed_fences_round_trip() {
    for value in [
        "```markdown\n## 部门：Fake\n#### 职责\n\\## 公共背景\n```",
        "````markdown\n```\n## 简介\n````\nAfter the fence",
        "  ~~~text\n# 组织：Fake\n  ~~~~\t\nTail",
        "```unclosed\n## 部门：Literal prompt heading\n#### 交付要求",
        "before\n~~~\nunterminated",
        "\\```\n\\\\~~~\n\\## 公共背景",
        "```\n````\n~~~\n# Prompt heading\nkind: organization\n---\n",
    ] {
        let mut source = definition();
        source.background = value.into();
        for node in &mut source.nodes {
            node.agent_mut().task = value.into();
        }
        let markdown = export(&source, "zh-CN").unwrap();
        let restored =
            import(&markdown).unwrap_or_else(|error| panic!("{value:?}: {error}\n{markdown}"));
        assert_eq!(portable(&source), portable(&restored), "{value:?}");
        assert_eq!(export(&restored, "zh-CN").unwrap(), markdown);
    }
}

#[test]
fn markdown_names_use_standard_escapes_without_hidden_identity_metadata() {
    let mut source = definition();
    source.name = "组织 &amp; \"quotes\" *stars* \\path\nnext\rline".into();
    source.departments[0].name = "**Research** & development".into();
    source.nodes[0].name = "[CEO] \\literal & <name>\t\nnext".into();
    let markdown = export(&source, "zh-CN").unwrap();
    assert!(markdown.contains("&amp;amp;"));
    assert!(markdown.contains("&#xA;"));
    let restored = import(&markdown).unwrap();
    assert_eq!(portable(&source), portable(&restored));
    assert_eq!(export(&restored, "zh-CN").unwrap(), markdown);
}

#[test]
fn markdown_rejects_missing_duplicate_or_unknown_structural_sections() {
    let markdown = export(&definition(), "zh-CN").unwrap();
    for malformed in [
        markdown.replacen("## 公共背景", "## 未知章节", 1),
        markdown.replacen("## 公共背景", "## 简介", 1),
        markdown.replacen("#### 职责", "#### 接收内容", 1),
        markdown.replacen("#### 接收内容", "#### 交付要求", 1),
        markdown.replacen("- 管理身份：组织管理员", "- 管理身份：Other", 1),
        markdown.replacen("- 职级：8", "- 职级：100", 1),
        markdown.replacen("- 职级：8", "- 职级：0", 1),
        markdown.replacen("- 职级：8", "- 职级：eight", 1),
        markdown.replacen("## 部门：Research\n", "", 1),
        format!("Preamble\n{markdown}"),
        format!("{markdown}## 未知部门：Typo\n"),
        format!("{markdown}# Another organization\n"),
        format!("{markdown}#### 接收内容\nDuplicate field\n"),
    ] {
        assert_eq!(
            import(&malformed).unwrap_err(),
            INVALID_FORMAT,
            "{malformed}"
        );
    }
    assert_eq!(
        import("# Organization\n\n```yaml\nkind: organization\nversion: 1\n```\n").unwrap_err(),
        INVALID_FORMAT
    );
    assert_eq!(import("").unwrap_err(), INVALID_FORMAT);
    assert_eq!(import("# 组织：Only a name").unwrap_err(), INVALID_FORMAT);
}

#[test]
fn markdown_rejects_ambiguous_department_paths_and_member_names() {
    let markdown = export(&definition(), "zh-CN").unwrap();
    for malformed in [
        markdown.replacen(
            "## 部门：Research / Review team",
            "## 部门：Missing / Review team",
            1,
        ),
        markdown.replacen("## 部门：Research / Review team", "## 部门：Research", 1),
        markdown.replacen("## 部门：Research / Review team", "## 部门： research ", 1),
        markdown.replacen("## 部门：Research / Review team", "## 部门：Research / ", 1),
        markdown.replacen("### 成员：review", "### 成员：IMPLEMENT", 1),
        markdown.replacen("## 部门：Research / Review team", "## 直属成员", 1),
    ] {
        assert_eq!(
            import(&malformed).unwrap_err(),
            INVALID_FORMAT,
            "{malformed}"
        );
    }
}

#[test]
fn markdown_accepts_bom_crlf_and_hand_authored_empty_sections() {
    let source = "Language: 中文\n\n# 组织：团队\n\n## 简介\n\n简介。\n\n## 公共背景\n\n背景。\n\n## 直属成员\n\n### 成员：审核员\n\n- 职级：8\n- 管理身份：普通成员\n\n#### 接收内容\n\n#### 职责\n\n完成审核。\n\n#### 交付要求\n\n报告。\n";
    for markdown in [
        source.to_string(),
        format!("\u{feff}{}", source.replace('\n', "\r\n")),
    ] {
        let restored = import(&markdown).unwrap();
        assert_eq!(restored.description, "简介。");
        assert_eq!(restored.background, "背景。");
        let NodeConfig::Agent(agent) = &restored.nodes[0].config;
        assert_eq!(agent.receives, "");
        assert_eq!(agent.task, "完成审核。");
        assert_eq!(agent.delivers, "报告。");
    }
}

#[test]
fn markdown_enforces_limits_and_suggests_safe_filenames() {
    let mut source = definition();
    source.nodes[0].agent_mut().task = "x".repeat(128_001);
    assert_eq!(export(&source, "zh-CN").unwrap_err(), INVALID_FORMAT);
    assert_eq!(
        import(&" ".repeat(MAX_MARKDOWN_BYTES + 1)).unwrap_err(),
        TOO_LARGE
    );
    assert_eq!(suggested_file_name(" ../销售:Q4?*| "), "_销售_Q4___.md");
    assert_eq!(suggested_file_name(". \n.."), "organization-template.md");
    assert!(suggested_file_name(&"long".repeat(100)).len() <= 83);
    assert!(suggested_file_name(&"🙂界".repeat(100)).len() <= 203);
    assert_eq!(suggested_file_name("CON"), "organization-CON.md");
}

#[test]
fn markdown_handles_many_unmatched_fences_without_rescanning_each_suffix() {
    let mut source = definition();
    source.background = "```x\n".repeat(20_000);
    let markdown = export(&source, "zh-CN").unwrap();
    let restored = import(&markdown).unwrap();
    assert_eq!(restored.background, source.background);
    assert_eq!(export(&restored, "zh-CN").unwrap(), markdown);
}

#[test]
fn markdown_supports_every_registered_application_language() {
    let registry = include_str!("../../../../src/shared/i18n/languageRegistry.ts");
    let app_codes = registry
        .lines()
        .filter_map(|line| line.strip_prefix("  '"))
        .filter_map(|line| line.strip_suffix("': {"))
        .collect::<std::collections::BTreeSet<_>>();
    let document_codes = language::LANGUAGES
        .iter()
        .map(|language| language.code)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!app_codes.is_empty());
    assert_eq!(document_codes, app_codes);
    let names = language::LANGUAGES
        .iter()
        .map(|language| language.name)
        .collect::<HashSet<_>>();
    assert_eq!(names.len(), document_codes.len());
}

#[test]
fn markdown_every_language_pair_preserves_all_authored_content_and_switches_back_exactly() {
    let mut source = definition();
    source.name = "Polymerization *Research* & 中文\nNotes".into();
    source.departments[0].name = "Research & 科研".into();
    source.departments[1].name = "Review [日本語]".into();
    source.nodes[0].name = "Captain — 한국어 & <name>\t".into();
    let mut direct_member = source.nodes[0].clone();
    direct_member.id = "direct-member".into();
    direct_member.name = "Direct member Русский".into();
    direct_member.department_id = None;
    source.nodes.push(direct_member);
    let mut empty_department = source.departments[1].clone();
    empty_department.id = "empty-department".into();
    empty_department.name = "Empty Français".into();
    source.departments.push(empty_department);
    // Include every locale's reserved fields as literal prose, with existing escapes,
    // fenced examples and headings. Changing the framing must not translate these.
    let vocabularies = language::LANGUAGES
        .iter()
        .map(|language| {
            format!(
                "{}\n{}\n{}\n{}\n{}20\n{}{}\n\\{}\n\\\\{}\n```md\n{}\n{}\n```\n",
                language.description,
                language.receives,
                language.task,
                language.delivers,
                language.rank,
                language.role,
                language.role_member,
                language.task,
                language.rank,
                language.background,
                language.delivers,
            )
        })
        .collect::<String>();
    let body = format!(
        "\nAuthored text.\r\n**Bold** and [link](https://example.test/a).\n\n{vocabularies}\n~~~unclosed\n## Literal heading\n\n"
    );
    source.description = body.clone();
    source.background = body.clone();
    for (index, node) in source.nodes.iter_mut().enumerate() {
        let agent = node.agent_mut();
        agent.receives = format!("{index}\n{body}");
        agent.task = body.clone();
        agent.delivers = format!("{body}\0end\n");
    }
    let expected = portable(&source);
    for from in language::LANGUAGES {
        let original = export(&source, from.code).unwrap();
        assert!(original.starts_with(&format!("Language: {}\n\n", from.name)));
        let imported = import(&original).unwrap();
        assert_eq!(portable(&imported), expected, "{} import", from.code);
        assert_eq!(export(&imported, from.code).unwrap(), original);
        for to in language::LANGUAGES {
            let converted = export(&imported, to.code).unwrap();
            let restored = import(&converted).unwrap_or_else(|error| {
                panic!("{} -> {}: {error}\n{converted}", from.code, to.code)
            });
            assert_eq!(
                portable(&restored),
                expected,
                "{} -> {}",
                from.code,
                to.code
            );
            assert_eq!(export(&restored, to.code).unwrap(), converted);
            assert_eq!(export(&restored, from.code).unwrap(), original);
        }
    }
}

#[test]
fn markdown_language_declaration_is_required_first_and_selects_the_only_structural_vocabulary() {
    let chinese = export(&definition(), "zh-CN").unwrap();
    for valid in [
        chinese.clone(),
        chinese.replacen("Language: 中文", "Language：中文", 1),
        format!("\u{feff}{}", chinese.replace('\n', "\r\n")),
    ] {
        assert_eq!(portable(&import(&valid).unwrap()), portable(&definition()));
    }
    for malformed in [
        chinese.replacen("Language: 中文\n", "", 1),
        chinese.replacen("Language: 中文", "Language: Klingon", 1),
        chinese.replacen("Language: 中文", "Language: English", 1),
        chinese.replacen("Language: 中文", "语言：中文", 1),
        chinese.replacen("Language: 中文", "language: 中文", 1),
        format!("\n{chinese}"),
        format!("  {chinese}"),
        chinese.replacen("## 简介", "## Description", 1),
        chinese.replacen("#### 职责", "#### Responsibilities", 1),
        chinese.replacen("- 职级：", "- Rank: ", 1),
        chinese.replacen(
            "- 管理身份：组织管理员",
            "- 管理身份：Organization administrator",
            1,
        ),
    ] {
        assert_eq!(
            import(&malformed).unwrap_err(),
            INVALID_FORMAT,
            "{malformed}"
        );
    }
    for code in ["", "en", "en-AU", "English", "zh-CN\n"] {
        assert_eq!(export(&definition(), code).unwrap_err(), INVALID_FORMAT);
    }
}
