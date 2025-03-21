use core::fmt;
use std::hash::Hash;

use alloy::primitives::B256;
use derive_more::derive::{Deref, From, Into};
use k256::{
    ecdsa::{Signature as EcdsaSignatureInner, VerifyingKey as EcdsaPublicKeyInner},
    elliptic_curve::generic_array::GenericArray,
};
use serde::{Deserialize, Serialize};
use serde_utils::hex;
use ssz_types::{
    typenum::{U33, U64},
    FixedVector,
};
use tree_hash::TreeHash;

use crate::{
    constants::COMMIT_BOOST_DOMAIN,
    signature::{compute_domain, compute_signing_root},
    types::Chain,
};

pub type EcdsaSecretKey = k256::ecdsa::SigningKey;

type CompressedPublicKey = [u8; 33];

#[derive(Debug, Clone, Copy, From, Into, Serialize, Deserialize, PartialEq, Eq, Deref, Hash)]
#[serde(transparent)]
pub struct EcdsaPublicKey {
    #[serde(with = "alloy::hex::serde")]
    encoded: CompressedPublicKey,
}

impl EcdsaPublicKey {
    /// Size of the public key in bytes. We store the SEC1 encoded affine point
    /// compressed, thus 33 bytes.
    const SIZE: usize = 33;
}

impl Default for EcdsaPublicKey {
    fn default() -> Self {
        Self { encoded: [0; Self::SIZE] }
    }
}

impl TreeHash for EcdsaPublicKey {
    fn tree_hash_type() -> tree_hash::TreeHashType {
        tree_hash::TreeHashType::Vector
    }

    fn tree_hash_packed_encoding(&self) -> tree_hash::PackedEncoding {
        unreachable!("Vector should never be packed.")
    }

    fn tree_hash_packing_factor() -> usize {
        unreachable!("Vector should never be packed.")
    }

    fn tree_hash_root(&self) -> tree_hash::Hash256 {
        // NOTE:
        // Unnecessary copying into a `FixedVector` just for its `tree_hash_root`
        // implementation.  If this becomes a performance issue, we could use
        // `ssz_types::tree_hash::vec_tree_hash_root`,  which is unfortunately
        // not public.
        let vec = self.encoded.to_vec();
        FixedVector::<u8, U33>::from(vec).tree_hash_root()
    }
}

impl From<EcdsaPublicKeyInner> for EcdsaPublicKey {
    fn from(value: EcdsaPublicKeyInner) -> Self {
        let encoded: [u8; Self::SIZE] = value.to_encoded_point(true).as_bytes().try_into().unwrap();

        EcdsaPublicKey { encoded }
    }
}

impl AsRef<[u8]> for EcdsaPublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.encoded
    }
}

impl fmt::LowerHex for EcdsaPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.as_ref()))?;
        Ok(())
    }
}

impl fmt::Display for EcdsaPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:x}")
    }
}

#[derive(Clone, Deref, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EcdsaSignature {
    #[serde(with = "alloy::hex::serde")]
    encoded: [u8; 64],
}

impl Default for EcdsaSignature {
    fn default() -> Self {
        Self { encoded: [0; 64] }
    }
}

impl From<EcdsaSignatureInner> for EcdsaSignature {
    fn from(value: EcdsaSignatureInner) -> Self {
        Self { encoded: value.to_bytes().as_slice().try_into().unwrap() }
    }
}

impl AsRef<[u8]> for EcdsaSignature {
    fn as_ref(&self) -> &[u8] {
        &self.encoded
    }
}

impl TryFrom<&[u8]> for EcdsaSignature {
    type Error = k256::ecdsa::Error;

    fn try_from(value: &[u8]) -> std::result::Result<Self, Self::Error> {
        Ok(EcdsaSignatureInner::from_slice(value)?.into())
    }
}

impl fmt::LowerHex for EcdsaSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.as_ref()))?;
        Ok(())
    }
}

impl fmt::Display for EcdsaSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:x}")
    }
}

// SIGNER
#[derive(Clone)]
pub enum EcdsaSigner {
    Local(EcdsaSecretKey),
}

impl EcdsaSigner {
    pub fn new_random() -> Self {
        let secret = B256::random();
        Self::new_from_bytes(secret.as_slice()).unwrap()
    }

    pub fn new_from_bytes(bytes: &[u8]) -> eyre::Result<Self> {
        let secret = EcdsaSecretKey::from_slice(bytes)?;
        Ok(Self::Local(secret))
    }

    pub fn pubkey(&self) -> EcdsaPublicKey {
        match self {
            EcdsaSigner::Local(secret) => EcdsaPublicKeyInner::from(secret).into(),
        }
    }

    pub fn secret(&self) -> Vec<u8> {
        match self {
            EcdsaSigner::Local(secret) => secret.to_bytes().to_vec(),
        }
    }

    pub async fn sign(&self, chain: Chain, object_root: [u8; 32]) -> EcdsaSignature {
        match self {
            EcdsaSigner::Local(sk) => {
                let domain = compute_domain(chain, COMMIT_BOOST_DOMAIN);
                let signing_root = compute_signing_root(object_root, domain);
                k256::ecdsa::signature::Signer::<EcdsaSignatureInner>::sign(sk, &signing_root)
                    .into()
            }
        }
    }

    pub async fn sign_msg(&self, chain: Chain, msg: &impl TreeHash) -> EcdsaSignature {
        self.sign(chain, msg.tree_hash_root().0).await
    }
}

pub fn verify_ecdsa_signature(
    pubkey: &EcdsaPublicKey,
    msg: &[u8],
    signature: &EcdsaSignature,
) -> Result<(), k256::ecdsa::Error> {
    use k256::ecdsa::signature::Verifier;
    let ecdsa_pubkey = EcdsaPublicKeyInner::from_sec1_bytes(&pubkey.encoded)?;
    let ecdsa_sig =
        EcdsaSignatureInner::from_bytes(GenericArray::<u8, U64>::from_slice(signature.as_ref()))?;
    ecdsa_pubkey.verify(msg, &ecdsa_sig)
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn test_ecdsa_signer() {
        let signer = EcdsaSigner::new_random();
        let pubkey = signer.pubkey();
        let object_root = B256::random().0;
        let signature = signer.sign(Chain::Holesky, object_root).await;

        let domain = compute_domain(Chain::Holesky, COMMIT_BOOST_DOMAIN);
        let msg = compute_signing_root(object_root, domain);

        let verified = verify_ecdsa_signature(&pubkey, &msg, &signature);
        assert!(verified.is_ok());
    }
}
