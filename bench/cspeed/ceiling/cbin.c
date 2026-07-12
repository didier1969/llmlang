/* C baseline, argv N K R : in-place O(1)/element, R outer repeats. Self-timed
 * (CLOCK_MONOTONIC, per-run seconds to stderr) so sub-ms compute is measured
 * without shell-timer / process-startup noise. */
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
int main(int argc, char **argv) {
    long n = argc > 1 ? atol(argv[1]) : 2000;
    long k = argc > 2 ? atol(argv[2]) : 4000;
    long r = argc > 3 ? atol(argv[3]) : 1;
    long total = 0;
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    for (long rep = 0; rep < r; rep++) {
        long *a = malloc((size_t) n * sizeof(long));
        for (long i = 0; i < n; i++) a[i] = n - i;
        for (long p = 0; p < k; p++)
            for (long i = 0; i < n; i++) a[i] += 1;
        long acc = 0;
        for (long i = 0; i < n; i++) acc += a[i];
        total += acc;
        free(a);
    }
    clock_gettime(CLOCK_MONOTONIC, &t1);
    double per = ((t1.tv_sec - t0.tv_sec) + (t1.tv_nsec - t0.tv_nsec) / 1e9) / (double) r;
    fprintf(stderr, "%.8f\n", per);
    printf("%ld\n", total);
    return 0;
}
