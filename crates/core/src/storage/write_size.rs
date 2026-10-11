//! Allocation-free estimates for bounded storage admission, computed before owning a connection.
//! These are payload estimates, not promises about allocator overhead or total process RSS.

pub(super) fn estimated_json_bytes<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<usize, String> {
    struct Size(usize);
    impl std::io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut size = Size(0);
    serde_json::to_writer(&mut size, value).map_err(|error| error.to_string())?;
    Ok(size.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_json_escaping_and_utf8_without_retaining_an_encoded_copy() {
        let payload = serde_json::json!({
            "message": "line one\n第二行\t\"quoted\"",
            "items": [null, true, -42, {"nested": "\\"}],
        });
        assert_eq!(
            estimated_json_bytes(&payload).unwrap(),
            serde_json::to_vec(&payload).unwrap().len()
        );
    }
}
