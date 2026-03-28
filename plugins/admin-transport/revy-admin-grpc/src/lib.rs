#![allow(clippy::multiple_crate_versions)]

pub mod admin {
    tonic::include_proto!("revy.admin.v1");
}
