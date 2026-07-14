/* C baseline for the arithmetic-bound LCG kernel (REQ-LLL-162).
 *
 * FAIRNESS: llmlang's `mod` is EUCLIDEAN (DEC-LLL-026) while C's `%` is truncated, so a
 * bare `%` would be a DIFFERENT computation, not a faster one. `emod` restores euclidean
 * semantics — this is the same handicap the old "~10x faster than C" claim rested on (C
 * needs a sign fixup where llmlang's euclidean `mod 2^n` lowers to a bare AND). Keeping
 * it makes the comparison honest in BOTH directions: it is the reason C looked slow then,
 * and it must stay now that llmlang looks slow. */
#include <stdio.h>
#include <stdint.h>

static int64_t emod(int64_t a, int64_t m) {
    int64_t r = a % m;
    return r < 0 ? r + (m < 0 ? -m : m) : r;
}

int main(void) {
    int64_t seed = 42;
    for (int64_t n = 100000000; n; n--) {
        seed = emod(seed * 1103515245 + 12345, 2147483648LL);
    }
    printf("%lld\n", (long long) seed);
    return 0;
}
