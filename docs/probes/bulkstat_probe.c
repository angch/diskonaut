/* Probe: is XFS_IOC_BULKSTAT usable unprivileged on this kernel? */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>

struct xfs_bulk_ireq {
    uint64_t ino;
    uint32_t flags;
    uint32_t icount;
    uint32_t ocount;
    uint32_t agno;
    uint64_t reserved[5];
};

/* subset; we only read the leading fields */
struct xfs_bulkstat {
    uint64_t bs_ino, bs_size, bs_blocks, bs_xflags;
    int64_t  bs_atime, bs_mtime, bs_ctime, bs_btime;
    uint32_t bs_gen, bs_uid, bs_gid, bs_projectid;
    uint32_t bs_atime_nsec, bs_mtime_nsec, bs_ctime_nsec, bs_btime_nsec;
    uint32_t bs_blksize, bs_rdev, bs_cowextsize_blks, bs_extsize_blks;
    uint32_t bs_nextents, bs_version;
    uint16_t bs_forkoff, bs_sick, bs_checked, bs_mode;
    uint32_t bs_pad32;
    uint64_t bs_extents64;
    uint64_t bs_pad[6];
};

#define XFS_BULK_IREQ_AGNO (1U << 1)
#define XFS_IOC_BULKSTAT  _IOR('X', 127, struct xfs_bulk_ireq)
#define XFS_IOC_INUMBERS  _IOR('X', 128, struct xfs_bulk_ireq)
#define XFS_IOC_FSGEOMETRY _IOR('X', 126, char[256])

#define N 1024
struct req { struct xfs_bulk_ireq hdr; struct xfs_bulkstat bs[N]; };

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "/data";
    printf("sizeof(xfs_bulk_ireq)=%zu  sizeof(xfs_bulkstat)=%zu  (kernel expects 64 and 192)\n",
           sizeof(struct xfs_bulk_ireq), sizeof(struct xfs_bulkstat));
    int fd = open(path, O_RDONLY);
    if (fd < 0) { perror("open"); return 1; }

    char geo[256];
    printf("XFS_IOC_FSGEOMETRY: %s\n",
           ioctl(fd, XFS_IOC_FSGEOMETRY, geo) == 0 ? "ok" : strerror(errno));

    static struct req r;
    memset(&r, 0, sizeof r);
    r.hdr.ino = 0;
    r.hdr.icount = N;
    if (ioctl(fd, XFS_IOC_BULKSTAT, &r) < 0) {
        printf("XFS_IOC_BULKSTAT as uid %d: FAILED errno=%d (%s)\n", getuid(), errno, strerror(errno));
        return 2;
    }
    printf("XFS_IOC_BULKSTAT as uid %d: OK, %u records returned\n", getuid(), r.hdr.ocount);
    for (unsigned i = 0; i < r.hdr.ocount && i < 3; i++)
        printf("   ino=%llu mode=%06o size=%llu blocks=%llu\n",
               (unsigned long long)r.bs[i].bs_ino, r.bs[i].bs_mode,
               (unsigned long long)r.bs[i].bs_size, (unsigned long long)r.bs[i].bs_blocks);
    return 0;
}
