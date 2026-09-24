use crate::RuntimeError;
use crate::runtime::LOGIN_VERIFY_TOKEN_LEN;
use num_bigint::BigInt;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey};
use rsa::rand_core::{OsRng, RngCore};
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};

pub(crate) struct OnlineAuthKeys {
    pub(crate) private_key: RsaPrivateKey,
    pub(crate) public_key_der: Vec<u8>,
}

impl OnlineAuthKeys {
    pub(in crate::runtime) fn generate() -> Result<Self, RuntimeError> {
        let mut rng = OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 1024).map_err(|error| {
            RuntimeError::Auth(format!("failed to generate RSA keypair: {error}"))
        })?;
        let public_key_der = RsaPublicKey::from(&private_key)
            .to_public_key_der()
            .map_err(|error| {
                RuntimeError::Auth(format!("failed to encode RSA public key: {error}"))
            })?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private_key,
            public_key_der,
        })
    }

    pub(in crate::runtime) fn export_process_transfer(&self) -> Result<Vec<u8>, RuntimeError> {
        self.private_key
            .to_pkcs8_der()
            .map(|document| document.as_bytes().to_vec())
            .map_err(|error| {
                RuntimeError::Auth(format!("failed to encode RSA private key: {error}"))
            })
    }

    pub(in crate::runtime) fn import_process_transfer(bytes: &[u8]) -> Result<Self, RuntimeError> {
        let private_key = RsaPrivateKey::from_pkcs8_der(bytes).map_err(|error| {
            RuntimeError::Auth(format!(
                "failed to decode transferred RSA private key: {error}"
            ))
        })?;
        let public_key_der = RsaPublicKey::from(&private_key)
            .to_public_key_der()
            .map_err(|error| {
                RuntimeError::Auth(format!("failed to encode RSA public key: {error}"))
            })?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private_key,
            public_key_der,
        })
    }
}

pub(super) fn random_verify_token() -> [u8; LOGIN_VERIFY_TOKEN_LEN] {
    let mut verify_token = [0_u8; LOGIN_VERIFY_TOKEN_LEN];
    OsRng.fill_bytes(&mut verify_token);
    verify_token
}

pub(super) fn decrypt_login_blob(
    private_key: &RsaPrivateKey,
    bytes: &[u8],
) -> Result<Vec<u8>, RuntimeError> {
    private_key
        .decrypt(Pkcs1v15Encrypt, bytes)
        .map_err(|error| RuntimeError::Auth(format!("failed to decrypt login blob: {error}")))
}

pub(super) fn minecraft_server_hash(
    server_id: &str,
    shared_secret: &[u8; 16],
    public_key_der: &[u8],
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(server_id.as_bytes());
    hasher.update(shared_secret);
    hasher.update(public_key_der);
    let digest = hasher.finalize();
    BigInt::from_signed_bytes_be(&digest).to_str_radix(16)
}
