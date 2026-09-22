# Scan performance

Notes from making a whole-disk scan fast enough to be worth waiting for, on macOS first and then on
Linux. Everything below was measured with the `--benchmark` harness that ships in the binary, so the
numbers are reproducible rather than remembered.

Read in order, the file is a record of being wrong in useful ways: the macOS findings at the top
set expectations for Linux that the Linux measurements then contradicted. Where a later section
supersedes an earlier one there is a note saying so — the earlier claim is kept rather than edited
away, because what looked true and why is the useful part.

## Test machine

| | |
| --- | --- |
| Hardware | Apple M4 Pro, 14 cores |
| OS | macOS 26.6.2 (arm64) |
| Filesystem | APFS, 926 GiB volume, 884 GiB used, ~11M inodes |
| Layout | sealed system volume at `/` (`/dev/disk3s1s1`), data volume at `/System/Volumes/Data` (`/dev/disk3s5`) |

All runs were warm-cache: the same scan repeated back-to-back gave times within a few percent, so
these measure CPU and kernel work, not disk reads. A cold-cache scan of a mechanical disk would be
dominated by seeks and none of this would matter much.

## Results

Whole disk, `/`, both stages from one run of the same binary:

| | time | entries | unreadable | reported size |
| --- | --- | --- | --- | --- |
| before (`dua-tree`) | 59.9s | 20,300,480 | 986 | 1.4 TiB |
| after (`tree`) | 36.5s | 10,409,705 | 479 | 716.5 GiB |
| after, with `-x` | 36.4s | 10,389,061 | 473 | 688.8 GiB |

The entry count is the more interesting column. The volume has ~11M inodes, so the old scan was
visiting nearly everything twice, and the 1.4 TiB it reported was about 1.6x the 884 GiB actually
in use. It was not just slow, it was wrong.

Run-to-run variance is a few percent, and a whole-disk scan moves a little between runs as the
machine writes files; quieter runs of the new path landed at 35–37s. Treat the 1.5x as the claim,
not the individual seconds.

## How to reproduce

```sh
cargo build --release
./target/release/diskonaut --benchmark /                       # all stages
./target/release/diskonaut --benchmark --bench-stage pipeline / # just the app's real path
./target/release/diskonaut --benchmark --max-depth 4 /          # partial scan, fast iteration
./target/release/diskonaut --benchmark --threads 6 --bench-repeat 3 /
```

The stages nest, so subtracting one from the next attributes cost to a layer:

| Stage | What it measures |
| --- | --- |
| `dua-walk` | the general-purpose `dua-core` walk alone, entries discarded |
| `dua-tree` | that walk feeding the folder tree |
| `walk` | the walk diskonaut uses now, alone |
| `tree` | that walk feeding the folder tree |
| `tree-only` | the folder tree alone — entries are collected first, untimed, then fed to the model |
| `pipeline` | scan and tree build on separate threads, as the app runs them |

`dua-*` against the others is a like-for-like walker comparison on the same tree. `walk` against
`tree` is the cost of the data model. `tree` against `pipeline` is the cost of the channel.
`tree-only` is the model's cost with the walk taken out of the measurement entirely — use it rather
than the `walk`/`tree` subtraction, which on Linux conflates the model with the walk stalling
behind a busy consumer.

Useful alongside it: `/usr/bin/time -l` (macOS) or `/usr/bin/time -v` (Linux) to see the
user/system split. That split is what turned out to matter most.

## Findings

### 1. The walk is everything; the data model is free

The first measurement worth taking. On a 3.07M-entry single-volume tree, warm, every stage comes
out within a few percent of every other:

```
dua-walk      7.127s   3073489 entries   431232 entries/s
dua-tree      7.975s   3073489 entries   385380 entries/s
walk          8.188s   3073488 entries   375377 entries/s
tree          8.363s   3073488 entries   367492 entries/s
pipeline      8.441s   3073488 entries   364106 entries/s
```

Building the folder tree, allocating a `PathBuf` per entry, and shipping everything across a
channel together cost a couple of percent against the traversal. On the whole disk the tree build
does not register at all — `walk` 37.99s against `tree` 38.02s over 10.4M entries, a difference
smaller than the run-to-run variance.

This is worth internalising before optimising anything here. The obvious-looking targets in the
data model — the per-entry `sync_channel(1)`, the `O(depth²)` allocation in the recursive insert —
are real inefficiencies and were worth fixing, but fixing them alone would have changed the
whole-disk time by a couple of percent. **Measure the walker first.**

### 2. `/` was traversed twice, and that was most of the win

macOS presents the data volume in two places at once: mounted at `/System/Volumes/Data`, and
grafted into `/` through *firmlinks* (`/Users`, `/Applications`, `/Library`, … are firmlinks whose
targets live on the data volume). A walk that follows both arrives at every user file twice.

Hence 20.4M entries against ~11M inodes, and 1.4 TiB against 884 GiB used.

Detecting this is fiddly because **`/` and `/System/Volumes/Data` report the same `st_dev`**
(16777233 on this machine). The usual `du -x` trick of comparing device numbers does not see the
boundary at all:

```
$ stat -f "%d %N" / /System /System/Volumes/Data /Users
16777233 /
16777233 /System
16777233 /System/Volumes/Data
16777233 /Users
```

What does work is comparing inodes across the mount. `getattrlistbulk` enumerates without crossing
mount points, so a directory that something is mounted over is *listed* with the inode of the
directory it covers; `open`ing it does cross, so `fstat` on the descriptor returns the mounted
volume's root inode instead. When those two disagree, something is mounted there:

```rust
if read.inode != job.listed_inode && !job.firmlink {
    continue;   // skip: this volume gets scanned on its own terms, if at all
}
```

Firmlinks are the deliberate exception — they are the only route to what they point at, so they
stay followed. `SF_FIRMLINK` (`0x00800000`) in the entry's `ATTR_CMN_FLAGS` identifies them.

The skip is deliberately narrow: it applies only where the mount leads back to the filesystem the
scan started on, which is what makes it a *second* route to files already being counted. Genuinely
separate filesystems are crossed by default, like `du`, and `-x` / `--one-file-system` declines to
cross those too. On this machine the difference is the auxiliary APFS volumes (`VM`, `Preboot`,
`Update`, `xarts`, `iSCPreboot`, `Hardware`), worth 27.7 GiB and about 20,000 entries.

An earlier version of this fix skipped *every* mount point, which also stops the double count but
takes `-x` behaviour away from anyone who wanted the default. Worth noting because it looks
equivalent from the `/` benchmark alone; the difference only shows on a machine with other volumes
mounted, and scanning a directory that contains nothing but mount points then reports nothing at
all.

One known limitation: because the discriminator is "same filesystem as the scan root", scanning
`/System/Volumes` directly will not descend into `Data`, even though nothing else in that scan
reaches it. Scanning `/` or `/System/Volumes/Data` behaves as expected.

### 3. More threads is slower

Counterintuitive and the most portable finding. On a 14-core machine, whole-disk scan time by
worker count:

| threads | time |
| --- | --- |
| 4 | 42.5s |
| 6 | **35.3s** |
| 8 | 36.8s |
| 10 | 41.1s |
| 12 | (worse) |
| 14 (all cores) | (worse) |

The reason shows in the user/system split. At 8 threads a whole-disk scan spends **185s of system
time against 7s of user time**. The work is essentially all in the kernel, and past a handful of
threads the extra workers spend their time contending on filesystem locks rather than reading
anything. Single-threaded, the same scan costs ~5µs of kernel time per entry; at 14 threads it
costs ~28µs per entry.

For comparison, `find ~/project | wc -l` over a 3M-entry tree used ~3 cores and 17.7s of system
time, and finished in 6.3s — less kernel time than our 14-thread run and faster wall-clock.

`thread_count()` therefore caps workers at `min(cores, 8)`.

> **The explanation here does not hold on Linux — see "Linux XFS" below.** On a 32-core Linux box
> the same cliff appears on both XFS and ext4, but 16 *independent* walker processes scale to 13.3M
> entries/s against the same tree, so the kernel and the filesystem are not the limit; jwalk's
> thread model is. The cap is still worth keeping; the reason given for it is not the reason.

**This cap is a macOS/APFS measurement and should be re-derived on Linux**, where the contention profile of ext4/xfs/btrfs is different.

### 4. The lean attribute set was not, by itself, the win

Worth recording because it is the finding that contradicts the obvious story.

`dua-core` reproduces the Apple FTS contract: it requests the full `stat` attribute set for every
entry (`ATTR_CMN_CRTIME`, `MODTIME`, `CHGTIME`, `ACCTIME`, `OWNERID`, `GRPID`, `ACCESSMASK`,
`FLAGS`, `FILEID`, plus `ATTR_FILE_LINKCOUNT`, `ALLOCSIZE`, `IOBLOCKSIZE`, `DEVTYPE`,
`DATALENGTH`), and because bulk enumeration does not synthesize `stat` fields for directories, it
follows up with a **path-based `lstat` on every directory**. The replacement asks for a name, an
object type, flags, an inode, and one size field, and never leaves the bulk call.

That sounded like it should be the whole story. It is not. On a single-volume subtree with no
duplication the lean walker is **not faster — in the numbers above it is marginally slower**
(`walk` 8.19s against `dua-walk` 7.13s on the same tree). The saved syscall per directory and the
smaller attribute set are real but small: directories are a minority of entries, and the kernel's
cost per entry is dominated by fetching the inode record at all, which neither walker avoids.

**The whole-disk win is structural: not visiting 10M redundant entries.** The lean walker's value
is that it is ours, so the mount logic could live in it — not that it shaves microseconds. If a
future change makes `dua-core` skip duplicate volumes, most of this file's macOS-specific code
could be deleted and little speed would be lost.

### 5. A parser bug that hid inside a plausible number

Recorded as a cautionary tale, because the wrong version looked *better*.

`ATTR_FILE_*` attributes do not apply to directories, and — unlike invalid attributes under
`FSOPT_PACK_INVAL_ATTRS` — they are **omitted from a directory's record entirely rather than
zero-filled**. The first parser read the size field unconditionally, so on a directory record it
ran past the end of the record. For a directory whose name was ≥8 bytes it silently read name
bytes as a size; for a shorter name the cursor ran out and the entry was dropped — taking that
directory's entire subtree with it.

The resulting scan of `/` reported 7.7M entries and 768 GiB. That is *closer to the truth* than
the 20.4M/1.4 TiB it replaced, and it was tempting to read it as the mount fix working. It was
not: the mount fix was not working at all, and two bugs were partly cancelling.

What caught it was the unrelated-looking 233,390 "unreadable" count against `dua-core`'s 1,028.
The fix is to consult the returned-attributes bitmap before reading the field:

```rust
let size = if returned_file & size_attribute != 0 { cursor.u64()? } else { 0 };
```

Lesson: a total that moves toward the expected value is not evidence the intended change worked.
Check the entry count and the error count too.

### 6. Secondary fixes, worth doing but individually small

- Entries reached the UI thread one per message through a `sync_channel(1)`. Millions of
  round-trips; now batched at 4096 entries.
- `std::fs::Metadata` (~100 bytes) travelled with every entry. Replaced with `EntryMeta` — one
  `u64` and a `bool`.
- The tree insert recursed from the root per entry, rebuilding a `PathBuf` suffix at every level
  (`O(depth²)` allocations per file). Now entries arrive grouped by directory and
  `Folder::add_dir_entries` resolves the parent once per directory, then inserts the whole batch.
- `FileTree::add_entry` recomputed `path_in_filesystem.components().count()` for every entry.

### 7. What the audit caught afterwards

The performance work was reviewed after the fact, and the review found more than the performance
work did. Recorded because the same traps are waiting on Linux.

**Dropping the walk early did not stop it.** `BulkWalk::drop` drained the channel to release
workers blocked on a full send — but draining guarantees every send *succeeds*, so the workers
happily walked the entire remaining tree while the consumer waited to join them. Quitting
diskonaut partway through a scan of `/` took **32.2s**; with a `stop` flag checked in the queue's
`pop()`, it takes **0.1s**. `dua-core` had a stop flag and this walker did not, which is exactly
the kind of thing you lose when replacing a mature component.

The measurement is easy to repeat and worth repeating on any walker: start a scan of something
large, quit a few seconds in, and time how long the process takes to exit.

**The Linux fallback grouping dropped an entry at every directory boundary.** `group_by_directory`
declared its accumulator *inside* the `iter::from_fn` closure, so it was reset on every call
rather than held across them; the freshly started group — which already held the first entry of
the new directory — was discarded on return. The first entry of every directory after the first
was lost, along with the entire final group. On a Linux scan that is missing files and understated
sizes scaling with directory count.

It survived review, compilation and clippy because on macOS it is `cfg`-selected away and never
runs. There are now tests (`scan::tests::fallback_grouping_reports_every_entry_exactly_once` and
friends) that call it directly on every platform, and assert every entry appears exactly once.
**Type-checking dead code is not testing it.**

**The `dua-core` walk yields the scan root itself first**, with the root's *parent* as its parent
path. Grouping that entry produced a batch addressed outside the tree, which
`FileTree::add_dir_entries` folded into the base folder because it located a relative path by
skipping a component count. The result was a phantom empty folder named after the scan root,
inside the scan root. Both halves are fixed: depth-0 entries are skipped, and `add_dir_entries`
now uses `strip_prefix` and ignores anything outside the tree.

**`--max-depth` meant different depths in the two walkers**, so the benchmark was not comparing
like with like under that flag. They now agree exactly:

```
max-depth=2:  dua-walk 804 entries / 40.7 MiB      walk 803 entries / 40.7 MiB
max-depth=3:  dua-walk 10110 entries / 193.4 MiB   walk 10109 entries / 193.4 MiB
```

The remaining single entry is dua's own root entry, which the model ignores either way.

**`--max-depth`, `--threads` and `--single-thread` did nothing outside `--benchmark`** — the app's
scan path hardcoded its options. Now wired through.

**The record parser trusted a layout it had not checked.** It verified `ATTR_CMN_RETURNED_ATTRS`
but then read the error, name, type, flags and inode fields unconditionally. On a filesystem that
does not vend one of them — plausible for SMB, NFS, FUSE or exFAT, none of which could be tested
here — every following field shifts, and a garbage inode makes the mount check fire and silently
discard an entire subtree. The parser now requires the full set and falls back to `readdir` +
`lstat` for that directory when it is not there. On APFS the guard never fires: entry counts and
totals are unchanged.

### 8. Still open

Two audit findings were deliberately left alone, both pre-existing:

Both have since been dealt with: `delete_path` now subtracts the folder itself along with its
contents, and hard links are counted once per folder (see above).

## What a folder's size means, and hard links

A folder's size answers **"how much space is held under here"**: every distinct file beneath it,
counted once. Hard links make that different from "the sum of the entries", because one file can
be reached by several names.

The rule is per folder. A file counts once in any folder that can reach it, and once in any folder
above that — but never twice in the same folder. With `a/a`, `a/b` and `b/a` all links to the same
1 KiB file:

```
root   1 KiB     one file, however many names point at it
├── a  1 KiB     a/a and a/b are the same blocks
└── b  1 KiB     b/a is those same blocks, held here too
```

Deleting `a` on its own frees nothing, and neither does deleting `b`; deleting both frees 1 KiB.
That is a real property of hard links, not an artefact of the accounting.

`du` answers a different question. It deduplicates across the whole run in traversal order, so the
first directory it happens to visit is charged and the rest show nothing:

```
$ du -sk a b .
4    a
0    b        <- not "b holds nothing", just "b was visited second"
4    .
```

Which directory gets the 4 KiB depends on the order of the walk. diskonaut's answer does not: the
tests cover both orderings, and `HardLinks::charge` is deliberately order-independent.

### The gotchas

**Sizes are not additive.** A folder's size can be less than the sum of its children's sizes, and
the file tiles inside a folder can add up to more than the folder they sit in. In the example
above `a` is 1 KiB while the two files inside it each show 1 KiB. Both numbers are correct answers
to different questions — the tile shows how big that file is, the folder shows how much space it
holds — but they will not reconcile by addition wherever hard links are involved.

**Deleting one link frees nothing.** Space comes back only when the last link goes. diskonaut
subtracts the file's full size from its ancestors on delete, so after removing one of several
links the "space freed" figure and the folder sizes are optimistic until the last one is gone. The
subtraction saturates at zero so the tree cannot go negative, and a rescan always restores the
truth.

**Identity is the inode number plus the size.** Inode numbers are unique only within a filesystem,
and a scan of `/` on macOS spans a volume group whose volumes number their inodes independently.
Two entries claiming one inode but different sizes are therefore treated as different files. Two
genuinely different same-size files that collide on an inode number across volumes would be
merged; that needs both to be hard-linked as well, which makes it unlikely rather than impossible.

**Only files with more than one link are tracked.** Everything else takes the plain additive path,
which is what keeps the cost invisible: a whole-disk scan here found about 20,000 distinct
hard-linked files out of 10.4M entries, so the ledger is negligible and the measured scan time did
not move. It did move the total, by about 16 GiB — that much of the disk was being counted twice.

The walk supplies the two fields this needs, `EntryMeta::inode` and `EntryMeta::links`. Anything
that does not fill them in gets the old additive behaviour rather than a wrong answer.

## What is macOS-specific

| Piece | Portable? |
| --- | --- |
| Benchmark harness, stages, flags | yes, already builds and runs everywhere |
| "Measure the walker before the model" | yes |
| "More threads can be slower" | yes as a phenomenon; the number 8 is not |
| Batching, `EntryMeta`, per-directory tree insert | yes, already in shared code |
| `getattrlistbulk` walker (`scan/bulk.rs`) | no, `#[cfg(target_os = "macos")]` |
| Firmlink handling | no, macOS has no counterpart elsewhere |
| Inode-vs-listed-inode mount detection | the *technique* ports; on Linux `st_dev` is simpler and sufficient |

Non-macOS builds use `fallback::group_by_directory` in `libdiskonaut/src/scan/mod.rs`, which groups
the `dua-core` walk into per-directory batches so the rest of the pipeline is identical. It is
compiled on every platform (`#[cfg_attr(target_os = "macos", allow(dead_code))]`) and the tests in
`scan/tests.rs` call it directly everywhere, so it is exercised on macOS even though it is never
selected there. It still runs for real only on Linux — **run the test suite first and trust it
less than the numbers.**

## Repeating this on Linux

### Start here

1. `cargo build --release && cargo test` — the fallback path is exercised by tests but has never
   run against a real filesystem at scale. Start there.
2. Establish the baseline and confirm where the time goes:
   ```sh
   /usr/bin/time -v ./target/release/diskonaut --benchmark --bench-stage all /
   ```
   Note the user/system split. If system time dwarfs user time as it does on macOS, the work is in
   the kernel and the data model is not the problem.
3. Sweep the worker count before optimising anything — it may be the largest single lever and
   costs nothing to find:
   ```sh
   for t in 1 2 4 6 8 12 16 24 32; do
     ./target/release/diskonaut --benchmark --bench-stage pipeline --threads $t / | tail -1 |
       sed "s/^/threads=$t /"
   done
   ```
4. Sanity-check the total against `df` and the entry count against `df -i`. A scan that reports
   more entries than the filesystem has inodes is traversing something twice.
5. Time quitting mid-scan. Start a scan of `/`, press `q` then `y` a few seconds in, and check the
   process exits immediately rather than finishing the walk first — see finding 7.

### The structural difference to expect

This is the part that does not carry over, and it is the important one.

`getattrlistbulk(2)` returns **names and sizes together** in one call per directory's worth of entries.
Linux has no such syscall. `getdents64(2)` returns names, inode numbers and a type hint (`d_type`),
but **no size** — so a size still costs a `statx`/`fstatat` per file. The per-entry syscall that
macOS avoids is unavoidable in the portable Linux path.

That reframes the problem. On macOS the question was "what is the walker doing that it needn't
be"; on Linux it will be "how do we make several million `statx` calls cheaply, or avoid them".

Candidates, roughly in order of expected value. The first is portable; the rest trade portability
or privilege for speed, which is how WizTree gets its numbers on Windows (it reads the NTFS MFT
directly rather than asking about files one at a time).

- **Batch the `statx` calls with `io_uring`.** `IORING_OP_STATX` lets thousands of stats be
  submitted with a single syscall, which directly attacks the per-entry syscall cost. Probably the
  best portable-in-practice win; needs a reasonably modern kernel and a dependency such as
  `io-uring` or `tokio-uring`.
- **`statx(AT_STATX_DONT_SYNC)`** with a minimal `mask` (`STATX_TYPE | STATX_BLOCKS`, or
  `STATX_SIZE` for apparent size). Cheap to try, avoids revalidation on network filesystems, and
  asking for less may let some filesystems do less.
- **Trust `d_type` from `getdents64`** to decide what to descend into, so only non-directories need
  a stat, and skip stats entirely for entry kinds that cannot hold data. Note `d_type` is
  `DT_UNKNOWN` on some filesystems, so a fallback is required.
- **Filesystem-specific bulk paths**, worth measuring as an upper bound even if not shipped:
  - XFS has `XFS_IOC_BULKSTAT`, which returns inode stat records in bulk — the nearest thing Linux
    has to reading the metadata table directly.
  - btrfs has `BTRFS_IOC_TREE_SEARCH_V2`, which can read directory and inode items straight out of
    the filesystem trees.
  - ext4 has no supported userspace bulk-metadata API. Raw inode-table reads (the `e2image`
    approach) need root and a quiescent filesystem, and are not appropriate for a live tool.

  All of these need `CAP_SYS_ADMIN` or root, and each covers one filesystem, so they belong behind
  a capability check with the generic path as the fallback — if they are pursued at all. Verify the
  details against current man pages; they are cited here from background knowledge, not tested.

> **Tested since, and mostly wrong — see "Linux XFS" below.** io_uring `statx` is 3.8x *slower*
> than a plain `statx`, not the best win; the minimal mask, `AT_STATX_DONT_SYNC` and `d_type`-based
> stat elision are all worth nothing measurable; `XFS_IOC_BULKSTAT` is indeed `EPERM` unprivileged
> *and* returns no paths, so it cannot replace a walker; and `XFS_IOC_GETFSMAP` is the exception to
> "all of these need root" — it is callable by any user, but redacts every owner, which makes it
> useless for this purpose by a different route.

### Correctness requirements on Linux

The firmlink problem is macOS-only, but duplicated and runaway traversal have Linux analogues, and
they matter just as much as speed:

- **Pseudo-filesystems.** `/proc`, `/sys`, `/dev`, `/run` must not be walked as if they held data.
  `/proc` in particular is effectively unbounded.
- **Mount points.** Unlike macOS, `st_dev` genuinely differs across a Linux mount, so comparing a
  directory's `st_dev` with its parent's is sufficient — the inode comparison used on macOS is not
  needed. `du -x` semantics: do not cross, except for the scan root itself.
- **Bind mounts** make the same subtree reachable at two paths with the same `st_dev`, which the
  device check will not catch. `/proc/self/mountinfo` enumerates them.
- **btrfs subvolumes** report differing `st_dev` values within one filesystem, so a naive
  device check will *under*-count by refusing to descend. Check against `mountinfo`.
- **Hard links** are handled by `HardLinks` in the shared model, keyed on the inode number and
  size reported by the walk, so a Linux walker gets the behaviour for free as long as it fills in
  `EntryMeta::inode` and `EntryMeta::links`. `statx` supplies both (`stx_ino`, `stx_nlink`), and
  `getdents64` alone does not — another reason a size-bearing stat is unavoidable there.
- **Symlinks** are not followed, and should stay that way.

### Where to put the code

`libdiskonaut::scan::scan_directories()` is the seam. It returns `impl Iterator<Item = DirEntries>`
and picks an implementation by `cfg`. A Linux walker slots in beside `bulk`, yields the same
`DirEntries { path, entries, failed }`, and everything downstream — batching, tree building, the
UI — is unchanged. Add a `linux-*` benchmark stage next to the `dua-*` ones so the old and new
walkers can be compared on the same tree in one run, which is what made the macOS work tractable.

## Linux ext4 performance & allocator pressure

> **Stale as of 2026-09-22.** The `/data` this section measured was ext4 with ~2.24M entries. The
> path now holds an XFS filesystem with ~4.23M entries on different hardware, so none of the
> numbers below are comparable with the XFS section that follows. The *changes* described here are
> still in the code and still correct; only the measurements are of a filesystem that no longer
> exists at that path.

On Linux (ext4, ~2.24M entries, ~170k hard-linked files on `/data`), `dua-tree` initially outperformed
diskonaut's tree building because of allocator pressure in the model and hard link accounting.

### Results on `/data` (warm cache)

| Stage | Time | Entries | Throughput | Reported Size | Hard Links |
| --- | --- | --- | --- | --- | --- |
| `dua-walk` | 2.23s | 2,236,001 | 1,000,975 entries/s | 288.6 GiB | (none) |
| `dua-tree` | 9.05s | 2,236,001 | 247,168 entries/s | 221.2 GiB | 170,057 |
| `walk` | 2.50s | 2,236,000 | 893,250 entries/s | 288.6 GiB | (none) |
| `tree` | 6.58s | 2,236,000 | 339,893 entries/s | 221.2 GiB | 170,057 |
| `pipeline` | 5.49s | 2,236,000 | 407,173 entries/s | 221.2 GiB | 170,057 |

`pipeline` completes in **5.49s** (~407k entries/s), beating `dua-tree` (9.05s) by ~39% by overlapping
the parallel walk with concurrent tree building across an MPSC channel.

### Key optimizations

1. **Eliminated 2.2M heap allocations in tree construction**: `FileTree::add_dir_entries` and
   `Folder::add_dir_entries` accept `Vec<NamedEntry>` by value. Instead of allocating a cloned `OsString`
   for every file and then dropping the original in the caller, `entry.name` moves directly into
   `Folder.contents`.
2. **Removed redundant `File.name` field**: `File` previously stored an unused `name: OsString` that was already
   the key in `Folder.contents`. Removing it eliminated 2.2M string allocations and shrunk the struct.
3. **Optimized `HardLinks`**:
   - Replaced `Vec<Vec<OsString>>` path storage with `Vec<(PathBuf, usize)>`, cutting allocations down to 1 per path.
   - Introduced a fast non-cryptographic `U64Hasher` (`SplitMix64`) for the 170k-inode map.
   - Added `charge_with_depth` to reuse the caller's directory depth instead of repeatedly parsing path components.
4. **Pointer equality in `group_by_directory`**: Replaced string equality with `Arc::ptr_eq(&open.path, &parent_path)`.
5. **Pre-allocated channel batches**: Sized batch vectors and enlarged sync channel buffer to prevent worker stalls.
6. **Symlink root canonicalization**: Ensured symlinked scan roots evaluate correctly under `dua-core` walker.

### Second pass: the tree build was the bottleneck after all

The first Linux pass left `pipeline` at 5.2s against a 2.4s `walk`: unlike macOS, the single
tree-building thread was taking twice as long as the eight-thread walk it was consuming. With
`perf` unavailable (`perf_event_paranoid=4`), timers around the three phases of
`FileTree::add_relative_dir_entries` attributed the ~4.3s of consumer time on `/data`:

| phase | time | what it is |
| --- | --- | --- |
| path prep | 0.09s | `strip_prefix`, component count |
| hard-link charging | **3.0s** | `HardLinks::charge` for 726k links to 170k inodes |
| tree insert | 1.2s | resolving the parent folder and inserting the entries |

Hard-link charging dominated because `HardLinks` compared every new link against every folder
already holding a link to the same inode, and each comparison parsed both paths component by
component (`Path == Path` does that too, not a byte compare). With 13k inodes linked from 20 or more
folders that is 5.1M path parses of ~15 components each.

Three changes, measured old binary against new on the same tree back to back:

| | before | after |
| --- | --- | --- |
| `tree` | 6.6–6.9s | 3.0–3.1s |
| `pipeline` | 5.0–5.5s | 2.1–2.8s |
| hard-link charging | 3.0s | 0.43s |
| tree insert | 1.2s | 0.75s |
| peak RSS (`pipeline`) | ~870 MB | ~430 MB |

Entries, total size and hard-linked count are identical between the two binaries, and a
randomised test (`model::tests::hard_links::interned_ledger_matches_component_wise_reference`)
checks the new ledger against the old algorithm on 20,000 charges.

1. **Directories are interned in `HardLinks`.** Each directory that holds a hard link is resolved
   once to a `DirRef` (an index into a `parent`/`depth` table), and a link records that id rather
   than a `PathBuf`. "How many leading components do these two folders share" becomes a
   lowest-common-ancestor walk over integers, and "is this the same folder" an integer compare.
   Interning is lazy — a directory with no hard links never touches the ledger — so the map of
   paths to ids holds only the directories that need it.
2. **`FileOrFolder::Folder` is boxed.** A `Folder` is 96 bytes; a `File` is 16. Every one of the
   2.2M files was paying for the larger variant in its map slot (120 bytes with the key), so the
   folder maps were ~2.5x bigger than they needed to be and inserts moved that much more memory.
   The slot is now 48 bytes. This is where the RSS halved, and part of the insert speed-up.
3. **A word-at-a-time hasher replaces `SipHash`** for the folder maps and the inode map
   (`model/files/hash.rs`). Resolving a directory's parent chain costs a lookup per component, so
   with 670k groups several components deep the tree build hashes several million names on top of
   the 2.2M inserts. The hasher is seeded once per process: its step is invertible, and a scan of
   `/` reads directories other users can write to, so an unseeded state would let them choose names
   that all collide in one folder's map.

A review of the change caught that the ledger first interned directories by their raw spelling,
so `a/b` and `a/b/` were two folders where the old component-wise compare saw one; paths are now
normalised through `components()` before lookup, and the randomised test generates the odd
spellings too. The review also pointed out that the ledger's directory table duplicates what the
`Folder` tree already resolves for the same batch. Storing a `DirRef` on each `Folder` would remove
the path-keyed map and its per-directory allocation; left for a later pass, since only directories
holding hard links are interned and it did not register in the timings.

With these, `pipeline` sits at or just above `walk` — the consumer is hidden behind the walk
again and further work on the model will not show in the app until the walker gets faster.

Two things noticed and left alone, for whoever picks the walker up next:

- The `dua-core` grouping yields **~670k directory groups for ~307k directories**: a directory's
  entries arrive in more than one chunk (`ENTRY_CHUNK_SIZE = 4`, several workers), so its parent
  is resolved and its hard-linked entries de-duplicated about twice as often as necessary. Harmless
  for correctness, worth ~0.3s of the remaining consumer time. A Linux walker that emits one group
  per directory would remove it.
- Re-sweeping worker counts on this machine (8 cores, ext4, warm cache): 2 threads 5.1s, 4 threads
  3.0s, 6 through 16 threads all within 2.2–2.9s of each other, with run-to-run noise of ±0.3s.
  The cap of 8 is not wrong here, and there is no better number to replace it with.

## FAT: vfat, msdos and exFAT

### The bug: FAT32 on macOS scanned as entirely empty

`msdosfs` **sets the `ATTR_FILE_ALLOCSIZE` bit in a record's returned-attributes bitmap and then
packs the value as zero.** The bitmap says the attribute is there, so the `returned_file &
size_attribute != 0` check in `parse_record` passes and reads a legitimate-looking `0`. Every file
on a FAT12/16/32 volume therefore had size zero, the treemap was blank, and nothing said so.
`--apparent-size` was the only working mode, since it asks for `ATTR_FILE_DATALENGTH` instead.

Measured on a 200 MB FAT32 image, before the fix:

```
FAT32   dua-walk  1683 entries  19.1 MiB     exFAT   walk  44.0 KiB   correct
        walk      1682 entries   0.0 B       APFS    walk  correct
        pipeline  1682 entries   0.0 B
```

A probe requesting all three file attributes at once shows it directly:

```
FAT32 : returned_file=0x205  linkcount=1  allocsize=0      datalength=5120
exFAT : returned_file=0x205  linkcount=1  allocsize=8192   datalength=5120
APFS  : returned_file=0x205  linkcount=1  allocsize=4096   datalength=353
```

The `REQUIRED_COMMON` guard added in finding 5 does not catch this: it covers *common* attributes
only, and only checks the first record of the first batch. The comment there guessed exFAT as the
filesystem at risk; exFAT is in fact fine, and FAT32 is the one that broke. **A filesystem can
misreport an attribute it claims to return — the bitmap is not a guarantee of the value.**

There is no allocated size to recover on such a volume. `msdosfs` reports `f_bsize` as 512 rather
than the cluster size, and `st_blocks` as `ceil(size / 512)`, so neither `statfs` nor `lstat` knows
the real allocation either — rounding the data length up to the cluster size is not available as a
fix. The data length is the closest honest answer.

`SizeAttribute` in `scan/bulk.rs` now picks the attribute per filesystem, identified by one
`fstatfs` per *device* (cached, not per directory: a scan can span a FAT stick and an APFS disk, so
a single answer for the whole walk would be wrong, but probing every directory would tax every
filesystem to catch a rare one).

### Testing it

`scan::bulk::tests::a_fat32_volume_does_not_scan_as_empty` creates a FAT32 image with `hdiutil`,
mounts it, scans it and asserts a non-zero total. It is `#[ignore]`d because it mounts a disk image:

```
cargo test -p libdiskonaut --lib -- --ignored fat32
```

Nothing synthetic reproduces this. A hand-built record either carries the attribute or does not,
and neither case is the one that broke; only the real driver claims an attribute and then zeroes
it. Confirmed to fail (`FAT32 volume scanned as 0 bytes`) with the fix reverted.

The test reads its mount point back from `hdiutil attach` output rather than deriving it from the
volume name: **a FAT label longer than eleven characters is silently replaced with `NO NAME`**, so
a name-derived path is wrong for long labels.

### For Linux — unverified, to check when someone has a Linux box

None of the following was tested; it is reasoning from the drivers, recorded so it can be confirmed
or knocked down rather than rediscovered. Test with a FAT stick and a loopback `mkfs.vfat` image.

- **Sizes should already be right, and should differ from macOS.** Linux's `fat_fill_inode` sets
  `i_blocks` from the size rounded up to the cluster size, so `st_blocks` is a true allocated size
  there, unlike macOS. The consequence: **the same stick totals differently on Linux and macOS**,
  and dramatically so on a 32 KB-cluster FAT32 full of small files. Confirm this rather than
  letting someone chase it as a bug.
- **Hard-link accounting is inert, correctly.** FAT has no hard links; `nlink` is always 1, and
  Linux `vfat` reports 1 for directories too. `HardLinks` never fires. No cost, no risk.
- **Synthetic inodes are harmless only by accident.** `vfat` derives `st_ino` from directory-entry
  position and they are not stable across remounts. This is safe today *only* because `links > 1`
  is never true, so the inode never reaches the dedup map. Anything that later keys a map on inode
  unconditionally will break here first.
- **The thread count is the open performance question.** `MAX_SCAN_THREADS = 8` is tuned for APFS
  on NVMe. FAT serialises FAT-chain traversal and usually lives on slow removable media, so 1–2
  workers may well beat 8. This was **not** measured: a disk image backed by NVMe does not model a
  real stick's seek cost, and the test tree scans in 13 ms either way. Needs a real USB stick and a
  `--threads 1/2/4/8` sweep before the constant is touched.
- **exFAT on Linux** is a separate driver (`exfat`, not `vfat`) and, like macOS's, is expected to
  be fine. Worth one confirming run, not more.

## Linux XFS: where the time actually goes (2026-09-22)

The exercise the "Repeating this on Linux" section set up — repeat the macOS walker work on
Linux, and look for an XFS bulk-metadata path — run against a real XFS volume. The short version
is that **the XFS-specific ideas are all dead ends without root, and the scan's remaining cost is
the walker's thread model**, which is not an XFS matter at all. The tree build is 0.85s of a 2.4s
scan, and the walk is the rest. Both halves are measured below.

It also turned up a correctness bug that has nothing to do with speed: XFS reflink sharing is
present on this volume and the model over-counts it (section 6).

### Test machine

| | |
| --- | --- |
| Hardware | 32 cores, 38 GiB RAM |
| OS | Linux 6.8.0-124-generic (Ubuntu 24.04) |
| Filesystem | XFS on `/dev/bcache0`, 8 TiB volume, 915 GiB used |
| `xfs_info` | `agcount=8`, `crc=1`, `finobt=1`, `sparse=1`, `rmapbt=1`, `reflink=1`, `bigtime=1`, `ftype=1`, `inode64`, `bsize=4096` |
| Nested mount | `/data/home/angch/project/myalamat-db` — a subtree mount of a second XFS (`/dev/sdd`) |
| Privilege | ordinary user, uid 1000, no `sudo` |

The tree: **4,228,429 entries**, 12 unreadable, 21,791 distinct hard-linked files, 806.8 GiB
reported (745.9 GiB with `-x`). `df -i` reports 4,217,342 inodes in use, and the ~11k excess is
the extra names of the hard-linked files — so nothing is being traversed twice. The 61 GiB
difference `-x` makes is the nested mount, which holds two multi-gigabyte tarballs and five other
entries; that is why excluding it moves the total by 61 GiB while moving the entry count by 5.

Everything below is warm-cache. "Warm" is not a guess here: `/proc/diskstats` shows **zero sectors
read from `bcache0` during a full scan**, so the whole 4.2M-inode working set is resident and every
number is CPU and kernel time, not I/O. A cold scan of this volume was not measured and would be a
different problem.

### Baseline

Default settings (8 threads), two runs of every stage:

```
dua-walk      4.999s / 4.202s    4228416 entries    896.0 GiB
dua-tree     10.388s /10.814s    4228417 entries    806.8 GiB   21791 hard-linked
walk          3.028s / 3.024s    4228424 entries    896.0 GiB
tree          7.026s / 7.929s    4228424 entries    806.8 GiB   21791 hard-linked
pipeline      2.932s / 3.116s    4228424 entries    806.8 GiB   21791 hard-linked
```

`/usr/bin/time -v` on `pipeline`: **5.32s user against 9.52s system**, 745 MB peak RSS, 483,793
voluntary context switches. System time dominates, as on macOS, but only by 1.8x rather than 26x.

### 1. Thread count: 6 is better than 8, and past 8 it falls off a cliff

`pipeline`, `/data`, three runs each:

| threads | 1 | 2 | 4 | 5 | 6 | 7 | 8 | 10 | 12 | 16 | 24 | 32 | 48 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| time | 11.40s | 5.25s | 2.92s | 2.50s | 2.48s | **2.26s** | 2.53s | 4.18s | 5.75s | 6.53s | 6.85s | 6.81s | 6.96s |

`MAX_SCAN_THREADS = 8` lands on the shoulder of the cliff rather than at the optimum, and 10
threads is already 85% slower than 7. On a 32-core machine the default (`min(cores, 8)`) is
therefore doing real work — without the cap this scan would run at 6.8s instead of 2.5s.

5 through 8 are within noise of each other (2.26–2.65s across runs), so this does not justify
retuning the constant to a precise value. It does justify not raising it.

### 2. The cliff is not XFS, and it is not the kernel — it is the walker's thread model

This is the finding that matters, and it contradicts finding #3 above.

**Control on another filesystem.** The same sweep on `/` (ext4 on `/dev/sda2`, 303,805 entries,
`-x`), `walk` stage:

| threads | 2 | 4 | 6 | 8 | 12 | 16 | 24 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| time | 0.403s | 0.230s | **0.167s** | 0.191s | 0.389s | 0.453s | 0.474s |

Identical shape: optimum at 6, cliff after 8, ~2.7x worse by 16 threads. Whatever this is, it is
not an XFS property.

**Does the kernel scale?** Run N *independent* single-threaded walkers over the same tree at once
(`docs/probes/statbench.c`, mode 1) and measure aggregate throughput:

| concurrent processes | 1 | 4 | 8 | 16 |
| --- | --- | --- | --- | --- |
| wall clock | 4.38s | 4.67s | 4.84s | 5.09s |
| aggregate | 965k/s | 3.62M/s | 6.99M/s | **13.3M entries/s** |

Sixteen processes hammering the *same* inodes get 13.8x the throughput of one, for a 16%
wall-clock penalty. XFS, the dcache and the VFS scale essentially linearly here. The premise of
finding #3 — "past a handful of workers the extra workers spend their time contending on
filesystem locks" — is simply not true on this machine.

**So it is the walker.** `docs/probes/mtwalk.c` is a deliberately naive in-process parallel walker:
one global mutex, a LIFO queue of directory fds, N pthreads, `getdents64` + `fstatat` per entry, no
work stealing. 90 lines. It reports the same 4,228,423 entries, the same 896.0 GiB and the same 12
failures as diskonaut.

**Read the table for its shape, not its multiple.** `mtwalk` is doing a strictly smaller job than
the Rust walker: it never allocates a name (it passes `d_name` straight to `fstatat` and keeps
subdirectory *file descriptors*, not paths), it builds no `NamedEntry`, no `Vec`, no `Arc<Path>`
group, and it hands nothing downstream. This document's own ext4 section records that removing 2.2M
`OsString` allocations was worth measuring, so 4.2M of them are not free. It also holds an open fd
for every discovered-but-unvisited directory — it needs `ulimit -n 65536` — which a shipped walker
cannot do; opening lazily costs an extra `openat` per descent. So 0.387s is a floor no real walker
will reach.

| threads | 1 | 4 | 6 | 8 | 12 | 16 | 24 | 32 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| mtwalk | 4.408s | 1.185s | 0.831s | 0.633s | 0.510s | 0.465s | **0.387s** | 0.395s |
| diskonaut `walk` | — | 2.896s | 2.216s | 2.558s | 6.340s | 6.647s | — | — |

What the table does establish is the **scaling shape**, and allocation cannot explain a *collapse*:
`mtwalk` improves monotonically to 24 threads while `dua-core` peaks at 6 and is 3x worse by 16.
Together with the 16-process result above — the kernel delivering 13.3M entries/s — that is enough
to say the limiter is jwalk's thread model, not XFS, not the kernel, and not the syscalls.

A real replacement therefore lands somewhere between 0.39s and 2.2s at its best thread count, and
where in that range is unmeasured. It does not need to be near the bottom of it: section 5 measures
the tree build at 0.85s, so anything under ~0.9s already makes the model the binding constraint.

On Linux every benchmark stage goes through `dua-core` — `bulk` is macOS-only, and
`fallback::group_by_directory` groups the same `dua-core` walk — so `dua-walk` and `walk` measure
the same underlying jwalk traversal. The collapse past 8 threads lives in there, not in XFS.

### 3. The per-entry stat is 78% of the walk, and nothing portable makes it cheaper

`docs/probes/statbench.c` — one process, one thread, `getdents64` recursion, varying only what it
asks per entry. Two passes over `/data`:

| strategy | time | ns/entry |
| --- | --- | --- |
| `getdents64` only (names + `d_type`) | 0.95s / 0.93s | 220 |
| `fstatat(AT_SYMLINK_NOFOLLOW)` | 4.30s / 4.50s | 1017–1063 |
| `statx`, minimal mask (`TYPE\|MODE\|INO\|NLINK\|BLOCKS`) | 4.47s / 4.46s | 1055 |
| `statx` + `AT_STATX_DONT_SYNC` | 4.56s / 4.28s | 1012–1079 |
| `fstatat`, skipping the 385k directories | 4.55s / 4.16s | 984–1075 |
| `io_uring` batched `statx`, queue depth 512 | 16.91s | 3998 |
| `io_uring` batched `statx` + `DONT_SYNC` | 16.71s | 3952 |

GNU `find` as an outside check: `find /data -printf '.'` 1.67s, `find /data -printf '%s.'` 5.23s.

Three of the four candidates the previous section proposed are now measured, and all three are
worth nothing:

- **A minimal `statx` mask does nothing.** XFS populates the whole in-core inode either way; asking
  for less does not let it do less.
- **`AT_STATX_DONT_SYNC` does nothing.** There is nothing to revalidate on a local filesystem.
- **Trusting `d_type` to skip stats does nothing measurable.** It removes 9% of the calls (385k of
  4.23M) and the result is inside run-to-run noise. It is also not applicable as stated: diskonaut
  counts a directory's own blocks, so it needs the directory's size too.
- **`io_uring` batched `statx` is 3.8x *slower*.** This was ranked "probably the best
  portable-in-practice win" and it is the worst option measured. `IORING_OP_STATX` is a blocking
  opcode: the ring hands each request to an `io-wq` kernel worker, and for a warm-cache metadata
  lookup that handoff costs several times the syscall it replaces. io_uring wins on operations that
  actually block; a cached `statx` does not block.

`ftype=1` is set on this volume, so `d_type` is always populated — `getdents64` returned
**zero** `DT_UNKNOWN` entries across 4.23M.

The honest reading: at ~1µs per entry, warm, the stat is real VFS and XFS work, not syscall entry
overhead. Batching the syscall cannot help because the syscall is not the cost. What *does* help is
doing those microseconds on more cores at once — which is finding 2, not an XFS question.

### 4. The XFS bulk-metadata ioctls, tested

Both were probed directly rather than taken from the man pages.

**`XFS_IOC_BULKSTAT` — `EPERM`.** `docs/probes/bulkstat_probe.c` opens `/data` and issues the v5
ioctl as uid 1000:

```
XFS_IOC_FSGEOMETRY: ok
XFS_IOC_BULKSTAT as uid 1000: FAILED errno=1 (Operation not permitted)
```

`FSGEOMETRY` succeeding on the same descriptor rules out the fd or the struct layout being at
fault. Bulkstat is `CAP_SYS_ADMIN`-gated on 6.8, as the previous section guessed.

Worth stating plainly, because the previous section calls bulkstat "the nearest thing Linux has to
reading the metadata table directly" without the caveat: **bulkstat returns inode records, not
paths.** No name, no parent. It cannot replace a walker for a treemap — the only shape that works
is `getdents64` for the namespace (names, `d_ino`, `d_type`) plus bulkstat as a bulk inode→size
oracle joined on inode number. That is a much larger change than dropping a walker into
`scan_directories()`, it needs a separate ioctl per filesystem (so the nested `myalamat-db` mount
needs its own), and per section 3 its entire ceiling is the ~3.4s of stat time.

**`XFS_IOC_GETFSMAP` — succeeds, and is useless.** This one is the surprise, in both directions.
It is *not* `CAP_SYS_ADMIN`-gated: as uid 1000 it walked the whole 8 TiB device in **25 ioctl calls,
99,822 records, 0.002 seconds**. The reverse-mapping btree (`rmapbt=1` here) really does answer
"what owns this block" at memory speed.

But every record comes back with `FMR_OF_SPECIAL_OWNER` set and an owner of `FMR_OWN_UNKNOWN` (2)
or `FMR_OWN_FREE` (1). Not one inode number in 99,822 records:

```
dev=64256 phys=0         len=634880   owner=2 flags=0x10
dev=64256 phys=634880    len=4096     owner=1 flags=0x10
dev=64256 phys=638976    len=38641664 owner=2 flags=0x10
...
LAST flag seen
batches=25 records=99822  last physical end=8796092989440 (8192.0 GiB)
```

The record count corroborates this: **99,822 records for a volume holding 4.2M inodes.** If owners
were real, adjacent extents belonging to different files could not be merged and the count would be
in the millions; collapsing every owner to `OWN_UNKNOWN` is exactly what lets them coalesce. The
mechanism is therefore inferred from the ioctl's behaviour, not read from the kernel source, but
both the owner values and the record count point the same way: ownership is redacted for
unprivileged callers, presumably so that any user cannot map out the sizes and layout of every
other user's files. An unprivileged caller learns which blocks are free and which are not, and
nothing about whose they are. As a size oracle it is worthless; as a "how
fragmented / how full is this volume" answer it is instant.

Under root it should return real inode numbers, which would make the `getdents64` + fsmap join in
section 4 viable and is the one number this exercise could not obtain. `docs/probes/fsmap_scan.c`
already accumulates blocks per owning inode and prints the distinct-inode count; running it as root
is the remaining measurement.

**ext4** has no supported userspace bulk-metadata API, as previously noted, so a bulk path would
cover XFS only — behind a capability check, with the generic walker as the fallback, for a ceiling
that section 3 shows is smaller than the walker win.

### 5. What to do

In order of measured value. **Items 1 and 2 were done on the same day — see "The native Linux
walker" below for what they cost and what they bought.**

1. ~~**Fix the reflink over-count**~~ (section 6). It is a wrong number, not a slow one. *Done.*
2. ~~**Replace the `dua-core`/jwalk Linux walker**~~ *Done.* with a directory-parallel walker built on
   `getdents64` + `fstatat`, feeding the existing `DirEntries` seam. Unlike every other candidate
   here it needs no privilege, no new dependency and no filesystem-specific code — it would help
   ext4 and btrfs too. It also removes the "~670k groups for ~307k directories" waste noted earlier,
   since a walker that owns its own enumeration emits one group per directory.

   The size of the prize is now measured rather than guessed. The new `tree-only` stage collects
   every directory first, untimed, then times only the model:

   ```
   tree-only     0.822s / 0.853s / 0.865s   4228430 entries   806.8 GiB   21791 hard-linked
   ```

   The tree build is **0.85s** — 4.97M entries/s, and identical totals to every other stage, so it
   is doing the whole job. (Superseded: fed by the native walker's one group per directory instead
   of `dua-core`'s ~2.2 groups per directory, the same stage reads 0.37–0.51s. Take the lower
   figure as the model's cost and this one as the model's cost *plus* the grouping waste. The stage
   is also distorted — see the following section's known gaps.) Against `pipeline` at 2.37–2.52s and `walk` at 2.22–2.32s, the split is
   roughly 2.2s of walk hiding 0.85s of model. A walker landing anywhere under ~0.9s makes the tree
   build the binding constraint, so the realistic end-to-end target is **~2.4s → ~1.0s, about
   2.5x** — not the 5.6x the walk stage alone suggests.
3. **Leave `MAX_SCAN_THREADS` at 8.** It is on the shoulder rather than the peak, but 5–8 are within
   noise and the cap is what keeps a 32-core machine off the 6.8s cliff. Revisit only after the
   walker is replaced, since the cliff is the walker's and a new walker will have a different curve
   — `mtwalk` was still improving at 24 threads.
4. **Do not spend further effort on the model.** At 0.85s for 4.2M entries it is no longer the
   bottleneck and will not become one until the walker is roughly 2.5x faster.
5. **Do not pursue io_uring, `statx` masks, `DONT_SYNC`, or `d_type`-based stat elision.** All four
   are measured at zero or negative value above.
6. **Do not pursue bulkstat or GETFSMAP** for the scan. Both are root-only in the form that would
   help, both are XFS-only, and both are capped by a stat cost smaller than the walker win.

### 6. Reflink sharing is real here, and the model over-counts it

`reflink=1` is enabled on this volume, and unlike `reflink=1` on an idle filesystem, it is **in
use**. Checking 200 files over 50 MB with `filefrag -v` (unprivileged, no root needed — the claim
that this needed a root run was wrong):

```
  block size 4096
  shared 18.20 GiB of 61.69 GiB across the sample (29.5%)
```

The sharers are `uv`'s package cache — `~/.cache/uv/archive-v0/**` holds reflinked copies of large
CUDA, cuDNN, Torch and Playwright binaries, and `uv` reflinks from there into each project's
virtualenv:

```
4    shared  /data/home/angch/.cache/uv/archive-v0/isUXovQuEaGC7fkl5QdqH/nvidia/cu13/lib/libcusolver.so.12
3    shared  /data/home/angch/.cache/uv/archive-v0/X0ppwh-44oKKYjNcdAZiz/nvidia/nccl/lib/libnccl.so.2
1    shared  /data/home/angch/.cache/uv/archive-v0/svekiP7w12JJm5lx4U0Fv/torch/lib/libtorch_cuda.so
```

Copy-on-write shared extents are allocated once but **each sharing file reports the full
`st_blocks`, and `nlink` stays 1** — so `HardLinks`, which keys on inode and link count, cannot see
them. Every reflinked copy inside a scan is charged in full. `cp --reflink`, `uv`, container image
stores and snapshot tooling all produce this, on btrfs as well as XFS, so it is not an exotic case.

That sample is not random — it is the first 200 large files `find` returned, heavily weighted to
the uv cache — so **29.5% is not a whole-volume figure** and should not be extrapolated. What it
does establish is that the effect is present and large where it occurs, which is enough to call the
number wrong.

It does not show up in the whole-volume total: the scan reports 745.9 GiB against `df`'s 915.3 GiB
for this device, so it is *under* `df` overall. Eleven unreadable directories (two container
Postgres/MySQL `pgdata` trees, `drwx------` under other uids) sit inside that gap and more than
offset the over-count. The user-visible damage is local — point diskonaut at `~/.cache/uv` or a
virtualenv and the answer is inflated, and "delete this to free 15 GB" is not true.

Unprivileged `GETFSMAP` reported zero `FMR_OF_SHARED` records, which given the redaction in section
4 is now confirmed to be an artefact of the redaction rather than evidence of absence — a useful
check on that inference.

The fix has the same shape as `HardLinks`: dedupe on physical extent rather than on inode, which
means FIEMAP (`FS_IOC_FIEMAP`) per file and charging each shared extent to a folder once. That is a
per-file ioctl on top of the per-file stat, so it would want to be opt-in, or restricted to files
whose `st_blocks` suggests sharing is plausible. Scoping it is a separate exercise; recording it
here because by this document's own standard (finding #5) a wrong number outranks a slow one.

### Reproducing

```sh
cargo build --release
./target/release/diskonaut --benchmark --bench-stage all --bench-repeat 2 /data
./target/release/diskonaut --benchmark --bench-stage tree-only --bench-repeat 3 /data
for t in 1 2 4 6 8 12 16 24 32; do
  ./target/release/diskonaut --benchmark --bench-stage pipeline --threads $t /data | tail -1 |
    sed "s/^/threads=$t /"
done

cd docs/probes
gcc -O2 -o statbench statbench.c            # modes 0..6, see the table in section 3
gcc -O2 -pthread -o mtwalk mtwalk.c         # ./mtwalk /data <threads>
gcc -O2 -o bulkstat_probe bulkstat_probe.c  # XFS_IOC_BULKSTAT permission check
gcc -O2 -o fsmap_scan fsmap_scan.c          # XFS_IOC_GETFSMAP per-inode block totals
gcc -O2 -o fsmap_dump fsmap_dump.c          # raw GETFSMAP records, for the redaction check
```

The reflink check in section 6 needs no root and no probe:

```sh
find /data/home/angch -xdev -type f -size +50M | head -200 |
  while read -r f; do filefrag -v "$f"; done |
  gawk 'match($0, /blocks of ([0-9]+) bytes/, m) { bs=m[1]+0; next }
        match($0, /^[ \t]*[0-9]+:[ \t]*[0-9]+\.\.[ \t]*[0-9]+:[ \t]*[0-9]+\.\.[ \t]*[0-9]+:[ \t]*([0-9]+):/, m) {
          tot += m[1]; if ($0 ~ /shared/) sh += m[1] }
        END { printf "shared %.2f GiB of %.2f GiB\n", sh*bs/2^30, tot*bs/2^30 }'
```

`statbench` and `mtwalk` are Linux-only and are not part of the build; they exist so the numbers
above can be re-derived rather than believed.

## The native Linux walker, and reflink accounting (2026-09-22)

Acting on the two findings above: `dua-core` is no longer used on Linux, and copy-on-write shared
extents are now counted once. Same machine, same volume, same `--benchmark` harness as the section
above.

### Results

Whole of `/data`, 4.23M entries, warm, at each walker's own best thread count:

| | before | after | |
| --- | --- | --- | --- |
| `walk` (traversal alone) | 2.32s | **0.49s** | 4.7x |
| `pipeline` (what the app waits for), default settings | 2.93s | **0.72s** | 4.1x |
| `pipeline`, best thread count either way | 2.26s | 0.69s | 3.3x |
| quitting mid-scan | — | **1.3ms** | against a 420ms full scan |
| reported total | 807.2 GiB | **785.3 GiB** | 21.9 GiB was counted twice |

The entry count, unreadable count and hard-linked count are identical between the two walkers on
every run, and `--max-depth` agrees exactly at every depth (the one-entry difference is `dua-core`
reporting the scan root itself, which the model ignores either way).

A caution when reading `--bench-stage all` now: every stage uses `thread_count()`, so the `dua-*`
stages run at the new default of 24 workers, which is far past where `dua-core` collapses. They
report 7s rather than the 2.3s they manage at six. **Compare `dua-walk --threads 6` against the
native walker, not the two lines of one `all` run.**

### What the walker does

`libdiskonaut/src/scan/linux.rs`. `getdents64` for names, `statx` for sizes, which is the same pair
of syscalls `dua-core` ends up making — section 3 above measured that no portable change to *what*
is asked per entry is worth anything. The whole difference is the thread model:

- **One shared queue of directories, N workers, and a local stack per worker.** A worker keeps the
  subdirectories it discovers to itself and publishes half of them only when the shared queue looks
  thin enough that someone might be about to go idle. Most directories are therefore claimed with
  no lock at all; the shared lock is taken a few thousand times rather than 385,000.
- **Batched handoff.** Directories go to the consumer in batches of ~4096 entries rather than one
  message each, which takes the channel out of the profile.
- **One `DirEntries` per directory.** The `dua-core` grouping emitted roughly two groups per
  directory (~670k for ~307k), so every parent was resolved and every hard link de-duplicated about
  twice. This is most of why the model got faster without being touched.
- **A stop flag, checked in the worker loop.** Dropping the walk part-way sets it and drains the
  channel. Both halves are needed, and this is the trap finding #7 above records: draining alone
  makes every send *succeed*, so the workers cheerfully finish the whole filesystem while the
  consumer waits to join them. Measured: dropping after 50 directories returns in **1.3ms**, against
  419ms for the full walk.

For reference, `docs/probes/mtwalk.c` — the throwaway C walker used above to prove the ceiling —
does the same traversal in 0.387s while allocating no names and emitting nothing. The real walker
lands at 0.49s while allocating 4.2M `OsString`s, building per-directory `Vec`s and `Arc<Path>`s,
and shipping it all to another thread. That is about 80% of a walker that does none of the work,
which is a reasonable place to stop.

### The thread cap moved from 8 to 24

The old cap was a `dua-core` property, not a kernel one. With the native walker the curve is flat
rather than cliffed, on both filesystems (`pipeline`, three runs each, best of):

| threads | 8 | 12 | 16 | 20 | 24 | 32 |
| --- | --- | --- | --- | --- | --- | --- |
| XFS, `/data` | 0.727s | 0.780s | 0.706s | 0.722s | **0.664s** | 0.665s |
| ext4, `/` `-x` | 0.068s | 0.060s | 0.058s | — | **0.054s** | 0.065s |

`MAX_SCAN_THREADS` is now 24 on Linux and stays 8 elsewhere — the macOS number is a real
`getattrlistbulk`/APFS contention measurement and does not transfer. Being wrong about this in
either direction is cheap now: everything from 12 to 32 is within about 10%.

### Reflink accounting

Section 6 above found that `uv`'s package cache reflinks large binaries, that XFS reports the full
`st_blocks` for every copy, and that `nlink` stays 1 so `HardLinks` could not see it.

The fix reuses the hard-link rule rather than inventing a second one. `EntryMeta` gained
`shared_extent`, and `HardLinks` gained a second ledger keyed on physical extent instead of inode —
kept separate because an inode number and a block offset are unrelated numbers that would otherwise
collide in one map. A file's identity is its first shared extent if it has one, else its inode if
it is hard-linked, else nothing; extent identity wins because every hard link to a file reports the
same first extent, so one file still gets one ledger entry.

The result is the same semantics hard links already had: the same blocks count once in any folder
that reaches them, and once in every folder above. A controlled pair of 200 MiB reflinks:

```
both together   200.0 MiB      (du -s says 400M)
a/ alone        200.0 MiB
b/ alone        200.0 MiB
```

**Finding it costs an `openat` and a `FS_IOC_FIEMAP` per file**, because there is no bulk answer
available unprivileged — `GETFSMAP` redacts owners and `BULKSTAT` is refused. Two guards keep that
affordable:

- **Only on filesystems that can share extents at all**, decided once by `statfs` magic (XFS,
  btrfs). On ext4 the probe never runs, and the ext4 scan time is unchanged at 0.058s.
- **Only on regular files of at least 64 KiB of allocated blocks**, which is 3.8% of the files on
  this volume (144k of 3.8M). Reflinks of small files exist and are missed; they cost the same two
  syscalls to find and are worth a rounding error.

Measured cost on the full scan: about **0.1s of 0.72s**. What it buys:

| | reported before | after | |
| --- | --- | --- | --- |
| `/data` | 807.2 GiB | 785.3 GiB | 10,804 distinct reflinked files |
| `~/.cache/uv` | 15.1 GiB | 12.0 GiB | 4,784 reflinked; `du -sh` still says 16G |

The identity is the file's **whole extent map**, folded into one number, and only when every
extent is shared. The first extent alone is not enough, and the first version of this got it
backwards — see the review finding below.

APFS clones are the same phenomenon and are *not* handled: `getattrlistbulk` does not report
sharing and macOS has no cheap per-file equivalent of FIEMAP. `scan/bulk.rs` sets `shared_extent: 0`
and says so.

### Testing

The `dua-core` grouping is still compiled and still tested — it is what platforms other than macOS
and Linux use — but it is no longer what Linux runs, so the new path needed its own tests
(`scan::tests::linux_walker`): every entry exactly once at 1, 2 and 8 threads; one group per
directory; apparent size; `--max-depth`; symlinks not followed; and dropping the walk early without
hanging.

`scan::tests::reflink::a_reflinked_copy_is_counted_once` is the end-to-end one. It calls `FICLONE`
directly and **skips itself when the filesystem cannot clone** — which `std::env::temp_dir()`
usually cannot, since `/tmp` is typically ext4. Point it at a real one to actually run it:

```sh
DISKONAUT_TEST_REFLINK_DIR=/data cargo test --workspace reflink -- --nocapture
```

Without that variable it prints `skipped: /tmp cannot reflink` and passes, which is worth knowing
before trusting a green run — this is the same trap as "type-checking dead code is not testing it"
in finding #7.

### Filesystem-aware recursion, and the bug it caught

Scanning `/` without `-x` was the check that found the worst defect in the new walker, and it was
not the one being looked for.

**A persistent `getdents64` error was an infinite loop.** The read loop counted a failed directory
read and asked again:

```rust
let Ok(entry) = entry else { failed += 1; continue; };   // wrong
```

A `getdents64` error is a property of the descriptor, not of one entry, so it is still there on the
next call. `/proc/<pid>/net` for a process that has since become a zombie returns `EINVAL` *every
time*, and the walker spun on it forever — one worker pinned at 100% while the other 23 slept on
the condvar. `dua-core` finished `/proc` in ~1.0s on all five attempts; the new walker hung on all
five. Found by `strace`, which showed the same call repeating at 65µs intervals:

```
getdents64(3, 0x7e93093eeb90, 65536) = -1 EINVAL (Invalid argument)   × forever
```

It now `break`s, which is what `std`'s `ReadDir` does. Worth recording as the general lesson:
**`continue` on an error is only safe when the error belongs to the item, not to the iterator.**

**Pseudo-filesystems are no longer crossed into.** `/proc` and `/sys` are kernel interfaces wearing
a directory shape: walking them costs about a million `statx` calls to total zero bytes, `/proc`
grows a subtree per process and per thread while the scan runs, and — as above — parts of it fail
permanently when the process they describe dies mid-walk. This was listed under "Correctness
requirements on Linux" above and had never been done.

The check is by `statfs` magic (`filesystem::is_pseudo`), and is deliberately narrow in two ways:

- **Only at mount points.** A directory whose `st_dev` differs from its parent's is a mount; the
  device is already in the `statx` the walk makes anyway, so the `statfs` costs one call per mount
  crossed rather than one per directory.
- **Never to the scan root.** `diskonaut /proc` still walks `/proc`, because that was asked for.
  The skip only applies to wandering into one part-way through a scan of something else.

`tmpfs` is deliberately *not* on the list: `/tmp` and `/dev/shm` hold real files that really occupy
memory, and `du` counts them. `devtmpfs` reports the same magic, so `/dev` is walked too; it is
small and bounded, unlike the rest.

Measured:

| | before | after |
| --- | --- | --- |
| `diskonaut /proc` | hung, 5 runs of 5 | **0.24s**, 894k entries, ~6,840 unreadable |
| `--bench-stage pipeline /` (no `-x`) | did not finish in 600s | **1.98s**, 9.6M entries |

Scanning `/proc` by name is now about four times faster than `dua-core` managed, which is a side
effect of the thread model rather than the point.

### This change made a pre-existing wrong number reachable

Read this as a caveat on the work above, not as a footnote. `/` completing in two seconds makes it
something a user might actually do, and it reports **1.8 TiB on a machine holding about 1 TiB**.
Before this change a scan of `/` never finished, so nobody ever saw the wrong number. Making a
broken path fast enough to reach is a real cost of the speed work, even though the bug underneath
is older than it. The cause is visible in `df`: `/dev/bcache0` is mounted at both
`/data` and `/home`, so a walk of `/` traverses that filesystem twice and counts every file on it
twice — 9.6M entries against 4.2M inodes.

This is the Linux form of the macOS firmlink problem in finding #2, and it is listed under
"Correctness requirements on Linux" above as the bind-mount case. It is pre-existing — `dua-core`
double-counted identically, on the runs that finished — and it is not fixed here.

The narrow fix available without new machinery would be to refuse a mount leading to a device the
walk has already entered by another path, which is the rule the macOS walker uses. It was
considered and rejected: with work-stealing workers, *which* of `/data` and `/home` wins the race
would vary between runs, so the treemap would move around at random. Doing it properly means
reading `/proc/self/mountinfo` once at the start and picking a canonical path per device, which
also covers the same-`st_dev` bind-mount case the device check cannot see at all. That is the way
in, and it is a separate piece of work.

Until then, **`-x` is the flag that gives a trustworthy whole-machine number.**

### What the audit of this pass caught

Reviewed after the fact, as the macOS work was (finding #7). Two defects, both in the new walker,
both invisible to the benchmark that motivated it.

**Empty directories never flushed the outbox.** Workers batch directories to the consumer until
4096 *entries* have accumulated. A directory with no entries added nothing, so a worker walking a
wide tree of empty directories would hold every one of them until it ran out of work entirely,
while the consumer sat idle waiting for a batch that could not fill. It never deadlocked — the
final flush always happens — but it converts the pipeline back into two serial phases and holds the
whole run's results in memory, in exactly the tree shape where that is worst. Counting
`entries.len().max(1)` fixes it, which is the convention the app's own batching already used.

**A signal mid-`getdents64` looked like a dead directory.** The `EINVAL` fix above breaks out of a
directory on any readdir error, which is right for errors that describe the descriptor — but
`EINTR` describes neither the descriptor nor the entry. It matters here rather than in theory: the
TUI handles `SIGWINCH`, so **resizing the terminal during a scan** could have silently truncated
whichever directory a worker happened to be reading, along with its entire subtree, and reported it
as one unreadable entry. `EINTR` is now retried and everything else still breaks.

Neither would have shown up in the numbers. The first makes the benchmark look *better* on a tree
of empty directories (no channel traffic), and the second needs a signal that no headless run
sends.

### The reflink fix crashed the renderer, and the bug was older than it

Reported from real use, a few minutes after the work above was declared done:

```
panicked at ratatui-core-0.1.2/src/buffer/buffer.rs:251:
index outside of buffer: the area is Rect { x: 0, y: 0, width: 170, height: 48 }
but index is (134, 80)
  4: diskonaut::ui::grid::draw_next_symbol::draw_next_symbol
  5: diskonaut::ui::grid::draw_rect::draw_rect_on_grid
```

Row 80 of a 48-row buffer: not an off-by-one, a tile laid out far outside the board.

The cause is the "sizes are not additive" property this document has described from the start,
finally meeting code that assumed otherwise. `files_in_folder` computed each entry's share as
`entry.size / folder.size`, which is only ≤ 1 when the entries add up to the folder. Shared blocks
mean they do not: four reflinked copies of one 1 MB file sit in a folder holding 1 MB, and the
shares come out at **4.0**. The squarify layout then places tiles well off the screen, and the UI
indexes the terminal buffer directly, so it panicked instead of drawing wrong.

Hard links could always have done this, and the gotcha above says so in as many words. What
changed is the odds: the reflink work added 10,804 newly-deduplicated files on this volume, and
`~/.cache/uv` went from "entries sum to the folder" to 15.1 GiB of entries in a 12.0 GiB folder.
A latent bug became a crash anyone scanning a `uv` cache would hit.

Two changes, because the second would have made the first a cosmetic glitch:

1. **`files_in_folder` divides by the larger of the folder and the sum of its entries.** A tile is
   a share of the space its siblings take between them, which fills the board exactly and is the
   only reading that stays self-consistent once blocks are shared.
2. **`RectangleGrid::render` skips a tile that does not fit the buffer.** The layout is float
   arithmetic over sizes that need not add up; "this tile does not fit" is a thing that can happen,
   not an invariant worth crashing over.

`tiles::tests::entries_larger_than_the_folder_holding_them_stay_on_the_board` builds the
four-copies-in-a-one-copy-folder case, asserts the shares stay within 1.0, and asserts every tile
lands on the board. It fails on the old code with `percentages must not exceed the board, got 4`.

The lesson is the one finding #5 already records, from the other direction: **a number that moves
toward the truth is not the whole story.** The reflink work produced a total that agreed with the
filesystem, and every test and benchmark passed, because nothing downstream of the total was being
checked. Hunting the crash by resizing the terminal and fuzzing the layout found nothing; writing
down the invariant the fix had quietly broken found it in one test.

### What the review caught, including a claim that was simply false

Reviewed after the commits were written. Five findings, all real; two are worth repeating.

**Keying on the first extent could halve a total, not overstate it.** The code above documented
itself as erring high: "a file that shares only part of itself keeps being counted in full". That
was wrong, and the ledger's size guard does not save it, because it only fires when the two sizes
*differ*. Two equal-sized files sharing nothing but their opening extent were merged, and one of
them counted as nothing:

```sh
cp --reflink=always a/img.bin b/img.bin        # 1 MiB each
dd if=/dev/urandom of=b/img.bin bs=1k seek=100 count=100 conv=notrunc
```

`filefrag` shows `b` with three extents — shared, *not* shared, shared — and diskonaut reported
**1.0 MiB for 2.0 MiB of files.** The identity is now the whole extent map (up to 64 extents,
FNV-folded), accepted only when every extent is shared and the `LAST` flag proves the map is
complete. Anything else is counted in full, which is what the comment always claimed.

The lesson is the one this file keeps relearning: the guard that was supposed to make this safe
(`seen.size != size`) was written for hard links, where two different files cannot share an inode
number *and* a size. Reused for extents, the same line stopped meaning what it said.

**`statx` is not `lstat`, and the difference automounts a network.** The walk asked for
`AT_SYMLINK_NOFOLLOW` and nothing else. `stat`, `lstat` and `fstatat` all behave as though
`AT_NO_AUTOMOUNT` were set; **bare `statx` does not**, and `man 2 statx` names this exact case —
"can be used in tools that scan directories to prevent mass-automounting of a directory of
automount points". The walker it replaced went through `std`'s `lstat` and was implicitly safe.

So merely *looking at* an autofs placeholder mounted it. A directory of NFS home maps would have
been mounted wholesale and one dead server would have hung the scan — and this fires at stat time,
before the descent decision, so the care taken over autofs in `filesystem` did not cover it at all.
That reasoning is only sound now the flag is set.

The other three, more briefly:

- **The thread cap was per-platform when it wanted to be per-walker.** `thread_count()` also feeds
  `scan_folder` (public) and the `dua-*` benchmark stages, so raising Linux to 24 ran the one
  walker that collapses past eight at 24 of them — 38% slower, and it quietly re-tuned the very
  baseline this document compares against. There are now two caps.
- **`i128::from(f_type)` sign-extends on 32-bit.** `__fsword_t` is signed and 32 bits wide on i686
  and armv7, so every magic with the top bit set — btrfs, selinuxfs, bpf, hugetlbfs — would never
  have matched there, silently turning the whole reflink feature off. Truncating to `u32` is
  lossless and correct on both.
- **`Drop` set the stop flag outside the lock**, which is exactly what `retire` takes the lock to
  avoid. It was rescued by the retire-to-zero path, but only by a multi-step argument; it now holds
  the lock, and the argument is not needed.

### Known gaps in this pass

- **`tree-only` is distorted, not merely noisy, and should be read as an upper bound.** It ranges
  from 0.37s to 0.87s across runs, and in one run reported 0.871s while `tree` — which contains it —
  reported 0.727s. A stage cannot cost more than the stage that contains it, so this is systematic:
  the untimed `collect()` of 4.2M `DirEntries` leaves the allocator and page cache in a state the
  real pipeline never sees, and the timed build then runs in it. The conclusion it was used for
  still holds (walk 0.49s, pipeline 0.72s), but the number itself should not be quoted as the
  model's cost without that caveat.
- **`dua-core` is still a dependency.** It backs the `dua-*` benchmark stages, which are how the
  comparison above is reproduced, and `fallback::group_by_directory` for platforms that are neither
  macOS nor Linux. Dropping it would mean giving up the baseline.
- **The reflink threshold is a guess, not a measurement.** 64 KiB was chosen because it leaves 3.8%
  of files to probe on this volume. Nobody has measured how many shared bytes live below it.
- **A scan of `/` double-counts filesystems mounted in two places**, as above. `-x` avoids it.
- **The macOS build is unverified.** `scan/bulk.rs` needed one field adding to two `EntryMeta`
  literals and nothing here can compile it — the module is `cfg`'d out on Linux, which is exactly
  the trap finding #7 records. It needs a build on a Mac before release.
- **`cargo deny check` was not run**; `cargo-deny` is not installed here. `Cargo.lock` is unchanged,
  but rustix's feature set is (`std` and `fs` added), so the licence and advisory gates are unproven.

## Known gaps

- The reported total for `/` is ~706 GiB against 884 GiB used. The difference is APFS snapshots,
  purgeable space, and the deliberately skipped auxiliary volumes. Whether that is the right
  definition of "the disk" is a product decision, not a settled one.
- ~470 entries under `/` are unreadable without Full Disk Access. Granting it to the terminal will
  change the total.
- Peak RSS for a whole-disk scan is ~3.5 GB, roughly 340 bytes per entry. `Folder` stores a
  `HashMap<OsString, FileOrFolder>` per directory and every `File` pays the size of the larger
  `Folder` variant. An arena or an interned-name representation would cut this substantially, and
  the allocator pressure may be costing time as well — unmeasured.
