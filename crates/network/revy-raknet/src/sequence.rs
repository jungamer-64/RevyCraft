use std::cmp::Ordering;

const MODULUS: u32 = 1 << 24;
const MASK: u32 = MODULUS - 1;
const HALF_RANGE: u32 = MODULUS / 2;

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
