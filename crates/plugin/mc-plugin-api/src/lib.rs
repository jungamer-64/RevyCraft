#![allow(clippy::multiple_crate_versions)]
pub mod abi;
mod auth_codec;
mod gameplay_codec;
pub mod host_api;
pub mod manifest;
pub(crate) mod protocol_codec;
mod storage_codec;

pub mod codec {
    pub mod auth {
        pub use crate::auth_codec::*;
    }

    pub mod gameplay {
        pub use crate::gameplay_codec::*;
    }

    pub mod protocol {
        pub use crate::protocol_codec::*;
    }

    pub mod storage {
        pub use crate::storage_codec::*;
    }
}
