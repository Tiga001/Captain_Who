use super::*;

pub(super) fn validate_descriptor_contract(
    source: &dyn SkillSource,
    descriptor: &SkillDescriptor,
) -> Result<(), String> {
    if descriptor.id().source_id() != source.id() {
        return Err(format!(
            "descriptor `{}` belongs to source `{}` instead of registered source `{}`",
            descriptor.id(),
            descriptor.id().source_id(),
            source.id()
        ));
    }
    if descriptor.source_kind() != source.kind() {
        return Err(format!(
            "descriptor `{}` reports source kind `{}` instead of `{}`",
            descriptor.id(),
            descriptor.source_kind().stable_name(),
            source.kind().stable_name()
        ));
    }
    if descriptor.trust() != source.trust() {
        return Err(format!(
            "descriptor `{}` reports trust `{}` instead of `{}`",
            descriptor.id(),
            descriptor.trust().stable_name(),
            source.trust().stable_name()
        ));
    }
    if descriptor.activation_scope() != source.activation_scope() {
        return Err(format!(
            "descriptor `{}` reports activation scope `{}` instead of `{}`",
            descriptor.id(),
            descriptor.activation_scope().stable_name(),
            source.activation_scope().stable_name()
        ));
    }
    match (source.kind(), descriptor.provenance()) {
        (
            SkillSourceKind::Workspace,
            SkillProvenance::Workspace {
                workspace_id,
                relative_path,
            },
        ) => {
            let expected = format!("workspace:{}", percent_encode(workspace_id.as_bytes()));
            if expected != source.id().as_str() {
                return Err(format!(
                    "descriptor `{}` has workspace provenance for a different source",
                    descriptor.id()
                ));
            }
            validate_workspace_provenance_path(descriptor, relative_path)?;
        }
        (
            SkillSourceKind::Bundled,
            SkillProvenance::Bundled {
                source_id,
                relative_path,
            },
        ) if source_id == source.id() => {
            if !is_canonical_display_path(relative_path) {
                return Err(format!(
                    "descriptor `{}` has a non-canonical bundled path `{relative_path}`",
                    descriptor.id()
                ));
            }
            let expected = format!("{}/SKILL.md", descriptor.id().local_id());
            if relative_path != &expected {
                return Err(format!(
                    "descriptor `{}` has bundled path `{relative_path}` instead of `{expected}`",
                    descriptor.id()
                ));
            }
        }
        (
            SkillSourceKind::Installed,
            SkillProvenance::Installed {
                source_id,
                installation_id,
                relative_path,
            },
        ) if source_id == source.id() => {
            if descriptor.id().local_id() != installation_id.as_str() {
                return Err(format!(
                    "descriptor `{}` is not bound to installation `{installation_id}`",
                    descriptor.id()
                ));
            }
            if !is_canonical_display_path(relative_path) {
                return Err(format!(
                    "descriptor `{}` has a non-canonical installed package path `{relative_path}`",
                    descriptor.id()
                ));
            }
            let expected =
                managed_package_relative_path(descriptor.revision()).map_err(|reason| {
                    format!(
                        "descriptor `{}` has an invalid managed package revision: {reason}",
                        descriptor.id()
                    )
                })?;
            if relative_path != &expected {
                return Err(format!(
                    "descriptor `{}` has installed package path `{relative_path}` instead of `{expected}`",
                    descriptor.id()
                ));
            }
        }
        _ => {
            return Err(format!(
                "descriptor `{}` has provenance incompatible with source kind `{}`",
                descriptor.id(),
                source.kind().stable_name()
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_workspace_provenance_path(
    descriptor: &SkillDescriptor,
    relative_path: &str,
) -> Result<(), String> {
    if !is_canonical_display_path(relative_path) {
        return Err(format!(
            "descriptor `{}` has a non-canonical workspace path `{relative_path}`",
            descriptor.id()
        ));
    }
    let components = relative_path.split('/').collect::<Vec<_>>();
    let [agents, skills, directory, skill_file] = components.as_slice() else {
        return Err(format!(
            "descriptor `{}` has workspace path `{relative_path}` outside the supported Skill layout",
            descriptor.id()
        ));
    };
    if *agents != AGENTS_DIRECTORY || *skills != SKILLS_DIRECTORY || *skill_file != SKILL_FILE_NAME
    {
        return Err(format!(
            "descriptor `{}` has workspace path `{relative_path}` outside the supported Skill layout",
            descriptor.id()
        ));
    }
    let expected_local_id = percent_encode(directory.as_bytes());
    if descriptor.id().local_id() != expected_local_id {
        return Err(format!(
            "descriptor `{}` is not bound to workspace directory `{directory}`",
            descriptor.id()
        ));
    }
    Ok(())
}

pub(super) fn is_canonical_display_path(relative_path: &str) -> bool {
    !relative_path.is_empty()
        && !relative_path.starts_with('/')
        && !relative_path.contains('\\')
        && !relative_path.chars().any(char::is_control)
        && relative_path
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

pub(super) fn validate_resolved_contract(
    source: &dyn SkillSource,
    selection: &SkillSelection,
    package: &ResolvedSkillPackage,
) -> Result<(), String> {
    validate_descriptor_contract(source, package.descriptor())?;
    if package.source_text().len() > MAX_SKILL_FILE_BYTES {
        return Err(format!(
            "resolved package source exceeds the {MAX_SKILL_FILE_BYTES}-byte limit"
        ));
    }
    let actual_revision = match package.format_version() {
        SKILL_PACKAGE_FORMAT_VERSION => package_revision(package.source_text().as_bytes()),
        SKILL_PACKAGE_FORMAT_VERSION_V2 | SKILL_PACKAGE_FORMAT_VERSION_V3 => {
            let manifest = PackageManifest::from_resolved(
                package.source_text().as_bytes(),
                package.resources(),
            )
            .map_err(|error| {
                format!(
                    "resolved package `{}` has an invalid resource index: {}",
                    package.id(),
                    error.message
                )
            })?;
            if manifest.format_version() != package.format_version() {
                return Err(format!(
                    "resolved package `{}` declares format {}, but its resource index requires format {}",
                    package.id(),
                    package.format_version(),
                    manifest.format_version()
                ));
            }
            manifest.revision()
        }
        version => {
            return Err(format!(
                "resolved package `{}` uses unsupported format {version}",
                package.id()
            ))
        }
    };
    if package.revision() != &actual_revision {
        return Err(format!(
            "resolved package revision `{}` does not match its source snapshot `{actual_revision}`",
            package.revision()
        ));
    }
    let default_name = match package.provenance() {
        SkillProvenance::Workspace { relative_path, .. } => {
            relative_path.split('/').nth(2).ok_or_else(|| {
                format!(
                    "resolved package `{}` has no workspace directory",
                    package.id()
                )
            })?
        }
        SkillProvenance::Bundled { .. } => package.id().local_id(),
        SkillProvenance::Installed {
            installation_id, ..
        } => installation_id.as_str(),
        SkillProvenance::Other { .. } => {
            return Err(format!(
                "resolved package `{}` has unsupported provenance",
                package.id()
            ));
        }
    };
    let parsed = parse_skill_document(package.source_text(), default_name).map_err(|error| {
        format!(
            "resolved package source cannot be parsed as its descriptor `{}`: {error}",
            package.id()
        )
    })?;
    if matches!(
        source.kind(),
        SkillSourceKind::Bundled | SkillSourceKind::Installed
    ) && parsed.metadata.name_was_defaulted
    {
        return Err(format!(
            "resolved non-workspace package `{}` must declare an explicit name",
            package.id()
        ));
    }
    if parsed.metadata.name != package.name() {
        return Err(format!(
            "resolved package metadata name `{}` does not match descriptor name `{}`",
            parsed.metadata.name,
            package.name()
        ));
    }
    if parsed.metadata.description != package.description() {
        return Err(format!(
            "resolved package metadata description does not match descriptor `{}`",
            package.id()
        ));
    }
    if &parsed.instructions_range != package.instructions_range() {
        return Err(format!(
            "resolved package instruction range does not match its parsed source for `{}`",
            package.id()
        ));
    }
    if package.id() != selection.skill_id() {
        return Err(format!(
            "resolved package id `{}` does not match selection `{}`",
            package.id(),
            selection.skill_id()
        ));
    }
    if package.revision() != selection.expected_revision() {
        return Err(format!(
            "resolved package revision `{}` does not match selected revision `{}`",
            package.revision(),
            selection.expected_revision()
        ));
    }
    if package.format_version() == SKILL_PACKAGE_FORMAT_VERSION && !package.resources().is_empty() {
        return Err("package format v1 cannot expose sibling resources".to_string());
    }
    Ok(())
}

pub(super) fn source_diagnostic(
    source_id: &SkillSourceId,
    code: SkillDiagnosticCode,
    message: impl Into<String>,
) -> SkillDiagnostic {
    SkillDiagnostic::new(
        code,
        SkillDiagnosticSeverity::Error,
        message.into(),
        source_id.as_str().to_string(),
    )
}

pub(super) fn validate_unique_selections(
    selections: &[SkillSelection],
) -> Result<Vec<(usize, &SkillSelection)>, SkillActivationError> {
    let mut first_by_id = HashMap::with_capacity(selections.len());
    let mut unique = Vec::with_capacity(selections.len());
    for (index, selection) in selections.iter().enumerate() {
        match first_by_id.get(selection.skill_id()) {
            Some(&first_index) => {
                let first: &SkillSelection = &selections[first_index];
                return Err(SkillActivationError::DuplicateSelection {
                    skill_id: selection.skill_id().clone(),
                    first_index,
                    duplicate_index: index,
                    first_revision: first.expected_revision().clone(),
                    duplicate_revision: selection.expected_revision().clone(),
                });
            }
            None => {
                first_by_id.insert(selection.skill_id().clone(), index);
                unique.push((index, selection));
            }
        }
    }
    Ok(unique)
}
