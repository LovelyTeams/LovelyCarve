//! Linux `/etc/fstab`: which volume a system mounts where.
//!
//! Only local block-device mounts are kept: pseudo filesystems, swap,
//! network shares and bind mounts never name a volume of the evidence.
//! Fields decode the octal escapes fstab uses for spaces (`\040`).

/// One mount line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FstabEntry {
    pub spec: String,
    /// Mount point (`/home`).
    pub target: String,
    pub fs_type: String,
    pub options: Vec<String>,
}

/// How a line names its volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceSpec {
    Uuid(String),
    Label(String),
    PartUuid(String),
    /// `/dev/mapper/<vg>-<lv>` or `/dev/<vg>/<lv>`.
    LogicalVolume {
        group: String,
        volume: String,
    },
    /// Any other device path (`/dev/sda2`).
    Device(String),
}

const PSEUDO_TYPES: [&str; 14] = [
    "swap",
    "proc",
    "sysfs",
    "tmpfs",
    "devpts",
    "devtmpfs",
    "cgroup",
    "cgroup2",
    "securityfs",
    "debugfs",
    "efivarfs",
    "nfs",
    "nfs4",
    "cifs",
];

/// The local block-device mounts of an fstab, in file order.
pub fn parse(text: &str) -> Vec<FstabEntry> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default().trim();
            let mut fields = line.split_whitespace();
            let spec = unescape(fields.next()?);
            let target = unescape(fields.next()?);
            let fs_type = fields.next().unwrap_or("auto").to_owned();
            let options: Vec<String> = fields
                .next()
                .unwrap_or("defaults")
                .split(',')
                .map(str::to_owned)
                .collect();
            let entry = FstabEntry {
                spec,
                target,
                fs_type,
                options,
            };
            entry.is_volume_mount().then_some(entry)
        })
        .collect()
}

impl FstabEntry {
    fn is_volume_mount(&self) -> bool {
        self.target.starts_with('/')
            && !PSEUDO_TYPES.contains(&self.fs_type.as_str())
            && !self
                .options
                .iter()
                .any(|option| option == "bind" || option == "rbind")
            && (self.spec.starts_with('/')
                || ["UUID=", "LABEL=", "PARTUUID="]
                    .iter()
                    .any(|prefix| self.spec.starts_with(prefix)))
    }

    pub fn device(&self) -> DeviceSpec {
        let spec = self.spec.as_str();
        if let Some(uuid) = spec.strip_prefix("UUID=") {
            return DeviceSpec::Uuid(unquote(uuid));
        }
        if let Some(label) = spec.strip_prefix("LABEL=") {
            return DeviceSpec::Label(unquote(label));
        }
        if let Some(uuid) = spec.strip_prefix("PARTUUID=") {
            return DeviceSpec::PartUuid(unquote(uuid));
        }
        if let Some(name) = spec.strip_prefix("/dev/mapper/") {
            if let Some((group, volume)) = split_mapper_name(name) {
                return DeviceSpec::LogicalVolume { group, volume };
            }
        }
        if let Some(rest) = spec.strip_prefix("/dev/") {
            if let Some((group, volume)) = rest.split_once('/') {
                if !group.is_empty() && !volume.is_empty() && !volume.contains('/') {
                    return DeviceSpec::LogicalVolume {
                        group: group.to_owned(),
                        volume: volume.to_owned(),
                    };
                }
            }
        }
        DeviceSpec::Device(spec.to_owned())
    }

    /// The mount point as a view path (`home`, `boot/efi`; `""` for `/`).
    pub fn view_path(&self) -> String {
        self.target.trim_matches('/').to_owned()
    }

    /// The Btrfs subvolume the line mounts (`subvol=@home`).
    pub fn subvolume(&self) -> Option<String> {
        self.options.iter().find_map(|option| {
            option
                .strip_prefix("subvol=")
                .map(|subvolume| subvolume.trim_matches('/').to_owned())
                .filter(|subvolume| !subvolume.is_empty())
        })
    }
}

/// Splits a device-mapper name: a single `-` separates group and volume,
/// `--` stands for a `-` inside either name.
fn split_mapper_name(name: &str) -> Option<(String, String)> {
    let bytes = name.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'-' {
            if bytes.get(index + 1) == Some(&b'-') {
                index += 2;
                continue;
            }
            let group = name[..index].replace("--", "-");
            let volume = name[index + 1..].replace("--", "-");
            return (!group.is_empty() && !volume.is_empty()).then_some((group, volume));
        }
        index += 1;
    }
    None
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').to_owned()
}

/// Decodes `\ooo` octal escapes.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if let Some(code) = bytes
                .get(index + 1..index + 4)
                .and_then(|digits| std::str::from_utf8(digits).ok())
                .and_then(|digits| u8::from_str_radix(digits, 8).ok())
            {
                decoded.push(code);
                index += 4;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_local_volume_mounts_and_decodes_their_specs() {
        let entries = parse(
            "# /etc/fstab\n\
             UUID=1b4e28ba-2fa1-11d2-883f-0016d3cc427e / ext4 errors=remount-ro 0 1\n\
             UUID=\"ABCD-1234\" /boot/efi vfat umask=0077 0 1\n\
             /dev/mapper/vg--data-lv_home /home xfs defaults 0 2\n\
             /dev/ubuntu-vg/var /var ext4 defaults 0 2\n\
             LABEL=Media\\040Disk /mnt/media ext4 nofail 0 0\n\
             UUID=5555 /srv btrfs subvol=/@srv 0 0\n\
             /swapfile none swap sw 0 0\n\
             proc /proc proc defaults 0 0\n\
             tmpfs /tmp tmpfs defaults 0 0\n\
             /srv/export /export none bind 0 0\n\
             server:/share /net nfs defaults 0 0\n",
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.device(), entry.view_path()))
                .collect::<Vec<_>>(),
            vec![
                (
                    DeviceSpec::Uuid("1b4e28ba-2fa1-11d2-883f-0016d3cc427e".to_owned()),
                    String::new()
                ),
                (
                    DeviceSpec::Uuid("ABCD-1234".to_owned()),
                    "boot/efi".to_owned()
                ),
                (
                    DeviceSpec::LogicalVolume {
                        group: "vg-data".to_owned(),
                        volume: "lv_home".to_owned()
                    },
                    "home".to_owned()
                ),
                (
                    DeviceSpec::LogicalVolume {
                        group: "ubuntu-vg".to_owned(),
                        volume: "var".to_owned()
                    },
                    "var".to_owned()
                ),
                (
                    DeviceSpec::Label("Media Disk".to_owned()),
                    "mnt/media".to_owned()
                ),
                (DeviceSpec::Uuid("5555".to_owned()), "srv".to_owned()),
            ]
        );
        assert_eq!(entries[5].subvolume().as_deref(), Some("@srv"));
        assert_eq!(entries[0].subvolume(), None);
    }

    #[test]
    fn device_mapper_names_double_their_dashes() {
        assert_eq!(
            split_mapper_name("my--vg-my--lv"),
            Some(("my-vg".to_owned(), "my-lv".to_owned()))
        );
        assert_eq!(split_mapper_name("cryptroot"), None);
    }
}
