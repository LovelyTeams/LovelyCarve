//! Names for synthetic volume entry points, never for evidence file entries.

/// Keep letters, numbers, dots, hyphens and underscores. Replace runs of
/// whitespace or punctuation with one underscore, without leading/trailing
/// replacement separators. Raw labels and node IDs remain separate metadata.
pub(crate) fn volume_name(label: &str) -> String {
    let mut name = String::with_capacity(label.len());
    let mut separator = false;
    for character in label.chars() {
        if character.is_alphanumeric() || matches!(character, '.' | '-' | '_') {
            if separator && !name.is_empty() && !name.ends_with('_') && character != '_' {
                name.push('_');
            }
            name.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    if name.is_empty() || name == "." || name == ".." {
        "volume".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_names_remove_shell_punctuation_and_path_separators() {
        for (label, expected) in [
            ("ext4 (md · host:0)", "ext4_md_host_0"),
            ("数据卷 （备份） · 2026", "数据卷_备份_2026"),
            ("APFS 0 · Macintosh HD", "APFS_0_Macintosh_HD"),
            ("a\u{a0}\t\r\nb", "a_b"),
            ("vg/lv\\data", "vg_lv_data"),
            (" a; &b|c$(d)`e`\"f'[]{}<>*?!% ", "a_b_c_d_e_f"),
            ("archive-1.2__数据", "archive-1.2__数据"),
            ("", "volume"),
            (" () · ", "volume"),
            (".", "volume"),
            ("..", "volume"),
        ] {
            assert_eq!(volume_name(label), expected, "{label}");
            assert_eq!(volume_name(expected), expected, "must be idempotent");
        }
    }
}
