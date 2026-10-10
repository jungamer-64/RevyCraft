use std::cmp::Ordering;

const MODULUS: u32 = 1 << 24;
const MASK: u32 = MODULUS - 1;
const HALF_RANGE: u32 = MODULUS / 2;

mod sealed {
    pub trait Sealed {}
}

/// The closed set of wrapping 24-bit sequence domains used by RakNet.
pub trait Sequence24: Copy + Eq + sealed::Sealed {
    /// Returns the successor in the same domain, wrapping at its upper boundary.
    fn next(self) -> Self;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
struct RunLength(u32);

impl RunLength {
    const SINGLE: Self = Self(1);

    fn grow(&mut self) -> bool {
        if self.0 == MODULUS {
            return false;
        }
        self.0 += 1;
        true
    }
}

impl<'de> serde::Deserialize<'de> for RunLength {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let count = <u32 as serde::Deserialize>::deserialize(deserializer)?;
        if count == 0 || count > MODULUS {
            return Err(serde::de::Error::custom(
                "sequence run length must fit one nonempty 24-bit cycle",
            ));
        }
        Ok(Self(count))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SequenceRun<S> {
    first: S,
    length: RunLength,
}

/// A receive-history checkpoint in arrival order. Consecutive sequence values share one run,
/// including across wraparound; gaps and reordered arrivals remain distinct runs.
///
/// Deserialization retains runs without expanding them. The receiving peer checks the complete
/// sequence count against its resource policy before building duplicate indexes or FIFO storage.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SequenceHistory<S> {
    runs: Vec<SequenceRun<S>>,
}

impl<S> Default for SequenceHistory<S> {
    fn default() -> Self {
        Self { runs: Vec::new() }
    }
}

impl<S: Sequence24> SequenceHistory<S> {
    /// Captures arrival order without sorting or discarding sequence values.
    #[must_use]
    pub fn from_sequences(sequences: impl IntoIterator<Item = S>) -> Self {
        let mut runs: Vec<SequenceRun<S>> = Vec::new();
        let mut previous: Option<S> = None;
        for sequence in sequences {
            if previous.is_some_and(|previous| previous.next() == sequence)
                && let Some(run) = runs.last_mut()
                && run.length.grow()
            {
                previous = Some(sequence);
                continue;
            }
            runs.push(SequenceRun {
                first: sequence,
                length: RunLength::SINGLE,
            });
            previous = Some(sequence);
        }
        Self { runs }
    }

    /// Counts the represented sequences before any expansion or index allocation.
    ///
    /// # Errors
    ///
    /// Returns [`crate::RakNetError::Wire`] if an untrusted checkpoint's aggregate count cannot
    /// be represented on the receiving target.
    pub fn sequence_count(&self) -> Result<usize, crate::RakNetError> {
        self.runs.iter().try_fold(0_usize, |count, run| {
            usize::try_from(run.length.0)
                .ok()
                .and_then(|length| count.checked_add(length))
                .ok_or_else(|| {
                    crate::RakNetError::Wire(
                        "sequence history count exceeds the target's count domain".to_string(),
                    )
                })
        })
    }

    /// Iterates in the captured arrival order. Resource admission must precede expansion of an
    /// untrusted checkpoint; this iterator performs no allocation.
    pub fn sequences(&self) -> impl Iterator<Item = S> + '_ {
        self.runs.iter().flat_map(|run| {
            (0..run.length.0).scan(run.first, |next, _| {
                let sequence = *next;
                *next = next.next();
                Some(sequence)
            })
        })
    }
}

macro_rules! sequence_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize)]
        pub struct $name(u32);

        impl $name {
            #[must_use]
            pub const fn new(value: u32) -> Self {
                Self(value & MASK)
            }

            #[must_use]
            pub const fn value(self) -> u32 {
                self.0
            }

            #[must_use]
            pub const fn next(self) -> Self {
                Self::new(self.0.wrapping_add(1))
            }

            #[must_use]
            pub const fn previous(self) -> Self {
                Self::new(self.0.wrapping_sub(1))
            }

            #[must_use]
            pub const fn wrapping_distance_from(self, older: Self) -> u32 {
                self.0.wrapping_sub(older.0) & MASK
            }

            #[must_use]
            pub const fn is_newer_than(self, other: Self) -> bool {
                let distance = self.wrapping_distance_from(other);
                distance != 0 && distance < HALF_RANGE
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                if self == other {
                    Some(Ordering::Equal)
                } else if self.is_newer_than(*other) {
                    Some(Ordering::Greater)
                } else {
                    Some(Ordering::Less)
                }
            }
        }

        impl sealed::Sealed for $name {}

        impl Sequence24 for $name {
            fn next(self) -> Self {
                $name::next(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <u32 as serde::Deserialize>::deserialize(deserializer)?;
                if value > MASK {
                    return Err(serde::de::Error::custom(
                        "24-bit sequence exceeds its wire domain",
                    ));
                }
                Ok(Self(value))
            }
        }
    };
}

sequence_type!(DatagramSequence);
sequence_type!(ReliableSequence);
sequence_type!(OrderSequence);

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn history_preserves_arrival_order_across_gaps_and_wraparound() -> Result<(), crate::RakNetError>
    {
        let arrivals = [MASK - 1, MASK, 0, 5, 4, 8, 9].map(DatagramSequence::new);
        let history = SequenceHistory::from_sequences(arrivals);
        assert_eq!(history.sequence_count()?, arrivals.len());
        assert_eq!(history.sequences().collect::<Vec<_>>(), arrivals);
        let repeated = SequenceHistory::from_sequences([ReliableSequence::new(7); 2]);
        assert_eq!(
            repeated.sequences().collect::<Vec<_>>(),
            [ReliableSequence::new(7); 2]
        );
        Ok(())
    }

    #[test]
    fn deserialization_checks_run_length_boundaries() -> Result<(), serde::de::value::Error> {
        for count in [0, MODULUS + 1] {
            let encoded = serde::de::value::U32Deserializer::<serde::de::value::Error>::new(count);
            assert!(RunLength::deserialize(encoded).is_err());
        }
        let encoded = serde::de::value::U32Deserializer::<serde::de::value::Error>::new(MODULUS);
        assert_eq!(RunLength::deserialize(encoded)?.0, MODULUS);
        Ok(())
    }

    #[test]
    fn sequence_wraparound_preserves_newer_relation() {
        let last = DatagramSequence::new(0x00ff_ffff);
        let wrapped = last.next();
        assert_eq!(wrapped.value(), 0);
        assert!(wrapped.is_newer_than(last));
        assert!(!last.is_newer_than(wrapped));
    }

    #[test]
    fn sequence_masks_values_to_twenty_four_bits() {
        assert_eq!(ReliableSequence::new(0xab12_3456).value(), 0x12_3456);
        assert_eq!(OrderSequence::new(MODULUS).value(), 0);
    }

    #[test]
    fn deserialization_rejects_values_outside_the_wire_domain() {
        let encoded = serde::de::value::U32Deserializer::<serde::de::value::Error>::new(1 << 24);
        assert!(DatagramSequence::deserialize(encoded).is_err());
    }
}
