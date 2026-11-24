// Lane W2 (oltp): an LD_PRELOAD shim that timestamps the syscalls of a write's stages (the log's
// writes, its open, flush and close, and the futex hand-offs between threads), the same for any
// engine in the process. Records go to a ring of the last RING entries, written to $STAGES_OUT
// (binary, 64 bytes each: t0 ns, t1 ns, ret, a, b, tid, kind, path tail) when the process exits.
// Build: cc -O2 -shared -fPIC -o stages.so stages.c -ldl
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

enum { K_PWRITE = 1, K_WRITE, K_PWRITEV, K_FDATASYNC, K_FSYNC, K_OPEN, K_CLOSE, K_FUTEX, K_SYNC_RANGE };

struct rec {
  uint64_t t0, t1;
  int64_t ret, a, b;
  int32_t tid;
  int16_t kind;
  char tail[10];
};
_Static_assert(sizeof(struct rec) == 56, "record size");

#define RING (1u << 20)
static struct rec *ring;
static _Atomic uint64_t next;
static __thread int32_t my_tid;

static long (*r_syscall)(long, ...);
static ssize_t (*r_pwrite)(int, const void *, size_t, off_t);
static ssize_t (*r_pwrite64)(int, const void *, size_t, off_t);
static ssize_t (*r_write)(int, const void *, size_t);
static ssize_t (*r_pwritev)(int, const struct iovec *, int, off_t);
static ssize_t (*r_pwritev64)(int, const struct iovec *, int, off_t);
static int (*r_fdatasync)(int);
static int (*r_fsync)(int);
static int (*r_sync_file_range)(int, off_t, off_t, unsigned int);
static int (*r_open)(const char *, int, ...);
static int (*r_open64)(const char *, int, ...);
static int (*r_openat)(int, const char *, int, ...);
static int (*r_openat64)(int, const char *, int, ...);
static int (*r_close)(int);

static uint64_t now(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000000000u + (uint64_t)ts.tv_nsec;
}

static int32_t tid(void) {
  if (!my_tid) my_tid = (int32_t)r_syscall(SYS_gettid);
  return my_tid;
}

static void put(int kind, uint64_t t0, uint64_t t1, int64_t ret, int64_t a, int64_t b, const char *path) {
  if (!ring) return;
  struct rec *r = &ring[atomic_fetch_add_explicit(&next, 1, memory_order_relaxed) & (RING - 1)];
  r->t0 = t0, r->t1 = t1, r->ret = ret, r->a = a, r->b = b, r->tid = tid(), r->kind = (int16_t)kind;
  memset(r->tail, 0, sizeof r->tail);
  if (path) {
    size_t n = strlen(path), k = n < sizeof r->tail ? n : sizeof r->tail;
    memcpy(r->tail, path + n - k, k);
  }
}

__attribute__((constructor)) static void init(void) {
  r_syscall = dlsym(RTLD_NEXT, "syscall");
  r_pwrite = dlsym(RTLD_NEXT, "pwrite");
  r_pwrite64 = dlsym(RTLD_NEXT, "pwrite64");
  r_write = dlsym(RTLD_NEXT, "write");
  r_pwritev = dlsym(RTLD_NEXT, "pwritev");
  r_pwritev64 = dlsym(RTLD_NEXT, "pwritev64");
  r_fdatasync = dlsym(RTLD_NEXT, "fdatasync");
  r_fsync = dlsym(RTLD_NEXT, "fsync");
  r_sync_file_range = dlsym(RTLD_NEXT, "sync_file_range");
  r_open = dlsym(RTLD_NEXT, "open");
  r_open64 = dlsym(RTLD_NEXT, "open64");
  r_openat = dlsym(RTLD_NEXT, "openat");
  r_openat64 = dlsym(RTLD_NEXT, "openat64");
  r_close = dlsym(RTLD_NEXT, "close");
  ring = calloc(RING, sizeof(struct rec));
}

__attribute__((destructor)) static void dump(void) {
  const char *out = getenv("STAGES_OUT");
  if (!out || !ring) return;
  struct rec *r = ring;
  ring = NULL;
  uint64_t n = atomic_load(&next), first = n > RING ? n - RING : 0;
  int fd = r_open(out, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (fd < 0) return;
  for (uint64_t i = first; i < n; i++) {
    if (r_write(fd, &r[i & (RING - 1)], sizeof *r) != (ssize_t)sizeof *r) break;
  }
  r_close(fd);
}

long syscall(long n, ...) {
  va_list ap;
  va_start(ap, n);
  long a1 = va_arg(ap, long), a2 = va_arg(ap, long), a3 = va_arg(ap, long);
  long a4 = va_arg(ap, long), a5 = va_arg(ap, long), a6 = va_arg(ap, long);
  va_end(ap);
  if (n != SYS_futex) return r_syscall(n, a1, a2, a3, a4, a5, a6);
  uint64_t t0 = now();
  long ret = r_syscall(n, a1, a2, a3, a4, a5, a6);
  put(K_FUTEX, t0, now(), ret, a2 & 0x7f, a1, NULL);
  return ret;
}

#define TIMED(kind, a, b, path, call) \
  uint64_t t0 = now();                \
  __typeof__(call) ret = call;        \
  put(kind, t0, now(), (int64_t)ret, (int64_t)(a), (int64_t)(b), path);

ssize_t pwrite(int fd, const void *buf, size_t n, off_t off) { TIMED(K_PWRITE, fd, n, NULL, r_pwrite(fd, buf, n, off)); return ret; }
ssize_t pwrite64(int fd, const void *buf, size_t n, off_t off) { TIMED(K_PWRITE, fd, n, NULL, r_pwrite64(fd, buf, n, off)); return ret; }
ssize_t write(int fd, const void *buf, size_t n) {
  if (fd <= 2) return r_write(fd, buf, n);
  TIMED(K_WRITE, fd, n, NULL, r_write(fd, buf, n));
  return ret;
}
ssize_t pwritev(int fd, const struct iovec *v, int c, off_t off) { TIMED(K_PWRITEV, fd, c, NULL, r_pwritev(fd, v, c, off)); return ret; }
ssize_t pwritev64(int fd, const struct iovec *v, int c, off_t off) { TIMED(K_PWRITEV, fd, c, NULL, r_pwritev64(fd, v, c, off)); return ret; }
int fdatasync(int fd) { TIMED(K_FDATASYNC, fd, 0, NULL, r_fdatasync(fd)); return ret; }
int fsync(int fd) { TIMED(K_FSYNC, fd, 0, NULL, r_fsync(fd)); return ret; }
int sync_file_range(int fd, off_t o, off_t n, unsigned int f) { TIMED(K_SYNC_RANGE, fd, n, NULL, r_sync_file_range(fd, o, n, f)); return ret; }
int close(int fd) { TIMED(K_CLOSE, fd, 0, NULL, r_close(fd)); return ret; }

#define OPENER(name, real, ...)                                            \
  int name(__VA_ARGS__, int flags, ...) {                                  \
    mode_t mode = 0;                                                       \
    if (flags & (O_CREAT | __O_TMPFILE)) {                                 \
      va_list ap;                                                          \
      va_start(ap, flags);                                                 \
      mode = va_arg(ap, mode_t);                                           \
      va_end(ap);                                                          \
    }
OPENER(open, r_open, const char *p)
  TIMED(K_OPEN, flags, 0, p, r_open(p, flags, mode));
  return ret;
}
OPENER(open64, r_open64, const char *p)
  TIMED(K_OPEN, flags, 0, p, r_open64(p, flags, mode));
  return ret;
}
OPENER(openat, r_openat, int d, const char *p)
  TIMED(K_OPEN, flags, d, p, r_openat(d, p, flags, mode));
  return ret;
}
OPENER(openat64, r_openat64, int d, const char *p)
  TIMED(K_OPEN, flags, d, p, r_openat64(d, p, flags, mode));
  return ret;
}
