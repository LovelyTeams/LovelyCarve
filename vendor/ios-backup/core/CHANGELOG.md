# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/SecurityRonin/ios-backup-forensic/releases/tag/ios-backup-core-v0.1.0) - 2026-08-23

### Added

- *(vfs)* mount a backup through the tree seam, making the feature real ([#4](https://github.com/SecurityRonin/ios-backup-forensic/pull/4))
- *(encryption)* GREEN — survey the effective state, not the declaration
- *(logical)* GREEN — project a backup as a logical file container
- *(audit)* GREEN — the analyzer's observations
- *(backup)* GREEN — open, list and read a backup, encrypted or not
- *(crypto)* GREEN — keybag key derivation and AES unwrap/decrypt
- *(keybag)* GREEN — parse the BackupKeyBag TLV grammar

### Documentation

- supersede ADR-0005, whose route was never true of its consumer ([#3](https://github.com/SecurityRonin/ios-backup-forensic/pull/3))
- drop an intra-doc link to a cfg(test) module
- ADRs, PRD, validation, provenance, README, CI

### Fixed

- *(rows)* GREEN — count manifest rows that cannot be identified
- *(security)* GREEN — refuse a fileID that is not a SHA-1 digest
- *(size)* GREEN — an absent Size no longer reads a file as empty
- *(msrv)* declare 1.88, the floor the graph actually imposes
