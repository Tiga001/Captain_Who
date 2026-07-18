mod discovery;
mod model;
mod parser;
mod resolver;
mod workspace;

pub use discovery::SkillsService;
pub use model::{
    ResolvedSkill, SkillCatalog, SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode,
    SkillDiagnosticSeverity, SkillDiscoveryError, SkillProvenance, SkillResolveError,
    SkillResolveRequest, SkillScope,
};
