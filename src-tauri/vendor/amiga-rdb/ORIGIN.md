# amiga-rdb 0.4.1

Simon Dick, Copyright 2026. MIT OR Apache-2.0; original license files retained.
https://github.com/sidick/amiga-rdb-rs
Published crate: https://static.crates.io/crates/amiga-rdb/amiga-rdb-0.4.1.crate
SHA-256: `362f43b2e4bb6094407669798235df5a9747d182eb74ce5aba2d65605b5ba6b3`.
Published VCS revision: `f46767166533f9cc17d1e84f006c75f545ae269d`.

Published source unchanged; standalone workspace marker added. The large
upstream lib.rs is retained intact for attribution and future comparison.
LovelyCarve implements only the read trait, bounds metadata reads, preserves
device spans separately from DosEnvec filesystem geometry, and explicitly
refuses nonempty BADB remapping. Driver bytes are never executed.
