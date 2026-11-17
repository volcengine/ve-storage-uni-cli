pub(crate) mod runtime;
use crate::platform::TccError;

/// Expected runtime, explicitly supplied by the trusted host bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedEnvironment {
    /// BOE runtime, excluding PPE and testing.
    Boe,
    /// Production runtime, excluding PPE and testing.
    Production,
}

/// Validated dedicated TCC namespace and configuration space.
pub struct Locator {
    pub(crate) namespace: String,
    pub(crate) space: String,
    pub(crate) environment: ExpectedEnvironment,
}

impl Locator {
    /// Validate namespace (<=256 bytes) and space (<=64 bytes) without I/O.
    /// Returns a locator or `InvalidLocator`; accepts only safe ASCII segments.
    pub fn new(
        namespace: &str,
        space: &str,
        environment: ExpectedEnvironment,
    ) -> Result<Self, TccError> {
        // [Review Fix #59] Reject path/query/control syntax before constructing SDK resource keys.
        if !is_namespace(namespace) || space.len() > 64 || !is_segment(space) {
            return Err(TccError::InvalidLocator);
        }
        Ok(Self {
            namespace: namespace.into(),
            space: space.into(),
            environment,
        })
    }
}

pub(crate) fn is_namespace(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.split('.').all(is_segment)
}

fn is_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
