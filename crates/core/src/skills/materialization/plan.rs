use super::*;

#[derive(Debug)]
pub(super) struct PreparedTemplateFile {
    pub(super) source: SkillResourceUri,
    pub(super) relative_path: String,
    pub(super) descriptor: SkillResourceDescriptor,
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct PreparedTemplateTree {
    pub(super) files: Vec<PreparedTemplateFile>,
    pub(super) fingerprint: TreeFingerprint,
    pub(super) byte_length: u64,
    pub(super) plan_digest: String,
}

pub(super) fn prepare_template_tree(
    session: &SkillResourceSession,
    request: &SkillTemplateTreeMaterializationRequest,
) -> Result<PreparedTemplateTree, SkillMaterializationError> {
    let prefix = request.source_prefix().as_str();
    let descendant_prefix = format!("{prefix}/");
    let mut after = None;
    let mut listed = BTreeMap::<String, (SkillResourceUri, SkillResourceDescriptor)>::new();

    loop {
        let mut options = SkillResourceListOptions::new(MAX_SKILL_RESOURCE_LIST_PAGE_SIZE)?
            .with_prefix(request.source_prefix().clone());
        if let Some(cursor) = after.take() {
            options = options.with_after(cursor);
        }
        let page = session.list(request.source(), &options)?;
        for entry in page.entries() {
            let Some(relative_path) = entry.descriptor().path().strip_prefix(&descendant_prefix)
            else {
                // `list` intentionally includes an exact prefix match. A tree
                // publication has no file location for that match, so only
                // strict descendants participate.
                continue;
            };
            let relative = SkillPackagePath::parse(relative_path.to_string()).map_err(|error| {
                SkillMaterializationError::Resource {
                    source: Box::new(SkillResourceError::IntegrityMismatch {
                        uri: Box::new(entry.uri().clone()),
                        reason: format!(
                            "template subtree contains an invalid relative path: {}",
                            error.message
                        ),
                    }),
                }
            })?;
            if listed.len() >= MAX_SKILL_MATERIALIZATION_TREE_FILES {
                return Err(SkillMaterializationError::ResourceBudgetExceeded {
                    reason: format!(
                        "template subtree contains more than {MAX_SKILL_MATERIALIZATION_TREE_FILES} files"
                    ),
                });
            }
            if listed
                .insert(
                    relative.as_str().to_string(),
                    (entry.uri().clone(), entry.descriptor().clone()),
                )
                .is_some()
            {
                return Err(SkillMaterializationError::Resource {
                    source: Box::new(SkillResourceError::IntegrityMismatch {
                        uri: Box::new(entry.uri().clone()),
                        reason: "template subtree contains a duplicate relative path".to_string(),
                    }),
                });
            }
        }
        after = page.next_after().cloned();
        if after.is_none() {
            break;
        }
    }

    if listed.is_empty() {
        return Err(SkillMaterializationError::EmptySource {
            package: request.source().clone(),
            prefix: request.source_prefix().clone(),
        });
    }

    let mut files = Vec::with_capacity(listed.len());
    let mut directories = BTreeSet::new();
    let mut fingerprints = BTreeMap::new();
    let mut byte_length = 0u64;
    for (relative_path, (uri, listed_descriptor)) in listed {
        for (index, _) in relative_path.match_indices('/') {
            directories.insert(relative_path[..index].to_string());
        }
        if directories.len() > MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES {
            return Err(SkillMaterializationError::ResourceBudgetExceeded {
                reason: format!(
                    "template subtree contains more than {MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES} directories"
                ),
            });
        }

        byte_length = byte_length
            .checked_add(listed_descriptor.byte_length())
            .ok_or_else(|| SkillMaterializationError::ResourceBudgetExceeded {
                reason: "template subtree byte length overflowed".to_string(),
            })?;
        if byte_length > u64::try_from(MAX_SKILL_MATERIALIZATION_TREE_BYTES).unwrap_or(u64::MAX) {
            return Err(SkillMaterializationError::ResourceBudgetExceeded {
                reason: format!(
                    "template subtree exceeds {MAX_SKILL_MATERIALIZATION_TREE_BYTES} bytes"
                ),
            });
        }

        let snapshot = session.read_verified_bytes(&uri)?;
        if snapshot.descriptor != listed_descriptor {
            return Err(SkillMaterializationError::Resource {
                source: Box::new(SkillResourceError::IntegrityMismatch {
                    uri: Box::new(uri),
                    reason:
                        "resource descriptor changed while the materialization plan was prepared"
                            .to_string(),
                }),
            });
        }
        fingerprints.insert(
            relative_path.clone(),
            TreeFileFingerprint {
                byte_length: snapshot.descriptor.byte_length(),
                content_digest: snapshot.descriptor.content_digest().to_string(),
            },
        );
        files.push(PreparedTemplateFile {
            source: uri,
            relative_path,
            descriptor: snapshot.descriptor,
            bytes: snapshot.bytes,
        });
    }

    let fingerprint = TreeFingerprint {
        directories,
        files: fingerprints,
    };
    let plan_digest = fingerprint.digest();
    Ok(PreparedTemplateTree {
        files,
        fingerprint,
        byte_length,
        plan_digest,
    })
}
