# Scan performance

Notes from making a whole-disk scan fast enough to be worth waiting for, and a plan for repeating
the exercise on Linux. Everything below was measured with the `--benchmark` harness that ships in
the binary, so the numbers are reproducible rather than remembered.

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
| `pipeline` | scan and tree build on separate threads, as the app runs them |

`dua-*` against the others is a like-for-like walker comparison on the same tree. `walk` against
`tree` is the cost of the data model. `tree` against `pipeline` is the cost of the channel.

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

`thread_count()` therefore caps workers at `min(cores, 8)`. **This cap is a macOS/APFS measurement
and should be re-derived on Linux**, where the contention profile of ext4/xfs/btrfs is different.

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

`getattrlistbulk(2)` returns **names and sizes together** in one call per directory-ful of entries.
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
