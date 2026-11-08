// SPDX-License-Identifier: (MIT OR Apache-2.0)

use nom::bytes::complete::{tag, take};
use nom::combinator::{map, map_parser};
use nom::number::complete::*;
use nom::sequence::tuple;
use time::OffsetDateTime;

use super::both_endian::{both_endian16, both_endian32};
use super::date_time::date_time_ascii;
use super::directory_entry::{directory_entry, DirectoryEntryHeader};
use super::{character_encoding, decode_string, CharacterEncoding};
use crate::error::NomRes;
use crate::Result;

#[derive(Clone, Debug)]
pub struct VolumeDescriptorTable {
    pub system_identifier: String,
    pub volume_identifier: String,
    pub character_encoding: CharacterEncoding,
    pub volume_space_size: u32,
    pub volume_set_size: u16,
    pub volume_sequence_number: u16,
    pub logical_block_size: u16,

    pub path_table_size: u32,
    pub path_table_loc: u32,
    pub optional_path_table_loc: u32,

    pub root_directory_entry: DirectoryEntryHeader,
    pub root_directory_entry_identifier: String,

    pub volume_set_identifier: String,
    pub publisher_identifier: String,
    pub data_preparer_identifier: String,
    pub application_identifier: String,
    pub copyright_file_identifier: String,
    pub abstract_file_identifier: String,
    pub bibliographic_file_identifier: String,

    pub creation_time: OffsetDateTime,
    pub modification_time: OffsetDateTime,
    pub expiration_time: OffsetDateTime,
    pub effective_time: OffsetDateTime,

    pub file_structure_version: u8,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum VolumeDescriptor {
    Primary(VolumeDescriptorTable),
    Supplementary(VolumeDescriptorTable),
    BootRecord {
        boot_system_identifier: String,
        boot_identifier: String,
        data: Vec<u8>,
    },
    VolumeDescriptorSetTerminator,
}

impl VolumeDescriptor {
    pub fn parse(bytes: &[u8]) -> Result<Option<VolumeDescriptor>> {
        Ok(volume_descriptor(bytes)?.1)
    }
}

fn boot_record(i: &[u8]) -> NomRes<&[u8], VolumeDescriptor> {
    let (i, (boot_system_identifier, boot_identifier, data)): (&[u8], (String, _, _)) = tuple((
        map_parser(take(32usize), decode_string(CharacterEncoding::Iso9660)),
        map_parser(take(32usize), decode_string(CharacterEncoding::Iso9660)),
        take(1977usize),
    ))(i)?;

    Ok((
        i,
        VolumeDescriptor::BootRecord {
            boot_system_identifier,
            boot_identifier,
            data: data.to_vec(),
        },
    ))
}

fn volume_descriptor(i: &[u8]) -> NomRes<&[u8], Option<VolumeDescriptor>> {
    let (i, type_code) = le_u8(i)?;
    let (i, _) = tag("CD001\u{1}")(i)?;
    match type_code {
        0 => map(boot_record, Some)(i),
        1 => map(primary_descriptor, Some)(i),
        2 => map(supplementary_descriptor, Some)(i),
        //3 => map!(volume_partition_descriptor, Some)(i),
        255 => Ok((i, Some(VolumeDescriptor::VolumeDescriptorSetTerminator))),
        _ => Ok((i, None)),
    }
}

fn descriptor_table(i: &[u8]) -> NomRes<&[u8], VolumeDescriptorTable> {
    let (i, _) = take(1usize)(i)?; // padding
    let (i, system_identifier) = take(32usize)(i)?;
    let (i, volume_identifier) = take(32usize)(i)?;
    let (i, _) = take(8usize)(i)?; // padding
    let (i, volume_space_size) = both_endian32(i)?;
    let (i, character_encoding) = character_encoding(i)?;
    let (i, volume_set_size) = both_endian16(i)?;
    let (i, volume_sequence_number) = both_endian16(i)?;
    let (i, logical_block_size) = both_endian16(i)?;

    let (i, path_table_size) = both_endian32(i)?;
    let (i, path_table_loc) = le_u32(i)?;
    let (i, optional_path_table_loc) = le_u32(i)?;
    let (i, _) = take(4usize)(i)?; // path_table_loc_be
    let (i, _) = take(4usize)(i)?; // optional_path_table_loc_be

    let (i, root_directory_entry) = directory_entry(i, character_encoding)?;

    let (i, volume_set_identifier) = take(128usize)(i)?;
    let (i, publisher_identifier) = take(128usize)(i)?;
    let (i, data_preparer_identifier) = take(128usize)(i)?;
    let (i, application_identifier) = take(128usize)(i)?;
    let (i, copyright_file_identifier) = take(38usize)(i)?;
    let (i, abstract_file_identifier) = take(36usize)(i)?;
    let (i, bibliographic_file_identifier) = take(37usize)(i)?;

    let (i, creation_time) = date_time_ascii(i)?;
    let (i, modification_time) = date_time_ascii(i)?;
    let (i, expiration_time) = date_time_ascii(i)?;
    let (i, effective_time) = date_time_ascii(i)?;

    let (i, file_structure_version) = le_u8(i)?;

    let (_, system_identifier) = decode_string(character_encoding)(system_identifier)?;
    let (_, volume_identifier) = decode_string(character_encoding)(volume_identifier)?;
    let (_, volume_set_identifier) = decode_string(character_encoding)(volume_set_identifier)?;
    let (_, publisher_identifier) = decode_string(character_encoding)(publisher_identifier)?;
    let (_, data_preparer_identifier) =
        decode_string(character_encoding)(data_preparer_identifier)?;
    let (_, application_identifier) = decode_string(character_encoding)(application_identifier)?;
    let (_, copyright_file_identifier) =
        decode_string(character_encoding)(copyright_file_identifier)?;
    let (_, abstract_file_identifier) =
        decode_string(character_encoding)(abstract_file_identifier)?;
    let (_, bibliographic_file_identifier) =
        decode_string(CharacterEncoding::Iso9660)(bibliographic_file_identifier)?;

    Ok((
        i,
        VolumeDescriptorTable {
            system_identifier,
            volume_identifier,
            character_encoding,
            volume_space_size,
            volume_set_size,
            volume_sequence_number,
            logical_block_size,

            path_table_size,
            path_table_loc,
            optional_path_table_loc,

            root_directory_entry: root_directory_entry.0,
            root_directory_entry_identifier: root_directory_entry.1,

            volume_set_identifier,
            publisher_identifier,
            data_preparer_identifier,
            application_identifier,
            copyright_file_identifier,
            abstract_file_identifier,
            bibliographic_file_identifier,

            creation_time,
            modification_time,
            expiration_time,
            effective_time,

            file_structure_version,
        },
    ))
}

fn supplementary_descriptor(i: &[u8]) -> NomRes<&[u8], VolumeDescriptor> {
    map(descriptor_table, VolumeDescriptor::Supplementary)(i)
}

fn primary_descriptor(i: &[u8]) -> NomRes<&[u8], VolumeDescriptor> {
    map(descriptor_table, VolumeDescriptor::Primary)(i)
}
