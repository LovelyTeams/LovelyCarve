//! Synthetic DDF metadata for tests: a container description and a writer
//! that lays one member disk's metadata out at the end of its image.
use super::*;

/// Position-table capacity of every configuration record.
pub(crate) const MPPE: usize = 16;
const CONFIG_RECORD_SECTORS: usize = 2;
const MAX_PARTITIONS: usize = 4;
/// Section offsets inside one metadata copy, in sectors from its header.
const CONTROLLER_AT: usize = 1;
const PHYS_AT: usize = 2;
const PHYS_SECTORS: usize = 2;
const VIRT_AT: usize = 4;
const VIRT_SECTORS: usize = 2;
const CONFIG_AT: usize = 6;
const CONFIG_SECTORS: usize = MAX_PARTITIONS * CONFIG_RECORD_SECTORS;
const DATA_AT: usize = CONFIG_AT + CONFIG_SECTORS;
/// Sectors one metadata copy occupies.
pub(crate) const COPY_SECTORS: usize = DATA_AT + 1;

#[derive(Clone)]
pub(crate) struct PhysFields {
    pub refnum: u32,
    pub guid: [u8; 24],
    pub kind: u16,
    pub state: u16,
}

#[derive(Clone)]
pub(crate) struct VirtualFields {
    pub guid: [u8; 24],
    pub name: &'static str,
    pub state: u8,
    pub init_state: u8,
}

#[derive(Clone)]
pub(crate) struct ConfigFields {
    pub guid: [u8; 24],
    pub seqnum: u32,
    pub prl: u8,
    pub rlq: u8,
    pub srl: u8,
    pub sec_elmnt_count: u8,
    pub sec_elmnt_seq: u8,
    pub chunk_shift: u8,
    pub blocks: u64,
    pub array_blocks: u64,
    pub refnums: Vec<u32>,
    pub lba_offsets: Vec<u64>,
}

#[derive(Clone)]
pub(crate) struct ContainerFields {
    pub guid: [u8; 24],
    pub revision: &'static str,
    pub seq: u32,
    pub open: bool,
    pub disks: Vec<PhysFields>,
    pub virtuals: Vec<VirtualFields>,
}

/// Printable disk GUID, as controllers build them.
pub(crate) fn disk_guid(refnum: u32) -> [u8; 24] {
    let text = format!("ATA     TESTDISK{refnum:08X}");
    let mut guid = [0u8; 24];
    guid.copy_from_slice(text.as_bytes());
    guid
}

impl PhysFields {
    pub(crate) fn online(refnum: u32) -> Self {
        Self {
            refnum,
            guid: disk_guid(refnum),
            kind: 0x02,
            state: PD_ONLINE,
        }
    }
}

impl VirtualFields {
    pub(crate) fn new(guid: [u8; 24], name: &'static str) -> Self {
        Self {
            guid,
            name,
            state: 0,
            init_state: VD_INIT_FULL,
        }
    }
}

impl ConfigFields {
    /// A single-level record over `refnums`, starting at sector 0 of every
    /// disk.
    pub(crate) fn new(guid: [u8; 24], prl: u8, rlq: u8, refnums: Vec<u32>, blocks: u64) -> Self {
        Self {
            guid,
            seqnum: 1,
            prl,
            rlq,
            srl: 0xFF,
            sec_elmnt_count: 1,
            sec_elmnt_seq: 0,
            chunk_shift: 3,
            blocks,
            array_blocks: 0,
            lba_offsets: vec![0; refnums.len()],
            refnums,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = vec![0xFFu8; CONFIG_RECORD_SECTORS * SECTOR];
        out[0..4].copy_from_slice(&VD_CONFIG_MAGIC.to_be_bytes());
        out[8..32].copy_from_slice(&self.guid);
        out[32..36].copy_from_slice(&0u32.to_be_bytes());
        out[36..40].copy_from_slice(&self.seqnum.to_be_bytes());
        out[64..66].copy_from_slice(&(self.refnums.len() as u16).to_be_bytes());
        out[66] = self.chunk_shift;
        out[67] = self.prl;
        out[68] = self.rlq;
        out[69] = self.sec_elmnt_count;
        out[70] = self.sec_elmnt_seq;
        out[71] = self.srl;
        out[72..80].copy_from_slice(&self.blocks.to_be_bytes());
        out[80..88].copy_from_slice(&self.array_blocks.to_be_bytes());
        for (position, refnum) in self.refnums.iter().enumerate() {
            let at = CONFIG_FIXED_BYTES + position * 4;
            out[at..at + 4].copy_from_slice(&refnum.to_be_bytes());
        }
        for (position, offset) in self.lba_offsets.iter().enumerate() {
            let at = CONFIG_FIXED_BYTES + MPPE * 4 + position * 8;
            out[at..at + 8].copy_from_slice(&offset.to_be_bytes());
        }
        seal(&mut out);
        out
    }
}

impl ContainerFields {
    pub(crate) fn new(disks: Vec<PhysFields>, virtuals: Vec<VirtualFields>) -> Self {
        Self {
            guid: [0x11; 24],
            revision: "01.02.00",
            seq: 7,
            open: false,
            disks,
            virtuals,
        }
    }

    /// Write the metadata of disk `refnum`, holding `configs`, at the end of
    /// `image`: the primary copy right before the anchor.
    pub(crate) fn place(&self, image: &mut [u8], refnum: u32, configs: &[ConfigFields]) {
        let sectors = image.len() / SECTOR;
        let primary = sectors - 1 - COPY_SECTORS;
        self.write_copy(
            image,
            primary,
            refnum,
            configs,
            HEADER_PRIMARY,
            primary,
            None,
        );
        self.write_anchor(image, primary, None);
    }

    /// Like [`place`](Self::place), plus a secondary copy described by
    /// `secondary` before the primary one.
    pub(crate) fn place_with_secondary(
        &self,
        secondary: &ContainerFields,
        image: &mut [u8],
        refnum: u32,
        configs: &[ConfigFields],
    ) {
        let sectors = image.len() / SECTOR;
        let primary = sectors - 1 - COPY_SECTORS;
        let second = primary - COPY_SECTORS;
        self.write_copy(
            image,
            primary,
            refnum,
            configs,
            HEADER_PRIMARY,
            primary,
            Some(second),
        );
        secondary.write_copy(
            image,
            second,
            refnum,
            configs,
            HEADER_SECONDARY,
            primary,
            Some(second),
        );
        self.write_anchor(image, primary, Some(second));
    }

    fn header(&self, kind: u8, primary: usize, secondary: Option<usize>) -> Vec<u8> {
        let mut out = vec![0xFFu8; SECTOR];
        out[0..4].copy_from_slice(&HEADER_MAGIC.to_be_bytes());
        out[8..32].copy_from_slice(&self.guid);
        out[32..40].copy_from_slice(self.revision.as_bytes());
        out[40..44].copy_from_slice(&self.seq.to_be_bytes());
        out[44..48].copy_from_slice(&0u32.to_be_bytes());
        out[48] = u8::from(self.open);
        out[49] = 0;
        out[50] = 0;
        out[96..104].copy_from_slice(&(primary as u64).to_be_bytes());
        out[104..112].copy_from_slice(&secondary.map_or(u64::MAX, |lba| lba as u64).to_be_bytes());
        out[112] = kind;
        out[116..120].copy_from_slice(&0u32.to_be_bytes());
        out[120..128].copy_from_slice(&0u64.to_be_bytes());
        out[128..130].copy_from_slice(&15u16.to_be_bytes());
        out[130..132].copy_from_slice(&15u16.to_be_bytes());
        out[132..134].copy_from_slice(&(MAX_PARTITIONS as u16).to_be_bytes());
        out[134..136].copy_from_slice(&(CONFIG_RECORD_SECTORS as u16).to_be_bytes());
        out[136..138].copy_from_slice(&(MPPE as u16).to_be_bytes());
        let sections = [
            (CONTROLLER_AT, 1),
            (PHYS_AT, PHYS_SECTORS),
            (VIRT_AT, VIRT_SECTORS),
            (CONFIG_AT, CONFIG_SECTORS),
            (DATA_AT, 1),
            (0, 0),
            (0, 0),
            (0, 0),
        ];
        for (index, (offset, length)) in sections.iter().enumerate() {
            let at = 192 + index * 8;
            out[at..at + 4].copy_from_slice(&(*offset as u32).to_be_bytes());
            out[at + 4..at + 8].copy_from_slice(&(*length as u32).to_be_bytes());
        }
        seal(&mut out);
        out
    }

    fn write_anchor(&self, image: &mut [u8], primary: usize, secondary: Option<usize>) {
        let mut anchor = self.header(0, primary, secondary);
        anchor[40..44].copy_from_slice(&u32::MAX.to_be_bytes());
        anchor[48] = 0xFF;
        seal(&mut anchor);
        let at = image.len() - SECTOR;
        image[at..].copy_from_slice(&anchor);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_copy(
        &self,
        image: &mut [u8],
        at: usize,
        refnum: u32,
        configs: &[ConfigFields],
        kind: u8,
        primary: usize,
        secondary: Option<usize>,
    ) {
        let mut copy = vec![0xFFu8; COPY_SECTORS * SECTOR];
        copy[..SECTOR].copy_from_slice(&self.header(kind, primary, secondary));

        let mut controller = vec![0u8; SECTOR];
        controller[0..4].copy_from_slice(&0xAD11_1111u32.to_be_bytes());
        seal(&mut controller);
        put(&mut copy, CONTROLLER_AT, &controller);

        let mut phys = vec![0xFFu8; PHYS_SECTORS * SECTOR];
        phys[0..4].copy_from_slice(&PHYS_RECORDS_MAGIC.to_be_bytes());
        phys[8..10].copy_from_slice(&(self.disks.len() as u16).to_be_bytes());
        phys[10..12].copy_from_slice(&15u16.to_be_bytes());
        for (index, disk) in self.disks.iter().enumerate() {
            let entry = RECORDS_OFFSET + index * RECORD_BYTES;
            phys[entry..entry + 24].copy_from_slice(&disk.guid);
            phys[entry + 24..entry + 28].copy_from_slice(&disk.refnum.to_be_bytes());
            phys[entry + 28..entry + 30].copy_from_slice(&disk.kind.to_be_bytes());
            phys[entry + 30..entry + 32].copy_from_slice(&disk.state.to_be_bytes());
        }
        seal(&mut phys);
        put(&mut copy, PHYS_AT, &phys);

        let mut virt = vec![0xFFu8; VIRT_SECTORS * SECTOR];
        virt[0..4].copy_from_slice(&VIRT_RECORDS_MAGIC.to_be_bytes());
        virt[8..10].copy_from_slice(&(self.virtuals.len() as u16).to_be_bytes());
        virt[10..12].copy_from_slice(&15u16.to_be_bytes());
        for (index, virtual_disk) in self.virtuals.iter().enumerate() {
            let entry = RECORDS_OFFSET + index * RECORD_BYTES;
            virt[entry..entry + 24].copy_from_slice(&virtual_disk.guid);
            virt[entry + 32] = virtual_disk.state;
            virt[entry + 33] = virtual_disk.init_state;
            let mut name = [b' '; 16];
            name[..virtual_disk.name.len()].copy_from_slice(virtual_disk.name.as_bytes());
            virt[entry + 48..entry + 64].copy_from_slice(&name);
        }
        seal(&mut virt);
        put(&mut copy, VIRT_AT, &virt);

        for (index, config) in configs.iter().enumerate() {
            put(
                &mut copy,
                CONFIG_AT + index * CONFIG_RECORD_SECTORS,
                &config.encode(),
            );
        }

        let mut data = vec![0xFFu8; SECTOR];
        data[0..4].copy_from_slice(&PHYS_DATA_MAGIC.to_be_bytes());
        data[8..32].copy_from_slice(&disk_guid(refnum));
        data[32..36].copy_from_slice(&refnum.to_be_bytes());
        seal(&mut data);
        put(&mut copy, DATA_AT, &data);

        image[at * SECTOR..(at + COPY_SECTORS) * SECTOR].copy_from_slice(&copy);
    }
}

/// Store the CRC of `bytes` in its CRC field.
pub(crate) fn seal(bytes: &mut [u8]) {
    let value = crc(bytes);
    bytes[4..8].copy_from_slice(&value.to_be_bytes());
}

fn put(copy: &mut [u8], sector: usize, bytes: &[u8]) {
    let at = sector * SECTOR;
    copy[at..at + bytes.len()].copy_from_slice(bytes);
}
