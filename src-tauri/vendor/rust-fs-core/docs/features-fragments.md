# Features-page rows are fragments

Most repositories in the family keep `docs/features.md`: one table per `## `
section, a row per feature, with its state, the release it shipped in
(**Since**), its tracking issue and the test that checks it. Nearly every pull
request changes a row of it. Two pull requests that both edit the page touch
the same table, often neighbouring rows, so the merge queue drops one of them
as conflicting, exactly as it did for two pull requests that both edited the
top of `CHANGELOG.md`. Changelog entries became fragments for that reason
(agent-core v6, "Changelog entries are fragments"); features-page rows follow
the same convention, with the same two scripts' shape:

- `scripts/features-check.sh`, in every adopting repository's `chore lint`;
- `scripts/features-gather.sh VERSION`, in its release commit.

Both are family scripts: run in place from the `../rust-fs-core` checkout,
never copied (`family-check.sh` refuses a copy). They need `python3`, standard
library only, which `ci-gate.sh` already requires of every repository's lint.

## The convention

A pull request never edits `docs/features.md`. It adds one file under
`features.d/`, named for its branch like a changelog fragment
(`fix/run-end-bound` writes `features.d/fix-run-end-bound.md`), holding one or
more `## Section` headings the page already has, each over lines of three
kinds:

```markdown
## Reading

| Inodes: v1, v2, v3 cores, `bigtime` | Supported | 0.1.0; v3 checksums Unreleased (#512) | | `inode_oracle.rs` |
| Parent pointers read | Experimental | Unreleased (#512) | | `parent_oracle.rs` |

rename: Old feature name
| New feature name | Supported | 0.4.0; renamed Unreleased (#512) | | `x.rs` |

## Writing

remove: A feature that is gone
```

- **A row**, in that section's column layout (the same number of cells as its
  header). Its first cell is its **key**. A row whose key the section already
  has replaces that row where it stands; a new key is appended to the end of
  the section's table.
- **`remove: KEY`** deletes the row whose key is exactly `KEY`.
- **`rename: KEY`**, followed on the next line by a row, replaces the row whose
  key is exactly `KEY` with that row, in the same place, under the new row's
  key.

Keys are compared exactly, after trimming the cell: case, backticks and
punctuation all count. A `remove:` or `rename:` of a key the section does not
hold, or holds more than once, is refused, as is a rename onto a key the
section already holds.

**Since** says `Unreleased (#N)`, or ends with it (`0.8.0 (#99); writes
Unreleased (#373)`), until a release. The release turns every `Unreleased` in
a Since cell into the version it ships as.

**One fragment writes a row.** Two fragments that write the same key of the
same section -- add, replace, remove, or either side of a rename -- are
refused, naming both files. Two pull requests that both change one row are a
real conflict, not a textual one: the second edits the fragment that already
writes the row rather than adding its own. Pull requests that change different
rows never conflict, whatever their order in the queue.

## What the scripts do

`features-check.sh` (`chore lint`, and so CI and the merge queue's run of the
queued group) refuses, naming the file and line:

- a `features.d/` entry that is not a `*.md` fragment, its `README.md` or a
  `.gitkeep`;
- a fragment that is empty, has text before its first `## ` heading, a heading
  of another level, a heading with no rows, a line that is not a row, a
  `remove:`, a `rename:` or a heading, a separator or header row (the page
  supplies the header), or a row whose first cell is empty;
- a section the page does not have, has twice, or holds no table or more than
  one table under; a row with the wrong number of cells; a `remove:` or
  `rename:` that does not apply; two fragments (or one, twice) writing the same
  key of the same section;
- **a Since cell on the page that says `Unreleased`**: an unreleased row is a
  fragment until a release gathers it, as an unreleased changelog entry is.

A repository with no `docs/features.md` passes. A page with no `features.d/`
beside it fails, as `changelog-check.sh` fails without `changelog.d/`.

`features-gather.sh VERSION` (the release commit, beside
`changelog-gather.sh`) applies every fragment in file-name order, turns every
`Unreleased` in a Since cell into `VERSION` -- in the rows it wrote and in any
the page still held -- and deletes the fragments (`git rm` for tracked ones, so
the release commit records it). Everything is checked before anything is
written: a refusal changes neither the page nor the fragments. With no
fragments it still moves any `Unreleased` left on the page; with no page and no
fragments it does nothing. Prose is never touched: only table cells under a
`Since` header are.

## Adopting it

One pull request per repository, after a rust-fs-core release carries the
scripts.

1. **Pin the release** that carries `features-check.sh` and
   `features-gather.sh` (`FS_CORE_REF` in `chores.yml`, and wherever else the
   repository pins rust-fs-core's checkout).

2. **`chore lint`** runs the check beside the changelog one:

   ```yaml
   lint:
     cmds:
       - bash ../rust-fs-core/scripts/agents-core-check.sh
       - bash ../rust-fs-core/scripts/changelog-check.sh
       - bash ../rust-fs-core/scripts/features-check.sh
   ```

   A repository whose CI runs `changelog-check.sh` directly rather than
   through `chore lint` adds the same step there.

3. **The release commit** gathers both, from the repository's root:

   ```sh
   bash ../rust-fs-core/scripts/changelog-gather.sh VERSION
   bash ../rust-fs-core/scripts/features-gather.sh VERSION
   ```

4. **`features.d/README.md`**, so the directory exists and says what it is
   for:

   ```markdown
   # features.d

   Rows of [`docs/features.md`](../docs/features.md) that have not been
   released yet, one file per pull request. A release applies them to the
   page and deletes them, so this directory holds only what the next release
   will change ("Keep the features page current" in AGENTS.md).

   A fragment is named for its branch -- `fix/run-end-bound` writes
   `fix-run-end-bound.md` -- and holds one or more `## Section` headings the
   page has, each over rows in that section's columns, `remove: KEY` lines or
   `rename: KEY` lines followed by the row that replaces it:

       ## Reading

       | Feature name | Supported | Unreleased (#N) | | `test_file.rs` |

   A row's first cell is its key: an existing key replaces that row in place,
   a new key is appended to the section. One fragment writes a row; to change
   a row another unreleased fragment writes, edit that fragment.

   - `../rust-fs-core/scripts/features-check.sh` (run in place, in `chore
     lint`) refuses a fragment that does not apply, and an `Unreleased` row
     written into the page directly.
   - `../rust-fs-core/scripts/features-gather.sh VERSION` is the release step.
   ```

5. **Move the page's own `Unreleased` rows into a fragment**, named for the
   adoption branch, in the same pull request: a row that is wholly new leaves
   the page and becomes a row of the fragment (the release appends it to its
   section); a row whose Since cell only gained an `Unreleased` part goes back
   to what it said at the last release on the page, and the fragment carries
   the row as it reads now. `features-check.sh` fails until no Since cell on
   the page says `Unreleased`.

6. **AGENTS.md**: replace the repository's "Keep the features page current"
   section with:

   ```markdown
   ## Keep the features page current

   `docs/features.md` lists every feature with its state, the release it
   shipped in, its tracking issue and the test that checks it. **A pull
   request that adds, fixes, refuses or removes behaviour records its row in
   the same pull request** -- as a fragment, never as an edit to the page. A
   finding that leaves something unsupported gets a row too, naming its
   issue.

   A pull request adds one file under `features.d/`, named for its branch
   like its changelog fragment (`fix/run-end-bound` writes
   `features.d/fix-run-end-bound.md`), holding one or more `## Section`
   headings of the page, each over rows in that section's columns, with the
   PR number under **Since** (`Unreleased (#N)`). A row whose first cell, the
   Feature, matches an existing row replaces it in place; a new one is
   appended to the section. `remove: KEY` deletes a row and `rename: KEY`
   followed by a row replaces it under a new key, both by the exact Feature
   text. One fragment writes a row: to change a row another unreleased
   fragment writes, edit that fragment. Pull requests open at the same time
   then never conflict over neighbouring rows of one table.

   A release applies them: `../rust-fs-core/scripts/features-gather.sh
   VERSION` writes every fragment into the page, turns every `Unreleased`
   into the version, and deletes the fragments, in the release commit.
   `chore lint` runs `features-check.sh`: every fragment applies to the page,
   and the page holds no `Unreleased` row of its own. The convention is
   rust-fs-core's `docs/features-fragments.md`.
   ```

   and the page's own introduction, where it says every pull request updates
   its row on the page, says it adds a row to `features.d/` instead.
