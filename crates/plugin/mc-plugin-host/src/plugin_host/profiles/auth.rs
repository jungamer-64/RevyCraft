use super::{
    Arc, AuthGeneration, AuthGenerationHandle, AuthMode, BedrockAuthResult, CapabilitySet,
    PlayerId, PluginFailureDispatch, PluginGenerationId, PluginKind, RuntimeError, RwLock,
};

pub(crate) struct HotSwappableAuthProfile {
    plugin_id: String,
    pub(crate) generation: RwLock<Arc<AuthGeneration>>,
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
            generation: RwLock::new(generation),
            failures,
        }
    }

    fn current_generation(&self) -> Result<Arc<AuthGeneration>, String> {
        Ok(self
            .generation
            .read()
            .expect("auth generation lock should not be poisoned")
            .clone())
    }

    pub(crate) fn swap_generation(&self, generation: Arc<AuthGeneration>) {
        *self
            .generation
            .write()
            .expect("auth generation lock should not be poisoned") = generation;
    }

    fn capability_set(&self) -> CapabilitySet {
        self.current_generation()
            .map(|generation| generation.capabilities.clone())
            .unwrap_or_default()
    }

    fn plugin_generation_id(&self) -> Option<PluginGenerationId> {
        self.current_generation()
            .ok()
            .map(|generation| generation.generation_id)
    }

    fn mode(&self) -> Result<AuthMode, RuntimeError> {
        self.current_generation()
            .map(|generation| generation.mode())
            .map_err(RuntimeError::Config)
    }

    fn capture_generation(&self) -> Result<Arc<AuthGeneration>, RuntimeError> {
        self.current_generation().map_err(RuntimeError::Config)
    }

    fn authenticate_offline(&self, username: &str) -> Result<PlayerId, RuntimeError> {
        match self.capture_generation()?.authenticate_offline(username) {
            Ok(player_id) => Ok(player_id),
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

    fn authenticate_online(
        &self,
        username: &str,
        server_hash: &str,
    ) -> Result<PlayerId, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_online(username, server_hash)
        {
            Ok(player_id) => Ok(player_id),
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

    fn authenticate_bedrock_offline(
        &self,
        display_name: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_bedrock_offline(display_name)
        {
            Ok(result) => Ok(result),
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

    fn authenticate_bedrock_xbl(
        &self,
        chain_jwts: &[String],
        client_data_jwt: &str,
    ) -> Result<BedrockAuthResult, RuntimeError> {
        match self
            .capture_generation()?
            .authenticate_bedrock_xbl(chain_jwts, client_data_jwt)
        {
            Ok(result) => Ok(result),
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
