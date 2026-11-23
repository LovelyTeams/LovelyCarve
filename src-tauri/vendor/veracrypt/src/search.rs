//! The header search: derive candidate header keys on worker threads and try
//! every cipher chain as soon as enough key bytes exist.
//!
//! Each PBKDF2 output block is its own job, so every PRF and every header
//! location runs side by side. Jobs that complete the first 64 key bytes
//! (enough for a single cipher, by far the most common choice) run first; an
//! unlock with the right password usually ends after them. Workers poll for a
//! stop every 1024 iterations. Argon2id cannot be interrupted, so it runs on
//! detached threads whose results are dropped once the search is over; it
//! starts only after TrueCrypt's counts and the first VeraCrypt SHA-512 key
//! failed, so the common volumes never pay for its memory.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::cipher::CipherChain;
use crate::error::{Result, VeraError};
use crate::header::{self, Decoded, Header, HEADER_LEN, HEADER_SECTOR_LEN, SALT_LEN};
use crate::kdf::{self, Family, Kdf, Prf, HEADER_KEY_LEN};
use crate::layout::HeaderKind;
use crate::xts::Xts;

/// How often an idle coordinator checks for cancellation.
const POLL: Duration = Duration::from_millis(50);
/// Key bytes each further cipher of a cascade needs.
const KEY_STEP: usize = 64;

/// How far a search has come, in finished derivation jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Jobs finished.
    pub done: u64,
    /// Jobs in the search.
    pub total: u64,
}

/// How a search runs.
pub struct Control<'a> {
    /// Polled from every worker; `true` ends the search with
    /// [`VeraError::Cancelled`].
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Called on the calling thread after each finished job.
    pub progress: Option<&'a (dyn Fn(Progress) + Sync)>,
    /// Worker threads; all available cores when `None`.
    pub threads: Option<usize>,
}

/// A header sector the search tries, with the derivations that apply to it.
pub(crate) struct Candidate {
    pub(crate) kind: HeaderKind,
    pub(crate) sector: [u8; HEADER_SECTOR_LEN],
    pub(crate) kdfs: Vec<Kdf>,
}

/// The header that opened.
pub(crate) struct Found {
    pub(crate) kind: HeaderKind,
    pub(crate) kdf: Kdf,
    pub(crate) chain: CipherChain,
    pub(crate) header: Header,
}

/// One candidate under one KDF.
#[derive(Clone, Copy)]
struct Spec {
    candidate: usize,
    kdf: Kdf,
}

/// The key a derivation has produced so far.
struct State {
    key: Zeroizing<[u8; HEADER_KEY_LEN]>,
    done_blocks: u32,
    tested_size: usize,
}

struct BlockJob {
    spec: usize,
    index: u32,
    phase: usize,
}

enum Event {
    Block {
        spec: usize,
        index: u32,
        bytes: Zeroizing<Vec<u8>>,
    },
    Argon2 {
        spec: usize,
        key: Result<Zeroizing<[u8; HEADER_KEY_LEN]>>,
    },
}

fn block_len(kdf: &Kdf) -> usize {
    kdf.prf.block_len().unwrap_or(HEADER_KEY_LEN)
}

/// The 64-byte step a PBKDF2 block's first byte falls in: blocks of phase
/// `s` complete the key of an `s`-cipher chain. TrueCrypt's cheap counts run
/// before everything (phase 0).
fn phase(kdf: &Kdf, index: u32) -> usize {
    if kdf.family == Family::TrueCrypt {
        return 0;
    }
    (index as usize - 1) * block_len(kdf) / KEY_STEP + 1
}

/// Search `candidates` for the header `password` opens.
pub(crate) fn search(
    candidates: &[Candidate],
    password: &[u8],
    control: &Control,
) -> Result<Found> {
    let specs: Vec<Spec> = candidates
        .iter()
        .enumerate()
        .flat_map(|(candidate, entry)| {
            entry.kdfs.iter().map(move |kdf| Spec {
                candidate,
                kdf: *kdf,
            })
        })
        .collect();
    if specs.is_empty() {
        return Err(VeraError::AuthenticationFailed);
    }
    let mut jobs: Vec<BlockJob> = specs
        .iter()
        .enumerate()
        .filter(|(_, spec)| spec.kdf.prf != Prf::Argon2id)
        .flat_map(|(index, spec)| {
            (1..=spec.kdf.block_count() as u32).map(move |block| BlockJob {
                spec: index,
                index: block,
                phase: phase(&spec.kdf, block),
            })
        })
        .collect();
    jobs.sort_by_key(|job| (job.phase, job.spec, job.index));

    let (sender, receiver) = mpsc::channel::<Event>();
    let argon2: Vec<usize> = specs
        .iter()
        .enumerate()
        .filter(|(_, spec)| spec.kdf.prf == Prf::Argon2id)
        .map(|(index, _)| index)
        .collect();
    let gate = ArgonGate {
        specs: argon2,
        truecrypt_jobs: jobs.iter().filter(|job| job.phase == 0).count(),
        // The first VeraCrypt PBKDF2 derivation (SHA-512 unless filtered).
        first_veracrypt: specs
            .iter()
            .position(|spec| spec.kdf.family == Family::VeraCrypt && spec.kdf.prf != Prf::Argon2id),
        password,
        sender: sender.clone(),
    };
    let mut states: Vec<State> = specs
        .iter()
        .map(|_| State {
            key: Zeroizing::new([0u8; HEADER_KEY_LEN]),
            done_blocks: 0,
            tested_size: 0,
        })
        .collect();
    let stop = AtomicBool::new(false);
    let cursor = AtomicUsize::new(0);
    let threads = control
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, usize::from))
        .clamp(1, jobs.len().max(1));
    let pending = jobs.len();
    let total = (jobs.len() + gate.specs.len()) as u64;

    std::thread::scope(|scope| {
        if !jobs.is_empty() {
            for _ in 0..threads {
                let worker = Worker {
                    jobs: &jobs,
                    specs: &specs,
                    candidates,
                    password,
                    stop: &stop,
                    cursor: &cursor,
                    cancelled: control.cancelled,
                    sender: sender.clone(),
                };
                scope.spawn(move || worker.run());
            }
        }
        drop(sender);
        let outcome = coordinate(
            &receiver,
            &specs,
            &mut states,
            candidates,
            Progression { pending, total },
            gate,
            control,
        );
        stop.store(true, Ordering::Relaxed);
        outcome
    })
}

/// Holds the Argon2id derivations back until the cheap and the default
/// derivations failed.
struct ArgonGate<'a> {
    specs: Vec<usize>,
    truecrypt_jobs: usize,
    first_veracrypt: Option<usize>,
    password: &'a [u8],
    sender: mpsc::Sender<Event>,
}

impl ArgonGate<'_> {
    /// Whether the held derivations may start, given how many TrueCrypt jobs
    /// reported and the derivation states.
    fn open(&self, truecrypt_done: usize, states: &[State]) -> bool {
        truecrypt_done >= self.truecrypt_jobs
            && self
                .first_veracrypt
                .is_none_or(|spec| states[spec].tested_size > 0)
    }

    /// Start every held derivation; returns how many started.
    fn release(&mut self, specs: &[Spec], candidates: &[Candidate]) -> usize {
        let mut started = 0;
        for spec in std::mem::take(&mut self.specs) {
            let salt = &candidates[specs[spec].candidate].sector[..SALT_LEN];
            if spawn_argon2(
                spec,
                specs[spec].kdf,
                self.password,
                salt,
                self.sender.clone(),
            ) {
                started += 1;
            }
        }
        started
    }
}

/// Start an Argon2id derivation that may outlive the search.
fn spawn_argon2(
    spec: usize,
    kdf: Kdf,
    password: &[u8],
    salt: &[u8],
    sender: mpsc::Sender<Event>,
) -> bool {
    let password = Zeroizing::new(password.to_vec());
    let salt = salt.to_vec();
    std::thread::Builder::new()
        .name("veracrypt-argon2id".into())
        .spawn(move || {
            let key = kdf::argon2id(&password, &salt, kdf.iterations, kdf.memory_kib);
            // A finished search no longer listens; dropping the key zeroizes it.
            if let Err(unsent) = sender.send(Event::Argon2 { spec, key }) {
                drop(unsent);
            }
        })
        .is_ok()
}

/// Jobs still to report, and all jobs of the search.
struct Progression {
    pending: usize,
    total: u64,
}

struct Worker<'a> {
    jobs: &'a [BlockJob],
    specs: &'a [Spec],
    candidates: &'a [Candidate],
    password: &'a [u8],
    stop: &'a AtomicBool,
    cursor: &'a AtomicUsize,
    cancelled: &'a (dyn Fn() -> bool + Sync),
    sender: mpsc::Sender<Event>,
}

impl Worker<'_> {
    fn run(self) {
        let halt = || self.stop.load(Ordering::Relaxed) || (self.cancelled)();
        while !halt() {
            let Some(job) = self.jobs.get(self.cursor.fetch_add(1, Ordering::Relaxed)) else {
                return;
            };
            let spec = self.specs[job.spec];
            let start = (job.index as usize - 1) * block_len(&spec.kdf);
            let mut bytes =
                Zeroizing::new(vec![0u8; block_len(&spec.kdf).min(HEADER_KEY_LEN - start)]);
            let salt = &self.candidates[spec.candidate].sector[..SALT_LEN];
            let derived = kdf::pbkdf2_block(
                spec.kdf.prf,
                self.password,
                salt,
                spec.kdf.iterations,
                job.index,
                &mut bytes,
                &halt,
            );
            if derived.is_err() {
                return;
            }
            let event = Event::Block {
                spec: job.spec,
                index: job.index,
                bytes,
            };
            if self.sender.send(event).is_err() {
                return;
            }
        }
    }
}

fn coordinate(
    receiver: &mpsc::Receiver<Event>,
    specs: &[Spec],
    states: &mut [State],
    candidates: &[Candidate],
    progression: Progression,
    mut gate: ArgonGate,
    control: &Control,
) -> Result<Found> {
    let Progression { mut pending, total } = progression;
    let mut done = 0u64;
    let mut truecrypt_done = 0;
    let mut failure = None;
    loop {
        if (control.cancelled)() {
            return Err(VeraError::Cancelled);
        }
        if !gate.specs.is_empty() && gate.open(truecrypt_done, states) {
            pending += gate.release(specs, candidates);
        }
        if pending == 0 {
            break;
        }
        let event = match receiver.recv_timeout(POLL) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        pending -= 1;
        done += 1;
        if let Some(progress) = control.progress {
            progress(Progress { done, total });
        }
        let spec_index = match event {
            Event::Block { spec, index, bytes } => {
                if specs[spec].kdf.family == Family::TrueCrypt {
                    truecrypt_done += 1;
                }
                let start = (index as usize - 1) * block_len(&specs[spec].kdf);
                let state = &mut states[spec];
                state.key[start..start + bytes.len()].copy_from_slice(&bytes);
                state.done_blocks |= 1 << (index - 1);
                spec
            }
            Event::Argon2 { spec, key: Ok(key) } => {
                let state = &mut states[spec];
                state.key = key;
                state.done_blocks = 1;
                spec
            }
            Event::Argon2 {
                key: Err(error), ..
            } => {
                failure = Some(error);
                continue;
            }
        };
        let spec = specs[spec_index];
        if let Some(found) = test(spec, &mut states[spec_index], &candidates[spec.candidate])? {
            return Ok(found);
        }
    }
    if (control.cancelled)() {
        return Err(VeraError::Cancelled);
    }
    Err(failure.unwrap_or(VeraError::AuthenticationFailed))
}

/// Try every chain the derivation's finished key prefix now covers.
fn test(spec: Spec, state: &mut State, candidate: &Candidate) -> Result<Option<Found>> {
    let leading = state.done_blocks.trailing_ones() as usize;
    let covered = (leading * block_len(&spec.kdf)).min(HEADER_KEY_LEN);
    while state.tested_size < 3 && (state.tested_size + 1) * KEY_STEP <= covered {
        state.tested_size += 1;
        let size = state.tested_size;
        for chain in CipherChain::ALL
            .into_iter()
            .filter(|chain| chain.ciphers().len() == size)
        {
            let xts = match Xts::new(chain, &state.key[..size * KEY_STEP]) {
                Ok(xts) => xts,
                // A cipher this build lacks cannot be the volume's.
                Err(VeraError::Unsupported(_)) => continue,
                Err(error) => return Err(error),
            };
            let mut decrypted = Zeroizing::new([0u8; HEADER_LEN]);
            decrypted.copy_from_slice(&candidate.sector[SALT_LEN..]);
            xts.decrypt(decrypted.as_mut(), 0);
            match header::decode(&decrypted) {
                Decoded::NotHeader => {}
                Decoded::Valid(header) => {
                    return Ok(Some(Found {
                        kind: candidate.kind,
                        kdf: spec.kdf,
                        chain,
                        header: *header,
                    }))
                }
                Decoded::Unsupported(reason) => return Err(VeraError::Unsupported(reason)),
            }
        }
    }
    Ok(None)
}
