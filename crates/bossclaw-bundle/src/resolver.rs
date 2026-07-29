//! The L2 identity-resolution seam. Registry-mediated ONLY (spec §2.5 finding C7): keyed by did,
//! NEVER a fetch of the did's own domain. The real HTTPS impl is SP-V2; SP-V1 ships the trait, an
//! offline default (always None → stays L1), and a mock for tests.

/// Resolves an AIR did to the identity verifying key the REGISTRY publishes for it.
pub trait IdentityResolver {
    /// Resolve `did` to its registry-published identity verifying key (multibase), or `None` when
    /// unresolvable / non-registry / offline. `None` keeps the verdict at L1.
    fn resolve(&self, did: &str) -> Option<String>;
}

/// The `--offline` default: resolves nothing, so identity renders "unverified (offline)".
pub struct OfflineResolver;

impl IdentityResolver for OfflineResolver {
    fn resolve(&self, _did: &str) -> Option<String> {
        None
    }
}

/// Test double: resolves exactly one did to one key, everything else to `None`.
#[cfg(test)]
pub(crate) struct MockResolver {
    /// The single did this mock knows.
    pub did: String,
    /// The multibase key the mock's "registry" publishes for `did`.
    pub key: String,
}

#[cfg(test)]
impl IdentityResolver for MockResolver {
    fn resolve(&self, did: &str) -> Option<String> {
        if did == self.did {
            Some(self.key.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_resolver_resolves_nothing() {
        assert_eq!(OfflineResolver.resolve("did:wba:example.com:me"), None);
    }

    #[test]
    fn mock_resolver_only_answers_for_its_own_did() {
        let r = MockResolver { did: "did:wba:example.com:me".into(), key: "zKey".into() };
        assert_eq!(r.resolve("did:wba:example.com:me"), Some("zKey".into()));
        assert_eq!(r.resolve("did:wba:evil.com:attacker"), None);
    }
}
