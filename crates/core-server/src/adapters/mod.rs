pub(crate) mod agent_skill_installation;
pub(crate) mod git_dispatcher;
pub(crate) mod image_generation_dispatcher;
pub(crate) mod mcp_runtime;
pub(crate) mod skill_installation_workflow_adapter;
pub(crate) mod skill_source_resolution_adapter;
pub(crate) mod skills_adapter;
pub(crate) mod skills_dispatcher;

#[cfg(test)]
mod skills_installation_tests;
#[cfg(test)]
pub(crate) mod skills_test_support;
