pub(crate) mod permissions;
pub(crate) mod schedule;
mod service;

pub(crate) use service::{automation_event_dto, AutomationService, AutomationServiceError};
