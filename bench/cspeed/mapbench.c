/* C baseline for mapbench.lll — an APPLES-TO-APPLES *ordered* map: a sorted key array probed
 * by binary search (O(log n), like llmlang's persistent Rc<BTreeMap>). This is the fair
 * comparison: it charges C for the SAME ordered-lookup asymptotics, not an O(1) hashmap
 * (which would overstate the gap by not providing ordering/persistence). See RESULTS.md.
 * Build sorted keys 1..n, run r rounds counting how many of keys 1..n are present. */
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    long n = argc > 1 ? atol(argv[1]) : 4000;
    long r = argc > 2 ? atol(argv[2]) : 2000;
    long *keys = malloc((size_t) n * sizeof(long));
    for (long i = 0; i < n; i++) keys[i] = i + 1;       /* sorted 1..n */
    long acc = 0;
    for (long round = 0; round < r; round++) {
        for (long key = 1; key <= n; key++) {
            long lo = 0, hi = n - 1, found = 0;
            while (lo <= hi) {
                long mid = (lo + hi) / 2, v = keys[mid];
                if (v == key) { found = 1; break; }
                else if (v < key) lo = mid + 1;
                else hi = mid - 1;
            }
            acc += found;
        }
    }
    printf("%ld\n", acc);
    free(keys);
    return 0;
}
