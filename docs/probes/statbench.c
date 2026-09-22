/* Single-threaded walker comparing per-entry metadata strategies on one tree.
   Each directory is fully read (and stat'ed) before descending, so no buffer is
   shared across recursion levels. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <dirent.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/mman.h>
#include <linux/io_uring.h>

static int mode;
static unsigned long long n_entries, n_dirs, n_stats, n_unknown, n_failed, total_blocks;

struct linux_dirent64 {
    uint64_t d_ino; int64_t d_off; unsigned short d_reclen; unsigned char d_type; char d_name[];
};

/* ---------------- io_uring (raw syscalls, no liburing) ---------------- */
#define QD 512
struct ring {
    int fd;
    unsigned *sq_tail,*sq_mask,*sq_array,*cq_head,*cq_tail,*cq_mask;
    struct io_uring_sqe *sqes; struct io_uring_cqe *cqes;
};
static struct ring R;
static struct statx stxbuf[QD];

static int ring_init(void) {
    struct io_uring_params p; memset(&p,0,sizeof p);
    int fd = syscall(__NR_io_uring_setup, QD, &p);
    if (fd < 0) return -1;
    R.fd = fd;
    size_t sq_sz = p.sq_off.array + p.sq_entries*sizeof(unsigned);
    size_t cq_sz = p.cq_off.cqes + p.cq_entries*sizeof(struct io_uring_cqe);
    if (p.features & IORING_FEAT_SINGLE_MMAP) { if (cq_sz > sq_sz) sq_sz = cq_sz; cq_sz = sq_sz; }
    void *sq = mmap(0,sq_sz,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_POPULATE,fd,IORING_OFF_SQ_RING);
    if (sq == MAP_FAILED) return -1;
    void *cq = (p.features & IORING_FEAT_SINGLE_MMAP) ? sq :
        mmap(0,cq_sz,PROT_READ|PROT_WRITE,MAP_SHARED|MAP_POPULATE,fd,IORING_OFF_CQ_RING);
    if (cq == MAP_FAILED) return -1;
    R.sqes = mmap(0,p.sq_entries*sizeof(struct io_uring_sqe),PROT_READ|PROT_WRITE,
                  MAP_SHARED|MAP_POPULATE,fd,IORING_OFF_SQES);
    if (R.sqes == MAP_FAILED) return -1;
    R.sq_tail=sq+p.sq_off.tail; R.sq_mask=sq+p.sq_off.ring_mask; R.sq_array=sq+p.sq_off.array;
    R.cq_head=cq+p.cq_off.head; R.cq_tail=cq+p.cq_off.tail; R.cq_mask=cq+p.cq_off.ring_mask;
    R.cqes=cq+p.cq_off.cqes;
    return 0;
}

static void uring_statx_batch(int dirfd, char **names, int n) {
    unsigned tail = *R.sq_tail;
    for (int i=0;i<n;i++) {
        unsigned idx = tail & *R.sq_mask;
        struct io_uring_sqe *s = &R.sqes[idx];
        memset(s,0,sizeof *s);
        s->opcode = IORING_OP_STATX;
        s->fd = dirfd;
        s->addr = (uint64_t)(uintptr_t)names[i];
        s->len  = STATX_TYPE|STATX_MODE|STATX_INO|STATX_NLINK|STATX_BLOCKS;
        s->off  = (uint64_t)(uintptr_t)&stxbuf[i];
        s->statx_flags = AT_SYMLINK_NOFOLLOW | (mode==6 ? AT_STATX_DONT_SYNC : 0);
        s->user_data = i;
        R.sq_array[idx] = idx;
        tail++;
    }
    __atomic_store_n(R.sq_tail, tail, __ATOMIC_RELEASE);
    int left = n;
    while (left > 0) {
        int r = syscall(__NR_io_uring_enter, R.fd, left, left, IORING_ENTER_GETEVENTS, NULL, 0);
        if (r < 0 && errno == EINTR) continue;
        if (r < 0) { perror("io_uring_enter"); exit(4); }
        unsigned chead = *R.cq_head, ctail = __atomic_load_n(R.cq_tail, __ATOMIC_ACQUIRE);
        while (chead != ctail) {
            struct io_uring_cqe *c = &R.cqes[chead & *R.cq_mask];
            if (c->res == 0) total_blocks += stxbuf[c->user_data].stx_blocks; else n_failed++;
            chead++; left--;
        }
        __atomic_store_n(R.cq_head, chead, __ATOMIC_RELEASE);
    }
    n_stats += n;
}

/* ---------------- walker ---------------- */
static void do_stat(int dirfd, const char *name) {
    n_stats++;
    if (mode == 1 || mode == 4) {
        struct stat st;
        if (!fstatat(dirfd,name,&st,AT_SYMLINK_NOFOLLOW)) total_blocks += st.st_blocks; else n_failed++;
    } else {
        struct statx s;
        int fl = AT_SYMLINK_NOFOLLOW | (mode==3 ? AT_STATX_DONT_SYNC : 0);
        if (!statx(dirfd,name,fl,STATX_TYPE|STATX_MODE|STATX_INO|STATX_NLINK|STATX_BLOCKS,&s))
            total_blocks += s.stx_blocks; else n_failed++;
    }
}

static void walk(int dirfd) {
    char buf[1<<16];
    char **subdirs = NULL; int nsub = 0, capsub = 0;
    char **batch = NULL; int nbat = 0;
    if (mode>=5) batch = malloc(QD*sizeof(char*));

    for (;;) {
        long nread = syscall(__NR_getdents64, dirfd, buf, sizeof buf);
        if (nread <= 0) break;
        for (long off = 0; off < nread; ) {
            struct linux_dirent64 *d = (void *)(buf + off);
            if (d->d_reclen == 0) break;
            off += d->d_reclen;
            if (d->d_name[0]=='.' && (!d->d_name[1] || (d->d_name[1]=='.' && !d->d_name[2]))) continue;
            n_entries++;
            if (d->d_type == DT_UNKNOWN) n_unknown++;
            int isdir = d->d_type == DT_DIR;
            if (isdir) {
                n_dirs++;
                if (nsub == capsub) { capsub = capsub?capsub*2:16; subdirs = realloc(subdirs,capsub*sizeof(char*)); }
                subdirs[nsub++] = strdup(d->d_name);
            }
            switch (mode) {
            case 0: break;
            case 4: if (!isdir) do_stat(dirfd,d->d_name); break;
            case 5: case 6:
                batch[nbat++] = strdup(d->d_name);
                if (nbat == QD) { uring_statx_batch(dirfd,batch,nbat);
                                  for(int i=0;i<nbat;i++) free(batch[i]); nbat=0; }
                break;
            default: do_stat(dirfd,d->d_name); break;
            }
        }
    }
    if (nbat) { uring_statx_batch(dirfd,batch,nbat); for(int i=0;i<nbat;i++) free(batch[i]); }
    free(batch);
    for (int i=0;i<nsub;i++) {
        int sub = openat(dirfd, subdirs[i], O_RDONLY|O_DIRECTORY|O_NOFOLLOW);
        if (sub >= 0) { walk(sub); close(sub); } else n_failed++;
        free(subdirs[i]);
    }
    free(subdirs);
}

static const char *NAMES[] = {"getdents only","fstatat","statx minimal","statx DONT_SYNC",
                              "fstatat non-dirs","io_uring statx","io_uring DONT_SYNC"};

int main(int argc, char **argv) {
    const char *path = argc>1?argv[1]:"/data";
    mode = argc>2?atoi(argv[2]):0;
    if (mode>=5 && ring_init()<0) { fprintf(stderr,"io_uring unavailable: %s\n",strerror(errno)); return 1; }
    int fd = open(path,O_RDONLY|O_DIRECTORY);
    if (fd<0){perror("open");return 1;}
    struct timespec a,b; clock_gettime(CLOCK_MONOTONIC,&a);
    walk(fd);
    clock_gettime(CLOCK_MONOTONIC,&b);
    double el = (b.tv_sec-a.tv_sec)+(b.tv_nsec-a.tv_nsec)/1e9;
    printf("%-19s %7.3fs %9llu entries %9llu stats %7.0f ns/entry %8.1f GiB  unknown=%llu failed=%llu\n",
           NAMES[mode], el, n_entries, n_stats, el*1e9/n_entries,
           total_blocks*512.0/(1<<30), n_unknown, n_failed);
    return 0;
}
