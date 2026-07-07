// Rust agent core.
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub fn content_revision(content: &[u8]) -> String {
    let mut forward = FNV_OFFSET_BASIS;
    for byte in content {
        forward ^= u64::from(*byte);
        forward = forward.wrapping_mul(FNV_PRIME);
    }

    let mut reverse = FNV_OFFSET_BASIS;
    for byte in content.iter().rev() {
        reverse ^= u64::from(*byte);
        reverse = reverse.wrapping_mul(FNV_PRIME);
    }

    format!("v1-{:x}-{forward:016x}{reverse:016x}", content.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_changes_with_content_and_order() {
        assert_eq!(content_revision(b"hello"), content_revision(b"hello"));
        assert_ne!(content_revision(b"hello"), content_revision(b"hello\n"));
        assert_ne!(content_revision(b"ab"), content_revision(b"ba"));
    }
}
