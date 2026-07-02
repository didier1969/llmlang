/* C reference for the LCG kernel (Euclidean mod on non-negative operands
   coincides with C's % here). Written tail-recursively like the .lll source;
   gcc -O2 turns it into a loop. */
#include <stdio.h>

static long long lcg(long long seed, long long n) {
    if (n == 0) return seed;
    return lcg((seed * 1103515245 + 12345) % 2147483648LL, n - 1);
}

int main(void) {
    printf("%lld\n", lcg(42, 100000000));
    return 0;
}
