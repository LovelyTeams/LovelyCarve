//! Shared path identity for evidence readers and case registration.

/// Windows evidence paths compare case-insensitively with equivalent separators.
/// Casework re-exports this same function at its established public path.
pub fn evidence_path_key(path: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        path.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_identity_obeys_platform_path_rules() {
        #[cfg(target_os = "windows")]
        assert_eq!(
            evidence_path_key("D:/Evidence/Disk.E01"),
            "d:\\evidence\\disk.e01"
        );
        #[cfg(not(target_os = "windows"))]
        assert_ne!(
            evidence_path_key("/Evidence/Disk.E01"),
            evidence_path_key("/evidence/disk.e01")
        );
    }
}
