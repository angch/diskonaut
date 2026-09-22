/* Enumerate every extent on an XFS filesystem via XFS_IOC_GETFSMAP and
   accumulate allocated blocks per owning inode.  This is the "sizes without
   stat" oracle: combined with getdents64 (which supplies name + d_ino) it
   could replace the per-file statx entirely. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdint.h>
#include <time.h>
#include <sys/ioctl.h>
#include <linux/fsmap.h>

#define XFS_IOC_GETFSMAP _IOWR('X', 59, struct fsmap_head)
#define N 4096

static struct { struct fsmap_head h; struct fsmap recs[N]; } q;

/* open-addressing map: inode -> bytes */
static uint64_t *keys; static uint64_t *vals; static size_t cap, used;
static void map_init(size_t c){cap=c;keys=calloc(cap,8);vals=calloc(cap,8);}
static void map_add(uint64_t k, uint64_t v){
    size_t i = (k * 0x9E3779B97F4A7C15ull) & (cap-1);
    for(;;){ if(!keys[i]){keys[i]=k;vals[i]=v;used++;return;}
             if(keys[i]==k){vals[i]+=v;return;} i=(i+1)&(cap-1); }
}

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "/data";
    int fd = open(path, O_RDONLY);
    if (fd < 0) { perror("open"); return 1; }
    map_init(1u << 24);

    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);

    unsigned long long calls = 0, records = 0, special = 0, shared = 0;
    unsigned long long file_bytes = 0, special_bytes = 0;

    memset(&q, 0, sizeof q);
    q.h.fmh_count = N;
    q.h.fmh_keys[1].fmr_device = UINT32_MAX;
    q.h.fmh_keys[1].fmr_physical = UINT64_MAX;
    q.h.fmh_keys[1].fmr_owner = UINT64_MAX;
    q.h.fmh_keys[1].fmr_offset = UINT64_MAX;

    for (;;) {
        if (ioctl(fd, XFS_IOC_GETFSMAP, &q) < 0) { perror("GETFSMAP"); return 2; }
        calls++;
        unsigned n = q.h.fmh_entries;
        if (n == 0) break;
        for (unsigned i = 0; i < n; i++) {
            struct fsmap *r = &q.recs[i];
            if (r->fmr_flags & FMR_OF_LAST) { n = i; goto done; }
            records++;
            if (r->fmr_flags & FMR_OF_SPECIAL_OWNER) { special++; special_bytes += r->fmr_length; }
            else { file_bytes += r->fmr_length; map_add((uint64_t)r->fmr_owner, r->fmr_length); }
            if (r->fmr_flags & FMR_OF_SHARED) shared++;
        }
    done:
        if (q.h.fmh_entries < q.h.fmh_count) break;
        /* continue from the last record returned */
        q.h.fmh_keys[0] = q.recs[q.h.fmh_entries - 1];
    }

    clock_gettime(CLOCK_MONOTONIC, &b);
    double el = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
    printf("GETFSMAP %s: %.3fs  %llu ioctl calls  %llu records  %zu distinct inodes\n",
           path, el, calls, records, used);
    printf("  file extents: %.1f GiB   special-owner (metadata/free): %.1f GiB\n",
           file_bytes / (double)(1ull << 30), special_bytes / (double)(1ull << 30));
    printf("  reflink-shared extent records: %llu\n", shared);
    return 0;
}
