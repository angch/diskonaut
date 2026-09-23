//! Native Windows directory walk: one handle per directory, entries read in bulk.
//!
//! Replaces the `dua-core` walk on Windows. That walk costs a `CreateFileW` per *file*: `std`'s
//! `DirEntry::metadata` has no allocation size or file id, so both have to be asked for by opening
//! the file. `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` returns size, allocation,
//! attributes and the 128-bit file id for a whole buffer of entries at once, so a directory costs
//! one open and a handful of calls, however many files it holds.
//!
//! The thread model is the Linux walker's (see `linux.rs`): one shared queue behind one mutex,
//! workers that keep most of their findings local, results in batches over a bounded channel.
//!
//! What the directory listing does not have is a link count. See [`links`] for how hard links are
//! counted once without it.

use ::std::ffi::{OsString, c_void};
use ::std::os::windows::ffi::{OsStrExt, OsStringExt};
use ::std::path::{Path, PathBuf};
use ::std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use ::std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use ::std::sync::{Arc, Condvar, Mutex};
use ::std::thread::JoinHandle;

use super::{DirEntries, EntryMeta, LINKS_UNKNOWN, ScanOptions};

#[allow(non_snake_case, clippy::upper_case_acronyms)]
mod ffi {
    use ::std::ffi::c_void;

    pub type HANDLE = *mut c_void;
    pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

    pub const FILE_LIST_DIRECTORY: u32 = 0x0001;
    pub const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    pub const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    pub const OPEN_EXISTING: u32 = 3;
    pub const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    pub const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
    pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

    pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    pub const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

    pub const ERROR_NO_MORE_FILES: u32 = 18;
    pub const ERROR_INVALID_PARAMETER: u32 = 87;
    pub const ERROR_NOT_SUPPORTED: u32 = 50;
    pub const ERROR_INVALID_LEVEL: u32 = 124;

    /// `FILE_INFO_BY_HANDLE_CLASS` values.
    pub const FILE_FULL_DIRECTORY_INFO: i32 = 14;
    pub const FILE_ID_EXTD_DIRECTORY_INFO: i32 = 19;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: u32,
            dwShareMode: u32,
            lpSecurityAttributes: *mut c_void,
            dwCreationDisposition: u32,
            dwFlagsAndAttributes: u32,
            hTemplateFile: HANDLE,
        ) -> HANDLE;

        pub fn GetFileInformationByHandleEx(
            hFile: HANDLE,
            FileInformationClass: i32,
            lpFileInformation: *mut c_void,
            dwBufferSize: u32,
        ) -> i32;

        pub fn GetVolumeInformationByHandleW(
            hFile: HANDLE,
            lpVolumeNameBuffer: *mut u16,
            nVolumeNameSize: u32,
            lpVolumeSerialNumber: *mut u32,
            lpMaximumComponentLength: *mut u32,
            lpFileSystemFlags: *mut u32,
            lpFileSystemNameBuffer: *mut u16,
            nFileSystemNameSize: u32,
        ) -> i32;

        pub fn CloseHandle(hObject: HANDLE) -> i32;

        pub fn GetLastError() -> u32;
    }
}

/// An open handle, closed on drop.
struct Handle(ffi::HANDLE);

impl Handle {
    /// Open `path` for `access`, not following a final reparse point when `no_follow` is set.
    fn open(path: &Path, access: u32, no_follow: bool) -> Option<Handle> {
        let wide = wide_path(path);
        let flags = ffi::FILE_FLAG_BACKUP_SEMANTICS
            | if no_follow {
                ffi::FILE_FLAG_OPEN_REPARSE_POINT
            } else {
                0
            };
        // SAFETY: `wide` is NUL-terminated and outlives the call; every pointer argument is either
        // valid or null where the API allows it.
        let handle = unsafe {
            ffi::CreateFileW(
                wide.as_ptr(),
                access,
                ffi::FILE_SHARE_ALL,
                ::std::ptr::null_mut(),
                ffi::OPEN_EXISTING,
                flags,
                ::std::ptr::null_mut(),
            )
        };
        (handle != ffi::INVALID_HANDLE_VALUE).then_some(Handle(handle))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned open by `CreateFileW` and is closed exactly once.
        unsafe {
            ffi::CloseHandle(self.0);
        }
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    wide
}

/// The volume serial number of the filesystem `handle` is on, and whether its file ids are stable.
fn volume_of(handle: &Handle) -> (u64, bool) {
    let mut serial = 0u32;
    let mut name = [0u16; 32];
    // SAFETY: every buffer is live for the call and its length is the one passed.
    let ok = unsafe {
        ffi::GetVolumeInformationByHandleW(
            handle.0,
            ::std::ptr::null_mut(),
            0,
            &raw mut serial,
            ::std::ptr::null_mut(),
            ::std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return (0, false);
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    (
        u64::from(serial),
        matches!(
            OsString::from_wide(&name[..len]).to_str(),
            Some("NTFS" | "ReFS")
        ),
    )
}

/// Hard links, counted once without learning which files have them.
///
/// No directory listing on Windows carries a link count, and learning one means opening the file:
/// about 100µs of thread time each, against a few microseconds for everything else the walk does.
/// Asking every file on `C:\` (2M entries) took the scan from 8s to 31s.
///
/// The listing does carry each file's id, and that is enough without the count. A file whose
/// link count is [`LINKS_UNKNOWN`] goes through the hard-link ledger keyed on its id, and the
/// ledger charges the first name it sees along its whole path, exactly as it would an ordinary
/// file; only a second name with the same id is treated as a link. So the sizes come out as though
/// every count had been asked for, and the price is a ledger entry per file — about 100 bytes —
/// rather than a file open. On `C:\`, tracking every file cost 1.2s and 200 MB over tracking none.
///
/// So by default only files in the places hard links are normally made are tracked ([`HOT_SPOTS`]
/// and [`HOT_PATHS`]): 90 MB and a few hundred milliseconds on `C:\`, for all but 240 MB of its
/// 17 GB of double-counted links. `--hard-link-threshold` tracks every file at least that large,
/// wherever it is. An untracked link is counted once per name, which overstates, never
/// understates.
///
/// Only volumes whose ids are known to be stable are tracked: NTFS and ReFS. A filesystem driver
/// that reports one id for every file would otherwise have its equal-sized files merged into one.
mod links {
    use ::std::ffi::OsStr;
    use ::std::path::{Path, PathBuf};

    /// Directory names below which hard links are normally made, matched without regard to case.
    ///
    /// * `node_modules`, `.pnpm-store`, `pnpm` — pnpm links packages out of its content-addressed
    ///   store (`%LOCALAPPDATA%\pnpm\store`, or `.pnpm-store` at a drive root) into every
    ///   project. Both ends have to be tracked, or the store copy is counted as unshared.
    /// * `uv`, `.venv`, `site-packages` — uv links wheels from its cache into environments the
    ///   same way, by default on Windows.
    pub const HOT_SPOTS: &[&str] = &[
        "node_modules",
        ".pnpm-store",
        "pnpm",
        "uv",
        ".venv",
        "site-packages",
    ];

    /// Installations that link files between their own versions or into each other, as
    /// `(environment variable, path below it)`. Too generically named to match by name —
    /// `Microsoft` alone would take in all of `AppData` — so matched as whole paths.
    ///
    /// Found on one Windows 11 machine by asking every file on `C:\` for its link count: outside
    /// `%SystemRoot%` and [`HOT_SPOTS`], these held 6.6 GiB of hard-linked files, Edge's alone
    /// 4.7 GiB (each Edge, WebView2 and Copilot version links into `EdgeCore`).
    pub const HOT_PATHS: &[(&str, &str)] = &[
        ("SystemRoot", ""),
        ("ProgramFiles(x86)", "Microsoft"),
        ("ProgramFiles", "Microsoft"),
        ("ProgramFiles", "Docker"),
        ("ProgramFiles", "Git"),
        ("ProgramFiles", "Reference Assemblies"),
        ("ProgramFiles(x86)", "Reference Assemblies"),
        ("ProgramData", r"Microsoft\Windows Defender"),
    ];

    /// [`HOT_PATHS`] as they are on this machine, canonicalized to compare with walked paths.
    pub fn hot_paths() -> Vec<PathBuf> {
        HOT_PATHS
            .iter()
            .filter_map(|(variable, below)| {
                let base = ::std::env::var_os(variable)?;
                Path::new(&base).join(below).canonicalize().ok()
            })
            .collect()
    }

    pub fn is_hot_spot(name: &OsStr) -> bool {
        name.to_str()
            .is_some_and(|name| HOT_SPOTS.iter().any(|spot| spot.eq_ignore_ascii_case(name)))
    }
}

/// One directory waiting to be read.
struct Job {
    path: Arc<Path>,
    /// Depth below the scan root, which is depth 0.
    depth: usize,
    /// Whether this directory's files are tracked by id in case they are hard links.
    track: bool,
}

/// Which directory-information class a filesystem answers. Decided on the first directory and
/// kept: every directory of one scan is on one volume, since reparse points are never followed.
const CLASS_UNKNOWN: usize = 0;
const CLASS_ID_EXTD: usize = 1;
const CLASS_FULL: usize = 2;

struct Shared {
    jobs: Mutex<Vec<Job>>,
    ready: Condvar,
    queued: AtomicUsize,
    pending: AtomicUsize,
    stop: AtomicBool,
    class: AtomicUsize,
    /// Whether the volume's file ids can be trusted to tell files apart (NTFS, ReFS).
    stable_ids: bool,
    /// Files with fewer bytes allocated than this are not tracked.
    track_from: u64,
    /// [`links::hot_paths`]: the Windows directory, and installations that link into themselves.
    hot_paths: Vec<PathBuf>,
    /// Volume serial number, folded into file ids so that ids from two volumes cannot collide.
    volume: u64,
}

impl Shared {
    /// Wait for a directory published by another worker, or for the walk to end.
    fn steal(&self) -> Option<Job> {
        let mut jobs = self.jobs.lock().expect("scan queue poisoned");
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(job) = jobs.pop() {
                self.queued.store(jobs.len(), Ordering::Relaxed);
                return Some(job);
            }
            if self.pending.load(Ordering::Acquire) == 0 {
                return None;
            }
            jobs = self.ready.wait(jobs).expect("scan queue poisoned");
        }
    }

    /// Publish half of a worker's local stack so idle workers have something to take.
    fn donate(&self, local: &mut Vec<Job>) {
        let donated = local.len() / 2;
        let mut jobs = self.jobs.lock().expect("scan queue poisoned");
        jobs.extend(local.drain(..donated));
        self.queued.store(jobs.len(), Ordering::Relaxed);
        self.ready.notify_all();
    }

    /// Retire `count` directories, waking everyone if that was the last outstanding work.
    fn retire(&self, count: usize) {
        if count == 0 {
            return;
        }
        if self.pending.fetch_sub(count, Ordering::AcqRel) == count {
            let _jobs = self.jobs.lock().expect("scan queue poisoned");
            self.ready.notify_all();
        }
    }
}

/// Byte offsets into `FILE_ID_EXTD_DIR_INFO` and `FILE_FULL_DIR_INFO`, which agree up to the name
/// length. Read by offset rather than through a `repr(C)` struct because entries are only 8-byte
/// aligned by convention, and the name is a variable-length tail.
mod layout {
    pub const NEXT_ENTRY_OFFSET: usize = 0;
    pub const END_OF_FILE: usize = 40;
    pub const ALLOCATION_SIZE: usize = 48;
    pub const FILE_ATTRIBUTES: usize = 56;
    pub const FILE_NAME_LENGTH: usize = 60;
    /// `EaSize` in `FILE_FULL_DIR_INFO`; the reparse tag in `FILE_ID_EXTD_DIR_INFO`, since a file
    /// cannot have both and the kernel reuses the field.
    pub const EA_OR_REPARSE: usize = 64;
    pub const EXTD_REPARSE_TAG: usize = 68;
    pub const EXTD_FILE_ID: usize = 72;
    pub const EXTD_FILE_NAME: usize = 88;
    pub const FULL_FILE_NAME: usize = 68;
}

fn read_u32(buffer: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(buffer[at..at + 4].try_into().expect("four bytes"))
}

fn read_u64(buffer: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(buffer[at..at + 8].try_into().expect("eight bytes"))
}

/// Size of the listing buffer. 64 KiB is the most `NtQueryDirectoryFile` will fill over SMB, and
/// on NTFS a larger one bought nothing measurable.
const BUFFER_BYTES: usize = 64 * 1024;

/// Read one directory, returning its entries and the subdirectories to descend into.
fn read_directory(
    job: &Job,
    options: &ScanOptions,
    shared: &Shared,
    buffer: &mut [u64],
) -> (DirEntries, Vec<Job>) {
    let mut directory = DirEntries::new(Arc::clone(&job.path));
    let mut children = Vec::new();

    let Some(handle) = Handle::open(&job.path, ffi::FILE_LIST_DIRECTORY, false) else {
        directory.failed = 1;
        return (directory, children);
    };

    let descend = options.max_depth.is_none_or(|max| job.depth + 1 < max);
    let byte_len = buffer.len() * 8;
    let mut name_wide: Vec<u16> = Vec::with_capacity(260);

    loop {
        let mut class = shared.class.load(Ordering::Relaxed);
        if class == CLASS_UNKNOWN {
            class = CLASS_ID_EXTD;
        }
        let info_class = if class == CLASS_ID_EXTD {
            ffi::FILE_ID_EXTD_DIRECTORY_INFO
        } else {
            ffi::FILE_FULL_DIRECTORY_INFO
        };
        // SAFETY: `buffer` is live, 8-byte aligned as the kernel requires, and `byte_len` long.
        let ok = unsafe {
            ffi::GetFileInformationByHandleEx(
                handle.0,
                info_class,
                buffer.as_mut_ptr().cast::<c_void>(),
                byte_len as u32,
            )
        };
        if ok == 0 {
            // SAFETY: no preconditions.
            let error = unsafe { ffi::GetLastError() };
            match error {
                ffi::ERROR_NO_MORE_FILES => break,
                // FAT, exFAT and some network filesystems do not answer the id class. The first
                // call on a handle is also the one that fails, so retrying on it is safe.
                ffi::ERROR_INVALID_PARAMETER
                | ffi::ERROR_NOT_SUPPORTED
                | ffi::ERROR_INVALID_LEVEL
                    if class == CLASS_ID_EXTD =>
                {
                    shared.class.store(CLASS_FULL, Ordering::Relaxed);
                    continue;
                }
                _ => {
                    directory.failed += 1;
                    break;
                }
            }
        }
        shared.class.store(class, Ordering::Relaxed);

        // SAFETY: reinterpreting a `u64` buffer as bytes is always sound.
        let bytes = unsafe { ::std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), byte_len) };
        let name_at = if class == CLASS_ID_EXTD {
            layout::EXTD_FILE_NAME
        } else {
            layout::FULL_FILE_NAME
        };

        let mut at = 0usize;
        loop {
            let entry = &bytes[at..];
            let next = read_u32(entry, layout::NEXT_ENTRY_OFFSET) as usize;
            let name_len = read_u32(entry, layout::FILE_NAME_LENGTH) as usize;
            name_wide.clear();
            name_wide.extend(
                entry[name_at..name_at + name_len]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair)),
            );

            if !(name_wide[..] == [u16::from(b'.')] || name_wide[..] == [u16::from(b'.'); 2]) {
                let attributes = read_u32(entry, layout::FILE_ATTRIBUTES);
                let end_of_file = read_u64(entry, layout::END_OF_FILE);
                let allocated = read_u64(entry, layout::ALLOCATION_SIZE);
                let reparse_tag = if attributes & ffi::FILE_ATTRIBUTE_REPARSE_POINT == 0 {
                    0
                } else if class == CLASS_ID_EXTD {
                    read_u32(entry, layout::EXTD_REPARSE_TAG)
                } else {
                    read_u32(entry, layout::EA_OR_REPARSE)
                };
                // Junctions, mounted folders and symbolic links are not followed, as `lstat` does
                // not follow a symlink. Every other reparse point — OneDrive placeholders, dedup,
                // WSL's special files — is a real file or directory with a tag on it.
                let link = matches!(
                    reparse_tag,
                    ffi::IO_REPARSE_TAG_MOUNT_POINT | ffi::IO_REPARSE_TAG_SYMLINK
                );
                let is_dir = attributes & ffi::FILE_ATTRIBUTE_DIRECTORY != 0 && !link;

                let file_id = if class == CLASS_ID_EXTD {
                    // The low half is the NTFS file reference; the high half is zero on NTFS and
                    // meaningful on ReFS. Fold both, with the volume, into one 64-bit identity.
                    let low = read_u64(entry, layout::EXTD_FILE_ID);
                    let high = read_u64(entry, layout::EXTD_FILE_ID + 8);
                    low ^ high.rotate_left(32) ^ shared.volume.rotate_left(48)
                } else {
                    0
                };

                let name = OsString::from_wide(&name_wide);
                let size = if options.show_apparent_size {
                    end_of_file
                } else {
                    allocated
                };
                let links =
                    if !is_dir && job.track && file_id != 0 && allocated >= shared.track_from {
                        LINKS_UNKNOWN
                    } else {
                        1
                    };

                if is_dir && descend {
                    let path = job.path.join(&name);
                    let track = job.track
                        || (shared.stable_ids
                            && (links::is_hot_spot(&name) || shared.hot_paths.contains(&path)));
                    children.push(Job {
                        path: Arc::from(path.as_path()),
                        depth: job.depth + 1,
                        track,
                    });
                }

                directory.push(
                    &name,
                    EntryMeta {
                        size,
                        inode: file_id,
                        links,
                        is_dir,
                        shared_extent: 0,
                    },
                );
            }

            if next == 0 {
                break;
            }
            at += next;
        }
    }

    directory.shrink();
    (directory, children)
}

/// A walk in progress. Dropping it stops the workers rather than waiting for the tree.
pub struct WindowsWalk {
    batches: Receiver<Vec<DirEntries>>,
    current: ::std::vec::IntoIter<DirEntries>,
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Iterator for WindowsWalk {
    type Item = DirEntries;

    fn next(&mut self) -> Option<DirEntries> {
        loop {
            if let Some(directory) = self.current.next() {
                return Some(directory);
            }
            self.current = self.batches.recv().ok()?.into_iter();
        }
    }
}

impl Drop for WindowsWalk {
    fn drop(&mut self) {
        {
            let _jobs = self.shared.jobs.lock().expect("scan queue poisoned");
            self.shared.stop.store(true, Ordering::Relaxed);
            self.shared.ready.notify_all();
        }
        while self.batches.recv().is_ok() {}
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// Walk `root` with `threads` workers, yielding one [`DirEntries`] per directory.
pub fn walk_windows(root: &Path, threads: usize, options: ScanOptions) -> WindowsWalk {
    let root: PathBuf = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root: Arc<Path> = Arc::from(root.as_path());

    let (volume, stable_ids) = Handle::open(&root, ffi::FILE_READ_ATTRIBUTES, false)
        .map_or((0, false), |handle| volume_of(&handle));

    let hot_paths = links::hot_paths();
    // A root inside a hot spot is itself one: scanning `C:\Windows\WinSxS` directly must still
    // find its links.
    let inside_hot_spot = root
        .components()
        .any(|part| links::is_hot_spot(part.as_os_str()))
        || hot_paths.iter().any(|hot| root.starts_with(hot));
    let track = stable_ids && (options.hard_link_threshold.is_some() || inside_hot_spot);

    let shared = Arc::new(Shared {
        jobs: Mutex::new(vec![Job {
            path: Arc::clone(&root),
            depth: 0,
            track,
        }]),
        ready: Condvar::new(),
        queued: AtomicUsize::new(1),
        pending: AtomicUsize::new(1),
        stop: AtomicBool::new(false),
        class: AtomicUsize::new(CLASS_UNKNOWN),
        stable_ids,
        // Zero bytes allocated — empty, or small enough to live in the MFT record — costs nothing
        // however many names it has, so is never worth a ledger entry.
        track_from: options.hard_link_threshold.unwrap_or(1).max(1),
        hot_paths,
        volume,
    });

    let (sender, batches): (SyncSender<Vec<DirEntries>>, Receiver<Vec<DirEntries>>) =
        sync_channel(64);

    let threads = threads.max(1);
    let donate_below = threads;

    let workers = (0..threads)
        .map(|_| {
            let shared = Arc::clone(&shared);
            let sender = sender.clone();
            ::std::thread::spawn(move || worker(&shared, &sender, options, donate_below))
        })
        .collect();

    WindowsWalk {
        batches,
        current: Vec::new().into_iter(),
        shared,
        workers,
    }
}

/// How many entries a worker accumulates before handing a batch to the consumer.
const SEND_BATCH: usize = 4096;

fn worker(
    shared: &Shared,
    sender: &SyncSender<Vec<DirEntries>>,
    options: ScanOptions,
    donate_below: usize,
) {
    let mut local: Vec<Job> = Vec::new();
    let mut outbox: Vec<DirEntries> = Vec::new();
    let mut outbox_entries = 0usize;
    let mut buffer = vec![0u64; BUFFER_BYTES / 8];

    macro_rules! abandon {
        () => {{
            shared.retire(1 + local.len());
            return;
        }};
    }

    while let Some(job) = local.pop().or_else(|| shared.steal()) {
        let (directory, children) = read_directory(&job, &options, shared, &mut buffer);

        outbox_entries += directory.len().max(1);
        outbox.push(directory);
        if outbox_entries >= SEND_BATCH {
            outbox_entries = 0;
            if sender.send(::std::mem::take(&mut outbox)).is_err() {
                abandon!();
            }
        }
        if shared.stop.load(Ordering::Relaxed) {
            abandon!();
        }

        shared.pending.fetch_add(children.len(), Ordering::Relaxed);
        local.extend(children);
        if local.len() > 1 && shared.queued.load(Ordering::Relaxed) < donate_below {
            shared.donate(&mut local);
        }
        shared.retire(1);
    }

    if !outbox.is_empty() {
        let _ = sender.send(outbox);
    }
}
