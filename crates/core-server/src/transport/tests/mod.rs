use super::*;
use crate::adapters::skills_test_support::write_installed_skill;
use mycopilot_core::skills::{
    GitHubReference, PreparedSkillAcquisition, PreparedSkillPackage, PreparedSkillSourceResolution,
    PreparedSkillSourceResolutionCandidate, ResolvedSkillPackagePreview, ResolvedSkillSource,
    SkillInstallationAuthority, SkillInstallationProvenance, SkillInstallationRefresh,
    SkillInstallationSourceLocator, SkillInstallationSourceResolver, SkillPackageOrigin,
    SkillSourceCandidateId, SkillSourceResolution, SkillSourceResolutionCandidate,
    SkillSourceResolutionError, SkillSourceResolverId, GITHUB_SKILL_ORIGIN_PROVIDER,
};
use std::fs;
use std::sync::{mpsc as std_mpsc, Mutex};
use tokio::io::AsyncReadExt;

mod bootstrap;
mod collaboration_authorization;
mod collaboration_settings;
mod conversation_fork;
mod human_interaction;
mod manual_context_compaction;
mod prompt_preferences;
mod provider_profiles;
mod request_loop;
mod skills_catalog;
mod source_resolution;

struct StaticGitHubSourceResolver;

impl SkillInstallationSourceResolver for StaticGitHubSourceResolver {
    fn id(&self) -> SkillSourceResolverId {
        SkillSourceResolverId::parse("github").unwrap()
    }

    fn supported_hosts(&self) -> Vec<String> {
        vec!["github.com".to_string()]
    }

    fn resolve(
        &self,
        _locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        SkillSourceResolution::new(
            "https://github.com/example/skills/tree/main/skills/auditor",
            self.id(),
            commit,
            vec![SkillSourceResolutionCandidate::new(
                "candidate-auditor",
                ResolvedSkillSource::GitHub {
                    owner: "example".to_string(),
                    repository: "skills".to_string(),
                    tracking_reference: GitHubReference::named("main").unwrap(),
                    resolved_commit: commit.to_string(),
                    subdirectory: Some("skills/auditor".to_string()),
                },
                ResolvedSkillPackagePreview::new(
                    1,
                    SkillRevision::parse(format!("sha256:{}", "a".repeat(64))).unwrap(),
                    "auditor",
                    "Audit a repository.",
                    1,
                    128,
                ),
            )],
        )
    }

    fn resolve_prepared(
        &self,
        _locator: &SkillInstallationSourceLocator,
    ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let package = PreparedSkillPackage::from_bytes(
            concat!(
                "---\n",
                "name: auditor\n",
                "description: Audit a repository.\n",
                "---\n",
                "# Instructions\n",
                "Audit the repository.\n"
            )
            .as_bytes()
            .to_vec(),
            SkillPackageOrigin::new(
                GITHUB_SKILL_ORIGIN_PROVIDER,
                serde_json::json!({
                    "schemaVersion": 1,
                    "owner": "example",
                    "repository": "skills",
                    "requestedReference": { "kind": "named", "value": "main" },
                    "resolvedCommit": commit,
                    "subdirectory": "skills/auditor"
                })
                .to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        let provenance = SkillInstallationProvenance::new(
            SkillInstallationAuthority::new(
                GITHUB_SKILL_ORIGIN_PROVIDER,
                1,
                serde_json::json!({
                    "owner": "example",
                    "repository": "skills",
                    "resolvedCommit": commit,
                    "subdirectory": "skills/auditor"
                })
                .to_string(),
            )
            .unwrap(),
            Some(
                SkillInstallationRefresh::new(
                    GITHUB_SKILL_ORIGIN_PROVIDER,
                    1,
                    serde_json::json!({
                        "owner": "example",
                        "repository": "skills",
                        "reference": { "kind": "named", "value": "main" },
                        "subdirectory": "skills/auditor"
                    })
                    .to_string(),
                )
                .unwrap(),
            ),
        );
        PreparedSkillSourceResolution::new(
            "https://github.com/example/skills/tree/main/skills/auditor",
            self.id(),
            commit,
            vec![PreparedSkillSourceResolutionCandidate::new(
                SkillSourceCandidateId::parse("candidate-auditor").unwrap(),
                ResolvedSkillSource::GitHub {
                    owner: "example".to_string(),
                    repository: "skills".to_string(),
                    tracking_reference: GitHubReference::named("main").unwrap(),
                    resolved_commit: commit.to_string(),
                    subdirectory: Some("skills/auditor".to_string()),
                },
                PreparedSkillAcquisition::new(package, provenance),
            )],
        )
    }
}

struct BlockingGitHubSourceResolver {
    started: std_mpsc::Sender<()>,
    release: Mutex<std_mpsc::Receiver<()>>,
}

impl SkillInstallationSourceResolver for BlockingGitHubSourceResolver {
    fn id(&self) -> SkillSourceResolverId {
        StaticGitHubSourceResolver.id()
    }

    fn supported_hosts(&self) -> Vec<String> {
        StaticGitHubSourceResolver.supported_hosts()
    }

    fn resolve(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<SkillSourceResolution, SkillSourceResolutionError> {
        StaticGitHubSourceResolver.resolve(locator)
    }

    fn resolve_prepared(
        &self,
        locator: &SkillInstallationSourceLocator,
    ) -> Result<PreparedSkillSourceResolution, SkillSourceResolutionError> {
        self.started.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        StaticGitHubSourceResolver.resolve_prepared(locator)
    }
}
