//! VMDK disk database (`ddb.*`) — the descriptor's metadata namespace.
//!
//! `VMware` writes a `# The Disk Data Base` section of `ddb.<key> = "<value>"` pairs
//! at the end of the descriptor: virtual geometry, controller type, VM hardware /
//! tools versions, disk UUID, long content ID, thin-provisioning, and the
//! descriptor text encoding. Both `qemu-img` and libvmdk parse the descriptor but
//! **discard every `ddb.*` value**; surfacing them is high-value forensic metadata
//! (image dating, controller provenance, cross-snapshot disk identity).
//!
//! Source: libvmdk VMDK format spec — "The disk database"
//!   <https://github.com/libyal/libvmdk/blob/main/documentation/VMware%20Virtual%20Disk%20Format%20(VMDK).asciidoc>

/// Virtual disk CHS geometry from `ddb.geometry.{cylinders,heads,sectors}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiskGeometry {
    pub cylinders: u32,
    pub heads: u32,
    pub sectors: u32,
}

impl DiskGeometry {
    /// Total sectors implied by the CHS geometry (`cylinders * heads * sectors`).
    #[must_use]
    pub fn chs_sectors(&self) -> u64 {
        u64::from(self.cylinders) * u64::from(self.heads) * u64::from(self.sectors)
    }
}

/// Parsed `ddb.*` disk database from a VMDK descriptor.
///
/// Typed access to the common keys plus a raw key/value list for any others.
/// All fields are `None`/empty when the descriptor carries no disk database
/// (e.g. a snapshot delta, whose ddb section is stripped).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiskDatabase {
    /// `ddb.adapterType` — virtual controller (`ide`/`buslogic`/`lsilogic`/`lsisas1068`/`pvscsi`/`legacyESX`).
    pub adapter_type: Option<String>,
    /// `ddb.geometry.{cylinders,heads,sectors}` — present only when all three exist.
    pub geometry: Option<DiskGeometry>,
    /// `ddb.geometry.bios{Cylinders,Heads,Sectors}` — BIOS-reported geometry.
    pub bios_geometry: Option<DiskGeometry>,
    /// `ddb.virtualHWVersion` — VM hardware version (dates the creating platform).
    pub virtual_hw_version: Option<String>,
    /// `ddb.toolsVersion` — installed `VMware` Tools build.
    pub tools_version: Option<String>,
    /// `ddb.uuid` — disk UUID (space-separated hex bytes as written).
    pub uuid: Option<String>,
    /// `ddb.longContentID` — 128-bit content ID (used when `CID == 0xFFFFFFFE`).
    pub long_content_id: Option<String>,
    /// `ddb.thinProvisioned` — thin (`true`) vs thick (`false`).
    pub thin_provisioned: Option<bool>,
    /// `ddb.encoding` — descriptor text encoding (e.g. `UTF-8`, `windows-1252`).
    pub encoding: Option<String>,
    /// Every `ddb.*` key/value as written, including ones without a typed field.
    pub entries: Vec<(String, String)>,
}

impl DiskDatabase {
    /// Parse the `ddb.*` lines from a descriptor's text.
    #[must_use]
    pub fn parse(descriptor_text: &str) -> Self {
        let mut db = DiskDatabase::default();
        let (mut cyl, mut head, mut sect) = (None, None, None);
        let (mut bcyl, mut bhead, mut bsect) = (None, None, None);

        for line in descriptor_text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("ddb.") else {
                continue;
            };
            // `ddb.<key> = "<value>"` (value may or may not be quoted).
            let Some((key, value)) = rest.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"').to_owned();
            let full_key = format!("ddb.{key}");
            db.entries.push((full_key, value.clone()));

            match key {
                "adapterType" => db.adapter_type = Some(value),
                "virtualHWVersion" => db.virtual_hw_version = Some(value),
                "toolsVersion" => db.tools_version = Some(value),
                "uuid" => db.uuid = Some(value),
                "longContentID" => db.long_content_id = Some(value),
                "encoding" => db.encoding = Some(value),
                "thinProvisioned" => db.thin_provisioned = Some(value.trim() == "1"),
                "geometry.cylinders" => cyl = value.parse().ok(),
                "geometry.heads" => head = value.parse().ok(),
                "geometry.sectors" => sect = value.parse().ok(),
                "geometry.biosCylinders" => bcyl = value.parse().ok(),
                "geometry.biosHeads" => bhead = value.parse().ok(),
                "geometry.biosSectors" => bsect = value.parse().ok(),
                _ => {}
            }
        }

        if let (Some(cylinders), Some(heads), Some(sectors)) = (cyl, head, sect) {
            db.geometry = Some(DiskGeometry {
                cylinders,
                heads,
                sectors,
            });
        }
        if let (Some(cylinders), Some(heads), Some(sectors)) = (bcyl, bhead, bsect) {
            db.bios_geometry = Some(DiskGeometry {
                cylinders,
                heads,
                sectors,
            });
        }
        db
    }

    /// `true` when the descriptor carried no `ddb.*` entries at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Raw value of a `ddb.*` key as written (full key, e.g. `"ddb.adapterType"`).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "# Disk DescriptorFile\nversion=1\nCID=12345678\nparentCID=ffffffff\ncreateType=\"monolithicSparse\"\n\nddb.adapterType = \"lsilogic\"\nddb.geometry.cylinders = \"16383\"\nddb.geometry.heads = \"16\"\nddb.geometry.sectors = \"63\"\nddb.virtualHWVersion = \"7\"\nddb.toolsVersion = \"10338\"\nddb.uuid = \"60 00 C2 97 1a 2b 3c 4d-5e 6f 70 81 92 a3 b4 c5\"\nddb.longContentID = \"deadbeefcafef00d1122334455667788\"\nddb.thinProvisioned = \"1\"\nddb.encoding = \"UTF-8\"\n";

    #[test]
    fn parses_adapter_type_and_versions() {
        let db = DiskDatabase::parse(FULL);
        assert_eq!(db.adapter_type.as_deref(), Some("lsilogic"));
        assert_eq!(db.virtual_hw_version.as_deref(), Some("7"));
        assert_eq!(db.tools_version.as_deref(), Some("10338"));
        assert_eq!(db.encoding.as_deref(), Some("UTF-8"));
    }

    #[test]
    fn parses_geometry() {
        let db = DiskDatabase::parse(FULL);
        let g = db.geometry.expect("geometry present");
        assert_eq!(g.cylinders, 16383);
        assert_eq!(g.heads, 16);
        assert_eq!(g.sectors, 63);
        // CHS-reported sector count.
        assert_eq!(g.chs_sectors(), 16383 * 16 * 63);
    }

    #[test]
    fn parses_uuid_thin_and_long_content_id() {
        let db = DiskDatabase::parse(FULL);
        assert_eq!(
            db.uuid.as_deref(),
            Some("60 00 C2 97 1a 2b 3c 4d-5e 6f 70 81 92 a3 b4 c5")
        );
        assert_eq!(
            db.long_content_id.as_deref(),
            Some("deadbeefcafef00d1122334455667788")
        );
        assert_eq!(db.thin_provisioned, Some(true));
    }

    #[test]
    fn empty_when_no_ddb_section() {
        let db = DiskDatabase::parse(
            "# Disk DescriptorFile\nversion=1\ncreateType=\"monolithicFlat\"\n",
        );
        assert!(db.is_empty());
        assert_eq!(db.adapter_type, None);
        assert_eq!(db.geometry, None);
        assert_eq!(db.thin_provisioned, None);
    }

    #[test]
    fn unknown_ddb_keys_are_retained() {
        let db = DiskDatabase::parse("ddb.somethingNew = \"42\"\nddb.adapterType = \"ide\"\n");
        assert_eq!(db.adapter_type.as_deref(), Some("ide"));
        assert_eq!(db.get("ddb.somethingNew"), Some("42"));
        assert!(!db.is_empty());
    }

    #[test]
    fn thin_provisioned_zero_is_false() {
        let db = DiskDatabase::parse("ddb.thinProvisioned = \"0\"\n");
        assert_eq!(db.thin_provisioned, Some(false));
    }

    #[test]
    fn parses_bios_geometry() {
        let db = DiskDatabase::parse(
            "ddb.geometry.biosCylinders = \"100\"\nddb.geometry.biosHeads = \"8\"\nddb.geometry.biosSectors = \"32\"\n",
        );
        let g = db.bios_geometry.expect("bios geometry present");
        assert_eq!(g.cylinders, 100);
        assert_eq!(g.heads, 8);
        assert_eq!(g.sectors, 32);
    }

    #[test]
    fn ddb_line_without_equals_is_skipped() {
        // A malformed `ddb.` line with no `=` is skipped, not an error.
        let db = DiskDatabase::parse("ddb.brokenline\nddb.adapterType = \"ide\"\n");
        assert_eq!(db.adapter_type.as_deref(), Some("ide"));
    }

    #[test]
    fn partial_geometry_is_ignored() {
        // Geometry requires all three of cylinders/heads/sectors.
        let db =
            DiskDatabase::parse("ddb.geometry.cylinders = \"100\"\nddb.geometry.heads = \"4\"\n");
        assert_eq!(db.geometry, None);
    }
}
