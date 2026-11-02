pub const PROTOCOL_VERSION: u32 = 1;

/// Placeholder replaced by task 2.1.
pub fn protocol_version() -> u32 {
    PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_stable() {
        assert_eq!(protocol_version(), 1);
    }
}
