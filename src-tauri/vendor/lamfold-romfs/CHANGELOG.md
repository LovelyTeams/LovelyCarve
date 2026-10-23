# Changelog

All notable changes to lamfold-romfs are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-10-15

### Fixed

- **Hard links read as empty `Other` entries.** genromfs writes the second and
  later names of a file as type-0 headers naming the first; they resolve to
  that file now, same node and same data.

### Changed

- The description names Lamco Development LLC; the README gains website,
  documentation, install and contributing sections; the crate ships this
  changelog.

## [0.1.0] - 2026-06-06

- First release: Linux romfs over the lamfold substrate.
