use super::{
    Arc, AuthGeneration, CapabilitySet, GameplayGeneration, PluginGenerationId, RwLock,
    StorageGeneration,
};

pub(crate) trait ProfileGenerationMetadata {
    fn capabilities(&self) -> &CapabilitySet;

    fn generation_id(&self) -> PluginGenerationId;
}

pub(crate) struct GenerationSlot<G> {
    generation: RwLock<Arc<G>>,
    generation_poison_message: &'static str,
}

impl<G> GenerationSlot<G> {
    pub(crate) const fn new(generation: Arc<G>, generation_poison_message: &'static str) -> Self {
        Self {
            generation: RwLock::new(generation),
            generation_poison_message,
        }
    }

    pub(crate) fn current(&self) -> Arc<G> {
        self.generation
            .read()
            .expect(self.generation_poison_message)
            .clone()
    }

    pub(crate) fn swap(&self, generation: Arc<G>) {
        *self
            .generation
            .write()
            .expect(self.generation_poison_message) = generation;
    }
}

impl<G: ProfileGenerationMetadata> GenerationSlot<G> {
    pub(crate) fn capability_set(&self) -> CapabilitySet {
        self.current().capabilities().clone()
    }

    pub(crate) fn generation_id(&self) -> PluginGenerationId {
        self.current().generation_id()
    }
}

pub(crate) struct ReloadableGenerationSlot<G> {
    generation: GenerationSlot<G>,
    reload_gate: RwLock<()>,
    reload_gate_poison_message: &'static str,
}

impl<G> ReloadableGenerationSlot<G> {
    pub(crate) const fn new(
        generation: Arc<G>,
        generation_poison_message: &'static str,
        reload_gate_poison_message: &'static str,
    ) -> Self {
        Self {
            generation: GenerationSlot::new(generation, generation_poison_message),
            reload_gate: RwLock::new(()),
            reload_gate_poison_message,
        }
    }

    pub(crate) fn current(&self) -> Arc<G> {
        self.generation.current()
    }

    pub(crate) fn swap(&self, generation: Arc<G>) {
        let _guard = self
            .reload_gate
            .write()
            .expect(self.reload_gate_poison_message);
        self.generation.swap(generation);
    }

    pub(crate) fn swap_while_reloading(&self, generation: Arc<G>) {
        self.generation.swap(generation);
    }

    pub(crate) fn with_reload_read<R>(&self, f: impl FnOnce(Arc<G>) -> R) -> R {
        let _guard = self
            .reload_gate
            .read()
            .expect(self.reload_gate_poison_message);
        f(self.generation.current())
    }

    pub(crate) fn with_reload_write<R>(&self, f: impl FnOnce(Arc<G>) -> R) -> R {
        let _guard = self
            .reload_gate
            .write()
            .expect(self.reload_gate_poison_message);
        f(self.generation.current())
    }
}

impl<G: ProfileGenerationMetadata> ReloadableGenerationSlot<G> {
    pub(crate) fn capability_set(&self) -> CapabilitySet {
        self.generation.capability_set()
    }

    pub(crate) fn generation_id(&self) -> PluginGenerationId {
        self.generation.generation_id()
    }
}

impl ProfileGenerationMetadata for AuthGeneration {
    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    fn generation_id(&self) -> PluginGenerationId {
        self.generation_id
    }
}

impl ProfileGenerationMetadata for GameplayGeneration {
    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    fn generation_id(&self) -> PluginGenerationId {
        self.generation_id
    }
}

impl ProfileGenerationMetadata for StorageGeneration {
    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    fn generation_id(&self) -> PluginGenerationId {
        self.generation_id
    }
}

#[cfg(test)]
mod tests {
    use super::ReloadableGenerationSlot;
    use super::{
        Arc, CapabilitySet, GenerationSlot, PluginGenerationId, ProfileGenerationMetadata,
    };

    #[derive(Clone)]
    struct TestGeneration {
        capabilities: CapabilitySet,
        generation_id: PluginGenerationId,
    }

    impl ProfileGenerationMetadata for TestGeneration {
        fn capabilities(&self) -> &CapabilitySet {
            &self.capabilities
        }

        fn generation_id(&self) -> PluginGenerationId {
            self.generation_id
        }
    }

    fn test_generation(generation_id: u64, capability: &'static str) -> Arc<TestGeneration> {
        let mut capabilities = CapabilitySet::new();
        let _ = capabilities.insert(capability);
        Arc::new(TestGeneration {
            capabilities,
            generation_id: PluginGenerationId(generation_id),
        })
    }

    #[test]
    fn generation_slot_reports_current_metadata() {
        let generation = test_generation(3, "cap.alpha");
        let slot = GenerationSlot::new(
            Arc::clone(&generation),
            "test generation lock should not be poisoned",
        );

        assert!(Arc::ptr_eq(&slot.current(), &generation));
        assert_eq!(slot.generation_id(), PluginGenerationId(3));
        assert!(slot.capability_set().contains("cap.alpha"));
    }

    #[test]
    fn generation_slot_swap_replaces_generation() {
        let first = test_generation(1, "cap.first");
        let second = test_generation(2, "cap.second");
        let slot = GenerationSlot::new(first, "test generation lock should not be poisoned");

        slot.swap(Arc::clone(&second));

        assert!(Arc::ptr_eq(&slot.current(), &second));
        assert_eq!(slot.generation_id(), PluginGenerationId(2));
        assert!(slot.capability_set().contains("cap.second"));
        assert!(!slot.capability_set().contains("cap.first"));
    }

    #[test]
    fn reloadable_generation_slot_uses_latest_generation_for_reads_and_swaps() {
        let first = test_generation(7, "cap.read");
        let second = test_generation(8, "cap.swap");
        let slot = ReloadableGenerationSlot::new(
            first,
            "test generation lock should not be poisoned",
            "test reload gate should not be poisoned",
        );

        let before = slot.with_reload_read(|generation| generation.generation_id());
        slot.swap_while_reloading(Arc::clone(&second));
        let after = slot.with_reload_write(|generation| generation.generation_id());

        assert_eq!(before, PluginGenerationId(7));
        assert_eq!(after, PluginGenerationId(8));
        assert_eq!(slot.generation_id(), PluginGenerationId(8));
        assert!(slot.capability_set().contains("cap.swap"));
    }
}
