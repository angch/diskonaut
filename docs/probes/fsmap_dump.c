#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdint.h>
#include <sys/ioctl.h>
#include <linux/fsmap.h>
#define XFS_IOC_GETFSMAP _IOWR('X', 59, struct fsmap_head)
#define N 4096
static struct { struct fsmap_head h; struct fsmap recs[N]; } q;
int main(int argc, char **argv) {
    int fd = open(argc>1?argv[1]:"/data", O_RDONLY);
    memset(&q,0,sizeof q);
    q.h.fmh_count = N;
    q.h.fmh_keys[1].fmr_device = UINT32_MAX;
    q.h.fmh_keys[1].fmr_physical = UINT64_MAX;
    q.h.fmh_keys[1].fmr_owner = UINT64_MAX;
    q.h.fmh_keys[1].fmr_offset = UINT64_MAX;
    unsigned long long batches=0, tot=0, lastphys=0;
    for (;;) {
        if (ioctl(fd, XFS_IOC_GETFSMAP, &q) < 0) { perror("GETFSMAP"); return 2; }
        unsigned n = q.h.fmh_entries;
        if (!n) break;
        if (batches < 2) for (unsigned i=0;i<n && i<6;i++)
            printf("  batch%llu dev=%u phys=%llu len=%llu owner=%lld off=%llu flags=%#x\n",
                batches,q.recs[i].fmr_device,(unsigned long long)q.recs[i].fmr_physical,
                (unsigned long long)q.recs[i].fmr_length,(long long)q.recs[i].fmr_owner,
                (unsigned long long)q.recs[i].fmr_offset,q.recs[i].fmr_flags);
        tot += n; batches++;
        lastphys = q.recs[n-1].fmr_physical + q.recs[n-1].fmr_length;
        if (q.recs[n-1].fmr_flags & FMR_OF_LAST) { printf("  LAST flag seen\n"); break; }
        if (n < q.h.fmh_count) { printf("  short batch (%u < %u) -> stop\n", n, q.h.fmh_count); break; }
        q.h.fmh_keys[0] = q.recs[n-1];
    }
    printf("batches=%llu records=%llu  last physical end=%llu (%.1f GiB)\n",
           batches, tot, lastphys, lastphys/(double)(1ull<<30));
    return 0;
}
