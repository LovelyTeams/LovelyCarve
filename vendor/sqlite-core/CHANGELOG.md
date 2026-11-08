# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.0](https://github.com/SecurityRonin/sqlite-forensic/compare/sqlite-core-v0.11.0...sqlite-core-v0.12.0) - 2026-08-21

### Fixed

- *(cell)* [**breaking**] refuse a negative serial type instead of wrapping it into a length ([#16](https://github.com/SecurityRonin/sqlite-forensic/pull/16))

## [0.11.0](https://github.com/SecurityRonin/sqlite-forensic/compare/sqlite-core-v0.10.3...sqlite-core-v0.11.0) - 2026-08-09

### Fixed

- *(test)* acknowledge sqlite3 statements instead of sleeping on them

### Other

- Merge remote-tracking branch 'origin/main' into try/sqlcipher

## [0.10.3](https://github.com/SecurityRonin/sqlite-forensic/compare/sqlite-core-v0.10.2...sqlite-core-v0.10.3) - 2026-07-25

### Fixed

- cap overflow-payload alloc against untrusted payload_len (fuzz alloc bomb)
- free_regions guards lo>=hi (clamp panicked on inverted range; fuzz-found panic-free violation)
# Changelog

All notable changes to `sqlite-core` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Releases are prepared by [release-plz](https://release-plz.dev/) from conventional commits.
