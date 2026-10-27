//! Access tokens, sign-in and pairing.

use serde::{Deserialize, Serialize};

#[cfg(feature = "export-types")]
use ts_rs::TS;

/// Whether this node needs an access token, and whether this browser has it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AuthStatusDto {
    pub required: bool,
    pub signed_in: bool,
}

/// Who a node is, for anyone who asks: what a controller checks before it
/// approves a node, and what a node's Setup shows to compare against.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct IdentityDto {
    pub id: String,
    pub name: String,
    pub version: String,
    /// SHA-256 of its TLS key, in hex.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct SignInRequest {
    pub token: String,
}

/// A new access token. There's always one: it can be changed, not removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AccessTokenRequest {
    pub token: String,
}

/// This node's access token and pairing code, for a browser signed in to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AccessTokenDto {
    pub token: String,
    /// What a controller needs to add this node (`ABCD-EFGH`).
    pub pairing_code: String,
}

/// A controller adding a node: the code the node shows, and the token it
/// should use from now on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub token: String,
}

/// Approving a found node, or pairing an approved one that refuses this
/// controller's token. A node already using the token needs no code.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct PairingCodeRequest {
    #[serde(default)]
    pub pairing_code: Option<String>,
}

/// Nodes the token couldn't be set on; they keep their old one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AccessTokenResultDto {
    pub failed: Vec<AccessTokenFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "export-types", derive(TS), ts(export))]
pub struct AccessTokenFailure {
    pub node_id: String,
    pub name: String,
    pub error: String,
}
