//! Domain-separated Merkle tree over items (spec §2.2 finding A7): leaf = H(0x00 ‖ item), internal
//! = H(0x01 ‖ left ‖ right), odd node promoted UNPAIRED (no duplicate-last), leaf order = item order.
use sha2::{Digest, Sha256};
use crate::format::AirmemItem;

/// The 32-byte leaf hash of one item: `SHA256(0x00 ‖ jcs(item_without_display_fields))`. The `leaf`
/// and `carried_origin` fields are blanked first — `leaf` cannot hash itself, and `carried_origin`
/// is a cross-checked display token (verify recomputes origin from the signed event bytes).
pub fn leaf_hash(item: &AirmemItem) -> [u8; 32] {
    let mut naked = item.clone();
    naked.leaf = String::new();
    naked.carried_origin = None;
    let canon = crate::format::canonical_json(&naked).expect("item is always serializable");
    let mut h = Sha256::new();
    h.update([0x00]);
    h.update(&canon);
    h.finalize().into()
}

/// The Merkle root over pre-computed leaf hashes (leaf order preserved). Empty input is disallowed
/// upstream (`EmptySelection`); a single leaf is its own root.
pub fn root(leaves: &[[u8; 32]]) -> [u8; 32] {
    assert!(!leaves.is_empty(), "root() requires >= 1 leaf; empty selection is rejected upstream");
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0;
        while i < level.len() {
            if i + 1 < level.len() {
                let mut h = Sha256::new();
                h.update([0x01]); h.update(level[i]); h.update(level[i + 1]);
                next.push(h.finalize().into());
                i += 2;
            } else {
                next.push(level[i]); // odd node promoted UNPAIRED
                i += 1;
            }
        }
        level = next;
    }
    level[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{AirmemItem, ItemClass};
    fn item(leaf: &str, content: &str) -> AirmemItem {
        AirmemItem { leaf: leaf.into(), class: ItemClass::SealVouched, kind: "session".into(),
            event_bytes: None, signature: None, carried_origin: None,
            content: Some(content.into()), display: None }
    }
    #[test]
    fn leaf_excludes_leaf_and_carried_origin_fields() {
        let a = item("aa", "same");
        let mut b = item("bb", "same"); // different `leaf`
        b.carried_origin = Some("external".into()); // and a carried_origin
        assert_eq!(leaf_hash(&a), leaf_hash(&b), "leaf + carried_origin must not affect the leaf hash");
    }
    #[test]
    fn domain_separation_defeats_leaf_as_internal_second_preimage() {
        let l = leaf_hash(&item("", "l"));
        let r = leaf_hash(&item("", "r"));
        assert_ne!(root(&[l, r]), leaf_hash(&item("", "forge")));
    }
    #[test]
    fn odd_node_promoted_unpaired_not_duplicated() {
        let a = leaf_hash(&item("", "a"));
        let b = leaf_hash(&item("", "b"));
        let c = leaf_hash(&item("", "c"));
        let mut h = Sha256::new(); h.update([0x01]); h.update(a); h.update(b);
        let ab: [u8; 32] = h.finalize().into();
        let mut h2 = Sha256::new(); h2.update([0x01]); h2.update(ab); h2.update(c);
        let expected: [u8; 32] = h2.finalize().into();
        assert_eq!(root(&[a, b, c]), expected);
    }
    #[test]
    fn single_leaf_is_its_own_root() {
        let a = leaf_hash(&item("", "solo"));
        assert_eq!(root(&[a]), a);
    }
    #[test]
    fn order_matters() {
        let a = leaf_hash(&item("", "a"));
        let b = leaf_hash(&item("", "b"));
        assert_ne!(root(&[a, b]), root(&[b, a]));
    }
}
