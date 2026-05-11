use async_trait::async_trait;
use gaze_mcp_core::{AuthError, AuthHook, Principal};

/// v0.3.0 keeps authorization at the existing profile-as-scope layer.
///
/// Transports still pass a stable principal into `PiiEnvelope`; this hook only
/// acknowledges agent-tier calls so the new chokepoint can enforce ordering
/// without changing the v1 authentication posture.
pub struct LensAuthHook;

#[async_trait]
impl AuthHook for LensAuthHook {
    async fn authorize_agent(
        &self,
        _principal: &Principal,
        _tool_name: &str,
    ) -> Result<(), AuthError> {
        Ok(())
    }

    async fn authorize_operator(
        &self,
        _principal: &Principal,
        _tool_name: &str,
    ) -> Result<(), AuthError> {
        Err(AuthError::Denied(
            "operator-tier tools are not part of gaze-lens v1".to_string(),
        ))
    }
}
