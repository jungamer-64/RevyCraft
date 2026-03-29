#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionConflict {
    pub expected: u64,
    pub actual: u64,
}

#[derive(Clone, Debug)]
pub struct Revisioned<T> {
    state: T,
    revision: u64,
}

impl<T> Revisioned<T> {
    #[must_use]
    pub fn new(state: T) -> Self {
        Self { state, revision: 0 }
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn state(&self) -> &T {
        &self.state
    }

    pub fn try_apply<R, F>(
        &mut self,
        expected_revision: u64,
        apply: F,
    ) -> Result<(u64, R), RevisionConflict>
    where
        F: FnOnce(&mut T) -> R,
    {
        self.try_apply_if(expected_revision, apply, |_| true)
    }

    pub fn try_apply_if<R, F, P>(
        &mut self,
        expected_revision: u64,
        apply: F,
        should_increment: P,
    ) -> Result<(u64, R), RevisionConflict>
    where
        F: FnOnce(&mut T) -> R,
        P: FnOnce(&R) -> bool,
    {
        if self.revision != expected_revision {
            return Err(RevisionConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        let result = apply(&mut self.state);
        if should_increment(&result) {
            self.revision = self.revision.saturating_add(1);
        }
        Ok((self.revision, result))
    }
}

impl<T> Revisioned<T>
where
    T: Clone,
{
    #[must_use]
    pub fn snapshot(&self) -> T {
        self.state.clone()
    }
}
