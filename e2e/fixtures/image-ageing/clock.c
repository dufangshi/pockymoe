// Test-only macOS clock interposition. Only the isolated Supervisor loads it;
// neither the machine clock nor a running installed service is changed.
#include <time.h>
#include <sys/time.h>
#include <stdlib.h>
#include <stdio.h>
static long offset(void) {
  const char *path = getenv("POCKYMOE_TEST_CLOCK_FILE");
  if (!path) return 0;
  FILE *file = fopen(path, "r");
  if (!file) return 0;
  long seconds = 0;
  (void)fscanf(file, "%ld", &seconds);
  fclose(file);
  return seconds;
}
static int shifted_clock(clockid_t id, struct timespec *value) {
  int result = clock_gettime(id, value);
  if (result == 0 && id == CLOCK_REALTIME) value->tv_sec += offset();
  return result;
}
static int shifted_time(struct timeval *value, void *zone) {
  int result = gettimeofday(value, zone);
  if (result == 0) value->tv_sec += offset();
  return result;
}
#define INTERPOSE(replacement, original) \
  __attribute__((used)) static struct { const void *replace; const void *original; } \
  interpose_##original __attribute__((section("__DATA,__interpose"))) = { (const void *)&replacement, (const void *)&original };
INTERPOSE(shifted_clock, clock_gettime)
INTERPOSE(shifted_time, gettimeofday)
