use serde::{Deserialize, Serialize};

pub const CORE_SET_EXECUTION_ACCESS_METHOD: &str = "core.setExecutionAccess";
pub const EXECUTION_ACCESS_MAX_TTL_MS: i64 = 60_000;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAccessReason {
    Allowed,
    AccountSignedOut,
    LicenseRequired,
    LicenseUnavailable,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetExecutionAccessInput {
    pub revision: u64,
    pub identity_epoch: u64,
    pub reason: ExecutionAccessReason,
    pub issued_at: i64,
    pub valid_until: i64,
}

impl SetExecutionAccessInput {
    pub fn validate(&self) -> Result<(), &'static str> {
        const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
        if self.revision == 0
            || self.revision > MAX_SAFE_INTEGER
            || self.identity_epoch > MAX_SAFE_INTEGER
            || self.issued_at < 0
            || self.valid_until < 0
            || self.issued_at as u64 > MAX_SAFE_INTEGER
            || self.valid_until as u64 > MAX_SAFE_INTEGER
        {
            return Err("Invalid execution access snapshot identity or timestamp");
        }
        let ttl = self.valid_until - self.issued_at;
        match self.reason {
            ExecutionAccessReason::Allowed if !(1..=EXECUTION_ACCESS_MAX_TTL_MS).contains(&ttl) => {
                Err("Allowed execution access must expire within 60000 milliseconds")
            }
            ExecutionAccessReason::Allowed => Ok(()),
            _ if ttl != 0 => Err("Denied execution access must not carry a validity window"),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetExecutionAccessOutput {
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> SetExecutionAccessInput {
        SetExecutionAccessInput {
            revision: 1,
            identity_epoch: 0,
            reason: ExecutionAccessReason::Allowed,
            issued_at: 1_000,
            valid_until: 61_000,
        }
    }

    #[test]
    fn execution_access_contract_uses_only_finite_host_snapshot_fields() {
        let input = allowed();
        assert_eq!(input.validate(), Ok(()));
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            serde_json::json!({
                "revision": 1, "identityEpoch": 0, "reason": "allowed", "issuedAt": 1000, "validUntil": 61000
            })
        );
        let mut json = serde_json::to_value(input).unwrap();
        json["userId"] = "not-part-of-this-protocol".into();
        assert!(serde_json::from_value::<SetExecutionAccessInput>(json).is_err());
    }

    #[test]
    fn execution_access_contract_rejects_invalid_ttl_and_denial_windows() {
        for ttl in [-1, 0, 60_001] {
            let mut input = allowed();
            input.valid_until = input.issued_at + ttl;
            assert!(input.validate().is_err());
        }
        for reason in [
            ExecutionAccessReason::AccountSignedOut,
            ExecutionAccessReason::LicenseRequired,
            ExecutionAccessReason::LicenseUnavailable,
        ] {
            let mut input = allowed();
            input.reason = reason;
            assert!(input.validate().is_err());
            input.valid_until = input.issued_at;
            assert_eq!(input.validate(), Ok(()));
        }
    }

    #[test]
    fn execution_access_contract_rejects_zero_revision_and_unsafe_integers() {
        let mut input = allowed();
        input.revision = 0;
        assert!(input.validate().is_err());
        input = allowed();
        input.identity_epoch = 9_007_199_254_740_992;
        assert!(input.validate().is_err());
        input = allowed();
        input.issued_at = -1;
        assert!(input.validate().is_err());
        input = allowed();
        input.issued_at = i64::MAX;
        input.valid_until = i64::MAX;
        assert!(input.validate().is_err());
    }
}
