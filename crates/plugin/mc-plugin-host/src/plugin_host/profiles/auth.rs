use super::{
    Arc, AuthGeneration, AuthGenerationHandle, AuthMode, BedrockAuthResult, CapabilitySet,
    GenerationSlot, PlayerId, PluginFailureDispatch, PluginGenerationId, PluginKind, RuntimeError,
};

pub(crate) struct HotSwappableAuthProfile {
    plugin_id: String,
    generation: GenerationSlot<AuthGeneration>,
    failures: Arc<PluginFailureDispatch>,
}

impl HotSwappableAuthProfile {
    pub(crate) const fn new(
        plugin_id: String,
        generation: Arc<AuthGeneration>,
        failures: Arc<PluginFailureDispatch>,
    ) -> Self {
        Self {
            plugin_id,
            generation: GenerationSlot::new(
                generation,
                "auth generation lock should not be poisoned",
            ),
            failures,
        }
    }

    pub(crate) fn current_generation(&self) -> Arc<AuthGeneration> {
        self.generation.current()
    }

    pub(crate) fn swap_generation(&self, generation: Arc<AuthGeneration>) {
        self.generation.swap(generation);
    }

    fn capability_set(&self) -> CapabilitySet {
        self.generation.capability_set()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Some(self.generation.generation_id())
    }

    fn mode(&self) -> Result<AuthMode, RuntimeError> {
        Ok(self.current_generation().mode())
    }

    fn capture_generation(&self) -> Result<Arc<AuthGeneration>, RuntimeError> {
        Ok(self.current_generation())
    }

    fn handle_auth_result<T>(&self, result: Result<T, RuntimeError>) -> Result<T, RuntimeError> {
        match result {
            Ok(value) => Ok(value),
            Err(RuntimeError::Config(message)) => {
                let _ = self.failures.handle_runtime_failure(
                    PluginKind::Auth,
                    &self.plugin_id,
                    &message,
                );
                Err(RuntimeError::Config(message))
            }
            Err(error) => Err(error),
        }
    }

    fn with_captured_generation<T>(
        &self,
        f: impl FnOnce(&AuthGeneration) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let generation = self.capture_generation()?;
        self.handle_auth_result(f(&generation))
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        self.with_captured_generation(|generation| generation.authenticate_offline(username))
    }

    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        self.with_captured_generation(|generation| {
            generation.authenticate_online(username, server_hash)
        })
    }

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        self.with_captured_generation(|generation| {
            generation.authenticate_bedrock_offline(display_name)
        })
    }

    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        self.with_captured_generation(|generation| {
            generation.authenticate_bedrock_xbl(chain_jwts, client_data_jwt)
        })
    }
}

impl super::super::AuthProfileHandle for HotSwappableAuthProfile {
    fn capability_set(&self) -> CapabilitySet {
        Self::capability_set(self)
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        Self::plugin_generation_id(self)
    }

    fn mode(&self) -> Result<AuthMode, RuntimeError> {
        Self::mode(self)
    }

    fn capture_generation(&self) -> Result<Arc<dyn AuthGenerationHandle>, RuntimeError> {
        Self::capture_generation(self).map(|generation| generation as Arc<dyn AuthGenerationHandle>)
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        Self::authenticate_offline(self, username)
    }

    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        Self::authenticate_online(self, username, server_hash)
    }

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        Self::authenticate_bedrock_offline(self, display_name)
    }

    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        Self::authenticate_bedrock_xbl(self, chain_jwts, client_data_jwt)
    }
}
