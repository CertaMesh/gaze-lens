//! Connection-pinned authority. No source values or credentials are printable.
use crate::hex;
use gaze_lens_protocol::{
    Error, Result, bounds,
    wire::{DestinationBinding, Operation, Prepare},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Authority {
    principals: Vec<Principal>,
    resources: Vec<Resource>,
    grants: Vec<Grant>,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Principal {
    id: String,
    generation: String,
    sha256: String,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Resource {
    alias: String,
    id: String,
    generation: String,
    class: ResourceClass,
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResourceClass {
    Database,
    Log,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Grant {
    principal: String,
    resource: String,
    operation: Operation,
    enabled: bool,
    expires_unix: u64,
}

/// Owns the exact objects resolved at Prepare; revalidation cannot replace them.
pub struct Pinned {
    principal: Principal,
    resource: Resource,
    operation: Operation,
    binding: DestinationBinding,
}
impl Pinned {
    pub fn binding(&self) -> &DestinationBinding {
        &self.binding
    }
    /// The operation authorized at Prepare. Later checks read it from here
    /// instead of repeating an operation literal that could drift.
    pub fn operation(&self) -> Operation {
        self.operation
    }
}
impl Authority {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        bounds::json(bytes, 65536)?;
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Unauthorized)?;
        bounds::cap(value.principals.len(), 128)?;
        bounds::cap(value.resources.len(), 128)?;
        bounds::cap(value.grants.len(), 1024)?;
        for (i, p) in value.principals.iter().enumerate() {
            if !hex(&p.id, 32)
                || !hex(&p.generation, 32)
                || !hex(&p.sha256, 64)
                || value.principals[..i]
                    .iter()
                    .any(|x| x.id == p.id || x.sha256 == p.sha256)
            {
                return Err(Error::Unauthorized);
            }
        }
        for (i, r) in value.resources.iter().enumerate() {
            bounds::configured_id(&r.alias)?;
            // Principal and resource IDs share one nonoverlapping namespace;
            // `identities` may then rely on every enrolled ID being distinct.
            if !hex(&r.id, 32)
                || !hex(&r.generation, 32)
                || value.principals.iter().any(|p| p.id == r.id)
                || value.resources[..i]
                    .iter()
                    .any(|x| x.id == r.id || x.alias == r.alias)
            {
                return Err(Error::Unauthorized);
            }
        }
        for (i, g) in value.grants.iter().enumerate() {
            if !value.principals.iter().any(|p| p.id == g.principal)
                || !value.resources.iter().any(|r| r.id == g.resource)
                || value.grants[..i].iter().any(|x| {
                    x.principal == g.principal
                        && x.resource == g.resource
                        && x.operation == g.operation
                })
            {
                return Err(Error::Unauthorized);
            }
        }
        Ok(value)
    }
    pub(crate) fn identities(&self) -> Result<Vec<crate::history::Identity>> {
        use crate::history::{Identity, Kind};
        let mut identities = Vec::new();
        for p in &self.principals {
            identities.push(Identity {
                kind: Kind::Principal,
                id: p.id.clone(),
                generation: p.generation.clone(),
                fingerprint: fingerprint(p)?,
            });
        }
        for r in &self.resources {
            identities.push(Identity {
                kind: Kind::Resource,
                id: r.id.clone(),
                generation: r.generation.clone(),
                fingerprint: fingerprint(r)?,
            });
        }
        // Sorted so an unchanged definition set compares equal across restarts.
        identities.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(identities)
    }
    /// `p` arrives from `decode_prepare`, which already validated its shape.
    pub fn prepare(&self, p: &Prepare, now: u64) -> Result<Pinned> {
        let digest = format!("{:x}", Sha256::digest(p.credential.as_bytes()));
        // Folded rather than found: the comparison count does not depend on
        // where the matching principal sits in the file.
        let principal = self
            .principals
            .iter()
            .fold(None, |found, x| {
                if bool::from(x.sha256.as_bytes().ct_eq(digest.as_bytes())) {
                    Some(x)
                } else {
                    found
                }
            })
            .ok_or(Error::Unauthorized)?;
        let resource = self
            .resources
            .iter()
            .find(|x| x.alias == p.resource)
            .ok_or(Error::Unauthorized)?;
        self.grant(&principal.id, &resource.id, p.operation, now)?;
        Ok(Pinned {
            principal: principal.clone(),
            resource: resource.clone(),
            operation: p.operation,
            binding: DestinationBinding {
                principal: principal.id.clone(),
                principal_generation: principal.generation.clone(),
                resource: resource.id.clone(),
                resource_generation: resource.generation.clone(),
            },
        })
    }
    pub fn revalidate(&self, pin: &Pinned, now: u64) -> Result<()> {
        // Defense in depth behind `History::validate`, which has already
        // refused any identity-definition change for a running server.
        if !self.principals.iter().any(|p| p == &pin.principal)
            || !self.resources.iter().any(|r| r == &pin.resource)
        {
            return Err(Error::BindingChanged);
        }
        self.grant(&pin.principal.id, &pin.resource.id, pin.operation, now)
    }
    fn grant(&self, p: &str, r: &str, op: Operation, now: u64) -> Result<()> {
        if self.grants.iter().any(|g| {
            g.principal == p
                && g.resource == r
                && g.operation == op
                && g.enabled
                && g.expires_unix > now
        }) {
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    }
}
fn fingerprint<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::InternalFailure)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
