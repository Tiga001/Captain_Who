mod discovery;
mod model;
mod parser;

pub use discovery::SkillsService;
pub use model::{
    SkillCatalog, SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillScope,
};
