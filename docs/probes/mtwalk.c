/* In-process directory-parallel walker: a shared LIFO queue of directory fds,
   N pthreads, getdents64 + fstatat per entry.  Deliberately naive — no work
   stealing, one mutex — to establish what a straightforward replacement walker
   achieves in ONE process, against jwalk's collapse past 8 threads. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <dirent.h>
#include <pthread.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/syscall.h>

struct linux_dirent64 {
    uint64_t d_ino; int64_t d_off; unsigned short d_reclen; unsigned char d_type; char d_name[];
};

static int *queue; static size_t qn, qcap;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static int idle, nthreads, done;
static _Atomic unsigned long long n_entries, n_failed, total_blocks;

static void push(int fd) {
    if (qn == qcap) { qcap = qcap ? qcap*2 : 1024; queue = realloc(queue, qcap*sizeof(int)); }
    queue[qn++] = fd;
}

static void *worker(void *arg) {
    (void)arg;
    char buf[1<<16];
    unsigned long long le = 0, lf = 0, lb = 0;
    for (;;) {
        pthread_mutex_lock(&lock);
        while (qn == 0 && !done) {
            if (++idle == nthreads) { done = 1; pthread_cond_broadcast(&cv); }
            else pthread_cond_wait(&cv, &lock);
            if (done) break;
            idle--;
        }
        if (qn == 0) { pthread_mutex_unlock(&lock); break; }
        int dirfd = queue[--qn];
        pthread_mutex_unlock(&lock);

        int *subs = NULL; size_t ns = 0, cs = 0;
        for (;;) {
            long nread = syscall(__NR_getdents64, dirfd, buf, sizeof buf);
            if (nread <= 0) break;
            for (long off = 0; off < nread; ) {
                struct linux_dirent64 *d = (void *)(buf + off);
                if (!d->d_reclen) break;
                off += d->d_reclen;
                if (d->d_name[0]=='.' && (!d->d_name[1] || (d->d_name[1]=='.' && !d->d_name[2]))) continue;
                le++;
                struct stat st;
                if (!fstatat(dirfd, d->d_name, &st, AT_SYMLINK_NOFOLLOW)) lb += st.st_blocks; else lf++;
                if (d->d_type == DT_DIR) {
                    int sub = openat(dirfd, d->d_name, O_RDONLY|O_DIRECTORY|O_NOFOLLOW);
                    if (sub >= 0) {
                        if (ns == cs) { cs = cs ? cs*2 : 16; subs = realloc(subs, cs*sizeof(int)); }
                        subs[ns++] = sub;
                    } else lf++;
                }
            }
        }
        close(dirfd);
        if (ns) {
            pthread_mutex_lock(&lock);
            for (size_t i = 0; i < ns; i++) push(subs[i]);
            pthread_cond_broadcast(&cv);
            pthread_mutex_unlock(&lock);
        }
        free(subs);
    }
    n_entries += le; n_failed += lf; total_blocks += lb;
    return NULL;
}

int main(int argc, char **argv) {
    const char *path = argc > 1 ? argv[1] : "/data";
    nthreads = argc > 2 ? atoi(argv[2]) : 8;
    int fd = open(path, O_RDONLY|O_DIRECTORY);
    if (fd < 0) { perror("open"); return 1; }
    push(fd);
    pthread_t *t = malloc(nthreads * sizeof(pthread_t));
    struct timespec a, b; clock_gettime(CLOCK_MONOTONIC, &a);
    for (int i = 0; i < nthreads; i++) pthread_create(&t[i], NULL, worker, NULL);
    for (int i = 0; i < nthreads; i++) pthread_join(t[i], NULL);
    clock_gettime(CLOCK_MONOTONIC, &b);
    double el = (b.tv_sec-a.tv_sec) + (b.tv_nsec-a.tv_nsec)/1e9;
    printf("mtwalk threads=%-3d %7.3fs %9llu entries %10.0f entries/s %8.1f GiB failed=%llu\n",
           nthreads, el, n_entries, n_entries/el, total_blocks*512.0/(1<<30), n_failed);
    return 0;
}
