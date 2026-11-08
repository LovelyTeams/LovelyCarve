# Output-budget contract

`scripts/output-budget.sh` is the one canonical neutral wrapper for the
filesystem-driver family. It has no dependency on a VM harness, `chore`, or
Rust tooling. It only requires Bash and ordinary Unix utilities.

Every driver-family repository already resolves a pinned `../rust-fs-core`
sibling. Its repository-local tier adapter must invoke
`../rust-fs-core/scripts/output-budget.sh` directly. Consumers must not copy or
vendor the script: the pinned core checkout is both the dependency and the
version boundary, so fixes land once and every consumer receives them by
updating its core pin.

## Raising a budget

A budget is a measurement, not a preference, so a raise carries the measurement
that justifies it: run the tier, record the lines and bytes it actually
produced, and set the new ceiling above that number with the run named. A
budget moved to fit a run nobody measured is the same as no budget.

The two things not to do, in the order they are tempting:

- **Do not silence the run to fit.** Output that a budget makes inconvenient is
  still output somebody chose to print; dropping it to get under a ceiling
  removes the evidence rather than the noise.
- **Do not route around the wrapper.** A tier invoked directly is a tier with
  no budget, no log and no floor, and it reports exactly the same green.

Exit 65 is the status a reader meets having read nothing else, which is why the
verdict points here rather than assuming the rule is already known.

## Resolver contract for standalone clones

A standalone consumer checkout may not have initialized any siblings. The
consumer therefore checks in a small resolver, not a copy of this script. The
resolver has this order:

1. If `../rust-fs-core/scripts/output-budget.sh` exists, return it directly.
   This deliberately permits a developer to test coordinated, uncommitted
   core and driver changes.
2. Otherwise ask Cargo for the resolved `rust-fs-core` package. Package-based
   resolution requires `rust-fs-core` **0.2.11 or later**, because published
   crate archives are immutable. The package must contain this script and the
   resolver still verifies its API version and SHA-256.
3. Otherwise look in a user or repository tooling cache under a key containing
   the consumer's pinned full core commit, script API version, and expected
   SHA-256. Reuse the entry only after recomputing the digest and checking
   `--version`.
4. Otherwise download `scripts/output-budget.sh` from an immutable raw URL at
   that full core commit into a temporary file, verify its SHA-256 and
   `--version`, make it executable, then atomically install it in the cache.
5. Print only the resolved absolute path. Diagnostics go to stderr so command
   substitution remains safe.

The resolver's pin is consumer build metadata and must move in the same change
as the consumer's core pin. A mutable branch or tag URL is not a version
boundary, and a digest fetched from the same mutable location as the script is
not an integrity check.

**A digest pinned in each consumer was tried and dropped.** `rust-fs-ntfs`
carried one, and it made a comment added to this file a break in every
repository holding it until the pin was chased — the lockstep a single copy
exists to remove. Steps 3 and 4 still need a digest, because a cache entry and
a downloaded file have no other integrity story; steps 1 and 2 resolve a
checkout or an immutable crate archive, and there `--version` is the contract
this script publishes and the thing a consumer checks.

Offline + no sibling + no valid cached artifact cannot satisfy all three goals
(one canonical source, no checked-in copy, standalone operation). The resolver
must fail clearly in that case, naming the expected sibling path, pinned core
commit, cache path, and the command that can populate the cache. It must not
silently run an unverified script or fall back to an embedded copy.

The stable behavior is:

- a passing command is quiet and prints one verdict naming the full log;
- a failing command is quiet too: it prints a verdict naming its exit status,
  the log and the log's line count, and returns the command's own status;
- `--tail N`, or `OUTPUT_BUDGET_FAIL_TAIL=N`, prints the last N lines of the
  log on a failure. It defaults to 0, because the reader who pays most for a
  tail is an agent re-reading its transcript, and the assertion it wants is
  usually further up the log than its last page;
- a passing command over its measured line or byte budget returns 65, and the
  verdict names this document, because "raise the budget deliberately" does not
  by itself say what a deliberate raise has to carry;
- a passing, in-budget run exits 0 **even if its own verdict could not be
  written**. The script's status is a claim about the command it wrapped, so a
  caller whose stdout is closed or full must not read a green tier as a failing
  one. The write error still reaches stderr;
- `--verbose` or `OUTPUT_BUDGET_VERBOSE=1` streams output without lifting the
  budget, and a verbose failure prints no tail because the run was already on
  screen.

The environment variables are `OUTPUT_BUDGET_VERBOSE` and
`OUTPUT_BUDGET_FAIL_TAIL`. In `fs-linux-test-harness`, where this script was
written, they were `FLTH_VERBOSE` and `FLTH_FAIL_TAIL`; the names moved with
the script. That rename is the kind that fails silently — the old name is not
read and the run simply stays quiet — so the script reports a superseded name
on stderr rather than ignoring it. It does not honour one: a fallback keeps the
old name alive in habits and documentation forever.

Each consumer still owns its adapter, log location, measured budgets,
executed-test floors, and CI artifacts. Pointing at this script alone does not
make a consumer's test suite quiet or prove that its tests ran.

`tests/output_budget.rs` behavior-tests the canonical script here. Consumer
tests should verify that their tier adapter resolves the pinned sibling and
that every tier carries non-zero budgets and a floor; they should not duplicate
the script's behavior suite.

The Windows and Linux VM harnesses are infrastructure, not owners of another
permanent copy. A driver uses the pinned core script directly for local runs.
When it stages its test suite into a harness runtime, the staging step
materializes a fresh transient copy from the resolver's verified result,
records the core commit/API version/SHA-256 in the run manifest, and discards
the file with the run. The harness repository does not vendor the script and
must not reuse a prior run's staged file.

A future mechanism outside both the driver and harness dependency graphs could
also provide the runtime wrapper. Until then, the direction stays explicit:
core owns the script, drivers own their tier policy, and harnesses only receive
the staged runtime material.

## Why the wrapper did not move into the shared CI tooling (#156)

The same kind of duplication was ended one layer up: `tests/ci_aggregate_gate.rs`
existed in eleven repositories as ten distinct files. Two shared containers
were tried for those rules and both were reverted — `crates/am-ci-guard` here
(#157, reverted by #159) and a `ci:gate` subcommand inside `chore` (#158,
reverted). They were wrong the same way: each made cutting a release of a
shared tool a prerequisite for a change in this repository. The rules now live
in `scripts/ci-gate.sh`, run by `chore check:ci-gate`, which is this
repository's own file -- and, like `test-floor`, a family script every
repository runs from here as `scripts/core.sh ci-gate`. **The container changed twice; the conclusion below did
not**, and it is the conclusion this section exists to record — the obvious
next step is still to move this script into whatever shared thing exists, and
it is still rejected.

**A shell script cannot be delivered by `[dev-dependencies]` alone.** #153
first argued this from the guest: `rust-fs-ext4`, `rust-fs-xfs` and
`rust-fs-btrfs` run `chore test:vm`, the suite compiles and runs *inside* a
VM that has no cargo registry and no Rust toolchain, so a compiled entry
point would have to be cross-built and shipped there. **That premise is
wrong and #153 retracted it**: their `tier.sh` runs host-side and `exec`s
the wrapper; the *command* it wraps is what enters the guest, so nothing is
transferred and no consumer needed a transfer step. What survives the
retraction is the plainer half — the wrapper is the outermost thing in the
tier, it has to exist before any cargo command in that tier has run, and
`bash` is the only dependency it has. Rewriting it in Rust or Go would add
a build to the one step that must work before a build does.

**A crate already delivers it, and a second route would be the problem
again.** This script is inside the published `rust-fs-core` archive — `ci.yml`
has a step that fails the build if `cargo package --list` stops naming it —
and every converted consumer resolves it through `cargo metadata`, checking
`--version` before it runs. Adding a second delivery path would give one file
two ways to be reached, which is the shape of #153 ("sourced four different
ways"), not the fix for it.

**Moving the bytes is a cross-repo lockstep, not a refactor.** Every
consumer's resolver names this path — `scripts/output-budget.sh`, under a
sibling checkout or under the unpacked crate. Relocating it, even without
changing a character, breaks the sibling branch of all of them the moment a
developer has both checkouts. That is a deliberate sweep, not a side effect of
some other change.

**So: this file stays the one canonical copy, at this path.** `scripts/ci-gate.sh`
owns the CI *gate*; `scripts/output-budget.sh` owns test *output*.

#153's migration has since finished, and the count it was opened over is now
one. Read from each repository's default branch rather than remembered: no
repository in the family commits a `scripts/output-budget.sh` except this one.
`fs-linux-test-harness` deleted the other original, the four `rust-img-*`
crates and `go-networkfs` deleted their vendored copies, and
`fs-windows-test-harness` — the last holdout, and the one with a third
variable name — now carries a `scripts/resolve-output-budget.sh` instead. Every
other consumer resolves this file at run time from its tier adapter.

"byte for byte" is the one part of that sentence that has to go: it was true
only while `rust-fs-ntfs` pinned a SHA-256 of this file, and pinning a digest in
each consumer means a **comment** here breaks all of them until it is chased —
the lockstep a single copy exists to remove. Every consumer now verifies
`--version` instead, and treats a present-but-wrong copy as fatal rather than
as a reason to fall back.
