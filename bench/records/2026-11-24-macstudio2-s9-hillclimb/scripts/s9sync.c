// Counts fsync() and fcntl(F_FULLFSYNC), and with S9_FULLFSYNC=1 makes every fsync() an F_FULLFSYNC (the
// durability Amphora's own flush asks of macOS). Loaded with DYLD_INSERT_LIBRARIES; same code for every engine.
#include <fcntl.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
static _Atomic unsigned long n_fsync, n_full, n_barrier;
static int full_mode = -1;
static int s9_fsync(int fd) {
  atomic_fetch_add(&n_fsync, 1);
  if (full_mode < 0) { const char *v = getenv("S9_FULLFSYNC"); full_mode = v && v[0] == '1'; }
  if (full_mode) { int r = fcntl(fd, F_FULLFSYNC, 0); if (r != -1) return 0; }
  return fsync(fd);
}
static int s9_fcntl(int fd, int cmd, ...) {
  va_list ap; va_start(ap, cmd); void *arg = va_arg(ap, void *); va_end(ap);
  if (cmd == F_FULLFSYNC) atomic_fetch_add(&n_full, 1);
  if (cmd == F_BARRIERFSYNC) atomic_fetch_add(&n_barrier, 1);
  return fcntl(fd, cmd, arg);
}
__attribute__((destructor)) static void s9_report(void) {
  fprintf(stderr, "s9sync: fsync=%lu F_FULLFSYNC=%lu F_BARRIERFSYNC=%lu\n", atomic_load(&n_fsync), atomic_load(&n_full), atomic_load(&n_barrier));
}
__attribute__((used)) static struct { const void *r, *o; } interposers[] __attribute__((section("__DATA,__interpose"))) = {
  {(const void *)s9_fsync, (const void *)fsync}, {(const void *)s9_fcntl, (const void *)fcntl}};
