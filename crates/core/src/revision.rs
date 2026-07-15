const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContentRevisionHasher {
    state: u64,
}

impl ContentRevisionHasher {
    pub(crate) fn new() -> Self {
        Self {
            state: FNV_OFFSET_BASIS,
        }
    }

    pub(crate) fn update(&mut self, content: &[u8]) {
        for byte in content {
            self.update_byte(*byte);
        }
    }

    pub(crate) fn update_reversed(&mut self, content: &[u8]) {
        for byte in content.iter().rev() {
            self.update_byte(*byte);
        }
    }

    fn update_byte(&mut self, byte: u8) {
        self.state ^= u64::from(byte);
        self.state = self.state.wrapping_mul(FNV_PRIME);
    }

    pub(crate) fn finish(self) -> u64 {
        self.state
    }
}

pub(crate) fn compose_content_revision(length: u64, forward: u64, reverse: u64) -> String {
    format!("v1-{length:x}-{forward:016x}{reverse:016x}")
}

pub fn content_revision(content: &[u8]) -> String {
    let mut forward = ContentRevisionHasher::new();
    forward.update(content);
    let mut reverse = ContentRevisionHasher::new();
    reverse.update_reversed(content);
    compose_content_revision(
        u64::try_from(content.len()).unwrap_or(u64::MAX),
        forward.finish(),
        reverse.finish(),
    )
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

    #[test]
    fn incremental_hashing_preserves_revision_format() {
        let content = b"hello\nworld\n";
        let mut forward = ContentRevisionHasher::new();
        forward.update(&content[..4]);
        forward.update(&content[4..]);
        let mut reverse = ContentRevisionHasher::new();
        reverse.update_reversed(&content[5..]);
        reverse.update_reversed(&content[..5]);

        assert_eq!(
            compose_content_revision(
                u64::try_from(content.len()).unwrap(),
                forward.finish(),
                reverse.finish(),
            ),
            content_revision(content)
        );
    }
}
